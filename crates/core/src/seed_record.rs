//! Persisted seed-pack *recordings* — the baseline half of the
//! diff-based pack recorder (see `docs/workflows/recording.md`).
//!
//! The recorder authors a seed pack in three phases:
//!
//! 1. [`crate::seed::capture_first_run_config`] launches an installed
//!    instance headlessly and snapshots its untouched first-run config.
//!    That snapshot, plus the metadata needed to read the same config
//!    file again later, is stored here as a [`PackRecording`].
//! 2. The user launches the app normally and sets their settings by hand.
//! 3. [`crate::seed_pack_build`] reads the live config, diffs it against
//!    the stored baseline, and synthesises a `static`-only seed pack.
//!
//! Storage layout:
//! `~/.local/share/appimage-control/pack-recordings/<app-id>.<instance>.json`,
//! one file per `(app, instance)`. The filename stem is the canonical
//! key; the in-file `app_id`/`instance_index` fields are authoritative
//! for content. Both halves of the key are validated so they map onto a
//! filesystem path with no escaping.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::seed::SeedPreset;

const APP_ID_MAX_LEN: usize = 64;

/// One persisted recording: the untouched first-run config of an
/// instance plus everything needed to read the same config file again
/// and to label the synthesised pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackRecording {
    /// Catalog app id (also the storage-key prefix).
    pub app_id: String,
    /// 1-based instance index this baseline was captured from.
    pub instance_index: u32,
    /// `config_path` template relative to the instance dir, with any
    /// `{appimage}` placeholder preserved — becomes the synthesised
    /// pack's `config_path`.
    pub config_path: String,
    /// First-run AppImage args used for the headless baseline launch.
    #[serde(default)]
    pub first_run_args: Vec<String>,
    /// Host preset detected at capture time. Informational only — the
    /// recorder emits literal `static` values, so the preset does not
    /// affect the diff. Stored as `"ubuntu"`/`"whonix"`.
    #[serde(default)]
    pub preset: String,
    /// Capture time, seconds since the Unix epoch. Informational.
    #[serde(default)]
    pub created_unix: u64,
    /// The untouched first-run JSON object captured headlessly (no seed
    /// pack applied). This is the left-hand side of the diff.
    pub baseline: Map<String, Value>,
}

impl PackRecording {
    /// Build a recording, validating the app id and stamping
    /// `created_unix` from the current clock.
    pub fn new(
        app_id: impl Into<String>,
        instance_index: u32,
        config_path: impl Into<String>,
        first_run_args: Vec<String>,
        preset: SeedPreset,
        baseline: Map<String, Value>,
    ) -> Result<Self> {
        let app_id = app_id.into();
        validate_app_id(&app_id)?;
        let created_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(Self {
            app_id,
            instance_index,
            config_path: config_path.into(),
            first_run_args,
            preset: preset.as_str().to_string(),
            created_unix,
            baseline,
        })
    }
}

/// Reject app ids that wouldn't map cleanly onto a filesystem key. Same
/// character class as [`crate::seed_form`] form names: `[A-Za-z0-9._-]`,
/// non-empty, ≤ 64 chars, not leading-dot (so the key can't become a
/// hidden file or `..`).
fn validate_app_id(app_id: &str) -> Result<()> {
    if app_id.is_empty() {
        return Err(Error::InvalidPath("recording app id is empty".into()));
    }
    if app_id.len() > APP_ID_MAX_LEN {
        return Err(Error::InvalidPath(format!(
            "recording app id too long (max {APP_ID_MAX_LEN}): {app_id}"
        )));
    }
    for c in app_id.chars() {
        let ok = c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.');
        if !ok {
            return Err(Error::InvalidPath(format!(
                "recording app id '{app_id}': only A-Z, a-z, 0-9, '-', '_', '.' are allowed"
            )));
        }
    }
    if app_id.starts_with('.') {
        return Err(Error::InvalidPath(format!(
            "recording app id '{app_id}' must not start with '.'"
        )));
    }
    Ok(())
}

/// `<app-id>.<instance>` storage key (also the filename stem).
fn recording_key(app_id: &str, instance_index: u32) -> Result<String> {
    validate_app_id(app_id)?;
    Ok(format!("{app_id}.{instance_index}"))
}

fn recording_path(dir: &Path, app_id: &str, instance_index: u32) -> Result<PathBuf> {
    Ok(dir.join(format!("{}.json", recording_key(app_id, instance_index)?)))
}

/// Directory holding `<app-id>.<instance>.json` recording files for a
/// caller-supplied home (used by tests / non-default profiles).
pub fn recordings_dir_in(home: &Path) -> PathBuf {
    home.join(".local/share/appimage-control/pack-recordings")
}

/// Default location: `$HOME/.local/share/appimage-control/pack-recordings/`.
pub fn default_recordings_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| recordings_dir_in(&h))
}

/// Persist `rec`, overwriting any existing recording for the same
/// `(app, instance)`. Creates the directory on first save.
pub fn save_recording_to(dir: &Path, rec: &PackRecording) -> Result<()> {
    validate_app_id(&rec.app_id)?;
    fs::create_dir_all(dir)?;
    let path = recording_path(dir, &rec.app_id, rec.instance_index)?;
    let body = serde_json::to_string_pretty(rec)?;
    fs::write(&path, format!("{body}\n"))?;
    Ok(())
}

