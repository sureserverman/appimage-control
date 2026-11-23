//! Contract tests for the shipped `data/catalog.json` (strategies, limits, URLs).

mod common;

use std::fs;

use appimage_control_core::{
    parse_catalog_json, Catalog, CatalogApp, InstanceStrategy, MAX_INSTANCES_PER_APP,
};

fn catalog_has_strategy(catalog: &Catalog, strategy: InstanceStrategy) -> bool {
    catalog.apps.iter().any(|a| a.instance_strategy == strategy)
}

#[test]
fn shipped_catalog_loads_and_covers_instance_strategies() {
    let path = common::shipped_catalog_path();
    assert!(
        path.is_file(),
        "missing shipped catalog at {}",
        path.display()
    );
    let raw = fs::read_to_string(&path).expect("read catalog");
    let catalog: Catalog = parse_catalog_json(&raw).expect("deserialize and validate catalog");

    for app in &catalog.apps {
        if matches!(std::env::consts::ARCH, "x86_64" | "x86") {
            app.resolved_download_url()
                .unwrap_or_else(|e| panic!("{} needs an x86_64 download_url: {e}", app.id));
        }
    }

    for strategy in [
        InstanceStrategy::QtX11,
        InstanceStrategy::ElectronClass,
        InstanceStrategy::KeepassxcIsolated,
        InstanceStrategy::ElectronClassIsolated,
        InstanceStrategy::CryptomatorExtract,
        InstanceStrategy::JoplinAltInstance,
    ] {
        assert!(
            catalog_has_strategy(&catalog, strategy),
            "shipped catalog should include at least one app with {strategy:?}"
        );
    }
}

#[test]
fn electrum_btc_compact_layout_and_desktop_flags() {
    let path = common::shipped_catalog_path();
    let catalog = Catalog::load_from_path(&path).expect("load");
    let app = catalog
        .by_id()
        .get("electrum-btc")
        .copied()
        .expect("electrum-btc in catalog");
    assert_eq!(app.instance_filesystem_dirname(), "ebtc");
    assert!(!app.desktop_pass_file_urls());
}

#[test]
fn lm_studio_instance_cap_and_tooltip() {
    let path = common::shipped_catalog_path();
    let catalog = Catalog::load_from_path(&path).expect("load");
    let app = catalog
        .by_id()
        .get("lm-studio")
        .copied()
        .expect("lm-studio in catalog");
    assert_eq!(app.max_instances_allowed(), 1);
    assert!(app.instance_limit_tooltip_message().is_some());
    assert!(app.max_instances.unwrap_or(0) <= MAX_INSTANCES_PER_APP);
}

#[test]
fn standard_notes_display_name_fallback_would_work_without_display_name() {
    let app = CatalogApp {
        id: "standard-notes".into(),
        display_name: None,
        homepage: None,
        download_url: Some("https://example/x.AppImage".into()),
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
    assert_eq!(app.display_name_resolved(), "standard notes");
}

/// Feather's GitHub release carries no asset digest, so the catalog pins the hashes from
/// Feather's PGP-signed list in the 2.8.1 release notes (DEC-001).
#[test]
fn feather_pins_the_signed_release_hashes() {
    let path = common::shipped_catalog_path();
    let catalog = Catalog::load_from_path(&path).expect("load");
    let app = catalog
        .by_id()
        .get("feather")
        .copied()
        .expect("feather in catalog");
    assert_eq!(
        app.sha256.as_deref(),
        Some("c5e95c68ea932067c269d9aa6073450326ec845efba2dfe1e71d8de3d89c15f4"),
        "feather-2.8.1.AppImage"
    );
    assert_eq!(
        app.sha256_aarch64.as_deref(),
        Some("ed6e8e885539ee2a76e49ef6cbbeda8586959ae5ad30432f102669a83081aecd"),
        "feather-2.8.1-arm64.AppImage"
    );
}

/// electrum-ltc.org no longer answers, so the catalog downloads the Wayback Machine's raw
/// (`id_`) capture of the same file. The pinned hash is the original's and does not change.
#[test]
fn electrum_ltc_downloads_the_archived_hash_pinned_appimage() {
    let path = common::shipped_catalog_path();
    let catalog = Catalog::load_from_path(&path).expect("load");
    let app = catalog
        .by_id()
        .get("electrum-ltc")
        .copied()
        .expect("electrum-ltc in catalog");
    assert_eq!(
        app.download_url.as_deref(),
        Some("https://web.archive.org/web/20261020143408id_/https://electrum-ltc.org/download/electrum-ltc-4.2.2.1-x86_64.AppImage")
    );
    assert_eq!(
        app.sha256.as_deref(),
        Some("8c27621f87a51baf5b3a492696606a5b55c72b6a9804e3baa8f161cd7cc5d8f5")
    );
}
