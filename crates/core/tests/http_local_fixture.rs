//! Deterministic HTTP-backed tests (no live internet).

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
    };
    assert_eq!(resolve_icon_image_url(&app).as_deref(), Some(url.as_str()));
}
