//! Deterministic HTTP-backed tests (no live internet), plus one `#[ignore]`d
//! live check run on demand with `-- --ignored`.

mod common;

use std::fs;

use appimage_control_core::{
    download_to_file, fetch_url_bytes, resolve_icon_image_url, CatalogApp,
};

#[test]
fn download_to_file_writes_served_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let url = common::serve_bytes_once(b"fixture-bytes-a7f3", "application/octet-stream");
    download_to_file(&url, &dest).expect("download");
    assert_eq!(fs::read(&dest).unwrap(), b"fixture-bytes-a7f3");
}

#[test]
fn fetch_url_bytes_loads_local_icon_bytes() {
    let png = include_bytes!("../../gui/assets/catalog-icons/joplin.png");
    let url = common::serve_bytes_once(png, "image/png");
    let got = fetch_url_bytes(&url).expect("bytes");
    assert_eq!(got.len(), png.len());
}

/// A server that advertises Content-Length N but closes the connection after sending fewer
/// bytes must cause `download_to_file` to return an error, leave no final file, and clean
/// up the `.partdownload` scratch file.
#[test]
fn download_to_file_truncated_body_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let part = tmp.path().join("out.bin.partdownload");

    // Send 5 bytes but advertise 100 — simulates a dropped connection.
    let url = common::serve_truncated_once(b"hello", 100);
    let result = download_to_file(&url, &dest);

    assert!(
        result.is_err(),
        "expected an error for truncated download, got Ok"
    );
    assert!(!dest.exists(), "final file must not exist after truncation");
    assert!(
        !part.exists(),
        ".partdownload scratch file must be cleaned up after truncation"
    );
}

/// An exact-length download (Content-Length matches body) must still succeed end-to-end.
#[test]
fn download_to_file_exact_content_length_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let body = b"exact-content-length-fixture";
    // serve_bytes_once already sets Content-Length == body.len().
    let url = common::serve_bytes_once(body, "application/octet-stream");
    download_to_file(&url, &dest).expect("exact-length download should succeed");
    assert_eq!(fs::read(&dest).unwrap(), body);
}

#[test]
fn download_refuses_non_https_non_loopback_url_before_connecting() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let t = std::time::Instant::now();
    let err = download_to_file("http://example.com/x", &dest).unwrap_err();
    assert!(t.elapsed() < std::time::Duration::from_secs(1));
    assert!(
        err.to_string()
            .contains("refusing non-https download URL http://example.com/x"),
        "{err}"
    );
    assert!(!dest.exists());
}

#[test]
fn download_refuses_redirect_to_non_https_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let url = common::serve_redirect_once("http://example.com/");
    let err = download_to_file(&url, &dest).unwrap_err();
    let text = err.to_string();
    assert!(
        text.contains("redirect") && text.contains("example.com"),
        "{text}"
    );
    assert!(!dest.exists());
}

#[test]
fn download_connect_failure_carries_cause_chain() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("out.bin");
    let err = download_to_file(&common::closed_port_url(), &dest).unwrap_err();
    let text = err.to_string();
    assert!(text.matches(": ").count() >= 2, "{text}");
}

#[test]
fn resolve_icon_image_url_prefers_explicit_icon_url() {
    let url = common::serve_bytes_once(b"x", "image/png");
    let app = CatalogApp {
        id: "fixture".into(),
        display_name: None,
        homepage: None,
        download_url: None,
        download_url_aarch64: None,
        appimage_filename: None,
        instance_strategy: Default::default(),
        instance_exec_args_template: None,
        instance_name_template: None,
        desktop_name_template: None,
        reset_config_dirs: vec![],
        reset_cache_dirs: vec![],
        reset_data_dirs: vec![],
        icon_url: Some(url.clone()),
        wayland_app_id: None,
        desktop_pass_file_urls: None,
        instance_subdir: None,
        isolate_dbus_session: None,
        max_instances: None,
        max_instances_tooltip: None,
        category: None,
        sha256: None,
        sha256_aarch64: None,
    };
    assert_eq!(resolve_icon_image_url(&app).as_deref(), Some(url.as_str()));
}

