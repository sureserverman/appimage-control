//! Seed-pack schema, embedded fallback, and discovery.
//!
//! A *seed pack* is a declarative description of how to patch one supported
//! app's first-run config after the manager has launched it once under
//! `xvfb-run`. Packs live as JSON files at `data/seeds/<app-id>.json` in
//! the repo, are baked into the binary at compile time by `build.rs`, and
//! can be overridden on disk via the same fallback chain
//! [`crate::catalog::Catalog::discover_load`] uses for `catalog.json`.
//!
//! See `docs/seed-packs.md` (Stage 5 of the seed-pack-extraction plan) for
//! the authoring guide.
//!
//! The schema is deliberately tiny:
//!
//! - `static`: plain JSON literals merged into the config map.
//! - `rotation`: a per-instance value indexed by `(instance_index - 1) mod
//!   N`, drawn from inline `values`.
//! - `proxy_setup`: names the portmux recipe that points this app at its
//!   own Tor listener. portmux writes the proxy settings; a pack holds no
//!   port, proxy host or isolation class.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};

/// Environment variable that, when set to a non-empty path, forces the
/// loader to use exactly that directory of `*.json` packs and skip the
/// ancestor walk and `/app/share` / `/usr/share` fallbacks. Mirrors
/// `APPIMAGE_CONTROL_CATALOG`'s shape.
pub const ENV_SEEDS_DIR: &str = "APPIMAGE_CONTROL_SEEDS_DIR";

/// Bound on the executable ancestor walk.
/// Same value as `crate::catalog`'s walk.
const MAX_ANCESTOR_DEPTH: usize = 32;

/// Per-user seed-pack directory under `$XDG_DATA_HOME` (defaults to
/// `~/.local/share`). This is where companion-repo packs installed by
/// `--update-seed-packs` land, and where users can drop ad-hoc
/// overrides without touching system paths or exporting env vars.
pub fn user_seeds_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|p| p.join("appimage-control").join("seeds"))
}

/// One seed pack, parsed from `data/seeds/<app-id>.json`.
///
/// `Serialize` is derived (with `skip_serializing_if` on every optional
/// field) so the seed-pack *recorder* ([`crate::seed_pack_build`]) can
/// emit a hand-authorable file: a synthesised `static`-only pack
/// serialises to just `config_path` + `static` (+ `first_run_args` when
/// non-empty), with no empty `rotation`/`icons` noise.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SeedPack {
    /// Path to the JSON config the seeded AppImage is expected to write,
    /// relative to the instance directory. May contain a single
    /// `{appimage}` placeholder, replaced at runtime with
    /// [`crate::catalog::CatalogApp::resolved_appimage_filename`].
    pub config_path: String,

    /// Extra arguments passed to the AppImage on its first-run seed
    /// launch (e.g. `["--offline"]` for Electrum). Defaults to empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub first_run_args: Vec<String>,

    /// Plain JSON literals merged into the config map verbatim.
    #[serde(default, rename = "static", skip_serializing_if = "Map::is_empty")]
    pub static_keys: Map<String, Value>,

    /// Keys whose values rotate per instance index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rotation: BTreeMap<String, RotationValue>,

    /// Names the portmux recipe that writes this app's proxy settings.
    /// Absent for an app that needs no proxy, and portmux is then never run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_setup: Option<ProxySetup>,

    /// Accepted-but-ignored legacy `icons` block. Older on-disk packs (and
    /// the shipped companion packs under installers-linux) still carry an
    /// `icons` object. Icon resolution no longer reads it, but the field is
    /// retained so `deny_unknown_fields` does not reject those packs.
    /// Note: it still participates in the derived `PartialEq`/`Eq`, so two
    /// packs differing only in `icons` compare unequal even though seeding
    /// treats them identically (no current path compares `SeedPack` values).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icons: Option<Value>,
}

/// Body of the `proxy_setup` section. Named to avoid colliding with a
/// config key called `proxy`, which Feather and Electrum both use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProxySetup {
    /// Recipe id passed to `portmux-port configure-app --app`.
    pub app: String,
}

