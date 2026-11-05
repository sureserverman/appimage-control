//! Contract test for the companion-repo seed packs at
//! `anon-tools/appimage-control-seed-packs/`. Skipped on hosts where
//! the companion repo isn't checked out alongside this one.
//!
//! Verifies:
//! - Every `*.json` parses through serde with `deny_unknown_fields`.
//! - Every pack passes the loader's validation, which refuses the removed
//!   `preset` section and `port` verb by name.
//! - The three apps we're known to ship (`feather`, `electrum-btc`,
//!   `electrum-ltc`) declare `proxy_setup`, so portmux writes their
//!   proxy settings.

use std::path::PathBuf;

use appimage_control_core::SeedPackLibrary;

fn companion_pack_dir() -> Option<PathBuf> {
    // 1. Explicit env override.
    if let Ok(p) = std::env::var("APPIMAGE_CONTROL_COMPANION_PACK_DIR") {
        let pb = PathBuf::from(p);
        if pb.is_dir() {
            return Some(pb);
        }
    }
    // 2. Sibling checkout under the same parent as appimage-control.
    let here = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let manifest_dir = PathBuf::from(here);
    // crates/core → crates → appimage-control → anon-tools → companion repo
    let candidate = manifest_dir
        .ancestors()
        .nth(3)?
        .join("appimage-control-seed-packs");
    if candidate.is_dir() {
        Some(candidate)
    } else {
        None
    }
}

#[test]
fn companion_seed_packs_parse_and_validate() {
    let Some(dir) = companion_pack_dir() else {
        eprintln!(
            "skipping: companion seed-pack repo not found \
             (set APPIMAGE_CONTROL_COMPANION_PACK_DIR to test)"
        );
        return;
    };

    let lib = SeedPackLibrary::load_from_dir(&dir)
        .expect("load companion packs")
        .expect("companion repo has no *.json packs");
    let refused: Vec<_> = lib.refused().collect();
    assert!(refused.is_empty(), "companion packs refused: {refused:?}");
    let rows = lib.list_rows();
    assert!(!rows.is_empty(), "expected at least one companion pack");

    for (id, src) in &rows {
        assert!(!id.is_empty(), "pack id should be non-empty (from {src})");
    }
}

#[test]
fn companion_packs_declare_proxy_setup() {
    let Some(dir) = companion_pack_dir() else {
        eprintln!("skipping: companion seed-pack repo not found");
        return;
    };
    let lib = SeedPackLibrary::load_from_dir(&dir)
        .expect("load companion packs")
        .expect("companion repo has no *.json packs");
    let refused: Vec<_> = lib.refused().collect();
    assert!(refused.is_empty(), "companion packs refused: {refused:?}");
    for id in ["feather", "electrum-btc", "electrum-ltc"] {
        let pack = lib
            .get(id)
            .unwrap_or_else(|| panic!("companion repo has no `{id}` pack"));
        assert!(
            pack.proxy_setup.is_some(),
            "{id} has no proxy_setup, so seeding would leave it with no proxy"
        );
    }
}
