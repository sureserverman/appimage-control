//! Persistent GUI preferences.
//!
//! Backs the redesigned Preferences window (General / Network / Storage / Backup).
//! Stored as JSON at `~/.config/appimage-control/config.json` so it survives
//! restarts and is editable by hand. Every field has a default, so a missing or
//! partial file still loads — new keys added later read as their default.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::paths::AppPaths;

/// Appearance preference; maps to libadwaita's `AdwColorScheme` in the GUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    /// Follow the system light/dark preference.
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub fn as_index(self) -> u32 {
        match self {
            ThemePref::System => 0,
            ThemePref::Light => 1,
            ThemePref::Dark => 2,
        }
    }
    pub fn from_index(i: u32) -> Self {
        match i {
            1 => ThemePref::Light,
            2 => ThemePref::Dark,
            _ => ThemePref::System,
        }
    }
}

fn default_concurrent() -> u32 {
    2
}
fn default_route() -> String {
    "System proxy (Tor)".to_string()
}
fn default_runner() -> String {
    "Auto".to_string()
}

/// All user-tunable GUI preferences. Serialized verbatim to `config.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GuiConfig {
    // --- General ---
    pub theme: ThemePref,
    /// Compact launcher grid (tighter metrics) vs. the regular grid.
    pub compact_rows: bool,
    /// Re-discover the catalog when the window regains focus.
    pub reload_on_focus: bool,
    /// On launch, fetch update hints for installed instances.
    pub check_for_updates: bool,
    /// On launch, silently repair missing per-instance launchers/icons.
    pub repair_launchers_auto: bool,
    /// Run `--check-updates --notify` at login (autostart entry, or the Flatpak Background
    /// portal). On by default (OD-2), so a config written before the field reads as on.
    pub check_updates_at_login: bool,

    // --- Network ---
    /// Human label for the download route (e.g. "System proxy (Tor)", "Direct").
    pub route_through: String,
    /// Verify checksums & signatures on downloaded AppImages.
    pub verify_checksums: bool,
    /// Concurrent download workers (lower is gentler over Tor).
    pub concurrent_downloads: u32,
    /// Auto-seed newly created instances right after Apply.
    pub auto_seed: bool,
    /// Headless runner strategy label (e.g. "Auto").
    pub headless_runner: String,
}

impl Default for GuiConfig {
    fn default() -> Self {
        GuiConfig {
            theme: ThemePref::System,
            compact_rows: true,
            reload_on_focus: false,
            check_for_updates: true,
            repair_launchers_auto: true,
            check_updates_at_login: true,
            route_through: default_route(),
            verify_checksums: true,
            concurrent_downloads: default_concurrent(),
            auto_seed: true,
            headless_runner: default_runner(),
        }
    }
}

// These keep serde(default) happy field-by-field for forward compatibility when
// only some keys are present in an older file.
impl GuiConfig {
    /// `~/.config/appimage-control/config.json`.
    pub fn path(paths: &AppPaths) -> PathBuf {
        crate::paths::own_config_base(paths.home())
            .join("appimage-control")
            .join("config.json")
    }

    /// Load preferences, falling back to defaults when the file is missing or
    /// unreadable/corrupt (never an error path for the GUI — bad config just
    /// reverts to defaults).
    pub fn load(paths: &AppPaths) -> Self {
        let p = Self::path(paths);
        match fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => GuiConfig::default(),
        }
    }

    /// Persist preferences, creating the config directory if needed.
    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let p = Self::path(paths);
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&p, json)?;
        Ok(())
    }

    pub fn clamp_concurrent(&self) -> u32 {
        self.concurrent_downloads.clamp(1, 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_through_json() {
        let c = GuiConfig::default();
        let s = serde_json::to_string(&c).unwrap();
        let back: GuiConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.theme, ThemePref::System);
        assert!(back.compact_rows);
        assert_eq!(back.concurrent_downloads, 2);
        assert_eq!(back.route_through, "System proxy (Tor)");
    }

    #[test]
    fn partial_file_fills_missing_with_defaults() {
        let c: GuiConfig = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        assert_eq!(c.theme, ThemePref::Dark);
        assert!(c.compact_rows); // default preserved
        assert!(c.verify_checksums);
    }

    #[test]
    fn theme_index_maps_both_ways() {
        for t in [ThemePref::System, ThemePref::Light, ThemePref::Dark] {
            assert_eq!(ThemePref::from_index(t.as_index()), t);
        }
    }
}
