//! The Flatpak manifest's host folder grants.
//!
//! Flatpak binds a `--filesystem=` path into the sandbox only if it exists on the
//! host, unless the grant ends in `:create`. On a fresh account
//! `~/.local/share/appimages` does not exist, so without `:create` an install
//! wrote the AppImage into the sandbox's own temporary home, reported success,
//! and lost it when the app exited (seen on a Whonix 18 arm64 Workstation,
//! 2026-11-23). Every grant below `xdg-data` must therefore create its folder.

use std::path::Path;

const MANIFEST: &str = "../../flatpak/io.github.sureserverman.AppImageControl.yml";

fn filesystem_grants() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(MANIFEST);
    let text = std::fs::read_to_string(&path).expect("read the Flatpak manifest");
    text.lines()
        .map(str::trim)
        .filter_map(|l| l.strip_prefix("- --filesystem="))
        .map(|g| g.trim().trim_matches('"').to_string())
        .collect()
}

#[test]
fn manifest_xdg_data_grants_create_their_folders() {
    let grants = filesystem_grants();
    let xdg: Vec<&String> = grants.iter().filter(|g| g.starts_with("xdg-data/")).collect();
    for folder in ["appimages", "applications", "icons"] {
        assert!(
            xdg.iter().any(|g| g.split(':').next() == Some(&format!("xdg-data/{folder}"))),
            "the manifest grants xdg-data/{folder}: {grants:?}"
        );
    }
    let missing: Vec<&&String> = xdg.iter().filter(|g| !g.ends_with(":create")).collect();
    assert!(
        missing.is_empty(),
        "every xdg-data grant must end in :create, or a fresh account loses what is written there: {missing:?}"
    );
}
