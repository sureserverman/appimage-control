//! Synthesise a `static`-only seed pack from the diff between a baseline
//! config and a post-configuration config — phase 3 of the seed-pack
//! *recorder* (see `docs/workflows/recording.md`).
//!
//! ## Why top-level-key granularity
//!
//! [`crate::seed_pack::apply_pack`] inserts every `static` entry at the
//! **top level** of the config map (`base.insert(key, value)`); it cannot
//! merge a value into a nested subtree. So the only honest unit of capture
//! is the whole top-level key: if a user changed a deeply nested setting,
//! the synthesised pack must carry that top-level key's *entire* value for
//! `apply_pack` to reproduce the user's choice. [`diff_configs`] therefore
//! classifies the diff at top-level-key granularity, and
//! [`build_static_pack`] copies each selected key's full value from the
//! post-configuration config.
//!
//! Removed keys (present in the baseline, gone after configuration) are
//! reported by [`ConfigDiff`] but **not** encoded: a `static` pack can only
//! add/overwrite keys, never delete them.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{Map, Value};

use crate::error::Result;
use crate::seed_pack::{SeedPack, SeedPackLibrary};

/// Top-level-key classification of a baseline → current config diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigDiff {
    /// Keys present in `current` but not in `baseline`, sorted.
    pub added: Vec<String>,
    /// Keys present in both but whose value changed (deep compare), sorted.
    pub changed: Vec<String>,
    /// Keys present in `baseline` but absent from `current`, sorted. A
    /// `static` pack cannot express removal, so these are reported only.
    pub removed: Vec<String>,
    /// Count of top-level keys identical in both (informational).
    pub unchanged: usize,
}

impl ConfigDiff {
    /// The keys a `static` pack can carry: `added` ∪ `changed`, sorted.
    pub fn capturable_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .added
            .iter()
            .chain(self.changed.iter())
            .cloned()
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// True when there is nothing a `static` pack could capture (no key
    /// was added or changed). `removed`-only diffs count as empty.
    pub fn is_empty_capture(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty()
    }
}

/// Classify the top-level diff between `baseline` and `current`.
/// JSON value comparison is structural and key-order-insensitive for
/// objects, so a nested re-ordering is *not* reported as a change.
pub fn diff_configs(baseline: &Map<String, Value>, current: &Map<String, Value>) -> ConfigDiff {
    let mut diff = ConfigDiff::default();
    for (key, cur_val) in current {
        match baseline.get(key) {
            None => diff.added.push(key.clone()),
            Some(base_val) if base_val != cur_val => diff.changed.push(key.clone()),
            Some(_) => diff.unchanged += 1,
        }
    }
    for key in baseline.keys() {
        if !current.contains_key(key) {
            diff.removed.push(key.clone());
        }
    }
    diff.added.sort();
    diff.changed.sort();
    diff.removed.sort();
    diff
}

/// Build a `static`-only [`SeedPack`] carrying `selected_keys`, each taken
/// **in full** from `current`. `config_path` and `first_run_args` come
/// from the recording. Selected keys absent from `current` (e.g. a
/// vanished or mistyped key) are silently skipped.
pub fn build_static_pack(
    config_path: &str,
    first_run_args: &[String],
    current: &Map<String, Value>,
    selected_keys: &[String],
) -> SeedPack {
    let mut static_keys = Map::new();
    for key in selected_keys {
        if let Some(value) = current.get(key) {
            static_keys.insert(key.clone(), value.clone());
        }
    }
    SeedPack {
        config_path: config_path.to_string(),
        first_run_args: first_run_args.to_vec(),
        static_keys,
        preset: BTreeMap::new(),
        rotation: BTreeMap::new(),
        icons: None,
    }
}

/// Serialise `pack` to pretty JSON and confirm it re-parses and passes the
/// exact loader validation the binary runs at startup (under `app_id`).
/// This guarantees the file we are about to write will load cleanly later.
/// Returns the pretty JSON (no trailing newline).
pub fn render_pack_json(app_id: &str, pack: &SeedPack) -> Result<String> {
    let json = serde_json::to_string_pretty(pack)?;
    // Round-trip through the real loader + validator: a synthesised pack
    // that can't be re-read is a bug we want to catch before writing.
    SeedPackLibrary::from_pairs(vec![(app_id.to_string(), json.clone())])?.validate()?;
    Ok(json)
}