/// Live check (needs internet): the vendored TLS stack still reaches a real
/// GitHub release asset. Run with `-- --ignored live_github_download`.
#[test]
#[ignore = "live network: downloads a ~49 MB GitHub release asset"]
fn live_github_download() {
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("KeePassXC.AppImage");
    download_to_file(
        "https://github.com/keepassxreboot/keepassxc/releases/download/2.7.12/KeePassXC-2.7.12-x86_64.AppImage",
        &dest,
    )
    .expect("live download");
    let head = fs::read(&dest).unwrap();
    assert!(
        head.len() > 1_000_000,
        "suspiciously small: {} bytes",
        head.len()
    );
    assert_eq!(&head[..4], b"\x7fELF", "not an ELF executable");
}

// --- Task 3.3: a download is verified before anything is made executable -----------------

use appimage_control_core::{
    ensure_cached_appimage_with_policy, parse_catalog_json, AppPaths, Error, VerifyPolicy,
    CACHE_SHA256_FILENAME,
};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

fn sha256_hex(b: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(b);
    h.finalize().iter().map(|x| format!("{x:02x}")).collect()
}

/// A catalog app downloading from `url` into `t.AppImage`, with an optional pinned sha256.
fn verify_app(url: &str, sha256: Option<&str>) -> CatalogApp {
    let pin = sha256
        .map(|h| format!(r#","sha256":"{h}","sha256_aarch64":"{h}""#))
        .unwrap_or_default();
    let json = format!(
        r#"{{"apps":[{{"id":"vapp","download_url":"{url}","download_url_aarch64":"{url}","appimage_filename":"t.AppImage"{pin}}}]}}"#
    );
    parse_catalog_json(&json).unwrap().apps.remove(0)
}

fn strict() -> VerifyPolicy {
    VerifyPolicy {
        allow_unverified: false,
    }
}

fn allowed() -> VerifyPolicy {
    VerifyPolicy {
        allow_unverified: true,
    }
}

fn no_residue(dir: &std::path::Path) {
    let names: Vec<String> = fs::read_dir(dir)
        .map(|r| {
            r.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !names
            .iter()
            .any(|n| n.contains("partdownload") || n.contains("unverified")),
        "temporary download files left behind: {names:?}"
    );
}

#[test]
fn verify_matching_catalog_hash_installs_executable() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let body = b"\x7fELF verified payload";
    let url = common::serve_bytes_once(body, "application/octet-stream");
    let app = verify_app(&url, Some(&sha256_hex(body)));
    let p = ensure_cached_appimage_with_policy(&paths, &app, strict(), &mut |_| {}).unwrap();
    assert_eq!(fs::read(&p).unwrap(), body);
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let side =
        fs::read_to_string(paths.catalog_cache_dir("vapp").join(CACHE_SHA256_FILENAME)).unwrap();
    assert_eq!(
        side.trim(),
        format!("{} verified catalog", sha256_hex(body))
    );
}

#[test]
fn verify_mismatch_refuses_and_keeps_existing_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let dir = paths.catalog_cache_dir("vapp");
    fs::create_dir_all(&dir).unwrap();
    // A pre-existing cached blob that does not match the pin either, so a download happens.
    let old = b"old cached bytes";
    fs::write(dir.join("t.AppImage"), old).unwrap();
    let served = b"\x7fELF tampered upstream";
    let url = common::serve_bytes_once(served, "application/octet-stream");
    let expected = sha256_hex(b"what upstream published");
    let app = verify_app(&url, Some(&expected));
    let err = ensure_cached_appimage_with_policy(&paths, &app, allowed(), &mut |_| {})
        .expect_err("a mismatch must be refused, even with --allow-unverified");
    assert!(matches!(err, Error::DigestMismatch { .. }), "{err:?}");
    let msg = err.to_string();
    assert!(
        msg.contains("vapp: download does not match the expected sha256")
            && msg.contains(&expected)
            && msg.contains(&sha256_hex(served))
            && msg.contains("nothing was installed"),
        "{msg}"
    );
    assert_eq!(
        fs::read(dir.join("t.AppImage")).unwrap(),
        old,
        "cached blob must be untouched"
    );
    no_residue(&dir);
}

#[test]
fn verify_mismatch_without_cache_leaves_no_file() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let url = common::serve_bytes_once(b"\x7fELF bad", "application/octet-stream");
    let app = verify_app(&url, Some(&sha256_hex(b"good")));
    let err = ensure_cached_appimage_with_policy(&paths, &app, strict(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::DigestMismatch { .. }), "{err:?}");
    let dir = paths.catalog_cache_dir("vapp");
    assert!(!dir.join("t.AppImage").exists());
    no_residue(&dir);
}

