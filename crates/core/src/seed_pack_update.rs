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
use std::io::Read as _;
use std::path::{Path, PathBuf};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::signature::{UnparsedPublicKey, ED25519};
use sha2::{Digest, Sha512};

use crate::download::download_to_file;
use crate::error::{Error, Result};
use crate::seed_pack::{user_seeds_dir, SeedPackLibrary};

/// Default release base URL for the companion repo. **Placeholder by
/// default** — this public repo intentionally does not ship a release
/// URL, because the companion repo is operator-curated and meant to
/// reach only authorised users, not the general public. Authorised
/// builds replace this const before publishing the binary (or set
/// [`ENV_RELEASE_URL`] at runtime). While the placeholder is in
/// source AND no env override is set, [`update_seed_packs`] returns
/// [`Error::SeedPackSignature`](crate::error::Error::SeedPackSignature)
/// without touching the network.
pub const DEFAULT_RELEASE_URL: &str = "<unset-placeholder>";

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
/// Format version following the magic bytes. v1 (no release sequence) is refused.
const SIG_VERSION: u8 = 2;
/// v2 signature-file size: 4-byte magic + 1-byte version + 8-byte big-endian release sequence
/// + 64-byte ed25519 signature.
const SIG_FILE_LEN: usize = 4 + 1 + 8 + 64;
/// Domain tag at the head of the v2 signed message, so a v2 signature can never be read as
/// any other signed object.
const SIG_V2_DOMAIN: &[u8] = b"AICS-v2\0";

/// Unpack limits for a seed-pack release: per entry, in total, and entry count.
pub const MAX_PACK_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_PACK_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_PACK_ENTRIES: usize = 512;

/// File in the manager data dir recording the last accepted release sequence.
pub const RELEASE_SEQ_FILENAME: &str = "seed-pack-release-seq";

/// The exact bytes a v2 signature covers: `"AICS-v2\0" || sequence (u64 BE) || SHA-512(tarball)`.
/// Binding the sequence stops an older signed release from being replayed.
pub fn signed_message_v2(sequence: u64, tarball: &[u8]) -> Vec<u8> {
    let mut h = Sha512::new();
    h.update(tarball);
    let digest = h.finalize();
    let mut m = Vec::with_capacity(SIG_V2_DOMAIN.len() + 8 + digest.len());
    m.extend_from_slice(SIG_V2_DOMAIN);
    m.extend_from_slice(&sequence.to_be_bytes());
    m.extend_from_slice(&digest);
    m
}

/// A v2 signature file: magic, version 2, the sequence, then the 64-byte signature.
pub fn encode_signature_v2(sequence: u64, signature: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(SIG_FILE_LEN);
    out.extend_from_slice(SIG_MAGIC);
    out.push(SIG_VERSION);
    out.extend_from_slice(&sequence.to_be_bytes());
    out.extend_from_slice(signature);
    out
}

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
    if base == DEFAULT_RELEASE_URL {
        return Err(Error::SeedPackSignature(
            "this build has no seed-pack release URL configured \
             (DEFAULT_RELEASE_URL is still the placeholder). \
             Either set APPIMAGE_CONTROL_SEED_PACKS_URL at runtime, \
             or rebuild with the operator-managed release URL in \
             crates/core/src/seed_pack_update.rs. See \
             docs/seed-pack-companion-repo.md."
                .into(),
        ));
    }

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

    // Read once: the bytes verified are the bytes unpacked (no swap between the two reads).
    // A release is a few compressed JSON files; refuse anything implausibly large before
    // reading it into memory.
    if fs::metadata(&tarball)?.len() > MAX_PACK_TOTAL_BYTES {
        return Err(Error::InvalidPath(
            "seed-pack tarball exceeds the 32 MiB limit".into(),
        ));
    }
    let tarball_bytes = fs::read(&tarball)?;
    let sig_bytes = fs::read(&sig)?;
    let home = dirs::home_dir().ok_or(Error::NoHomeDir)?;
    let seq_store = crate::paths::AppPaths::new(home)?
        .manager_data_dir()
        .join(RELEASE_SEQ_FILENAME);
    on_progress("verifying signature, unpacking and validating");
    let installed_count = install_verified_release(
        &tarball_bytes,
        &sig_bytes,
        SEED_PACKS_PUB_KEY,
        &seq_store,
        &staging,
        &target,
    )?;
    let _ = fs::remove_dir_all(&staging);

    Ok(UpdateReport {
        installed_count,
        source_url: base,
        install_dir: target,
    })
}