/// Write `pack` to `path` as pretty JSON with a trailing newline, creating
/// parent directories as needed. The pack is validated (via
/// [`render_pack_json`]) before any bytes are written, so a malformed pack
/// never reaches disk. `app_id` is used only for the validation round-trip
/// and error attribution.
pub fn write_pack_to(path: &Path, app_id: &str, pack: &SeedPack) -> Result<()> {
    let json = render_pack_json(app_id, pack)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("{json}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .cloned()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn diff_classifies_added_changed_removed_unchanged() {
        let baseline = map(&[
            ("keep", json!(1)),
            ("change", json!("old")),
            ("gone", json!(true)),
        ]);
        let current = map(&[
            ("keep", json!(1)),
            ("change", json!("new")),
            ("added", json!([1, 2])),
        ]);
        let diff = diff_configs(&baseline, &current);
        assert_eq!(diff.added, vec!["added"]);
        assert_eq!(diff.changed, vec!["change"]);
        assert_eq!(diff.removed, vec!["gone"]);
        assert_eq!(diff.unchanged, 1);
        assert_eq!(diff.capturable_keys(), vec!["added", "change"]);
        assert!(!diff.is_empty_capture());
    }

    #[test]
    fn nested_change_surfaces_as_top_level_key_change() {
        // A change buried inside a nested object must mark the whole
        // top-level key as changed (apply_pack replaces the whole key).
        let baseline = map(&[("net", json!({"proxy": {"host": "a", "port": 1}}))]);
        let current = map(&[("net", json!({"proxy": {"host": "a", "port": 2}}))]);
        let diff = diff_configs(&baseline, &current);
        assert_eq!(diff.changed, vec!["net"]);
        let pack = build_static_pack("c.json", &[], &current, &diff.capturable_keys());
        // The pack carries the *entire* nested value, not a leaf path.
        assert_eq!(
            pack.static_keys["net"],
            json!({"proxy": {"host": "a", "port": 2}})
        );
    }

    #[test]
    fn key_order_change_is_not_a_diff() {
        let baseline = map(&[("o", json!({"a": 1, "b": 2}))]);
        let current = map(&[("o", json!({"b": 2, "a": 1}))]);
        let diff = diff_configs(&baseline, &current);
        assert!(
            diff.is_empty_capture(),
            "object key reordering must not register as a change"
        );
        assert_eq!(diff.unchanged, 1);
    }

    #[test]
    fn removed_only_diff_is_empty_capture() {
        let baseline = map(&[("gone", json!(1))]);
        let current = Map::new();
        let diff = diff_configs(&baseline, &current);
        assert_eq!(diff.removed, vec!["gone"]);
        assert!(diff.is_empty_capture());
        assert!(diff.capturable_keys().is_empty());
    }

    #[test]
    fn build_static_pack_copies_selected_keys_only() {
        let current = map(&[("a", json!(1)), ("b", json!("x")), ("c", json!(true))]);
        let pack = build_static_pack(
            "{appimage}.config/app.json",
            &["--offline".into()],
            &current,
            &["a".to_string(), "c".to_string(), "missing".to_string()],
        );
        assert_eq!(pack.config_path, "{appimage}.config/app.json");
        assert_eq!(pack.first_run_args, vec!["--offline"]);
        assert_eq!(pack.static_keys.len(), 2);
        assert_eq!(pack.static_keys["a"], json!(1));
        assert_eq!(pack.static_keys["c"], json!(true));
        assert!(!pack.static_keys.contains_key("b"));
        // "missing" is silently skipped (not in `current`).
        assert!(!pack.static_keys.contains_key("missing"));
    }

    #[test]
    fn rendered_pack_is_static_only_and_reparses() {
        let current = map(&[("telemetry", json!(false)), ("theme", json!("dark"))]);
        let pack = build_static_pack(
            "{appimage}.config/app.json",
            &[],
            &current,
            &["telemetry".to_string(), "theme".to_string()],
        );
        let json = render_pack_json("recorded-app", &pack).expect("must validate + render");
        // No empty preset/rotation/first_run_args/icons noise.
        assert!(!json.contains("preset"), "unexpected preset block: {json}");
        assert!(
            !json.contains("rotation"),
            "unexpected rotation block: {json}"
        );
        assert!(
            !json.contains("first_run_args"),
            "unexpected first_run_args: {json}"
        );
        assert!(!json.contains("icons"), "unexpected icons block: {json}");
        assert!(json.contains("\"static\""), "missing static block: {json}");
        assert!(json.contains("config_path"));

        // Re-parse confirms it is a loadable pack.
        let lib = SeedPackLibrary::from_pairs(vec![("recorded-app".to_string(), json)]).unwrap();
        let parsed = lib.get("recorded-app").unwrap();
        assert_eq!(parsed.static_keys["telemetry"], json!(false));
        assert_eq!(parsed.static_keys["theme"], json!("dark"));
    }

    #[test]
    fn synthesised_pack_round_trips_through_apply_pack() {
        // The whole point: applying the synthesised pack to the baseline
        // reproduces the user's post-configuration values.
        use crate::seed::{SeedConfig, SeedPreset};
        use crate::seed_pack::apply_pack;

        let baseline = map(&[("telemetry", json!(true)), ("keep", json!(1))]);
        let current = map(&[
            ("telemetry", json!(false)),
            ("keep", json!(1)),
            ("new", json!("v")),
        ]);
        let diff = diff_configs(&baseline, &current);
        let pack = build_static_pack("c.json", &[], &current, &diff.capturable_keys());

        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut replay = baseline.clone();
        apply_pack(&pack, "recorded-app", &cfg, 1, &mut replay).unwrap();
        assert_eq!(replay["telemetry"], json!(false));
        assert_eq!(replay["new"], json!("v"));
        assert_eq!(replay["keep"], json!(1));
    }

    #[test]
    fn write_pack_to_creates_parents_and_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("nested").join("recorded-app.json");
        let current = map(&[("k", json!(1))]);
        let pack = build_static_pack("c.json", &[], &current, &["k".to_string()]);
        write_pack_to(&out, "recorded-app", &pack).unwrap();
        let raw = fs::read_to_string(&out).unwrap();
        assert!(raw.ends_with('\n'));
        // The file we wrote loads through the directory loader.
        let lib = SeedPackLibrary::load_from_dir(out.parent().unwrap())
            .unwrap()
            .unwrap();
        assert!(lib.get("recorded-app").is_some());
    }
}
