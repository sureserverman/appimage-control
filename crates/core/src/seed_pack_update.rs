//! `--update-seed-packs`: fetch a signed seed-pack tarball from the
//! companion repo and install it into [`crate::seed_pack::user_seeds_dir`].
//!
//! ## Trust model
//!
//! Each release of the companion repo (`appimage-control-seed-packs`)
//! ships two artifacts:
//!
//! - `seed-packs.tar.zst` — gzip-free zstd-compressed tar of
//!   `feather.json`, `electrum-btc.json`, and any other curated packs
//!   the maintainer publishes.
//! - `seed-packs.tar.zst.sig` — a 69-byte file containing the magic
//!   bytes `AICS\x01` followed by a 64-byte ed25519 signature over the
//!   SHA-512 of the tarball.
//!
//! The verifying public key is committed as [`SEED_PACKS_PUB_KEY`] in
//! this file. The matching private key is held offline by the
//! maintainer; the companion repo's release CI signs each tarball
//! with the `aic-sign-pack` helper binary that ships in this same
//! crate (so anyone can audit the signing code; the user runs it on a
//! cold-storage machine only).
//!
//! ## File-format spec ("AICSIG-1")
//!
//! ```text
//! offset  bytes  meaning
//! 0       4      magic = b"AICS"
//! 4       1      version = 0x01
//! 5       64     ed25519 signature over SHA-512(tarball-bytes)
//! ```
//!
//! Total: 69 bytes. Anything else is rejected.
//!
//! ## Why not minisign-verify
//!
//! This codebase fully vendors crates.io (see `.cargo/config.toml`),
//! and adding `minisign-verify` would require a separate `cargo vendor`
//! pass with network access. `ring` + `sha2` are already vendored via
//! `reqwest`, and the cryptographic primitive (ed25519 over a hash of
//! the artifact) is identical. The verify call site is one function;
//! swapping it for `minisign-verify` later if that crate ever lands in
//! the vendor tree is local.

use std::fs;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::signature::{UnparsedPublicKey, ED25519};
use sha2::{Digest, Sha512};

use crate::download::download_to_file;
use crate::error::{Error, Result};
use crate::seed_pack::{user_seeds_dir, SeedPackLibrary};

/// Default GitHub-releases base URL for the companion repo. Override
/// via the [`ENV_RELEASE_URL`] env var (used by tests and forks).
pub const DEFAULT_RELEASE_URL: &str =
    "https://github.com/sureserverman/appimage-control-seed-packs/releases/latest/download";

/// Env var that, when set to a non-empty URL, replaces
/// [`DEFAULT_RELEASE_URL`] for one update-seed-packs invocation.
pub const ENV_RELEASE_URL: &str = "APPIMAGE_CONTROL_SEED_PACKS_URL";

/// Embedded ed25519 public key (base64-encoded, 32 raw bytes when
/// decoded) that signs every legitimate seed-pack release.
///
/// **PLACEHOLDER.** Replace before the first companion-repo release.
/// The matching private key must never enter version control or
/// online infrastructure. To generate (one-time, on the maintainer's
/// cold-storage machine):
///
/// ```sh
/// cargo run --bin aic-sign-pack -- keygen ~/.aic-keys/seed-packs.key
/// # prints the base64 public key — copy into SEED_PACKS_PUB_KEY below.
/// ```
///
/// While this constant equals the placeholder string, every call to
/// [`update_seed_packs`] returns
/// [`Error::SeedPackSignature`](crate::error::Error::SeedPackSignature)
/// without touching the network. This keeps the binary safe to ship
/// pre-keygen — `--update-seed-packs` simply tells the user
/// (the maintainer) to set the key.
pub const SEED_PACKS_PUB_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

/// Magic bytes at the head of a seed-pack signature file.
const SIG_MAGIC: &[u8] = b"AICS";
/// Format version following the magic bytes.
const SIG_VERSION: u8 = 1;
/// Total signature-file size: 4-byte magic + 1-byte version + 64-byte ed25519 sig.
const SIG_FILE_LEN: usize = 4 + 1 + 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateReport {
    pub installed_count: usize,
    pub source_url: String,
    pub install_dir: PathBuf,
}

