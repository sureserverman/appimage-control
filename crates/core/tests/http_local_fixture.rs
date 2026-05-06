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
    };
    assert_eq!(resolve_icon_image_url(&app).as_deref(), Some(url.as_str()));
}