/// Parse one pack, refusing the removed `preset` section and `port` verb by
/// name. Ignoring them would leave a wallet with no proxy and no warning.
fn parse_pack(raw: &str) -> std::result::Result<SeedPack, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if let Some(preset) = value.get("preset") {
        let port_key = preset
            .as_object()
            .and_then(|m| m.iter().find(|(_, v)| v.get("port").is_some()))
            .map(|(k, _)| k.as_str());
        let what = match port_key {
            Some(key) => format!("uses the removed `port` verb at `preset.{key}`"),
            None => "uses the removed `preset` section".to_string(),
        };
        return Err(format!(
            "{what}. portmux now writes proxy settings. Delete the `preset` section, move any \
             non-proxy key to `static`, and add \"proxy_setup\": {{ \"app\": \"<recipe id>\" }}. \
             See docs/integration-portmux-port.md"
        ));
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// One `rotation` entry. Today the only supported variant is
/// [`RotationValue::Values`]; the legacy `from_preset_array` variant
/// was retired with the `[u16; 3]` preset array it referenced.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, untagged)]
pub enum RotationValue {
    /// Rotate through an inline list of JSON literals, indexed by
    /// `(instance_index - 1) % values.len()`.
    Values { values: Vec<Value> },
}

// Generated by build.rs from `data/seeds/*.json`. Provides
// `pub static SHIPPED_SEED_PACKS: &[(&str, &str)]`.
include!(concat!(env!("OUT_DIR"), "/shipped_seeds.rs"));

/// In-memory library of seed packs keyed by app id (file stem).
#[derive(Debug, Clone, Default)]
pub struct SeedPackLibrary {
    packs: BTreeMap<String, SeedPack>,
    sources: BTreeMap<String, PathBuf>,
    /// Packs in the loaded directory that failed to parse or validate, keyed by app id (file
    /// stem), with the reason. They are skipped; every other pack still loads (WF-SEED-001).
    refused: BTreeMap<String, String>,
}

impl SeedPackLibrary {
    /// Build a library from the packs baked into the binary at compile time
    /// by `build.rs`. Used as the final fallback in
    /// [`SeedPackLibrary::discover_load`].
    pub fn embedded() -> Result<Self> {
        let pairs = SHIPPED_SEED_PACKS
            .iter()
            .map(|(id, raw)| (*id, *raw))
            .collect::<Vec<_>>();
        let lib = Self::from_pairs(pairs)?;
        lib.validate()?;
        Ok(lib)
    }

    /// One row per loaded pack, in stable id order. Each row is
    /// `(app_id, source)` where source is either the absolute path of
    /// the on-disk pack file or the literal string `<embedded>`.
    /// Backs the `appimage-control --list-seed-packs` CLI flag and
    /// gives users a way to confirm which override the loader picked.
    pub fn list_rows(&self) -> Vec<(String, String)> {
        self.packs
            .keys()
            .map(|id| {
                let src = self
                    .sources
                    .get(id)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<embedded>".into());
                (id.clone(), src)
            })
            .collect()
    }

    /// Load every `*.json` file in `dir` as a seed pack keyed by file
    /// stem. Returns `Ok(None)` when `dir` is missing or contains no
    /// `*.json` files. A pack that fails to parse or validate is refused:
    /// a warning names it and the reason, it is listed by
    /// [`SeedPackLibrary::refused`], and every other pack still loads.
    pub fn load_from_dir(dir: &Path) -> Result<Option<Self>> {
        if !dir.is_dir() {
            return Ok(None);
        }
        let mut lib = SeedPackLibrary::default();
        let read = fs::read_dir(dir).map_err(Error::Io)?;
        for entry in read {
            let entry = entry.map_err(Error::Io)?;
            let path = entry.path();
            if path.extension().and_then(OsStr::to_str) != Some("json") {
                continue;
            }
            // A non-UTF-8 file name cannot be an app id: refuse it under its lossy name.
            let Some(stem) = path.file_stem().and_then(OsStr::to_str).map(str::to_string) else {
                let shown = path.display().to_string();
                eprintln!("appimage-control: warning: seed pack {shown}: bad file name; skipping");
                lib.refused
                    .insert(shown.clone(), format!("seed pack {shown}: bad file name"));
                continue;
            };
            let checked = fs::read_to_string(&path)
                .map_err(|e| format!("seed pack {}: cannot read: {e}", path.display()))
                .and_then(|raw| {
                    parse_pack(&raw).map_err(|e| format!("seed pack {}: {e}", path.display()))
                })
                .and_then(|pack| {
                    validate_pack(&pack).map(|()| pack).map_err(|e| {
                        let msg = match e {
                            Error::InvalidPath(msg) => msg,
                            other => other.to_string(),
                        };
                        format!("seed pack `{stem}` ({}): {msg}", path.display())
                    })
                });
            match checked {
                Ok(pack) => {
                    lib.packs.insert(stem.clone(), pack);
                    lib.sources.insert(stem, path);
                }
                Err(reason) => {
                    eprintln!("appimage-control: warning: {reason}; skipping this pack");
                    lib.refused.insert(stem, reason);
                }
            }
        }
        if lib.is_empty() && lib.refused.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lib))
        }
    }

    /// Packs refused at load, as `(app_id, reason)` in id order.
    pub fn refused(&self) -> impl Iterator<Item = (&str, &str)> {
        self.refused.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// Why the pack for `app_id` was refused, if it was.
    pub fn refusal_for(&self, app_id: &str) -> Option<&str> {
        self.refused.get(app_id).map(String::as_str)
    }

    /// Resolve a seed-pack library against the same fallback chain
    /// `Catalog::discover_load` uses for `catalog.json`:
    ///
    /// 1. `explicit` directory (must exist and contain at least one pack).
    /// 2. `APPIMAGE_CONTROL_SEEDS_DIR` env var, if non-empty and a directory.
    /// 3. `~/.local/share/appimage-control/seeds/` — per-user override.
    ///    This is where `--update-seed-packs` (Stage 7 of the
    ///    seed-pack-extraction plan) installs companion-repo packs.
    /// 4. Walk `current_exe()` ancestors for `data/seeds/`, only when the binary runs from a
    ///    cargo `target/` dir (dev and test builds). Neither the current directory nor an
    ///    installed binary's location is walked: both can reach `$HOME`, where a planted
    ///    `data/seeds/` would win.
    /// 5. `/app/share/appimage-control/seeds/` (Flatpak install path).
    /// 6. `/usr/share/appimage-control/seeds/`.
    /// 7. Embedded slice baked in by `build.rs`.
    pub fn discover_load(explicit: Option<&Path>) -> Result<Self> {
        if let Some(dir) = explicit {
            if !dir.is_dir() {
                return Err(Error::InvalidPath(format!(
                    "seeds dir is not a directory: {}",
                    dir.display()
                )));
            }
            return Self::load_from_dir(dir)?.ok_or_else(|| {
                Error::InvalidPath(format!(
                    "seeds dir exists but contains no *.json packs: {}",
                    dir.display()
                ))
            });
        }
        if let Ok(s) = std::env::var(ENV_SEEDS_DIR) {
            let s = s.trim();
            if !s.is_empty() {
                let p = PathBuf::from(s);
                if let Some(lib) = Self::load_from_dir(&p)? {
                    return Ok(lib);
                }
            }
        }
        if let Some(user_dir) = user_seeds_dir() {
            if let Some(lib) = Self::load_from_dir(&user_dir)? {
                return Ok(lib);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent().and_then(crate::paths::dev_build_dir) {
                if let Some(lib) = Self::try_ancestors(parent)? {
                    return Ok(lib);
                }
            }
        }
        for p in [
            PathBuf::from("/app/share/appimage-control/seeds"),
            PathBuf::from("/usr/share/appimage-control/seeds"),
        ] {
            if let Some(lib) = Self::load_from_dir(&p)? {
                return Ok(lib);
            }
        }
        Self::embedded()
    }

    fn try_ancestors(start: &Path) -> Result<Option<Self>> {
        let mut dir = start;
        for _ in 0..MAX_ANCESTOR_DEPTH {
            let candidate = dir.join("data").join("seeds");
            if let Some(lib) = Self::load_from_dir(&candidate)? {
                return Ok(Some(lib));
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => return Ok(None),
            }
        }
        Ok(None)
    }
}

impl SeedPackLibrary {
    pub fn from_pairs<I, S>(pairs: I) -> Result<Self>
    where
        I: IntoIterator<Item = (S, S)>,
        S: AsRef<str>,
    {
        let mut lib = SeedPackLibrary::default();
        for (id, raw) in pairs {
            let id = id.as_ref().to_string();
            let pack = parse_pack(raw.as_ref())
                .map_err(|e| Error::InvalidPath(format!("seed pack {id} failed to parse: {e}")))?;
            lib.packs.insert(id.clone(), pack);
            lib.sources.insert(id, PathBuf::from("<embedded>"));
        }
        Ok(lib)
    }

    pub fn get(&self, app_id: &str) -> Option<&SeedPack> {
        self.packs.get(app_id)
    }

    // Stage 4 wires `ids` + `source_for` + `len` + `is_empty` into the
    // shipped-pack contract test; Stage 5 wires them into the
    // `--list-seed-packs` CLI subcommand. Allow until then.
    #[allow(dead_code)]
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.packs.keys().map(String::as_str)
    }

    #[allow(dead_code)]
    pub fn source_for(&self, app_id: &str) -> Option<&Path> {
        self.sources.get(app_id).map(PathBuf::as_path)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.packs.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.packs.is_empty()
    }

    /// Validate every pack without mutating any disk state, so a bad pack
    /// fails at load with a clear message rather than the next time someone
    /// seeds.
    pub fn validate(&self) -> Result<()> {
        for (id, pack) in &self.packs {
            validate_pack(pack).map_err(|e| match e {
                Error::InvalidPath(msg) => Error::InvalidPath(format!("seed pack `{id}`: {msg}")),
                other => other,
            })?;
        }
        Ok(())
    }
}

