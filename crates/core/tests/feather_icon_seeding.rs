//! Integration coverage for data-driven per-instance icons
//! (`SeedPack.icons.rotation`). Runs as its own process, so toggling
//! `APPIMAGE_CONTROL_SEEDS_DIR` here can't race the in-process unit
//! tests. See plan `2026-06-13-feather-icon-data-driven-plan.md`.

use std::fs;

use appimage_control_core::{
    instance_icon_theme_name, write_instance_icon_png, AppPaths, CatalogApp, ENV_SEEDS_DIR,
};

/// Serialize access to the process-global seeds-dir env var across the
/// tests in this binary.
fn env_guard() -> std::sync::MutexGuard<'static, ()> {
    static L: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

fn app(json: &str) -> CatalogApp {
    serde_json::from_str(json).expect("catalog app json")
}

/// 256px hicolor PNG path the writer uses for `app` instance `idx`.
fn written_256(paths: &AppPaths, app: &CatalogApp, idx: u32) -> std::path::PathBuf {
    paths
        .user_hicolor_apps_dir(256)
        .join(format!("{}.png", instance_icon_theme_name(app, idx)))
}

#[test]
fn feather_per_instance_icons_come_from_seed_pack_rotation() {
    let _g = env_guard();
    let seeds = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    // Three distinct, intentionally-undecodable blobs: scale_png_to_square
    // fails to decode and falls back to the raw bytes, so the written
    // file equals the blob — lets us assert exactly which slot landed.
    let blob = |n: u8| -> Vec<u8> { format!("BLOB-FEATHER-SLOT-{n}-not-a-real-png").into_bytes() };
    for (name, n) in [("feather1.png", 1u8), ("feather2.png", 2), ("feather3.png", 3)] {
        fs::write(seeds.path().join(name), blob(n)).unwrap();
    }
    fs::write(
        seeds.path().join("feather.json"),
        r#"{"config_path":"x","icons":{"rotation":["feather1.png","feather2.png","feather3.png"]}}"#,
    )
    .unwrap();

    std::env::set_var(ENV_SEEDS_DIR, seeds.path());
    let paths = AppPaths::new(home.path().to_path_buf()).unwrap();
    let feather = app(r#"{"id":"feather","instance_strategy":"qt_x11"}"#);

    for idx in 1..=4 {
        let got = write_instance_icon_png(&paths, &feather, idx, None).unwrap();
        assert!(got.is_some(), "instance {idx} should get a pack icon");
    }
    std::env::remove_var(ENV_SEEDS_DIR);

    let read256 = |idx: u32| fs::read(written_256(&paths, &feather, idx)).unwrap();
    // Rotation 1→slot1, 2→slot2, 3→slot3, 4 wraps to slot1.
    assert_eq!(read256(1), blob(1));
    assert_eq!(read256(2), blob(2));
    assert_eq!(read256(3), blob(3));
    assert_eq!(read256(4), blob(1));
    // The point of rotation: adjacent instances differ.
    assert_ne!(read256(1), read256(2));
}

#[test]
fn missing_pack_icon_file_degrades_to_none() {
    let _g = env_guard();
    let seeds = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();

    // Pack declares an icon, but the referenced file is absent. With no
    // homepage / icon_url / download_url the app has no other icon
    // source, so the writer must return Ok(None) rather than panic or
    // hit the network.
    fs::write(
        seeds.path().join("degradeapp.json"),
        r#"{"config_path":"x","icons":{"rotation":["missing.png"]}}"#,
    )
    .unwrap();

    std::env::set_var(ENV_SEEDS_DIR, seeds.path());
    let paths = AppPaths::new(home.path().to_path_buf()).unwrap();
    let degrade = app(r#"{"id":"degradeapp","instance_strategy":"none"}"#);
    let got = write_instance_icon_png(&paths, &degrade, 1, None).unwrap();
    std::env::remove_var(ENV_SEEDS_DIR);

    assert_eq!(got, None);
}
