//! Unsupported / partial catalog states remain parseable and predictable.

use appimage_control_core::{Catalog, CatalogApp, InstanceStrategy};

#[test]
fn app_with_no_urls_is_valid_but_not_installable_on_any_arch() {
    let app = CatalogApp {
        id: "offline-only".into(),
        display_name: Some("Offline".into()),
        homepage: None,
        download_url: None,
        download_url_aarch64: None,
        appimage_filename: None,
        instance_strategy: InstanceStrategy::None,
        instance_exec_args_template: None,
        instance_name_template: None,
        desktop_name_template: None,
        reset_config_dirs: vec![],
        reset_cache_dirs: vec![],
        reset_data_dirs: vec![],
        icon_url: None,
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
    assert!(!app.has_any_download_url());
    assert!(app.resolved_download_url().is_err());
}

#[test]
fn catalog_json_with_empty_apps_array_loads() {
    let c: Catalog = serde_json::from_str(r#"{"apps":[]}"#).expect("parse");
    assert!(c.apps.is_empty());
}
