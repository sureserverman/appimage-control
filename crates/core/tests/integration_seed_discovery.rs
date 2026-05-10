//! Disk-override discovery for seed packs.
//!
//! Verifies that `APPIMAGE_CONTROL_SEEDS_DIR` and the cwd ancestor walk
//! win over the embedded slice baked in by `build.rs`, and that an
//! override can introduce a new app id (so curating the supported set
//! at runtime is a one-file drop, no rebuild needed).

mod common;

use std::fs;
use std::sync::{Mutex, OnceLock};

use appimage_control_core::{recipe_for_supported_app, CatalogApp, InstanceStrategy};

/// Tests in this binary mutate `APPIMAGE_CONTROL_SEEDS_DIR`. Cargo runs
/// integration tests across multiple threads inside one process, so
/// every env-touching test must hold this mutex.
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// Build a minimal CatalogApp suitable for `recipe_for_supported_app`
/// — only `id` is consulted by the lookup path.
fn app(id: &str) -> CatalogApp {
    CatalogApp {
        id: id.into(),
        display_name: Some(id.into()),
        homepage: None,
        download_url: None,
        download_url_aarch64: None,
        appimage_filename: Some(format!("{id}.AppImage")),
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
    }
}

#[test]
fn env_var_override_replaces_embedded_pack_for_known_app() {
    let _g = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    // Ship a pack file under app id "feather" with a clearly-distinct
    // first_run_args list and a sentinel rotation, so we can tell at a
    // glance whether discover_load picked us up over the embedded pack.
    let pack_dir = tmp.path().to_path_buf();
    fs::write(
        pack_dir.join("feather.json"),
        r#"{
            "config_path":"sentinel.json",
            "first_run_args":["--from-disk-override"]
        }"#,
    )
    .unwrap();

    let prev = std::env::var("APPIMAGE_CONTROL_SEEDS_DIR").ok();
    std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", &pack_dir);

    let recipe = recipe_for_supported_app(&app("feather")).expect("feather pack must resolve");
    assert_eq!(recipe.first_run_args, vec!["--from-disk-override"]);
    assert_eq!(recipe.config_path_doc, "sentinel.json");

    match prev {
        Some(v) => std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", v),
        None => std::env::remove_var("APPIMAGE_CONTROL_SEEDS_DIR"),
    }
}

#[test]
fn env_var_override_introduces_new_supported_app_id() {
    let _g = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let pack_dir = tmp.path().to_path_buf();
    // Ship an entirely new pack id not present in the embedded slice.
    fs::write(
        pack_dir.join("brand-new-app.json"),
        r#"{"config_path":"x.json"}"#,
    )
    .unwrap();

    let prev = std::env::var("APPIMAGE_CONTROL_SEEDS_DIR").ok();
    std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", &pack_dir);

    let recipe = recipe_for_supported_app(&app("brand-new-app"));
    assert!(
        recipe.is_some(),
        "discover_load should pick up a new pack id from the disk override"
    );

    match prev {
        Some(v) => std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", v),
        None => std::env::remove_var("APPIMAGE_CONTROL_SEEDS_DIR"),
    }
}

#[test]
fn no_override_falls_back_to_shipped_packs() {
    let _g = env_lock().lock().unwrap();
    let prev = std::env::var("APPIMAGE_CONTROL_SEEDS_DIR").ok();
    std::env::remove_var("APPIMAGE_CONTROL_SEEDS_DIR");

    // The shipped library covers feather, electrum-btc, electrum-ltc.
    // (When the test process happens to live inside a checkout whose
    // cwd-ancestor walk hits `data/seeds/`, that's the *same* set —
    // they're the canonical files and the embedded slice is built
    // from them.)
    let recipe =
        recipe_for_supported_app(&app("feather")).expect("feather is shipped");
    assert!(!recipe.config_path_doc.is_empty());
    assert!(recipe_for_supported_app(&app("nonexistent-app")).is_none());

    if let Some(v) = prev {
        std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", v);
    }
}
