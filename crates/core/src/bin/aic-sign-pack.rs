//! `aic-sign-pack` — generate ed25519 keypairs and sign seed-pack
//! tarballs for the appimage-control-seed-packs companion repo.
//!
//! ## Usage
//!
//! ```sh
//! # One-time, on a cold-storage machine. Writes the private key to
//! # KEYFILE and prints the matching base64 public key on stdout.
//! aic-sign-pack keygen ~/.aic-keys/seed-packs.key
//!
//! # Sign one tarball. Reads private key from KEYFILE, writes
//! # TARBALL.sig next to TARBALL.
//! aic-sign-pack sign ~/.aic-keys/seed-packs.key seed-packs.tar.zst
//!
//! # Verify a signed tarball against an embedded base64 public key
//! # (debugging the signing flow without running the full manager).
//! aic-sign-pack verify <BASE64-PUBKEY> seed-packs.tar.zst
//! ```
//!
//! The signature format is described in
//! `crates/core/src/seed_pack_update.rs` ("AICSIG-1"): a 4-byte magic
//! `AICS`, a 1-byte version `0x01`, and a 64-byte ed25519 signature
//! over `SHA-512(tarball-bytes)`.
//!
//! Private-key file layout: 32 raw bytes (the ed25519 seed). Generate
//! once, store on a cold machine, never copy or commit. Anyone with
//! this file can sign a release tarball that `appimage-control` will
//! accept.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};
use sha2::{Digest, Sha512};

const SIG_MAGIC: &[u8] = b"AICS";
const SIG_VERSION: u8 = 1;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("keygen") => match args.get(2) {
            Some(path) => keygen(Path::new(path)),
            None => Err("keygen <KEYFILE>: missing KEYFILE".into()),
        },
        Some("sign") => match (args.get(2), args.get(3)) {
            (Some(key), Some(tarball)) => sign(Path::new(key), Path::new(tarball)),
            _ => Err("sign <KEYFILE> <TARBALL>: missing argument".into()),
        },
        Some("verify") => match (args.get(2), args.get(3)) {
            (Some(pk), Some(tarball)) => verify(pk, Path::new(tarball)),
            _ => Err("verify <BASE64-PUBKEY> <TARBALL>: missing argument".into()),
        },
        Some(other) => Err(format!("unknown subcommand `{other}`")),
        None => Err("usage: aic-sign-pack {keygen|sign|verify} ...".into()),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("aic-sign-pack: {e}");
            ExitCode::FAILURE
        }
    }
}

fn keygen(keyfile: &Path) -> Result<(), String> {
    if keyfile.exists() {
        return Err(format!(
            "{} already exists; refusing to overwrite",
            keyfile.display()
        ));
    }
    let rng = SystemRandom::new();
    let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|e| e.to_string())?;
    let kp = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).map_err(|e| e.to_string())?;

    // Store the raw 32-byte seed instead of PKCS8 so the keyfile is
    // tiny + easy to back up / paper-print.
    let seed = &pkcs8.as_ref()[16..16 + 32];

    if let Some(parent) = keyfile.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(keyfile, seed).map_err(|e| e.to_string())?;
    set_owner_only_perms(keyfile)?;
    let pub_b64 = STANDARD.encode(kp.public_key().as_ref());
    println!("{pub_b64}");
    eprintln!("Private key written to {} (mode 0600).", keyfile.display());
    eprintln!("Copy the line above into SEED_PACKS_PUB_KEY in");
    eprintln!("crates/core/src/seed_pack_update.rs.");
    Ok(())
}

fn sign(keyfile: &Path, tarball: &Path) -> Result<(), String> {
    let seed = fs::read(keyfile).map_err(|e| format!("reading keyfile: {e}"))?;
    if seed.len() != 32 {
        return Err(format!(
            "keyfile must be 32 bytes (raw ed25519 seed); got {}",
            seed.len()
        ));
    }
    let kp =
        Ed25519KeyPair::from_seed_unchecked(&seed).map_err(|e| format!("parsing keyfile: {e}"))?;

    let body = fs::read(tarball).map_err(|e| format!("reading tarball: {e}"))?;
    let mut hasher = Sha512::new();
    hasher.update(&body);
    let digest = hasher.finalize();
    let sig = kp.sign(&digest);

    let sig_path = sig_path_for(tarball);
    let mut f = fs::File::create(&sig_path).map_err(|e| e.to_string())?;
    f.write_all(SIG_MAGIC).map_err(|e| e.to_string())?;
    f.write_all(&[SIG_VERSION]).map_err(|e| e.to_string())?;
    f.write_all(sig.as_ref()).map_err(|e| e.to_string())?;
    eprintln!("Signature written to {}.", sig_path.display());
    Ok(())
}

fn verify(pub_b64: &str, tarball: &Path) -> Result<(), String> {
    use ring::signature::{UnparsedPublicKey, ED25519};
    let sig_path = sig_path_for(tarball);
    let sig_bytes =
        fs::read(&sig_path).map_err(|e| format!("reading {}: {e}", sig_path.display()))?;
    if sig_bytes.len() != 5 + 64 {
        return Err(format!(
            "signature has wrong size: expected {}, got {}",
            5 + 64,
            sig_bytes.len()
        ));
    }
    if &sig_bytes[..4] != SIG_MAGIC {
        return Err("signature magic mismatch".into());
    }
    if sig_bytes[4] != SIG_VERSION {
        return Err(format!("unsupported signature version {}", sig_bytes[4]));
    }
    let pk_bytes = STANDARD.decode(pub_b64.trim()).map_err(|e| e.to_string())?;
    let body = fs::read(tarball).map_err(|e| format!("reading tarball: {e}"))?;
    let mut hasher = Sha512::new();
    hasher.update(&body);
    let digest = hasher.finalize();
    let pk = UnparsedPublicKey::new(&ED25519, pk_bytes.as_slice());
    pk.verify(digest.as_slice(), &sig_bytes[5..])
        .map_err(|_| "signature does not verify".to_string())?;
    eprintln!(
        "OK: {} verifies against the supplied public key.",
        tarball.display()
    );
    Ok(())
}

fn sig_path_for(tarball: &Path) -> std::path::PathBuf {
    let mut s = tarball.as_os_str().to_owned();
    s.push(".sig");
    std::path::PathBuf::from(s)
}

#[cfg(unix)]
fn set_owner_only_perms(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(p).map_err(|e| e.to_string())?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(p, perms).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_owner_only_perms(_p: &Path) -> Result<(), String> {
    Ok(())
}
