//! Named, user-saved seed-form configurations.
//!
//! A "seed form" captures the exact inputs the seed dialog accepts —
//! a profile ([`SeedPreset`]) and a list of app ids — under a
//! human-chosen name, so the user can re-run the same seed selection
//! later (from the GUI or the CLI) without re-checking each row.
//!
//! Storage layout: `~/.local/share/appimage-control/seed-forms/<name>.json`.
//! One file per form, plain JSON. The filename's stem is the form's
//! canonical name; the in-file `name` field is informational.
//!
//! Names are constrained to `[A-Za-z0-9._-]+` (≤ 64 chars) so they map
//! straight onto a filesystem path with no escaping. Empty names and
//! names with path separators are rejected.
//!
//! This is the canonical persistent store consumed by both the GUI's
//! "Saved configurations" dropdown and the CLI's `--seed-load NAME`,
//! `--seed-save NAME`, `--seed-list-forms`, `--seed-delete-form NAME`
//! flags.
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::seed::SeedPreset;

const NAME_MAX_LEN: usize = 64;

/// One persisted seed-form. Mirror of the modal dialog's inputs:
/// which profile (Ubuntu / Whonix), which catalog app ids should be
/// seeded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedSeedForm {
    /// Canonical name (filesystem-safe, matches the filename stem).
    pub name: String,
    /// Informational/diagnostic only. Stored as `"ubuntu"` or `"whonix"` at save time but
    /// **ignored on load** — the active preset is always re-detected from the host environment
    /// (see [`crate::seed::SeedPreset::auto_default`]). Hand-edited files that omit this field
    /// are tolerated via `#[serde(default)]`; the value defaults to an empty string.
    ///
    /// Callers that need the resolved preset should use [`SavedSeedForm::resolved_preset`] and
    /// then apply the user's current host detection rather than trusting the persisted value.
    #[serde(default)]
    pub preset: String,
    /// Catalog app ids the user selected when saving.
    pub app_ids: Vec<String>,
}

impl SavedSeedForm {
    pub fn new(name: impl Into<String>, preset: SeedPreset, app_ids: Vec<String>) -> Result<Self> {
        let name = name.into();
        validate_name(&name)?;
        Ok(Self {
            name,
            preset: preset.as_str().to_string(),
            app_ids,
        })
    }

    /// Resolve `preset` back to the typed enum. Returns an error if
    /// the form was written with an unknown preset value (defends
    /// against hand-edited files).
    pub fn resolved_preset(&self) -> Result<SeedPreset> {
        SeedPreset::parse(&self.preset).ok_or_else(|| {
            Error::InvalidPath(format!(
                "seed form '{}' has unknown preset '{}'",
                self.name, self.preset
            ))
        })
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidPath("seed form name is empty".into()));
    }
    if name.len() > NAME_MAX_LEN {
        return Err(Error::InvalidPath(format!(
            "seed form name too long (max {NAME_MAX_LEN}): {name}"
        )));
    }
    for c in name.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if !ok {
            return Err(Error::InvalidPath(format!(
                "seed form name '{name}': only A-Z, a-z, 0-9, '-', '_', '.' are allowed"
            )));
        }
    }
    if name.starts_with('.') {
        return Err(Error::InvalidPath(format!(
            "seed form name '{name}' must not start with '.'"
        )));
    }
    Ok(())
}

/// Directory holding `<name>.json` files. Caller-supplied root must
/// be an absolute home (used by tests / non-default profiles).
pub fn seed_forms_dir_in(home: &Path) -> PathBuf {
    home.join(".local/share/appimage-control/seed-forms")
}

/// Resolves the default location: `$HOME/.local/share/appimage-control/seed-forms/`.
pub fn default_seed_forms_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| seed_forms_dir_in(&h))
}

fn form_path(dir: &Path, name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    Ok(dir.join(format!("{name}.json")))
}

/// Persist `form`. Overwrites any existing file with the same name.
/// Creates the directory on first save.
pub fn save_form_to(dir: &Path, form: &SavedSeedForm) -> Result<()> {
    validate_name(&form.name)?;
    fs::create_dir_all(dir)?;
    let path = form_path(dir, &form.name)?;
    let body = serde_json::to_string_pretty(form)?;
    fs::write(&path, format!("{body}\n"))?;
    Ok(())
}

