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
//! aic-sign-pack sign --sequence N ~/.aic-keys/seed-packs.key seed-packs.tar.zst
//!
//! # Verify a signed tarball against an embedded base64 public key
//! # (debugging the signing flow without running the full manager).
//! aic-sign-pack verify <BASE64-PUBKEY> seed-packs.tar.zst
//! ```
//!
//! The signature format (v2) is described in
//! `crates/core/src/seed_pack_update.rs`: a 4-byte magic `AICS`, a 1-byte
//! version `0x02`, the 8-byte big-endian release sequence, and a 64-byte
//! ed25519 signature over `b"AICS-v2\0" || sequence || SHA-512(tarball)`.
//!
//! Private-key file layout: 32 raw bytes (the ed25519 seed). Generate
//! once, store on a cold machine, never copy or commit. Anyone with
//! this file can sign a release tarball that `appimage-control` will
//! accept.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use appimage_control_core::{encode_signature_v2, signed_message_v2, verify_signature_bytes};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("keygen") => match args.get(2) {
            Some(path) => keygen(Path::new(path)),
            None => Err("keygen <KEYFILE>: missing KEYFILE".into()),
        },
        Some("sign") => match parse_sign_args(&args[2..]) {
            Ok((sequence, key, tarball)) => sign(Path::new(&key), Path::new(&tarball), sequence),
            Err(e) => Err(e),
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

/// `sign --sequence N <KEYFILE> <TARBALL>`: the release sequence is required — it is what stops
/// an older signed release from being replayed (format v2).
fn parse_sign_args(args: &[String]) -> Result<(u64, String, String), String> {
    let usage = "sign --sequence N <KEYFILE> <TARBALL>: N is this release's sequence number, \
                 higher than every earlier release";
    let mut sequence: Option<u64> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--sequence" {
            let v = args.get(i + 1).ok_or(usage)?;
            sequence = Some(
                v.parse()
                    .map_err(|_| format!("--sequence: not a number: {v}"))?,
            );
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    match (sequence, rest.as_slice()) {
        (Some(n), [key, tarball]) => Ok((n, key.clone(), tarball.clone())),
        _ => Err(usage.into()),
    }
}

fn sign(keyfile: &Path, tarball: &Path, sequence: u64) -> Result<(), String> {
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
    let sig = kp.sign(&signed_message_v2(sequence, &body));
    let sig_path = sig_path_for(tarball);
    fs::write(&sig_path, encode_signature_v2(sequence, sig.as_ref())).map_err(|e| e.to_string())?;
    eprintln!(
        "Signature (v2, release {sequence}) written to {}.",
        sig_path.display()
    );
    Ok(())
}

/// Verify `<tarball>.sig` against the public key; prints the signed release sequence.
fn verify(pub_b64: &str, tarball: &Path) -> Result<(), String> {
    let sig_path = sig_path_for(tarball);
    let sig_bytes =
        fs::read(&sig_path).map_err(|e| format!("reading {}: {e}", sig_path.display()))?;
    let body = fs::read(tarball).map_err(|e| format!("reading tarball: {e}"))?;
    let sequence = verify_signature_bytes(&body, &sig_bytes, pub_b64).map_err(|e| e.to_string())?;
    println!("{sequence}");
    eprintln!(
        "OK: {} verifies against the supplied public key (release {sequence}).",
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_seed(dir: &Path) -> (std::path::PathBuf, String) {
        let seed = [7u8; 32];
        let key = dir.join("key");
        fs::write(&key, seed).unwrap();
        let kp = Ed25519KeyPair::from_seed_unchecked(&seed).unwrap();
        (key, STANDARD.encode(kp.public_key().as_ref()))
    }

    #[test]
    fn sign_then_verify_round_trips_with_sequence() {
        let tmp = tempfile::tempdir().unwrap();
        let (key, pk) = write_seed(tmp.path());
        let tarball = tmp.path().join("seed-packs.tar.zst");
        fs::write(&tarball, b"not really a tarball, but bytes to sign").unwrap();
        let args: Vec<String> = [
            "--sequence",
            "9",
            key.to_str().unwrap(),
            tarball.to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let (n, k, t) = parse_sign_args(&args).unwrap();
        sign(Path::new(&k), Path::new(&t), n).unwrap();
        verify(&pk, &tarball).unwrap();
        let sig = fs::read(sig_path_for(&tarball)).unwrap();
        let body = fs::read(&tarball).unwrap();
        assert_eq!(verify_signature_bytes(&body, &sig, &pk).unwrap(), 9);
    }

    #[test]
    fn sign_requires_a_sequence() {
        let args: Vec<String> = ["key", "tarball"].iter().map(|s| s.to_string()).collect();
        assert!(parse_sign_args(&args).unwrap_err().contains("--sequence"));
    }
}
