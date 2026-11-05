//! Disk-override discovery for seed packs.
//!
//! This repo ships only the seed-pack *architecture* (loader,
//! interpreter, verifier, CLI surface). The embedded slice baked in
//! by `build.rs` is empty by default, and authorised users install
//! actual packs at runtime via
//! `appimage-control --update-seed-packs` (from an operator-curated
//! companion repo) or by dropping files into
//! `~/.local/share/appimage-control/seeds/` or
//! `$APPIMAGE_CONTROL_SEEDS_DIR`.
//!
//! These tests verify the discovery chain works against synthetic
//! packs without depending on any specific app id being shipped.

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
        category: None,
        sha256: None,
        sha256_aarch64: None,
    }
}

#[test]
fn env_var_override_supplies_a_pack_for_a_synthetic_app_id() {
    let _g = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let pack_dir = tmp.path().to_path_buf();
    fs::write(
        pack_dir.join("aic-disco-test-app.json"),
        r#"{
            "config_path":"sentinel.json",
            "first_run_args":["--from-disk-override"]
        }"#,
    )
    .unwrap();

    let prev = std::env::var("APPIMAGE_CONTROL_SEEDS_DIR").ok();
    std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", &pack_dir);

    let recipe = recipe_for_supported_app(&app("aic-disco-test-app"))
        .expect("override-supplied pack must resolve");
    assert_eq!(recipe.first_run_args, vec!["--from-disk-override"]);
    assert_eq!(recipe.config_path_doc, "sentinel.json");

    match prev {
        Some(v) => std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", v),
        None => std::env::remove_var("APPIMAGE_CONTROL_SEEDS_DIR"),
    }
}

#[test]
fn env_var_override_introduces_an_otherwise_unsupported_app_id() {
    let _g = env_lock().lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let pack_dir = tmp.path().to_path_buf();
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
fn unknown_app_id_returns_no_recipe() {
    let _g = env_lock().lock().unwrap();
    let prev = std::env::var("APPIMAGE_CONTROL_SEEDS_DIR").ok();
    // Point env at an empty dir so the chain reliably falls through
    // to the embedded slice (which is empty in this repo by design).
    let empty = tempfile::tempdir().unwrap();
    fs::create_dir_all(empty.path()).unwrap();
    std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", empty.path());

    assert!(
        recipe_for_supported_app(&app("an-id-with-no-pack-anywhere")).is_none(),
        "unknown app id must yield no recipe when no override supplies one"
    );

    match prev {
        Some(v) => std::env::set_var("APPIMAGE_CONTROL_SEEDS_DIR", v),
        None => std::env::remove_var("APPIMAGE_CONTROL_SEEDS_DIR"),
    }
}