fn validate_pack(pack: &SeedPack) -> Result<()> {
    // `config_path` is joined onto the instance directory, so it must stay inside it.
    if !crate::paths::is_safe_relative_path(&pack.config_path) {
        return Err(Error::InvalidPath(format!(
            "field `config_path` = {:?} must be a relative path inside the instance directory \
             (not absolute, no `..`, not empty)",
            pack.config_path
        )));
    }
    if pack.proxy_setup.as_ref().is_some_and(|p| p.app.is_empty()) {
        return Err(Error::InvalidPath(
            "proxy_setup.app must be non-empty".into(),
        ));
    }
    for (key, rv) in &pack.rotation {
        match rv {
            RotationValue::Values { values } => {
                if values.is_empty() {
                    return Err(Error::InvalidPath(format!(
                        "rotation `{key}`: values list must be non-empty"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// Apply a seed pack to a JSON object map already populated by the
/// AppImage's first-run write. Mutates `base` in place.
///
/// Order of operations is fixed: `static` literals first (so rotation values
/// can override them on key collisions, though well-formed packs don't
/// collide), then `rotation`. `proxy_setup` is not applied here: seeding runs
/// it before this, so the proxy keys portmux wrote are already in `base`.
///
/// `instance_index` is 1-based; the rotation index is `(idx - 1) %
/// values.len()` to match the historical `feather_skin()` behavior in
/// `seed.rs`.
pub fn apply_pack(
    pack: &SeedPack,
    instance_index: u32,
    base: &mut Map<String, Value>,
) -> Result<()> {
    // 1. Static literals.
    for (k, v) in &pack.static_keys {
        base.insert(k.clone(), v.clone());
    }

    // 2. Per-instance rotations.
    for (key, rv) in &pack.rotation {
        match rv {
            RotationValue::Values { values } => {
                if values.is_empty() {
                    return Err(Error::InvalidPath(format!(
                        "seed pack: rotation `{key}` has empty values list"
                    )));
                }
                let idx = rotation_index(instance_index, values.len());
                base.insert(key.clone(), values[idx].clone());
            }
        }
    }

    Ok(())
}

fn rotation_index(instance_index: u32, len: usize) -> usize {
    (instance_index.saturating_sub(1) as usize) % len
}

/// The one lock for tests that read or set the seeds dir env vars. Every test
/// that seeds reads them, so the tests of `seed` and of this module share it.
#[cfg(test)]
pub(crate) fn seeds_env_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn proxy_setup_section_parses_and_round_trips() {
        let raw = r#"{"config_path":"x.json","proxy_setup":{"app":"feather"}}"#;
        let lib = SeedPackLibrary::from_pairs(vec![("feather", raw)]).expect("must parse");
        lib.validate().expect("must validate");
        let pack = lib.get("feather").unwrap();
        assert_eq!(pack.proxy_setup.as_ref().unwrap().app, "feather");
        let again: SeedPack = serde_json::from_str(&serde_json::to_string(pack).unwrap()).unwrap();
        assert_eq!(&again, pack);
    }

    #[test]
    fn legacy_preset_section_is_refused_by_name() {
        let raw = r#"{"config_path":"x.json","preset":{"k":{"from":"some_field"}}}"#;
        let err = SeedPackLibrary::from_pairs(vec![("old-wallet", raw)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("old-wallet"), "names the pack: {err}");
        assert!(err.contains("`preset`"), "names the section: {err}");
        assert!(err.contains("proxy_setup"), "says how to migrate: {err}");
    }

    #[test]
    fn legacy_port_verb_is_refused_by_name() {
        let raw = r#"{"config_path":"x.json","preset":{"socks5Port":{"port":{"isolation":"dest-addr","field":"port"}}}}"#;
        let err = SeedPackLibrary::from_pairs(vec![("old-wallet", raw)])
            .unwrap_err()
            .to_string();
        assert!(err.contains("old-wallet"), "names the pack: {err}");
        assert!(err.contains("`port`"), "names the verb: {err}");
        assert!(err.contains("socks5Port"), "names the key: {err}");
        assert!(err.contains("proxy_setup"), "says how to migrate: {err}");
    }

    #[test]
    fn legacy_pack_on_disk_is_refused_by_file() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("old-wallet.json"),
            r#"{"config_path":"x.json","preset":{"k":{"port":{"isolation":"none","field":"url"}}}}"#,
        )
        .unwrap();
        // Refused by name, with the migration message; the directory still loads
        // (WF-SEED-001 step 2: a refused pack is skipped, the others load).
        let lib = SeedPackLibrary::load_from_dir(tmp.path()).unwrap().unwrap();
        assert!(
            lib.get("old-wallet").is_none(),
            "a legacy pack is not loaded"
        );
        let err = lib.refusal_for("old-wallet").expect("refused").to_string();
        assert!(err.contains("old-wallet.json"), "names the file: {err}");
        assert!(err.contains("proxy_setup"), "says how to migrate: {err}");
    }

    #[test]
    fn validate_rejects_empty_proxy_setup_app() {
        let lib = SeedPackLibrary::from_pairs(vec![(
            "p",
            r#"{"config_path":"x","proxy_setup":{"app":""}}"#,
        )])
        .unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(err.contains("proxy_setup.app"), "got: {err}");
    }

    #[test]
    fn minimal_pack_parses() {
        let raw = r#"{"config_path":"x.json"}"#;
        let p: SeedPack = serde_json::from_str(raw).unwrap();
        assert_eq!(p.config_path, "x.json");
        assert!(p.first_run_args.is_empty());
        assert!(p.static_keys.is_empty());
        assert!(p.proxy_setup.is_none());
        assert!(p.rotation.is_empty());
        assert!(p.icons.is_none());
    }

    /// Regression (F1, 2026-06-30 functional test): production companion packs
    /// still carry a legacy `icons` block. The `deny_unknown_fields` schema
    /// must accept (and ignore) it rather than reject the whole pack.
    #[test]
    fn pack_with_legacy_icons_field_is_accepted_and_ignored() {
        let raw = r#"{
            "config_path": "{appimage}.config/config.json",
            "icons": { "rotation": ["feather1.png", "feather2.png"] }
        }"#;
        let p: SeedPack = serde_json::from_str(raw)
            .expect("pack carrying a legacy `icons` field must still parse");
        assert_eq!(p.config_path, "{appimage}.config/config.json");
        // The field is captured but not interpreted as a recipe verb.
        assert!(p.icons.is_some());
        assert!(p.static_keys.is_empty() && p.proxy_setup.is_none() && p.rotation.is_empty());
    }

    #[test]
    fn full_pack_parses_all_verbs() {
        let raw = r#"{
            "config_path": "{appimage}.config/feather/settings.json",
            "first_run_args": ["--offline"],
            "static": { "proxy": 1, "torOnlyAllowOnion": true },
            "proxy_setup": { "app": "feather" },
            "rotation": {
                "skin":       { "values": ["a","b","c"] }
            }
        }"#;
        let p: SeedPack = serde_json::from_str(raw).unwrap();
        assert_eq!(p.first_run_args, vec!["--offline"]);
        assert_eq!(p.static_keys.len(), 2);
        assert_eq!(p.proxy_setup.as_ref().unwrap().app, "feather");
        assert!(matches!(
            p.rotation.get("skin"),
            Some(RotationValue::Values { values }) if values.len() == 3
        ));
    }

    #[test]
    fn rejects_unknown_top_level_key() {
        let raw = r#"{"config_path":"x","whatever":42}"#;
        let err = serde_json::from_str::<SeedPack>(raw).unwrap_err();
        assert!(
            err.to_string().contains("whatever") || err.to_string().contains("unknown field"),
            "expected unknown-field error, got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_rotation_key() {
        let raw = r#"{
            "config_path":"x",
            "rotation": { "k": { "vals": [1,2] } }
        }"#;
        assert!(serde_json::from_str::<SeedPack>(raw).is_err());
    }

    #[test]
    fn library_from_pairs_loads_and_indexes() {
        let lib = SeedPackLibrary::from_pairs(vec![
            ("alpha", r#"{"config_path":"a.json"}"#),
            ("beta", r#"{"config_path":"b.json"}"#),
        ])
        .unwrap();
        assert_eq!(lib.len(), 2);
        assert!(!lib.is_empty());
        assert_eq!(lib.get("alpha").unwrap().config_path, "a.json");
        assert_eq!(lib.get("beta").unwrap().config_path, "b.json");
        assert!(lib.get("missing").is_none());
        let mut ids: Vec<&str> = lib.ids().collect();
        ids.sort();
        assert_eq!(ids, vec!["alpha", "beta"]);
    }

    #[test]
    fn library_reports_parse_errors_with_pack_id() {
        let err = SeedPackLibrary::from_pairs(vec![("oops", "{not json")]).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("oops"), "expected pack id in error, got: {s}");
    }

    fn env_lock() -> &'static std::sync::Mutex<()> {
        seeds_env_lock()
    }

    /// Build a tempdir with `data/seeds/<id>.json` files so the ancestor
    /// walk and load_from_dir can be exercised in isolation.
    fn write_seeds_dir(root: &std::path::Path, packs: &[(&str, &str)]) -> PathBuf {
        let dir = root.join("data").join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        for (id, raw) in packs {
            std::fs::write(dir.join(format!("{id}.json")), raw).unwrap();
        }
        dir
    }

    #[test]
    fn discover_load_explicit_dir_wins() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_seeds_dir(tmp.path(), &[("alpha", r#"{"config_path":"a.json"}"#)]);
        let lib = SeedPackLibrary::discover_load(Some(&dir)).unwrap();
        assert_eq!(lib.len(), 1);
        assert!(lib.get("alpha").is_some());
        assert_eq!(
            lib.source_for("alpha").map(Path::to_path_buf),
            Some(dir.join("alpha.json"))
        );
    }

    #[test]
    fn discover_load_explicit_missing_dir_errors() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("does-not-exist");
        let err = SeedPackLibrary::discover_load(Some(&bogus)).unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }

    #[test]
    fn discover_load_explicit_empty_dir_errors() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let empty = tmp.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = SeedPackLibrary::discover_load(Some(&empty)).unwrap_err();
        assert!(
            err.to_string().contains("no *.json"),
            "expected empty-dir error, got: {err}"
        );
    }

    #[test]
    fn discover_load_env_var_wins_over_walk() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let dir = write_seeds_dir(tmp.path(), &[("from_env", r#"{"config_path":"e.json"}"#)]);
        let prev = std::env::var(ENV_SEEDS_DIR).ok();
        std::env::set_var(ENV_SEEDS_DIR, &dir);
        let lib = SeedPackLibrary::discover_load(None).unwrap();
        match prev {
            Some(v) => std::env::set_var(ENV_SEEDS_DIR, v),
            None => std::env::remove_var(ENV_SEEDS_DIR),
        }
        assert!(lib.get("from_env").is_some());
    }

    #[test]
    fn user_seeds_dir_returns_xdg_data_path() {
        // Just sanity-check the path shape — the actual location is
        // user-environment dependent. dirs::data_dir() returns
        // ~/.local/share on Linux unless XDG_DATA_HOME overrides.
        let p = user_seeds_dir().expect("user_seeds_dir resolves");
        let s = p.to_string_lossy();
        assert!(
            s.ends_with("appimage-control/seeds"),
            "expected path to end with appimage-control/seeds, got: {s}"
        );
    }

    #[test]
    fn discover_load_user_dir_wins_over_ancestor_walk() {
        let _g = env_lock().lock().unwrap();
        // Point XDG_DATA_HOME at a tempdir, populate the per-user
        // seeds dir, and verify discover_load picks it up over the
        // executable ancestor walk (which would otherwise hit the
        // repo's real data/seeds during local testing).
        let tmp = tempfile::tempdir().unwrap();
        let xdg = tmp.path().to_path_buf();
        let user_dir = xdg.join("appimage-control").join("seeds");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(
            user_dir.join("user-priority-test.json"),
            r#"{"config_path":"u.json"}"#,
        )
        .unwrap();

        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let prev_seeds = std::env::var(ENV_SEEDS_DIR).ok();
        std::env::set_var("XDG_DATA_HOME", &xdg);
        std::env::remove_var(ENV_SEEDS_DIR);

        let lib = SeedPackLibrary::discover_load(None).unwrap();
        let got = lib.get("user-priority-test").is_some();

        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        if let Some(v) = prev_seeds {
            std::env::set_var(ENV_SEEDS_DIR, v);
        }

        assert!(got, "user dir should win over ancestor walk");
    }

    #[test]
    fn discover_load_env_var_still_wins_over_user_dir() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let xdg = tmp.path().join("xdg");
        let user_dir = xdg.join("appimage-control").join("seeds");
        std::fs::create_dir_all(&user_dir).unwrap();
        std::fs::write(
            user_dir.join("from-user.json"),
            r#"{"config_path":"x.json"}"#,
        )
        .unwrap();

        let env_dir = tmp.path().join("env");
        std::fs::create_dir_all(&env_dir).unwrap();
        std::fs::write(env_dir.join("from-env.json"), r#"{"config_path":"y.json"}"#).unwrap();

        let prev_xdg = std::env::var("XDG_DATA_HOME").ok();
        let prev_seeds = std::env::var(ENV_SEEDS_DIR).ok();
        std::env::set_var("XDG_DATA_HOME", &xdg);
        std::env::set_var(ENV_SEEDS_DIR, &env_dir);

        let lib = SeedPackLibrary::discover_load(None).unwrap();
        let env_won = lib.get("from-env").is_some();
        let user_present = lib.get("from-user").is_some();

        match prev_xdg {
            Some(v) => std::env::set_var("XDG_DATA_HOME", v),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        match prev_seeds {
            Some(v) => std::env::set_var(ENV_SEEDS_DIR, v),
            None => std::env::remove_var(ENV_SEEDS_DIR),
        }

        assert!(env_won, "env var should win");
        assert!(
            !user_present,
            "user dir should not be visible when env wins"
        );
    }

    #[test]
    fn try_ancestors_finds_data_seeds_from_subdir() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        write_seeds_dir(tmp.path(), &[("anc", r#"{"config_path":"a.json"}"#)]);
        let nested = tmp.path().join("a").join("b").join("c");
        std::fs::create_dir_all(&nested).unwrap();
        let lib = SeedPackLibrary::try_ancestors(&nested).unwrap().unwrap();
        assert!(lib.get("anc").is_some());
    }

    #[test]
    fn try_ancestors_returns_none_when_nothing_found() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        // No data/seeds/ anywhere under tmp.
        let result = SeedPackLibrary::try_ancestors(tmp.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn load_from_dir_skips_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ok.json"), r#"{"config_path":"o.json"}"#).unwrap();
        std::fs::write(dir.join("README.md"), "just a readme").unwrap();
        std::fs::write(dir.join("template.json.txt"), "{not json}").unwrap();
        let lib = SeedPackLibrary::load_from_dir(&dir).unwrap().unwrap();
        assert_eq!(lib.len(), 1);
        assert!(lib.get("ok").is_some());
    }

    #[test]
    fn load_from_dir_reports_filename_on_malformed_pack() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("bad.json"), "{not json}").unwrap();
        // Refused by file name with the parse position, not a whole-directory failure.
        let lib = SeedPackLibrary::load_from_dir(&dir).unwrap().unwrap();
        let s = lib
            .refusal_for("bad")
            .expect("bad.json refused")
            .to_string();
        assert!(
            s.contains("bad.json"),
            "expected filename in error, got: {s}"
        );
        // serde_json's Display already includes "line N column M" for
        // structural errors — verify that text reaches the user.
        assert!(
            s.contains("line ") && s.contains("column "),
            "expected line/column from serde_json in error, got: {s}"
        );
    }

    #[test]
    fn load_from_dir_attributes_validation_errors_to_pack_id() {
        // Pack parses cleanly but has an empty rotation list. Each pack is
        // validated inside load_from_dir, so the mistake surfaces here (the
        // pack is refused by id), not at first seed.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("typo.json"),
            r#"{"config_path":"x","rotation":{"k":{"values":[]}}}"#,
        )
        .unwrap();
        let lib = SeedPackLibrary::load_from_dir(&dir).unwrap().unwrap();
        assert!(lib.get("typo").is_none(), "an invalid pack is not loaded");
        let err = lib.refusal_for("typo").expect("typo refused").to_string();
        assert!(
            err.contains("typo"),
            "expected pack id `typo` in error, got: {err}"
        );
        assert!(
            err.contains("rotation `k`"),
            "expected offending key name, got: {err}"
        );
    }

    fn parse(raw: &str) -> SeedPack {
        serde_json::from_str(raw).expect("test pack must parse")
    }

    fn apply(pack: &SeedPack, idx: u32) -> Map<String, Value> {
        let mut base = Map::new();
        apply_pack(pack, idx, &mut base).expect("apply_pack must succeed");
        base
    }

    #[test]
    fn apply_static_literals_of_each_type() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "static": { "n": 42, "b": true, "s": "hi", "arr": [1,2] }
        }"#,
        );
        let out = apply(&pack, 1);
        assert_eq!(out["n"], json!(42));
        assert_eq!(out["b"], json!(true));
        assert_eq!(out["s"], json!("hi"));
        assert_eq!(out["arr"], json!([1, 2]));
    }

    #[test]
    fn apply_rotation_values_indexes_by_instance() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "rotation": { "skin": { "values": ["A","B","C"] } }
        }"#,
        );
        assert_eq!(apply(&pack, 1)["skin"], json!("A"));
        assert_eq!(apply(&pack, 2)["skin"], json!("B"));
        assert_eq!(apply(&pack, 3)["skin"], json!("C"));
        assert_eq!(apply(&pack, 4)["skin"], json!("A"));
        // instance_index=0 should be treated as 1 (saturating_sub).
        assert_eq!(apply(&pack, 0)["skin"], json!("A"));
    }

    #[test]
    fn rotation_from_preset_array_variant_is_no_longer_a_valid_pack_shape() {
        // The legacy `from_preset_array` rotation variant was retired
        // together with the `feather_ports` preset-array field; ports are
        // portmux's now, through `proxy_setup`. Confirm the old shape
        // fails to parse.
        let raw = r#"{
            "config_path":"x",
            "rotation": {
                "socks5Port": { "from_preset_array":"feather_ports", "as_string":true }
            }
        }"#;
        let err = serde_json::from_str::<SeedPack>(raw).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("did not match any variant") || msg.contains("unknown"),
            "expected rejection of legacy rotation shape, got: {msg}"
        );
    }

    #[test]
    fn apply_rotation_empty_values_errors() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "rotation": { "k": { "values": [] } }
        }"#,
        );
        let mut base = Map::new();
        let err = apply_pack(&pack, 1, &mut base).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    // `validate_rejects_scalar_field_used_as_array` was retired with
    // the `from_preset_array` rotation variant. The equivalent
    // schema-level rejection is now covered by
    // `rotation_from_preset_array_variant_is_no_longer_a_valid_pack_shape`
    // above — `serde_json::from_str::<SeedPack>` itself rejects the
    // legacy shape before it ever reaches `validate`.

    #[test]
    fn validate_rejects_empty_config_path() {
        let lib = SeedPackLibrary::from_pairs(vec![("p5", r#"{"config_path":""}"#)]).unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(err.contains("p5"), "got: {err}");
    }

    #[test]
    fn validate_accepts_realistic_packs() {
        // Same JSON shapes the companion seed-pack repo ships for the
        // three real apps.
        let feather = r#"{
            "config_path":"{appimage}.config/feather/settings.json",
            "first_run_args":[],
            "static":{"torOnlyAllowOnion":true,"torPrivacyLevel":2},
            "proxy_setup":{"app":"feather"},
            "rotation":{
                "skin":{"values":["Breeze/Light","Breeze/Dark","Breeze/Light"]}
            }
        }"#;
        let btc = r#"{
            "config_path":"{appimage}.home/.electrum/config",
            "first_run_args":["--offline"],
            "static":{"oneserver":true,"server":"x.onion:50002:s"},
            "proxy_setup":{"app":"electrum-btc"}
        }"#;
        let ltc = r#"{
            "config_path":"{appimage}.home/.electrum-ltc/config",
            "first_run_args":["--offline"],
            "proxy_setup":{"app":"electrum-ltc"}
        }"#;
        let lib = SeedPackLibrary::from_pairs(vec![
            ("feather", feather),
            ("electrum-btc", btc),
            ("electrum-ltc", ltc),
        ])
        .unwrap();
        lib.validate()
            .expect("realistic packs should validate clean");
    }

    #[test]
    fn list_rows_marks_embedded_packs_and_reports_disk_source() {
        // Embedded packs surface as "<embedded>".
        let emb = SeedPackLibrary::from_pairs(vec![
            ("alpha", r#"{"config_path":"a.json"}"#),
            ("beta", r#"{"config_path":"b.json"}"#),
        ])
        .unwrap();
        let rows = emb.list_rows();
        assert_eq!(rows.len(), 2);
        for (id, src) in &rows {
            assert_eq!(src, "<embedded>", "row for {id} should be embedded");
        }
        // Stable id order.
        assert_eq!(rows[0].0, "alpha");
        assert_eq!(rows[1].0, "beta");

        // Disk-loaded packs surface their absolute path.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("foo.json"), r#"{"config_path":"f.json"}"#).unwrap();
        let lib = SeedPackLibrary::load_from_dir(&dir).unwrap().unwrap();
        let rows = lib.list_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "foo");
        assert!(
            rows[0].1.ends_with("foo.json"),
            "disk row should end with foo.json, got {}",
            rows[0].1
        );
    }

    #[test]
    fn embedded_library_loads_from_build_rs_output() {
        // This repo ships no curated packs (the architecture is
        // public, the content is in an operator-managed companion
        // repo). The embedded slice is allowed to be empty; the
        // contract is just that whatever build.rs emitted parses
        // through serde without panicking and validates clean.
        let lib = SeedPackLibrary::embedded().expect("embedded packs must parse");
        for id in lib.ids().collect::<Vec<_>>() {
            let pack = lib.get(id).expect("id from ids() resolves");
            assert!(
                !pack.config_path.is_empty(),
                "pack {id}: config_path should be non-empty"
            );
        }
    }

    // --- Task 2.3: config_path stays inside the instance directory ------------------------

    #[test]
    fn config_path_refuses_absolute_and_parent_paths_naming_pack_and_field() {
        for bad in [
            "/abs/x.json",
            "../x",
            "{appimage}/../../x",
            "a/./../b",
            "",
            ".",
        ] {
            let raw = format!(
                r#"{{"config_path":{}}}"#,
                serde_json::to_string(bad).unwrap()
            );
            let lib = SeedPackLibrary::from_pairs(vec![("evilpack", raw.as_str())]).unwrap();
            let err = lib
                .validate()
                .expect_err(&format!("config_path {bad:?} must be refused"))
                .to_string();
            assert!(
                err.contains("evilpack") && err.contains("`config_path`"),
                "config_path {bad:?}: error must name the pack and the field: {err}"
            );
        }
    }

    #[test]
    fn config_path_refused_pack_is_skipped_and_others_load() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("evilpack.json"),
            r#"{"config_path":"{appimage}/../../x"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("goodpack.json"),
            r#"{"config_path":"{appimage}.config/good/settings.json"}"#,
        )
        .unwrap();
        let lib = SeedPackLibrary::load_from_dir(&dir).unwrap().unwrap();
        assert!(
            lib.get("evilpack").is_none(),
            "the unsafe pack must not load"
        );
        assert!(lib.get("goodpack").is_some(), "the other pack still loads");
        let err = lib.refusal_for("evilpack").expect("evilpack refused");
        assert!(
            err.contains("evilpack") && err.contains("`config_path`"),
            "{err}"
        );
    }

    #[test]
    fn config_path_accepts_shipped_shapes() {
        for ok in [
            "{appimage}.config/feather/settings.json",
            "{appimage}.home/.electrum/config",
            "settings.json",
        ] {
            let raw = format!(
                r#"{{"config_path":{}}}"#,
                serde_json::to_string(ok).unwrap()
            );
            let lib = SeedPackLibrary::from_pairs(vec![("okpack", raw.as_str())]).unwrap();
            lib.validate()
                .unwrap_or_else(|e| panic!("{ok:?} must load: {e}"));
        }
    }

    #[test]
    fn unreadable_pack_is_refused_not_fatal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("good.json"), r#"{"config_path":"g.json"}"#).unwrap();
        // Not UTF-8: read_to_string fails.
        std::fs::write(dir.join("binary.json"), [0xff, 0xfe, 0x00, 0x80]).unwrap();
        let lib = SeedPackLibrary::load_from_dir(&dir)
            .expect("one unreadable pack must not fail the directory")
            .unwrap();
        assert!(lib.get("good").is_some());
        assert!(lib
            .refusal_for("binary")
            .is_some_and(|r| r.contains("binary.json")));
    }
}