/// Download, verify, unpack, validate, and atomically install the
/// latest companion-repo seed-pack release into
/// [`user_seeds_dir`].
///
/// Returns [`Error::SeedPackSignature`] without making any network
/// requests when [`SEED_PACKS_PUB_KEY`] still equals the placeholder
/// string — this is so the binary is safe to ship before keygen.
pub fn update_seed_packs(
    base_url_override: Option<&str>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<UpdateReport> {
    if SEED_PACKS_PUB_KEY == "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=" {
        return Err(Error::SeedPackSignature(
            "this build has no seed-pack signing key configured (placeholder \
             still in source). See docs/seed-pack-companion-repo.md for the \
             keygen workflow."
                .into(),
        ));
    }

    let base = base_url_override
        .map(str::to_string)
        .or_else(|| {
            std::env::var(ENV_RELEASE_URL)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| DEFAULT_RELEASE_URL.to_string());

    let target = user_seeds_dir().ok_or(Error::NoHomeDir)?;
    let staging = target.with_extension("update-staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;

    let tarball = staging.join("seed-packs.tar.zst");
    let sig = staging.join("seed-packs.tar.zst.sig");

    on_progress("downloading seed-packs.tar.zst");
    download_to_file(&format!("{base}/seed-packs.tar.zst"), &tarball)?;
    on_progress("downloading signature");
    download_to_file(&format!("{base}/seed-packs.tar.zst.sig"), &sig)?;

    on_progress("verifying signature");
    verify_signature(&tarball, &sig, SEED_PACKS_PUB_KEY)?;

    let unpacked = staging.join("seeds");
    on_progress("unpacking");
    unpack_tar_zst(&tarball, &unpacked)?;

    on_progress("validating packs");
    let lib = SeedPackLibrary::load_from_dir(&unpacked)?.ok_or_else(|| {
        Error::InvalidPath("seed-pack tarball contains no *.json packs".into())
    })?;
    let installed_count = lib.len();

    on_progress("installing");
    install_atomically(&unpacked, &target)?;
    let _ = fs::remove_dir_all(&staging);

    Ok(UpdateReport {
        installed_count,
        source_url: base,
        install_dir: target,
    })
}

/// Verify that `sig_path` is an AICSIG-1 file containing a valid
/// ed25519 signature over `SHA-512(tarball)` made by the private key
/// matching `pub_key_b64`.
pub fn verify_signature(tarball: &Path, sig_path: &Path, pub_key_b64: &str) -> Result<()> {
    let sig_bytes = fs::read(sig_path)?;
    if sig_bytes.len() != SIG_FILE_LEN {
        return Err(Error::SeedPackSignature(format!(
            "signature file has wrong size: expected {SIG_FILE_LEN}, got {}",
            sig_bytes.len()
        )));
    }
    if &sig_bytes[..4] != SIG_MAGIC {
        return Err(Error::SeedPackSignature(
            "signature file is missing AICS magic bytes".into(),
        ));
    }
    if sig_bytes[4] != SIG_VERSION {
        return Err(Error::SeedPackSignature(format!(
            "unsupported signature version: {} (this build only knows v{SIG_VERSION})",
            sig_bytes[4]
        )));
    }
    let raw_sig = &sig_bytes[5..];

    let pk_bytes = STANDARD
        .decode(pub_key_b64.trim())
        .map_err(|e| Error::SeedPackSignature(format!("public key not valid base64: {e}")))?;
    if pk_bytes.len() != 32 {
        return Err(Error::SeedPackSignature(format!(
            "public key has wrong length: expected 32 bytes, got {}",
            pk_bytes.len()
        )));
    }

    let mut hasher = Sha512::new();
    hasher.update(fs::read(tarball)?);
    let digest = hasher.finalize();

    let pk = UnparsedPublicKey::new(&ED25519, pk_bytes.as_slice());
    pk.verify(digest.as_slice(), raw_sig)
        .map_err(|_| Error::SeedPackSignature("ed25519 signature does not verify".into()))?;
    Ok(())
}

/// Unpack a `.tar.zst` whose entries are all top-level `*.json` files
/// (no nested directories, no path traversal). Anything else is a
/// hard error before any byte is written under `into`.
fn unpack_tar_zst(archive: &Path, into: &Path) -> Result<()> {
    fs::create_dir_all(into)?;
    let f = fs::File::open(archive)?;
    let dec = zstd::Decoder::new(f)?;
    let mut tar = tar::Archive::new(dec);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();

        if path.components().count() != 1 {
            return Err(Error::InvalidPath(format!(
                "seed-pack tarball: rejected non-flat entry: {}",
                path.display()
            )));
        }
        let fname = path.file_name().and_then(|s| s.to_str()).ok_or_else(|| {
            Error::InvalidPath(format!(
                "seed-pack tarball: bad entry name: {}",
                path.display()
            ))
        })?;
        if !fname.ends_with(".json") {
            return Err(Error::InvalidPath(format!(
                "seed-pack tarball: rejected non-json entry: {fname}"
            )));
        }

        // Hard-anchor the destination to `into`; defense-in-depth on
        // top of the components-count check above.
        let dest = into.join(fname);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

/// Move `unpacked` into `target` atomically: rename old `target` to
/// `target.backup` if present, rename `unpacked` to `target`. On
/// failure the old install survives at the backup path.
fn install_atomically(unpacked: &Path, target: &Path) -> Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup = target.with_extension("backup");
    if target.exists() {
        let _ = fs::remove_dir_all(&backup);
        fs::rename(target, &backup)?;
    }
    fs::rename(unpacked, target)?;
    let _ = fs::remove_dir_all(backup);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};
    use std::io::Write;

    /// Generate a fresh ed25519 keypair in PKCS8 form for tests.
    /// Returns (raw_seed_bytes_for_signing, base64_pubkey).
    fn test_keypair() -> (Ed25519KeyPair, String) {
        // A fixed test seed — never used in production. Reproducible
        // tests are nicer than non-deterministic ones for this layer.
        let seed = [42u8; 32];
        let kp = Ed25519KeyPair::from_seed_unchecked(&seed)
            .expect("test keypair");
        let pk_b64 = STANDARD.encode(kp.public_key().as_ref());
        (kp, pk_b64)
    }

    fn build_test_tarball(dir: &Path, packs: &[(&str, &str)]) -> PathBuf {
        let path = dir.join("seed-packs.tar.zst");
        let f = fs::File::create(&path).unwrap();
        let zenc = zstd::Encoder::new(f, 3).unwrap().auto_finish();
        let mut tar = tar::Builder::new(zenc);
        for (name, body) in packs {
            let bytes = body.as_bytes();
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            tar.append_data(&mut header, format!("{name}.json"), bytes).unwrap();
        }
        tar.finish().unwrap();
        path
    }

    fn sign_test_tarball(tarball: &Path, kp: &Ed25519KeyPair) -> PathBuf {
        let body = fs::read(tarball).unwrap();
        let mut hasher = Sha512::new();
        hasher.update(&body);
        let digest = hasher.finalize();
        let sig = kp.sign(&digest);

        let sig_path = tarball.with_extension("zst.sig");
        let mut f = fs::File::create(&sig_path).unwrap();
        f.write_all(SIG_MAGIC).unwrap();
        f.write_all(&[SIG_VERSION]).unwrap();
        f.write_all(sig.as_ref()).unwrap();
        sig_path
    }

    #[test]
    fn placeholder_pub_key_blocks_update_without_network_io() {
        // The default const value should be the placeholder shipped
        // in source. update_seed_packs must refuse to proceed before
        // any download attempt.
        assert_eq!(
            SEED_PACKS_PUB_KEY,
            "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
            "if this fails, the placeholder key was replaced — also \
             update or remove this test"
        );
        let mut log = Vec::new();
        let err = update_seed_packs(Some("http://invalid.invalid"), &mut |s| {
            log.push(s.to_string())
        })
        .unwrap_err();
        assert!(
            log.is_empty(),
            "no progress events should fire before the key check, got: {log:?}"
        );
        assert!(matches!(err, Error::SeedPackSignature(_)));
    }

    #[test]
    fn verify_signature_round_trips_a_valid_signature() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk_b64) = test_keypair();
        let tarball = build_test_tarball(
            tmp.path(),
            &[("feather", r#"{"config_path":"a.json"}"#)],
        );
        let sig = sign_test_tarball(&tarball, &kp);
        verify_signature(&tarball, &sig, &pk_b64).expect("valid sig must verify");
    }

    #[test]
    fn verify_signature_rejects_tampered_tarball() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk_b64) = test_keypair();
        let tarball = build_test_tarball(
            tmp.path(),
            &[("feather", r#"{"config_path":"a.json"}"#)],
        );
        let sig = sign_test_tarball(&tarball, &kp);

        // Tamper one byte.
        let mut body = fs::read(&tarball).unwrap();
        body[0] ^= 0xff;
        fs::write(&tarball, &body).unwrap();

        let err = verify_signature(&tarball, &sig, &pk_b64).unwrap_err();
        assert!(matches!(err, Error::SeedPackSignature(_)));
    }

    #[test]
    fn verify_signature_rejects_wrong_pubkey() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, _real_pk) = test_keypair();
        let tarball = build_test_tarball(tmp.path(), &[("p", r#"{"config_path":"x"}"#)]);
        let sig = sign_test_tarball(&tarball, &kp);
        // A different key (all zeroes is a valid 32-byte ed25519 point).
        let wrong = STANDARD.encode([0u8; 32]);
        let err = verify_signature(&tarball, &sig, &wrong).unwrap_err();
        assert!(matches!(err, Error::SeedPackSignature(_)));
    }

    #[test]
    fn verify_signature_rejects_bad_magic_or_version() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk_b64) = test_keypair();
        let tarball = build_test_tarball(tmp.path(), &[("p", r#"{"config_path":"x"}"#)]);
        let sig = sign_test_tarball(&tarball, &kp);

        // Bad magic.
        let mut bad = fs::read(&sig).unwrap();
        bad[0] = b'X';
        let bad_path = sig.with_file_name("bad-magic.sig");
        fs::write(&bad_path, &bad).unwrap();
        assert!(matches!(
            verify_signature(&tarball, &bad_path, &pk_b64).unwrap_err(),
            Error::SeedPackSignature(_)
        ));

        // Bad version.
        let mut bad = fs::read(&sig).unwrap();
        bad[4] = 99;
        let bad_path = sig.with_file_name("bad-version.sig");
        fs::write(&bad_path, &bad).unwrap();
        assert!(matches!(
            verify_signature(&tarball, &bad_path, &pk_b64).unwrap_err(),
            Error::SeedPackSignature(_)
        ));

        // Wrong size.
        fs::write(&bad_path, &bad[..68]).unwrap();
        assert!(matches!(
            verify_signature(&tarball, &bad_path, &pk_b64).unwrap_err(),
            Error::SeedPackSignature(_)
        ));
    }

    /// Build a tarball with arbitrary entries at low-level so we can
    /// craft rejection-test inputs that `tar::Builder::append_data`
    /// would refuse on write (e.g. `..` traversal). Each entry is
    /// 1-block header + body padded to 512 bytes; archive ends with
    /// two zero blocks.
    fn build_raw_tarball(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("raw.tar.zst");
        let f = fs::File::create(&path).unwrap();
        let mut zenc = zstd::Encoder::new(f, 3).unwrap().auto_finish();
        for (name, body) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            // Bypass append_data's path-validity check by writing the
            // name directly into the GNU header's name field.
            let name_bytes = name.as_bytes();
            let dst = header.as_old_mut().name.as_mut();
            let n = name_bytes.len().min(dst.len());
            dst[..n].copy_from_slice(&name_bytes[..n]);
            header.set_cksum();
            zenc.write_all(header.as_bytes()).unwrap();
            zenc.write_all(body).unwrap();
            // Pad body to 512-byte boundary.
            let pad = (512 - (body.len() % 512)) % 512;
            if pad > 0 {
                zenc.write_all(&vec![0u8; pad]).unwrap();
            }
        }
        // Two zero blocks = end of archive.
        zenc.write_all(&[0u8; 1024]).unwrap();
        drop(zenc);
        path
    }

    #[test]
    fn unpack_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = build_raw_tarball(
            tmp.path(),
            &[("../../etc/evil.json", b"{}".as_slice())],
        );
        let err = unpack_tar_zst(&archive, &tmp.path().join("out")).unwrap_err();
        let s = err.to_string();
        assert!(
            s.contains("non-flat") || s.contains(".."),
            "expected traversal rejection, got: {s}"
        );
    }

    #[test]
    fn unpack_rejects_subdirectory_entry() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = build_raw_tarball(
            tmp.path(),
            &[("nested/foo.json", b"{}".as_slice())],
        );
        let err = unpack_tar_zst(&archive, &tmp.path().join("out")).unwrap_err();
        assert!(err.to_string().contains("non-flat"));
    }

    #[test]
    fn unpack_rejects_non_json_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = build_raw_tarball(
            tmp.path(),
            &[("README.md", b"hello".as_slice())],
        );
        let err = unpack_tar_zst(&archive, &tmp.path().join("out")).unwrap_err();
        assert!(err.to_string().contains("non-json"), "got: {err}");
    }

    #[test]
    fn unpack_round_trips_packs() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = build_test_tarball(
            tmp.path(),
            &[
                ("feather", r#"{"config_path":"f"}"#),
                ("electrum-btc", r#"{"config_path":"b"}"#),
            ],
        );
        let out = tmp.path().join("out");
        unpack_tar_zst(&archive, &out).unwrap();
        assert!(out.join("feather.json").is_file());
        assert!(out.join("electrum-btc.json").is_file());
    }

    #[test]
    fn install_atomically_swaps_and_keeps_no_backup_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("seeds");
        let unpacked = tmp.path().join("unpacked");
        fs::create_dir_all(&unpacked).unwrap();
        fs::write(unpacked.join("foo.json"), "{}").unwrap();

        // Pre-existing target gets replaced.
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("old.json"), "{}").unwrap();

        install_atomically(&unpacked, &target).unwrap();
        assert!(target.join("foo.json").is_file());
        assert!(!target.join("old.json").exists());
        // Backup tempfile cleaned up on success.
        assert!(!target.with_extension("backup").exists());
    }
}