/// Load the unpacked release and refuse it unless every pack loads. The per-user seeds dir
/// shadows the embedded packs, so installing a release with a refused pack would silently stop
/// that app from seeding; an update is all or nothing. Returns the number of packs.
fn validate_unpacked_packs(unpacked: &Path) -> Result<usize> {
    let lib = SeedPackLibrary::load_from_dir(unpacked)?
        .ok_or_else(|| Error::InvalidPath("seed-pack tarball contains no *.json packs".into()))?;
    let refused: Vec<String> = lib.refused().map(|(_, why)| why.to_string()).collect();
    if !refused.is_empty() {
        return Err(Error::InvalidPath(format!(
            "seed-pack release refused, nothing installed: {}",
            refused.join("; ")
        )));
    }
    Ok(lib.len())
}

/// Verify a v2 seed-pack signature over `tarball` (bytes already in memory) and return the
/// release sequence it signs. A v1 file is refused by name.
pub fn verify_signature_bytes(tarball: &[u8], sig_bytes: &[u8], pub_key_b64: &str) -> Result<u64> {
    if sig_bytes.len() < 5 || &sig_bytes[..4] != SIG_MAGIC {
        return Err(Error::SeedPackSignature(
            "signature file is missing AICS magic bytes".into(),
        ));
    }
    match sig_bytes[4] {
        SIG_VERSION => {}
        1 => {
            return Err(Error::SeedPackSignature(
                "signature format v1 is no longer accepted: it binds no release sequence, so an \
                 older release could be replayed. Re-sign with `aic-sign-pack sign --sequence N` \
                 (format v2)"
                    .into(),
            ))
        }
        v => {
            return Err(Error::SeedPackSignature(format!(
                "unsupported signature version: {v} (this build only knows v{SIG_VERSION})"
            )))
        }
    }
    if sig_bytes.len() != SIG_FILE_LEN {
        return Err(Error::SeedPackSignature(format!(
            "signature file has wrong size: expected {SIG_FILE_LEN}, got {}",
            sig_bytes.len()
        )));
    }
    let sequence = u64::from_be_bytes(sig_bytes[5..13].try_into().expect("8 bytes"));
    let raw_sig = &sig_bytes[13..];

    let pk_bytes = STANDARD
        .decode(pub_key_b64.trim())
        .map_err(|e| Error::SeedPackSignature(format!("public key not valid base64: {e}")))?;
    if pk_bytes.len() != 32 {
        return Err(Error::SeedPackSignature(format!(
            "public key has wrong length: expected 32 bytes, got {}",
            pk_bytes.len()
        )));
    }
    let pk = UnparsedPublicKey::new(&ED25519, pk_bytes.as_slice());
    pk.verify(&signed_message_v2(sequence, tarball), raw_sig)
        .map_err(|_| Error::SeedPackSignature("ed25519 signature does not verify".into()))?;
    Ok(sequence)
}

/// [`verify_signature_bytes`] for files on disk.
#[cfg(test)]
fn verify_signature(tarball: &Path, sig_path: &Path, pub_key_b64: &str) -> Result<u64> {
    verify_signature_bytes(&fs::read(tarball)?, &fs::read(sig_path)?, pub_key_b64)
}

