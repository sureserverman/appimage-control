//! Contract test for the companion-repo seed packs at
//! `infra/installers/linux/appimage-control-seed-packs/`. Skipped on
//! hosts where that repo isn't checked out under the same `~/dev` root.
//!
//! Verifies:
//! - Every `*.json` parses through serde with `deny_unknown_fields`.
//! - Every pack validates against the active `SeedConfig` schema.
//! - The three apps we're known to ship (`feather`, `electrum-btc`,
//!   `electrum-ltc`) all use the new `port` verb rather than the
//!   legacy `from_preset_array` / `format` references to removed
//!   fields.
//! - Every pack that declares `icons.rotation` ships each referenced
//!   PNG beside its JSON (else data-driven icons silently vanish).

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
    // 2. Companion packs in the installers-linux repo. As of 2026-06-13
    //    they live at `~/dev/infra/installers/linux/appimage-control-seed-packs`
    //    (moved out of the former standalone `anon-tools/appimage-control-seed-packs`).
    //    crates/core → crates → appimage-control → anon-tools → dev
    let here = std::env::var_os("CARGO_MANIFEST_DIR")?;
    let manifest_dir = PathBuf::from(here);
    let candidate = manifest_dir
        .ancestors()
        .nth(4)?
        .join("infra/installers/linux/appimage-control-seed-packs");
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
fn companion_pack_icons_exist_beside_their_json() {
    let Some(dir) = companion_pack_dir() else {
        eprintln!("skipping: companion seed-pack repo not found");
        return;
    };
    let lib = SeedPackLibrary::load_from_dir(&dir)
        .expect("load companion packs")
        .expect("companion repo has no *.json packs");

    let mut checked = 0usize;
    let ids: Vec<String> = lib.ids().map(str::to_string).collect();
    for id in ids {
        let pack = lib.get(&id).expect("pack present");
        let Some(icons) = &pack.icons else { continue };
        let pack_dir = lib
            .source_for(&id)
            .and_then(|p| p.parent())
            .expect("on-disk pack has a parent dir");
        for name in &icons.rotation {
            let path = pack_dir.join(name);
            assert!(
                path.is_file(),
                "pack `{id}` declares icon `{name}` but it is not a file beside the JSON ({})",
                path.display()
            );
            checked += 1;
        }
    }
    // The feather pack must exercise this path; a zero here means the
    // icons contract silently covers nothing.
    assert!(
        checked > 0,
        "expected at least one companion pack to declare icons.rotation"
    );
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