/// Read the recording for `(app, instance)`. Returns `Error::InvalidPath`
/// when no recording exists, so callers can distinguish "never captured"
/// from an I/O failure.
pub fn load_recording_from(dir: &Path, app_id: &str, instance_index: u32) -> Result<PackRecording> {
    let path = recording_path(dir, app_id, instance_index)?;
    if !path.is_file() {
        return Err(Error::InvalidPath(format!(
            "no baseline recording for '{app_id}' instance {instance_index} — capture a baseline first"
        )));
    }
    let raw = fs::read_to_string(&path)?;
    let rec: PackRecording = serde_json::from_str(&raw).map_err(Error::Json)?;
    Ok(rec)
}

/// List recordings in `dir` as `(app_id, instance_index)` pairs, sorted.
/// Missing directory = empty list, no error.
pub fn list_recordings_in(dir: &Path) -> Result<Vec<(String, u32)>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        // Stem is `<app-id>.<instance>`; split on the final '.'.
        let Some((app_id, idx)) = stem.rsplit_once('.') else {
            continue;
        };
        let Ok(instance_index) = idx.parse::<u32>() else {
            continue;
        };
        if validate_app_id(app_id).is_err() {
            continue;
        }
        out.push((app_id.to_string(), instance_index));
    }
    out.sort();
    Ok(out)
}

/// Delete the recording for `(app, instance)`. Idempotent: deleting a
/// missing recording is `Ok(false)`; an existing one is `Ok(true)`.
pub fn delete_recording_in(dir: &Path, app_id: &str, instance_index: u32) -> Result<bool> {
    let path = recording_path(dir, app_id, instance_index)?;
    if !path.exists() {
        return Ok(false);
    }
    fs::remove_file(&path)?;
    Ok(true)
}

// ----- HOME-based convenience wrappers used by GUI / CLI ------------

pub fn save_recording(home: &Path, rec: &PackRecording) -> Result<()> {
    save_recording_to(&recordings_dir_in(home), rec)
}

pub fn load_recording(home: &Path, app_id: &str, instance_index: u32) -> Result<PackRecording> {
    load_recording_from(&recordings_dir_in(home), app_id, instance_index)
}

pub fn list_recordings(home: &Path) -> Result<Vec<(String, u32)>> {
    list_recordings_in(&recordings_dir_in(home))
}

pub fn delete_recording(home: &Path, app_id: &str, instance_index: u32) -> Result<bool> {
    delete_recording_in(&recordings_dir_in(home), app_id, instance_index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_baseline() -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("telemetry".into(), json!(true));
        m.insert("theme".into(), json!("light"));
        m
    }

    #[test]
    fn app_id_validation_rejects_obvious_pitfalls() {
        assert!(validate_app_id("").is_err());
        assert!(validate_app_id(".hidden").is_err());
        assert!(validate_app_id("foo/bar").is_err());
        assert!(validate_app_id("foo bar").is_err());
        assert!(validate_app_id(&"x".repeat(APP_ID_MAX_LEN + 1)).is_err());
        assert!(validate_app_id("electrum-btc").is_ok());
        assert!(validate_app_id("feather").is_ok());
    }

    #[test]
    fn save_load_list_delete_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("recordings");

        let rec = PackRecording::new(
            "feather",
            2,
            "{appimage}.config/feather/settings.json",
            vec!["--offline".into()],
            SeedPreset::Ubuntu,
            sample_baseline(),
        )
        .unwrap();

        save_recording_to(&dir, &rec).unwrap();
        let loaded = load_recording_from(&dir, "feather", 2).unwrap();
        assert_eq!(loaded, rec);

        // A second recording for a different instance coexists.
        let rec2 = PackRecording::new(
            "feather",
            1,
            "{appimage}.config/feather/settings.json",
            vec![],
            SeedPreset::Whonix,
            sample_baseline(),
        )
        .unwrap();
        save_recording_to(&dir, &rec2).unwrap();

        let mut listed = list_recordings_in(&dir).unwrap();
        listed.sort();
        assert_eq!(
            listed,
            vec![("feather".to_string(), 1), ("feather".to_string(), 2)]
        );

        assert!(delete_recording_in(&dir, "feather", 2).unwrap());
        assert!(!delete_recording_in(&dir, "feather", 2).unwrap());
        assert_eq!(list_recordings_in(&dir).unwrap(), vec![("feather".to_string(), 1)]);
    }

    #[test]
    fn load_missing_recording_is_a_clean_error() {
        let tmp = tempfile::tempdir().unwrap();
        let err = load_recording_from(tmp.path(), "nope", 1).unwrap_err();
        assert!(format!("{err}").contains("no baseline recording for 'nope' instance 1"));
    }

    #[test]
    fn list_missing_dir_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let names = list_recordings_in(&tmp.path().join("absent")).unwrap();
        assert!(names.is_empty());
    }

    #[test]
    fn list_ignores_malformed_stems() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("recordings");
        fs::create_dir_all(&dir).unwrap();
        // No instance suffix, non-numeric suffix, and a non-json file.
        fs::write(dir.join("noinstance.json"), "{}").unwrap();
        fs::write(dir.join("feather.abc.json"), "{}").unwrap();
        fs::write(dir.join("feather.3.json"), "{}").unwrap();
        fs::write(dir.join("README.md"), "x").unwrap();
        let listed = list_recordings_in(&dir).unwrap();
        assert_eq!(listed, vec![("feather".to_string(), 3)]);
    }

    #[test]
    fn recordings_dir_shape_is_under_xdg_data() {
        let home = Path::new("/home/example");
        assert_eq!(
            recordings_dir_in(home),
            Path::new("/home/example/.local/share/appimage-control/pack-recordings")
        );
    }
}