/// Read a single form by name. Returns `Error::InvalidPath` when the
/// file does not exist so callers can distinguish "not saved" from
/// "I/O failure".
pub fn load_form_from(dir: &Path, name: &str) -> Result<SavedSeedForm> {
    let path = form_path(dir, name)?;
    if !path.is_file() {
        return Err(Error::InvalidPath(format!(
            "no saved seed form named '{name}'"
        )));
    }
    let raw = fs::read_to_string(&path)?;
    let mut form: SavedSeedForm = serde_json::from_str(&raw).map_err(Error::Json)?;
    // The on-disk `name` field is informational; the filename stem
    // wins so a hand-renamed file still works.
    form.name = name.to_string();
    Ok(form)
}

/// List the saved forms in `dir`, sorted by name. Missing directory =
/// empty list, no error.
pub fn list_forms_in(dir: &Path) -> Result<Vec<String>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
        else {
            continue;
        };
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if validate_name(&stem).is_err() {
            continue;
        }
        out.push(stem);
    }
    out.sort();
    Ok(out)
}

/// Delete the form named `name`. Idempotent: deleting a missing form
/// is `Ok(false)`; existing form deletion is `Ok(true)`.
pub fn delete_form_in(dir: &Path, name: &str) -> Result<bool> {
    let path = form_path(dir, name)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path)?;
    Ok(true)
}

// ----- HOME-based convenience wrappers used by GUI / CLI ------------

pub fn save_form(home: &Path, form: &SavedSeedForm) -> Result<()> {
    save_form_to(&seed_forms_dir_in(home), form)
}

pub fn load_form(home: &Path, name: &str) -> Result<SavedSeedForm> {
    load_form_from(&seed_forms_dir_in(home), name)
}

pub fn list_forms(home: &Path) -> Result<Vec<String>> {
    list_forms_in(&seed_forms_dir_in(home))
}

pub fn delete_form(home: &Path, name: &str) -> Result<bool> {
    delete_form_in(&seed_forms_dir_in(home), name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation_rejects_obvious_pitfalls() {
        assert!(validate_name("").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("foo/bar").is_err());
        assert!(validate_name("foo bar").is_err());
        assert!(validate_name(&"x".repeat(NAME_MAX_LEN + 1)).is_err());
        assert!(validate_name("ok-name_1.v2").is_ok());
    }

    #[test]
    fn save_load_list_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seed-forms");

        let form = SavedSeedForm::new(
            "tor-trio",
            SeedPreset::Whonix,
            vec!["feather".into(), "electrum-btc".into()],
        )
        .unwrap();

        save_form_to(&dir, &form).unwrap();
        let loaded = load_form_from(&dir, "tor-trio").unwrap();
        assert_eq!(loaded, form);

        let names = list_forms_in(&dir).unwrap();
        assert_eq!(names, vec!["tor-trio".to_string()]);

        assert!(delete_form_in(&dir, "tor-trio").unwrap());
        assert!(!delete_form_in(&dir, "tor-trio").unwrap());
        assert!(list_forms_in(&dir).unwrap().is_empty());
    }

    #[test]
    fn load_missing_form_is_a_clean_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_form_from(tmp.path(), "nope").unwrap_err();
        assert!(format!("{err}").contains("no saved seed form named 'nope'"));
    }

    #[test]
    fn list_missing_dir_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let names = list_forms_in(&tmp.path().join("absent")).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn rejects_unknown_preset_on_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("forms");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("weird.json"),
            r#"{"name":"weird","preset":"nope","app_ids":[]}"#,
        )
        .unwrap();
        let f = load_form_from(&dir, "weird").unwrap();
        let err = f.resolved_preset().unwrap_err();
        assert!(format!("{err}").contains("unknown preset 'nope'"));
    }

    /// A hand-edited form JSON that omits the `preset` field must load successfully.
    /// The field defaults to `""` via `#[serde(default)]`.
    #[test]
    fn load_form_without_preset_field_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("forms");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("no-preset.json"),
            r#"{"name":"no-preset","app_ids":["feather","electrum-btc"]}"#,
        )
        .unwrap();
        let f = load_form_from(&dir, "no-preset").expect("load must succeed when preset is absent");
        // The field is empty-string (serde default), not an error on load.
        assert_eq!(f.preset, "");
        assert_eq!(f.app_ids, vec!["feather", "electrum-btc"]);
    }
}