#[test]
fn verify_unavailable_strict_refuses_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let url = common::serve_bytes_once(b"\x7fELF no hash", "application/octet-stream");
    let app = verify_app(&url, None);
    let err = ensure_cached_appimage_with_policy(&paths, &app, strict(), &mut |_| {}).unwrap_err();
    assert!(matches!(err, Error::UnverifiedDownload { .. }), "{err:?}");
    assert!(
        err.to_string().contains(
            "vapp: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway"
        ),
        "{err}"
    );
    let dir = paths.catalog_cache_dir("vapp");
    assert!(!dir.join("t.AppImage").exists());
    no_residue(&dir);
}

#[test]
fn verify_unavailable_allowed_installs_and_marks_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let body = b"\x7fELF user allowed";
    let url = common::serve_bytes_once(body, "application/octet-stream");
    let app = verify_app(&url, None);
    let p = ensure_cached_appimage_with_policy(&paths, &app, allowed(), &mut |_| {}).unwrap();
    assert_eq!(fs::read(&p).unwrap(), body);
    assert_eq!(
        fs::metadata(&p).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let side =
        fs::read_to_string(paths.catalog_cache_dir("vapp").join(CACHE_SHA256_FILENAME)).unwrap();
    assert_eq!(
        side.trim(),
        format!("{} unverified user-allowed", sha256_hex(body))
    );
}

#[test]
fn verify_tampered_cache_is_redownloaded() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let dir = paths.catalog_cache_dir("vapp");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("t.AppImage"), b"tampered after verification").unwrap();
    let body = b"\x7fELF the real thing";
    let url = common::serve_bytes_once(body, "application/octet-stream");
    let app = verify_app(&url, Some(&sha256_hex(body)));
    let p = ensure_cached_appimage_with_policy(&paths, &app, strict(), &mut |_| {}).unwrap();
    assert_eq!(
        fs::read(&p).unwrap(),
        body,
        "the tampered blob must not be reused"
    );
}

#[test]
fn verify_sidecar_mismatch_is_redownloaded() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let dir = paths.catalog_cache_dir("vapp");
    fs::create_dir_all(&dir).unwrap();
    // Verified once (allowed, no pin), then the blob changed under the sidecar.
    fs::write(dir.join("t.AppImage"), b"changed later").unwrap();
    fs::write(
        dir.join(CACHE_SHA256_FILENAME),
        format!(
            "{} unverified user-allowed\n",
            sha256_hex(b"what was allowed")
        ),
    )
    .unwrap();
    let body = b"\x7fELF fresh copy";
    let url = common::serve_bytes_once(body, "application/octet-stream");
    let app = verify_app(&url, None);
    let p = ensure_cached_appimage_with_policy(&paths, &app, allowed(), &mut |_| {}).unwrap();
    assert_eq!(fs::read(&p).unwrap(), body);
}

#[test]
fn verify_corrupt_sidecar_is_not_trusted() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let dir = paths.catalog_cache_dir("vapp");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("t.AppImage"), b"blob behind a damaged record").unwrap();
    // A write cut short: an empty sidecar must not read as "cached before verification".
    fs::write(dir.join(CACHE_SHA256_FILENAME), b"").unwrap();
    let body = b"\x7fELF fresh after corrupt record";
    let url = common::serve_bytes_once(body, "application/octet-stream");
    let app = verify_app(&url, None);
    let p = ensure_cached_appimage_with_policy(&paths, &app, allowed(), &mut |_| {}).unwrap();
    assert_eq!(fs::read(&p).unwrap(), body);
}
