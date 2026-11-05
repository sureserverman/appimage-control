//! A `data/catalog.json` or `data/seeds/` in the current directory (or its parents) must not be
//! loaded: the GUI starts with cwd = `$HOME`, so a planted `~/data/catalog.json` would otherwise
//! win over the installed or embedded catalog.
//!
//! This is its own test binary because it changes the process cwd; its two tests share a mutex
//! so they never race each other.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use appimage_control_core::{Catalog, SeedPackLibrary, ENV_SEEDS_DIR};

static CWD_LOCK: Mutex<()> = Mutex::new(());

/// Runs `f` with the process cwd set to `dir`, restoring the old cwd afterwards.
fn with_cwd<T>(dir: &std::path::Path, f: impl FnOnce() -> T) -> T {
    let prev: PathBuf = std::env::current_dir().expect("cwd");
    std::env::set_current_dir(dir).expect("chdir");
    let out = f();
    std::env::set_current_dir(prev).expect("restore cwd");
    out
}

#[test]
fn catalog_ignores_decoy_in_current_dir() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("data")).unwrap();
    fs::write(
        tmp.path().join("data").join("catalog.json"),
        r#"{"apps":[{"id":"cwd-decoy-app","download_url":"https://example.com/d.AppImage"}]}"#,
    )
    .unwrap();
    // Start the walk from a subdirectory too, so an ancestor walk would also reach it.
    let sub = tmp.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    std::env::remove_var("APPIMAGE_CONTROL_CATALOG");

    let c = with_cwd(&sub, || Catalog::discover_load(None)).expect("a catalog loads");
    assert!(
        !c.apps.iter().any(|a| a.id == "cwd-decoy-app"),
        "the current-directory decoy catalog was loaded"
    );
    assert!(!c.apps.is_empty(), "a real catalog still loads");
}

#[test]
fn seed_packs_ignore_decoy_in_current_dir() {
    let _g = CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let seeds = tmp.path().join("data").join("seeds");
    fs::create_dir_all(&seeds).unwrap();
    fs::write(
        seeds.join("cwd-decoy-app.json"),
        r#"{"config_path":"{appimage}.config/decoy/settings.json"}"#,
    )
    .unwrap();
    let sub = tmp.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    // Keep the earlier sources empty so the walk would reach the decoy: no env override, and a
    // per-user data dir with no seeds in it.
    std::env::remove_var(ENV_SEEDS_DIR);
    let empty_data = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_DATA_HOME", empty_data.path());

    let lib = with_cwd(&sub, || SeedPackLibrary::discover_load(None)).expect("library loads");
    assert!(
        lib.get("cwd-decoy-app").is_none(),
        "the current-directory decoy seed pack was loaded"
    );
}
