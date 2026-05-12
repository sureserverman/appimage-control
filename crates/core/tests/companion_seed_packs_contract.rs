//! Contract test for the companion-repo seed packs at
//! `anon-tools/appimage-control-seed-packs/`. Skipped on hosts where
//! the companion repo isn't checked out alongside this one.
//!
//! Verifies:
//! - Every `*.json` parses through serde with `deny_unknown_fields`.
//! - Every pack validates against the active `SeedConfig` schema.
//! - The three apps we're known to ship (`feather`, `electrum-btc`,
//!   `electrum-ltc`) all use the new `port` verb rather than the
//!   legacy `from_preset_array` / `format` references to removed
//!   fields.

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
    let rows = lib.list_rows();
    assert!(!rows.is_empty(), "expected at least one companion pack");

    for (id, src) in &rows {
        assert!(!id.is_empty(), "pack id should be non-empty (from {src})");
    }
}

#[test]
fn companion_packs_use_port_verb_not_legacy_port_fields() {
    let Some(dir) = companion_pack_dir() else {
        eprintln!("skipping: companion seed-pack repo not found");
        return;
    };
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path).unwrap();
        for legacy in ["feather_ports", "electrum_btc_port", "electrum_ltc_port"] {
            assert!(
                !raw.contains(legacy),
                "{} still references legacy preset field `{legacy}`; \
                 migrate to the `port` verb",
                path.display()
            );
        }
    }
}