/// Refuse a release older than the last one accepted (an equal one re-installs).
fn check_release_sequence(store: &Path, sequence: u64) -> Result<()> {
    let installed = match fs::read_to_string(store) {
        Ok(s) => s.trim().parse::<u64>().map_err(|_| {
            Error::SeedPackSignature(format!(
                "unreadable release sequence record {} — delete it to accept any signed \
                 release again",
                store.display()
            ))
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if sequence < installed {
        return Err(Error::SeedPackSignature(format!(
            "seed-pack release {sequence} is older than installed release {installed}"
        )));
    }
    Ok(())
}

/// Write the new sequence to a temp file next to the record, before anything is installed: a
/// record that cannot be written refuses the update with nothing changed, instead of letting
/// the install through and the replay check lapse. [`commit_release_sequence`] renames it in.
fn prepare_release_sequence(store: &Path, sequence: u64) -> Result<PathBuf> {
    if let Some(parent) = store.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = store.with_extension(format!("tmp.{}", std::process::id()));
    let r = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(format!("{sequence}\n").as_bytes())?;
        f.sync_all()
    })();
    if let Err(e) = r {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(tmp)
}

fn commit_release_sequence(prepared: &Path, store: &Path) -> Result<()> {
    fs::rename(prepared, store)?;
    Ok(())
}

/// Verify, check the sequence, unpack (with caps), validate and install one release held in
/// memory — the tarball is read once, so the bytes verified are the bytes unpacked. The
/// sequence is recorded only after a successful install. Returns the number of packs.
fn install_verified_release(
    tarball: &[u8],
    sig: &[u8],
    pub_key_b64: &str,
    seq_store: &Path,
    staging: &Path,
    target: &Path,
) -> Result<usize> {
    let sequence = verify_signature_bytes(tarball, sig, pub_key_b64)?;
    check_release_sequence(seq_store, sequence)?;
    let unpacked = staging.join("seeds");
    let _ = fs::remove_dir_all(&unpacked);
    unpack_tar_zst_bytes(tarball, &unpacked)?;
    let count = validate_unpacked_packs(&unpacked)?;
    let prepared = prepare_release_sequence(seq_store, sequence)?;
    if let Err(e) = install_atomically(&unpacked, target) {
        let _ = fs::remove_file(&prepared);
        return Err(e);
    }
    commit_release_sequence(&prepared, seq_store)?;
    Ok(count)
}

/// Unpack a `.tar.zst` whose entries are all top-level `*.json` files
/// (no nested directories, no path traversal). Anything else is a
/// hard error before any byte is written under `into`.
#[cfg(test)]
fn unpack_tar_zst(archive: &Path, into: &Path) -> Result<()> {
    unpack_tar_zst_bytes(&fs::read(archive)?, into)
}

/// Unpack a flat tarball of `*.json` packs, refusing an entry over 4 MiB, a total over 32 MiB
/// or more than 512 entries (nothing is installed from a refused release).
fn unpack_tar_zst_bytes(archive: &[u8], into: &Path) -> Result<()> {
    fs::create_dir_all(into)?;
    let dec = zstd::Decoder::new(archive)?;
    let mut tar = tar::Archive::new(dec);
    let (mut count, mut total) = (0usize, 0u64);
    for entry in tar.entries()? {
        let mut entry = entry?;
        count += 1;
        if count > MAX_PACK_ENTRIES {
            return Err(Error::InvalidPath(format!(
                "seed-pack tarball: more than {MAX_PACK_ENTRIES} entries"
            )));
        }
        // `Entry::size` is the logical size (larger than the stored one for a sparse entry).
        let size = entry.size();
        if size > MAX_PACK_ENTRY_BYTES {
            return Err(Error::InvalidPath(format!(
                "seed-pack tarball: an entry exceeds the 4 MiB limit ({size} bytes)"
            )));
        }
        total += size;
        if total > MAX_PACK_TOTAL_BYTES {
            return Err(Error::InvalidPath(
                "seed-pack tarball: entries exceed the 32 MiB total limit".into(),
            ));
        }
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
        let mut out = fs::File::create(&dest)?;
        let copied = std::io::copy(&mut (&mut entry).take(MAX_PACK_ENTRY_BYTES + 1), &mut out)?;
        if copied > MAX_PACK_ENTRY_BYTES {
            return Err(Error::InvalidPath(
                "seed-pack tarball: an entry exceeds the 4 MiB limit".into(),
            ));
        }
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
        let kp = Ed25519KeyPair::from_seed_unchecked(&seed).expect("test keypair");
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
            tar.append_data(&mut header, format!("{name}.json"), bytes)
                .unwrap();
        }
        tar.finish().unwrap();
        path
    }

    /// A v2 signature (sequence 1) written next to the tarball.
    fn sign_test_tarball(tarball: &Path, kp: &Ed25519KeyPair) -> PathBuf {
        let body = fs::read(tarball).unwrap();
        let sig_path = tarball.with_extension("zst.sig");
        fs::write(&sig_path, sign_v2(&body, 1, kp)).unwrap();
        sig_path
    }

    fn sign_v2(tarball: &[u8], sequence: u64, kp: &Ed25519KeyPair) -> Vec<u8> {
        let sig = kp.sign(&signed_message_v2(sequence, tarball));
        encode_signature_v2(sequence, sig.as_ref())
    }

    /// A tarball held in memory with `(name, body)` entries.
    fn tar_bytes(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
        let mut zenc = zstd::Encoder::new(Vec::new(), 1).unwrap();
        {
            let mut tar = tar::Builder::new(&mut zenc);
            for (name, body) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(body.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                tar.append_data(&mut header, name, body.as_slice()).unwrap();
            }
            tar.finish().unwrap();
        }
        zenc.finish().unwrap()
    }

    fn one_pack() -> Vec<u8> {
        tar_bytes(&[(
            "feather.json".into(),
            br#"{"config_path":"a.json"}"#.to_vec(),
        )])
    }

    #[test]
    fn v2_signature_verifies_and_returns_its_sequence() {
        let (kp, pk) = test_keypair();
        let t = one_pack();
        assert_eq!(
            verify_signature_bytes(&t, &sign_v2(&t, 7, &kp), &pk).unwrap(),
            7
        );
    }

    #[test]
    fn v2_flipped_sequence_byte_fails() {
        let (kp, pk) = test_keypair();
        let t = one_pack();
        let mut sig = sign_v2(&t, 5, &kp);
        sig[5 + 7] ^= 0x01; // last byte of the u64 sequence: claims release 4
        assert!(matches!(
            verify_signature_bytes(&t, &sig, &pk).unwrap_err(),
            Error::SeedPackSignature(_)
        ));
    }

    #[test]
    fn v1_signature_is_refused_naming_v2() {
        let (kp, pk) = test_keypair();
        let t = one_pack();
        let mut h = Sha512::new();
        h.update(&t);
        let mut v1 = b"AICS\x01".to_vec();
        v1.extend_from_slice(kp.sign(&h.finalize()).as_ref());
        let err = verify_signature_bytes(&t, &v1, &pk)
            .unwrap_err()
            .to_string();
        assert!(err.contains("v2") && err.contains("--sequence"), "{err}");
    }

    fn release_dirs(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (tmp.join("seq"), tmp.join("staging"), tmp.join("seeds"))
    }

    #[test]
    fn older_release_is_refused_and_installed_packs_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk) = test_keypair();
        let (seq, staging, target) = release_dirs(tmp.path());
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("installed.json"),
            br#"{"config_path":"i.json"}"#,
        )
        .unwrap();
        fs::write(&seq, "5\n").unwrap();
        let t = one_pack();
        let err = install_verified_release(&t, &sign_v2(&t, 3, &kp), &pk, &seq, &staging, &target)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("seed-pack release 3 is older than installed release 5"),
            "{err}"
        );
        assert!(
            target.join("installed.json").is_file(),
            "installed packs untouched"
        );
        assert!(!target.join("feather.json").exists());
        assert_eq!(fs::read_to_string(&seq).unwrap().trim(), "5");
    }

    #[test]
    fn equal_release_reinstalls_and_newer_records_its_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk) = test_keypair();
        let (seq, staging, target) = release_dirs(tmp.path());
        fs::write(&seq, "5\n").unwrap();
        let t = one_pack();
        install_verified_release(&t, &sign_v2(&t, 5, &kp), &pk, &seq, &staging, &target)
            .expect("equal sequence re-installs");
        assert!(target.join("feather.json").is_file());
        install_verified_release(&t, &sign_v2(&t, 6, &kp), &pk, &seq, &staging, &target)
            .expect("newer installs");
        assert_eq!(fs::read_to_string(&seq).unwrap().trim(), "6");
    }

    #[test]
    fn unpack_caps_refuse_large_entry_total_and_count() {
        let tmp = tempfile::tempdir().unwrap();
        let big = tar_bytes(&[("a.json".into(), vec![b' '; 5 * 1024 * 1024])]);
        let e = unpack_tar_zst_bytes(&big, &tmp.path().join("o1"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("4 MiB"), "{e}");
        let many_large: Vec<(String, Vec<u8>)> = (0..9)
            .map(|i| (format!("p{i}.json"), vec![b' '; 4 * 1024 * 1024 - 1024]))
            .collect();
        let e = unpack_tar_zst_bytes(&tar_bytes(&many_large), &tmp.path().join("o2"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("32 MiB"), "{e}");
        let many: Vec<(String, Vec<u8>)> = (0..513)
            .map(|i| (format!("p{i}.json"), b"{}".to_vec()))
            .collect();
        let e = unpack_tar_zst_bytes(&tar_bytes(&many), &tmp.path().join("o3"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("512"), "{e}");
    }

    #[test]
    fn placeholder_pub_key_blocks_update_without_network_io() {
        // The default const value should be the placeholder shipped
        // in source. update_seed_packs must refuse to proceed before
        // any download attempt.
        assert_eq!(
            SEED_PACKS_PUB_KEY, "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
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
        let tarball = build_test_tarball(tmp.path(), &[("feather", r#"{"config_path":"a.json"}"#)]);
        let sig = sign_test_tarball(&tarball, &kp);
        verify_signature(&tarball, &sig, &pk_b64).expect("valid sig must verify");
    }

    #[test]
    fn verify_signature_rejects_tampered_tarball() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk_b64) = test_keypair();
        let tarball = build_test_tarball(tmp.path(), &[("feather", r#"{"config_path":"a.json"}"#)]);
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
        let archive = build_raw_tarball(tmp.path(), &[("../../etc/evil.json", b"{}".as_slice())]);
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
        let archive = build_raw_tarball(tmp.path(), &[("nested/foo.json", b"{}".as_slice())]);
        let err = unpack_tar_zst(&archive, &tmp.path().join("out")).unwrap_err();
        assert!(err.to_string().contains("non-flat"));
    }

    #[test]
    fn unpack_rejects_non_json_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let archive = build_raw_tarball(tmp.path(), &[("README.md", b"hello".as_slice())]);
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

    #[test]
    fn validate_unpacked_refuses_a_tarball_with_any_refused_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("good.json"),
            r#"{"config_path":"{appimage}.config/g.json"}"#,
        )
        .unwrap();
        fs::write(dir.join("evil.json"), r#"{"config_path":"../../x"}"#).unwrap();
        let err = super::validate_unpacked_packs(&dir)
            .expect_err("an update carrying a refused pack must not install")
            .to_string();
        assert!(err.contains("evil") && err.contains("config_path"), "{err}");
    }

    #[test]
    fn validate_unpacked_counts_good_packs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.json"), r#"{"config_path":"a.json"}"#).unwrap();
        fs::write(dir.join("b.json"), r#"{"config_path":"b.json"}"#).unwrap();
        assert_eq!(super::validate_unpacked_packs(&dir).unwrap(), 2);
    }

    #[test]
    fn unwritable_sequence_record_refuses_with_nothing_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk) = test_keypair();
        let (_, staging, target) = release_dirs(tmp.path());
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("installed.json"),
            br#"{"config_path":"i.json"}"#,
        )
        .unwrap();
        // The record's parent is a file, so the record cannot be written.
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, b"x").unwrap();
        let seq = blocker.join("seed-pack-release-seq");
        let t = one_pack();
        assert!(
            install_verified_release(&t, &sign_v2(&t, 4, &kp), &pk, &seq, &staging, &target)
                .is_err()
        );
        assert!(
            target.join("installed.json").is_file(),
            "installed packs untouched"
        );
        assert!(!target.join("feather.json").exists(), "nothing installed");
    }

    #[test]
    fn corrupt_sequence_record_says_how_to_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk) = test_keypair();
        let (seq, staging, target) = release_dirs(tmp.path());
        fs::write(&seq, b"garbage").unwrap();
        let t = one_pack();
        let err = install_verified_release(&t, &sign_v2(&t, 4, &kp), &pk, &seq, &staging, &target)
            .unwrap_err()
            .to_string();
        assert!(err.contains("delete it"), "{err}");
    }

    #[test]
    fn capped_release_installs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let (kp, pk) = test_keypair();
        let (seq, staging, target) = release_dirs(tmp.path());
        fs::create_dir_all(&target).unwrap();
        fs::write(
            target.join("installed.json"),
            br#"{"config_path":"i.json"}"#,
        )
        .unwrap();
        let big = tar_bytes(&[("a.json".into(), vec![b' '; 5 * 1024 * 1024])]);
        assert!(install_verified_release(
            &big,
            &sign_v2(&big, 1, &kp),
            &pk,
            &seq,
            &staging,
            &target
        )
        .is_err());
        let names: Vec<_> = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["installed.json"]);
        assert!(!seq.exists(), "no sequence recorded");
    }
}
