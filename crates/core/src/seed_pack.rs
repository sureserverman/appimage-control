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
//! The schema is deliberately tiny — three "verbs" cover every
//! transformation today's hand-written `patch_*_config` functions perform:
//!
//! - `static`: plain JSON literals merged into the config map.
//! - `preset`: a value derived from the active [`crate::seed::SeedConfig`]
//!   (either a direct field copy via `from`, or a `format` template that
//!   interpolates one or more scalar preset fields), with optional
//!   `drop_if` to omit the key when a bool preset field is true.
//! - `rotation`: a per-instance value indexed by `(instance_index - 1) mod
//!   N`, drawn either from inline `values` or from a named u16 preset
//!   array (with optional `as_string` to stringify the result, matching
//!   Feather's `socks5Port` quirk).

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::{Error, Result};
use crate::seed::{PresetField, SeedConfig};

/// Environment variable that, when set to a non-empty path, forces the
/// loader to use exactly that directory of `*.json` packs and skip the
/// ancestor walk and `/app/share` / `/usr/share` fallbacks. Mirrors
/// `APPIMAGE_CONTROL_CATALOG`'s shape.
pub const ENV_SEEDS_DIR: &str = "APPIMAGE_CONTROL_SEEDS_DIR";

/// Bound on the ancestor walk so a misconfigured cwd can't loop forever.
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
/// non-empty), with no empty `preset`/`rotation`/`icons` noise.
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

    /// Keys whose values come from the active [`crate::seed::SeedConfig`].
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub preset: BTreeMap<String, PresetValue>,

    /// Keys whose values rotate per instance index.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rotation: BTreeMap<String, RotationValue>,

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

/// One `preset` entry. `untagged` so the JSON shape disambiguates: the
/// presence of `from` means [`PresetValue::From`], `format` means
/// [`PresetValue::Format`], `port` means [`PresetValue::Port`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, untagged)]
pub enum PresetValue {
    /// Direct copy of one preset field (`socks5_host`,
    /// `feather_use_local_tor`, etc.) into the JSON map.
    From {
        from: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drop_if: Option<String>,
    },
    /// Templated string with `{field_name}` placeholders that resolve
    /// against the active preset's scalar fields.
    Format {
        format: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drop_if: Option<String>,
    },
    /// Per-(app, instance) port assignment resolved at apply time by
    /// shelling out to `portmux-port ensure …` (see
    /// [`crate::portmux_port::ensure_port`]). The chosen field
    /// (`port` / `host` / `url`) of the returned `EnsuredPort` lands
    /// in the JSON map under this key.
    Port { port: PortLookup },
}

/// Body of a `port` verb. `app` defaults to the seed pack's own app
/// id (passed into [`apply_pack`]). `purpose` defaults to `socks`.
/// `isolation` and `field` are required.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PortLookup {
    /// App label fed to `portmux-port --app`. Defaults to the
    /// seed-pack's own app id; specify explicitly when one pack
    /// needs ports owned by a different app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Tor listener kind (`socks` / `http` / `trans` / `dns`).
    /// Defaults to `socks`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purpose: Option<PortPurposeSpec>,
    /// Stream-isolation class (`none`, `dest-addr`, `dest-port`,
    /// `both`, `http-tunnel`).
    pub isolation: IsolationSpec,
    /// Which field of the resolved `EnsuredPort` lands in the JSON
    /// map.
    pub field: PortFieldSpec,
    /// When `true`, emit `port` as a JSON string (`"9168"`) instead
    /// of a number (`9168`). Only meaningful when `field = "port"`.
    /// Mirrors today's rotation `as_string` flag.
    #[serde(default, skip_serializing_if = "is_false")]
    pub as_string: bool,
    /// Optional bool preset-field name; when that field is `true`,
    /// the resolved value is omitted entirely (and any existing key
    /// is removed from the base map).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drop_if: Option<String>,
}

/// `skip_serializing_if` predicate for `bool` fields that default to
/// `false` (so a synthesised pack never emits a redundant `false`).
fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PortPurposeSpec {
    Socks,
    Http,
    Trans,
    Dns,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationSpec {
    None,
    DestAddr,
    DestPort,
    Both,
    HttpTunnel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PortFieldSpec {
    Port,
    Host,
    Url,
}

impl PortPurposeSpec {
    fn to_runner(self) -> crate::portmux_port::Purpose {
        use crate::portmux_port::Purpose;
        match self {
            PortPurposeSpec::Socks => Purpose::Socks,
            PortPurposeSpec::Http => Purpose::Http,
            PortPurposeSpec::Trans => Purpose::Trans,
            PortPurposeSpec::Dns => Purpose::Dns,
        }
    }
}

impl IsolationSpec {
    fn to_runner(self) -> crate::portmux_port::IsolationClass {
        use crate::portmux_port::IsolationClass;
        match self {
            IsolationSpec::None => IsolationClass::None,
            IsolationSpec::DestAddr => IsolationClass::DestAddr,
            IsolationSpec::DestPort => IsolationClass::DestPort,
            IsolationSpec::Both => IsolationClass::Both,
            IsolationSpec::HttpTunnel => IsolationClass::HttpTunnel,
        }
    }
}

/// One `rotation` entry. Today the only supported variant is
/// [`RotationValue::Values`]; the legacy `from_preset_array` variant
/// was retired with the `[u16; 3]` preset array it referenced. New
/// per-instance port assignments go through the `port` verb in the
/// `preset` section instead.
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
    /// `*.json` files; `Err` when a present `*.json` fails to parse.
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
            let stem = path
                .file_stem()
                .and_then(OsStr::to_str)
                .ok_or_else(|| {
                    Error::InvalidPath(format!("seed pack: bad filename: {}", path.display()))
                })?
                .to_string();
            let raw = fs::read_to_string(&path).map_err(Error::Io)?;
            let pack: SeedPack = serde_json::from_str(&raw)
                .map_err(|e| Error::InvalidPath(format!("seed pack {}: {e}", path.display())))?;
            lib.packs.insert(stem.clone(), pack);
            lib.sources.insert(stem, path);
        }
        if lib.is_empty() {
            Ok(None)
        } else {
            lib.validate()?;
            Ok(Some(lib))
        }
    }

    /// Resolve a seed-pack library against the same fallback chain
    /// `Catalog::discover_load` uses for `catalog.json`:
    ///
    /// 1. `explicit` directory (must exist and contain at least one pack).
    /// 2. `APPIMAGE_CONTROL_SEEDS_DIR` env var, if non-empty and a directory.
    /// 3. `~/.local/share/appimage-control/seeds/` — per-user override.
    ///    This is where `--update-seed-packs` (Stage 7 of the
    ///    seed-pack-extraction plan) installs companion-repo packs.
    /// 4. Walk cwd ancestors (depth 32) for `data/seeds/`.
    /// 5. Walk `current_exe()` ancestors for `data/seeds/`.
    /// 6. `/app/share/appimage-control/seeds/` (Flatpak install path).
    /// 7. `/usr/share/appimage-control/seeds/`.
    /// 8. Embedded slice baked in by `build.rs`.
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
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(lib) = Self::try_ancestors(&cwd)? {
                return Ok(lib);
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
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
            let pack: SeedPack = serde_json::from_str(raw.as_ref())
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

    /// Validate every pack against a reference [`SeedConfig`] without
    /// mutating any disk state. Surfaces bad field names and bad
    /// templates at load time so a typo crashes startup with a clear
    /// message rather than the next time someone seeds.
    ///
    /// Both presets share the same SeedConfig field set, so checking
    /// against Ubuntu (or Whonix) is sufficient to catch every typo.
    pub fn validate(&self) -> Result<()> {
        let cfg = SeedConfig::for_preset(crate::seed::SeedPreset::Ubuntu);
        for (id, pack) in &self.packs {
            validate_pack(id, pack, &cfg).map_err(|e| match e {
                Error::InvalidPath(msg) => Error::InvalidPath(format!("seed pack `{id}`: {msg}")),
                other => other,
            })?;
        }
        Ok(())
    }
}

fn validate_pack(_id: &str, pack: &SeedPack, cfg: &SeedConfig) -> Result<()> {
    if pack.config_path.is_empty() {
        return Err(Error::InvalidPath("config_path must be non-empty".into()));
    }
    for (key, pv) in &pack.preset {
        match pv {
            PresetValue::From { from, drop_if } => {
                if cfg.field(from).is_none() {
                    return Err(Error::InvalidPath(format!(
                        "preset `{key}`: from references unknown field `{from}`"
                    )));
                }
                check_drop_if(key, drop_if.as_deref(), cfg)?;
            }
            PresetValue::Format { format, drop_if } => {
                check_format_template(key, format, cfg)?;
                check_drop_if(key, drop_if.as_deref(), cfg)?;
            }
            PresetValue::Port { port } => {
                check_drop_if(key, port.drop_if.as_deref(), cfg)?;
                if let Some(app) = &port.app {
                    if app.is_empty() {
                        return Err(Error::InvalidPath(format!(
                            "preset `{key}`: port.app must be non-empty when specified"
                        )));
                    }
                }
                // `as_string` is only meaningful for field=port; warn-like
                // tightening: reject it on the other two fields so packs
                // can't accumulate dead flags.
                if port.as_string && port.field != PortFieldSpec::Port {
                    return Err(Error::InvalidPath(format!(
                        "preset `{key}`: port.as_string only applies when field=`port`"
                    )));
                }
            }
        }
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

fn check_drop_if(key: &str, drop_if: Option<&str>, cfg: &SeedConfig) -> Result<()> {
    let Some(name) = drop_if else { return Ok(()) };
    match cfg.field(name) {
        Some(PresetField::Bool(_)) => Ok(()),
        Some(_) => Err(Error::InvalidPath(format!(
            "preset `{key}`: drop_if `{name}` is not a bool field"
        ))),
        None => Err(Error::InvalidPath(format!(
            "preset `{key}`: drop_if references unknown field `{name}`"
        ))),
    }
}

fn check_format_template(key: &str, template: &str, cfg: &SeedConfig) -> Result<()> {
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| {
            Error::InvalidPath(format!(
                "preset `{key}`: format template missing closing `}}`: {template}"
            ))
        })?;
        let name = &after[..close];
        match cfg.field(name) {
            Some(_) => {}
            None => {
                return Err(Error::InvalidPath(format!(
                    "preset `{key}`: format template references unknown field `{name}`"
                )));
            }
        }
        rest = &after[close + 1..];
    }
    Ok(())
}

/// Apply a seed pack to a JSON object map already populated by the
/// AppImage's first-run write. Mutates `base` in place.
///
/// Order of operations is fixed: `static` literals first (so subsequent
/// preset/rotation values can override them on key collisions, though
/// well-formed packs don't collide), then `preset`, then `rotation`.
///
/// `instance_index` is 1-based; the rotation index is `(idx - 1) %
/// values.len()` to match the historical `feather_skin()` behavior in
/// `seed.rs`.
pub fn apply_pack(
    pack: &SeedPack,
    app_id: &str,
    cfg: &SeedConfig,
    instance_index: u32,
    base: &mut Map<String, Value>,
) -> Result<()> {
    // 1. Static literals.
    for (k, v) in &pack.static_keys {
        base.insert(k.clone(), v.clone());
    }

    // 2. Preset-derived values.
    //
    // `port_cache` short-circuits duplicate `portmux-port ensure`
    // subprocess spawns when one pack hits two `port` verbs that
    // resolve to the same `(app, instance, purpose, isolation)` tuple
    // — typical for Feather where one verb emits `host` and a sibling
    // emits `port`.
    let mut port_cache: BTreeMap<PortCacheKey, crate::portmux_port::EnsuredPort> = BTreeMap::new();
    for (key, pv) in &pack.preset {
        match pv {
            PresetValue::From { from, drop_if } => {
                if drop_if_true(cfg, drop_if.as_deref())? {
                    base.remove(key);
                    continue;
                }
                let field = cfg.field(from).ok_or_else(|| {
                    Error::InvalidPath(format!(
                        "seed pack: preset.from references unknown field `{from}`"
                    ))
                })?;
                base.insert(key.clone(), preset_field_to_json(field));
            }
            PresetValue::Format { format, drop_if } => {
                if drop_if_true(cfg, drop_if.as_deref())? {
                    base.remove(key);
                    continue;
                }
                let rendered = render_format(format, cfg)?;
                base.insert(key.clone(), Value::String(rendered));
            }
            PresetValue::Port { port } => {
                if drop_if_true(cfg, port.drop_if.as_deref())? {
                    base.remove(key);
                    continue;
                }
                let resolved =
                    resolve_port_lookup(port, app_id, cfg, instance_index, &mut port_cache)?;
                let v = match port.field {
                    PortFieldSpec::Host => Value::String(resolved.host.clone()),
                    PortFieldSpec::Port => {
                        if port.as_string {
                            Value::String(resolved.port.to_string())
                        } else {
                            json!(resolved.port)
                        }
                    }
                    PortFieldSpec::Url => {
                        Value::String(format!("socks5:{}:{}", resolved.host, resolved.port))
                    }
                };
                base.insert(key.clone(), v);
            }
        }
    }

    // 3. Per-instance rotations.
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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PortCacheKey {
    app: String,
    instance: u32,
    purpose: u8,
    isolation: u8,
}

fn resolve_port_lookup(
    spec: &PortLookup,
    pack_app_id: &str,
    cfg: &SeedConfig,
    instance_index: u32,
    cache: &mut BTreeMap<PortCacheKey, crate::portmux_port::EnsuredPort>,
) -> Result<crate::portmux_port::EnsuredPort> {
    let app = spec
        .app
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| pack_app_id.to_string());
    let purpose_spec = spec.purpose.unwrap_or(PortPurposeSpec::Socks);
    let purpose = purpose_spec.to_runner();
    let isolation = spec.isolation.to_runner();

    let key = PortCacheKey {
        app: app.clone(),
        instance: instance_index,
        purpose: purpose as u8,
        isolation: isolation as u8,
    };
    if let Some(hit) = cache.get(&key) {
        return Ok(hit.clone());
    }

    let host_loopback = cfg.socks5_host.clone();
    let ensured = crate::portmux_port::ensure_port(
        &app,
        instance_index,
        purpose,
        isolation,
        &host_loopback,
    )?;
    cache.insert(key, ensured.clone());
    Ok(ensured)
}

fn rotation_index(instance_index: u32, len: usize) -> usize {
    (instance_index.saturating_sub(1) as usize) % len
}

/// Read a bool preset field by name. Errors when the name is unknown
/// or refers to a non-bool field.
fn drop_if_true(cfg: &SeedConfig, name: Option<&str>) -> Result<bool> {
    let Some(n) = name else {
        return Ok(false);
    };
    match cfg.field(n) {
        Some(PresetField::Bool(b)) => Ok(b),
        Some(_) => Err(Error::InvalidPath(format!(
            "seed pack: drop_if `{n}` is not a bool field"
        ))),
        None => Err(Error::InvalidPath(format!(
            "seed pack: drop_if references unknown field `{n}`"
        ))),
    }
}

fn preset_field_to_json(f: PresetField<'_>) -> Value {
    match f {
        PresetField::Bool(b) => Value::Bool(b),
        PresetField::Str(s) => Value::String(s.to_string()),
    }
}

/// Render a `preset.format` template by replacing `{field_name}`
/// placeholders with scalar preset field values. Errors when a
/// placeholder names an unknown field, or names a non-scalar
/// (`feather_ports`-style) field that can't be inlined into a string.
fn render_format(template: &str, cfg: &SeedConfig) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let close = after.find('}').ok_or_else(|| {
            Error::InvalidPath(format!(
                "seed pack: format template missing closing `}}`: {template}"
            ))
        })?;
        let name = &after[..close];
        let field = cfg.field(name).ok_or_else(|| {
            Error::InvalidPath(format!(
                "seed pack: format template references unknown field `{name}`"
            ))
        })?;
        match field {
            PresetField::Bool(b) => out.push_str(if b { "true" } else { "false" }),
            PresetField::Str(s) => out.push_str(s),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seed::SeedPreset;

    #[test]
    fn minimal_pack_parses() {
        let raw = r#"{"config_path":"x.json"}"#;
        let p: SeedPack = serde_json::from_str(raw).unwrap();
        assert_eq!(p.config_path, "x.json");
        assert!(p.first_run_args.is_empty());
        assert!(p.static_keys.is_empty());
        assert!(p.preset.is_empty());
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
        assert!(p.static_keys.is_empty() && p.preset.is_empty() && p.rotation.is_empty());
    }

    #[test]
    fn full_pack_parses_all_verbs() {
        let raw = r#"{
            "config_path": "{appimage}.config/feather/settings.json",
            "first_run_args": ["--offline"],
            "static": { "proxy": 1, "torOnlyAllowOnion": true },
            "preset": {
                "useLocalTor": { "from": "feather_use_local_tor" },
                "socks5Host":  { "from": "socks5_host", "drop_if": "feather_use_local_tor" },
                "proxy":       { "format": "socks5:{socks5_host}:{socks5_host}" },
                "socks5Port":  { "port": { "isolation": "dest-addr", "field": "port", "as_string": true } }
            },
            "rotation": {
                "skin":       { "values": ["a","b","c"] }
            }
        }"#;
        let p: SeedPack = serde_json::from_str(raw).unwrap();
        assert_eq!(p.first_run_args, vec!["--offline"]);
        assert_eq!(p.static_keys.len(), 2);
        assert!(matches!(
            p.preset.get("useLocalTor"),
            Some(PresetValue::From { from, drop_if: None }) if from == "feather_use_local_tor"
        ));
        assert!(matches!(
            p.preset.get("socks5Host"),
            Some(PresetValue::From { from, drop_if: Some(d) })
                if from == "socks5_host" && d == "feather_use_local_tor"
        ));
        assert!(matches!(
            p.preset.get("proxy"),
            Some(PresetValue::Format { format, drop_if: None })
                if format == "socks5:{socks5_host}:{socks5_host}"
        ));
        assert!(matches!(
            p.preset.get("socks5Port"),
            Some(PresetValue::Port { port }) if port.field == PortFieldSpec::Port
                && port.as_string
                && port.isolation == IsolationSpec::DestAddr
        ));
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
    fn rejects_unknown_preset_key() {
        // `from` typo'd as `frm` — should fail because neither variant matches.
        let raw = r#"{
            "config_path":"x",
            "preset": { "k": { "frm": "socks5_host" } }
        }"#;
        let err = serde_json::from_str::<SeedPack>(raw).unwrap_err();
        // serde's untagged error is generic; just ensure parsing fails.
        let _ = err;
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
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
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
        // cwd ancestor walk (which would otherwise hit the repo's
        // real data/seeds during local testing).
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
        let err = SeedPackLibrary::load_from_dir(&dir).unwrap_err();
        let s = err.to_string();
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
        // Pack parses cleanly but references a nonexistent preset
        // field. validate() is called from inside load_from_dir, so
        // the typo surfaces here, not at first seed.
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("seeds");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("typo.json"),
            r#"{"config_path":"x","preset":{"k":{"from":"socks_host"}}}"#,
        )
        .unwrap();
        let err = SeedPackLibrary::load_from_dir(&dir)
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("typo"),
            "expected pack id `typo` in error, got: {err}"
        );
        assert!(
            err.contains("socks_host"),
            "expected offending field name, got: {err}"
        );
    }

    fn parse(raw: &str) -> SeedPack {
        serde_json::from_str(raw).expect("test pack must parse")
    }

    fn apply(pack: &SeedPack, preset: SeedPreset, idx: u32) -> Map<String, Value> {
        let cfg = SeedConfig::for_preset(preset);
        let mut base = Map::new();
        apply_pack(pack, "test-app", &cfg, idx, &mut base).expect("apply_pack must succeed");
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
        let out = apply(&pack, SeedPreset::Ubuntu, 1);
        assert_eq!(out["n"], json!(42));
        assert_eq!(out["b"], json!(true));
        assert_eq!(out["s"], json!("hi"));
        assert_eq!(out["arr"], json!([1, 2]));
    }

    #[test]
    fn apply_preset_from_bool_and_string() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": {
                "useLocalTor": { "from": "feather_use_local_tor" },
                "socks5Host":  { "from": "socks5_host" }
            }
        }"#,
        );
        let u = apply(&pack, SeedPreset::Ubuntu, 1);
        assert_eq!(u["useLocalTor"], json!(true));
        assert_eq!(u["socks5Host"], json!("127.0.0.1"));
        let w = apply(&pack, SeedPreset::Whonix, 1);
        assert_eq!(w["useLocalTor"], json!(false));
        assert_eq!(w["socks5Host"], json!("10.152.152.10"));
    }

    #[test]
    fn apply_preset_from_drop_if_when_condition_true() {
        // useLocalTor=true under Ubuntu → drop socks5Host.
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": {
                "socks5Host": { "from":"socks5_host", "drop_if":"feather_use_local_tor" }
            }
        }"#,
        );
        let u = apply(&pack, SeedPreset::Ubuntu, 1);
        assert!(!u.contains_key("socks5Host"));
        // Whonix: useLocalTor=false → key kept.
        let w = apply(&pack, SeedPreset::Whonix, 1);
        assert_eq!(w["socks5Host"], json!("10.152.152.10"));
    }

    #[test]
    fn apply_preset_from_drop_if_removes_existing_key() {
        // Pre-populate base with the key; drop_if=true should remove it.
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": {
                "socks5Host": { "from":"socks5_host", "drop_if":"feather_use_local_tor" }
            }
        }"#,
        );
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        base.insert("socks5Host".into(), json!("stale"));
        apply_pack(&pack, "test-app", &cfg, 1, &mut base).unwrap();
        assert!(!base.contains_key("socks5Host"));
    }

    #[test]
    fn apply_preset_format_composes_template() {
        // After the Stage-4 slim, the only scalar preset fields are
        // `socks5_host` (string) and `feather_use_local_tor` (bool),
        // so format-template coverage uses these.
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": {
                "msg": { "format":"host={socks5_host} local={feather_use_local_tor}" }
            }
        }"#,
        );
        let u = apply(&pack, SeedPreset::Ubuntu, 1);
        assert_eq!(u["msg"], json!("host=127.0.0.1 local=true"));
        let w = apply(&pack, SeedPreset::Whonix, 1);
        assert_eq!(w["msg"], json!("host=10.152.152.10 local=false"));
    }

    #[test]
    fn apply_preset_format_unknown_placeholder_errors() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": { "k": { "format":"x{not_a_field}y" } }
        }"#,
        );
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        let err = apply_pack(&pack, "test-app", &cfg, 1, &mut base).unwrap_err();
        assert!(err.to_string().contains("not_a_field"));
    }

    #[test]
    fn apply_preset_from_unknown_field_errors() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "preset": { "k": { "from": "no_such" } }
        }"#,
        );
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        let err = apply_pack(&pack, "test-app", &cfg, 1, &mut base).unwrap_err();
        assert!(err.to_string().contains("no_such"));
    }

    #[test]
    fn apply_rotation_values_indexes_by_instance() {
        let pack = parse(
            r#"{
            "config_path":"x",
            "rotation": { "skin": { "values": ["A","B","C"] } }
        }"#,
        );
        assert_eq!(apply(&pack, SeedPreset::Ubuntu, 1)["skin"], json!("A"));
        assert_eq!(apply(&pack, SeedPreset::Ubuntu, 2)["skin"], json!("B"));
        assert_eq!(apply(&pack, SeedPreset::Ubuntu, 3)["skin"], json!("C"));
        assert_eq!(apply(&pack, SeedPreset::Ubuntu, 4)["skin"], json!("A"));
        // instance_index=0 should be treated as 1 (saturating_sub).
        assert_eq!(apply(&pack, SeedPreset::Ubuntu, 0)["skin"], json!("A"));
    }

    #[test]
    fn rotation_from_preset_array_variant_is_no_longer_a_valid_pack_shape() {
        // The legacy `from_preset_array` rotation variant was retired
        // together with the `feather_ports` preset-array field; new
        // per-instance port assignment goes through the `port` verb in
        // the `preset` section. Confirm the old shape fails to parse.
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
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        let err = apply_pack(&pack, "test-app", &cfg, 1, &mut base).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn validate_rejects_unknown_preset_from_field() {
        let lib = SeedPackLibrary::from_pairs(vec![(
            "p1",
            r#"{"config_path":"x","preset":{"k":{"from":"no_such"}}}"#,
        )])
        .unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(err.contains("p1") && err.contains("no_such"), "got: {err}");
    }

    #[test]
    fn validate_rejects_unknown_format_placeholder() {
        let lib = SeedPackLibrary::from_pairs(vec![(
            "p2",
            r#"{"config_path":"x","preset":{"k":{"format":"a{not_a_field}b"}}}"#,
        )])
        .unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(
            err.contains("p2") && err.contains("not_a_field"),
            "got: {err}"
        );
    }

    // `validate_rejects_scalar_field_used_as_array` was retired with
    // the `from_preset_array` rotation variant. The equivalent
    // schema-level rejection is now covered by
    // `rotation_from_preset_array_variant_is_no_longer_a_valid_pack_shape`
    // above — `serde_json::from_str::<SeedPack>` itself rejects the
    // legacy shape before it ever reaches `validate`.

    #[test]
    fn validate_rejects_non_bool_drop_if() {
        let lib = SeedPackLibrary::from_pairs(vec![(
            "p4",
            r#"{"config_path":"x","preset":{"k":{"from":"socks5_host","drop_if":"socks5_host"}}}"#,
        )])
        .unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(
            err.contains("p4") && err.contains("not a bool"),
            "got: {err}"
        );
    }

    #[test]
    fn validate_rejects_empty_config_path() {
        let lib = SeedPackLibrary::from_pairs(vec![("p5", r#"{"config_path":""}"#)]).unwrap();
        let err = lib.validate().unwrap_err().to_string();
        assert!(err.contains("p5"), "got: {err}");
    }

    #[test]
    fn validate_accepts_realistic_packs() {
        // Same JSON shapes the companion seed-pack repo ships for the
        // three real apps after the Stage-3 / Stage-4 migration to
        // the `port` verb.
        let feather = r#"{
            "config_path":"{appimage}.config/feather/settings.json",
            "first_run_args":[],
            "static":{"proxy":1,"torOnlyAllowOnion":true,"torPrivacyLevel":2},
            "preset":{
                "useLocalTor":{"from":"feather_use_local_tor"},
                "socks5Host":{"port":{"isolation":"dest-addr","field":"host","drop_if":"feather_use_local_tor"}},
                "socks5Port":{"port":{"isolation":"dest-addr","field":"port","as_string":true}}
            },
            "rotation":{
                "skin":{"values":["Breeze/Light","Breeze/Dark","Breeze/Light"]}
            }
        }"#;
        let btc = r#"{
            "config_path":"{appimage}.home/.electrum/config",
            "first_run_args":["--offline"],
            "static":{"oneserver":true,"server":"x.onion:50002:s"},
            "preset":{"proxy":{"port":{"isolation":"dest-addr","field":"url"}}}
        }"#;
        let ltc = r#"{
            "config_path":"{appimage}.home/.electrum-ltc/config",
            "first_run_args":["--offline"],
            "preset":{"proxy":{"port":{"isolation":"dest-addr","field":"url"}}}
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
}

#[cfg(test)]
mod port_verb_tests {
    use super::*;
    use crate::seed::{SeedConfig, SeedPreset};
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "aic-port-verb-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Write a stub `portmux-port` to `dir`. The stub appends one
    /// line per invocation to `call_log` so caching tests can count
    /// spawns, and echoes a fixed JSON document with a port derived
    /// from `--instance`.
    fn write_stub(dir: &std::path::Path, call_log: &std::path::Path) -> PathBuf {
        let p = dir.join("portmux-port");
        let body = format!(
            "#!/bin/sh
echo \"$@\" >> {log_q}
APP=\"\"
INST=\"\"
ISO=\"\"
while [ $# -gt 0 ]; do
  case \"$1\" in
    --app) APP=\"$2\"; shift 2 ;;
    --instance) INST=\"$2\"; shift 2 ;;
    --isolation) ISO=\"$2\"; shift 2 ;;
    *) shift ;;
  esac
done
# Synthesise a port: 9160 + instance.
PORT=$((9160 + INST))
printf '{{\"host\":\"10.152.152.10\",\"port\":%s,\"isolation\":\"%s\",\"source_file\":\"/tmp/wp.conf\",\"tor_reload_done\":false,\"already_assigned\":false}}\\n' \"$PORT\" \"$ISO\"
",
            log_q = sh_quote(call_log.to_str().unwrap())
        );
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        let mut perms = std::fs::metadata(&p).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&p, perms).unwrap();
        p
    }

    fn sh_quote(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('\'');
        for c in s.chars() {
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
        out.push('\'');
        out
    }

    fn with_bin<R>(stub: &std::path::Path, f: impl FnOnce() -> R) -> R {
        let guard = crate::portmux_port::env_gate()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var_os(crate::portmux_port::ENV_PORTMUX_PORT_BIN);
        // SAFETY: `env_gate` serialises access to this process-wide
        // env var across this test module's parallel test threads,
        // and we restore the previous value before returning.
        unsafe { std::env::set_var(crate::portmux_port::ENV_PORTMUX_PORT_BIN, stub) };
        let out = f();
        unsafe {
            match prev {
                Some(p) => std::env::set_var(crate::portmux_port::ENV_PORTMUX_PORT_BIN, p),
                None => std::env::remove_var(crate::portmux_port::ENV_PORTMUX_PORT_BIN),
            }
        }
        drop(guard);
        out
    }

    #[test]
    fn port_verb_field_port_returns_u16_for_current_instance() {
        let dir = tmpdir("p-port");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Port": {
                  "port": {
                    "isolation": "dest-addr",
                    "field": "port"
                  }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        with_bin(&stub, || {
            apply_pack(&pack, "feather", &cfg, 4, &mut base).unwrap()
        });
        assert_eq!(base["socks5Port"], json!(9164));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_field_url_composes_socks5_uri() {
        let dir = tmpdir("p-url");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "proxy": {
                  "port": {
                    "isolation": "dest-addr",
                    "field": "url"
                  }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Whonix);
        let mut base = Map::new();
        with_bin(&stub, || {
            apply_pack(&pack, "electrum-btc", &cfg, 1, &mut base).unwrap()
        });
        assert_eq!(base["proxy"], json!("socks5:10.152.152.10:9161"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_as_string_emits_quoted_number() {
        let dir = tmpdir("p-str");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Port": {
                  "port": {
                    "isolation": "dest-addr",
                    "field": "port",
                    "as_string": true
                  }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        with_bin(&stub, || {
            apply_pack(&pack, "feather", &cfg, 2, &mut base).unwrap()
        });
        assert_eq!(base["socks5Port"], json!("9162"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_caches_within_single_apply_pack_call() {
        let dir = tmpdir("p-cache");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        // Two port verbs on the same (app, instance, isolation) key
        // should fire `portmux-port ensure` exactly once.
        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Host": {
                  "port": { "isolation": "dest-addr", "field": "host" }
                },
                "socks5Port": {
                  "port": { "isolation": "dest-addr", "field": "port" }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Whonix);
        let mut base = Map::new();
        with_bin(&stub, || {
            apply_pack(&pack, "feather", &cfg, 7, &mut base).unwrap()
        });
        assert_eq!(base["socks5Host"], json!("10.152.152.10"));
        assert_eq!(base["socks5Port"], json!(9167));
        let lines = std::fs::read_to_string(&log).unwrap_or_default();
        let call_count = lines.lines().count();
        assert_eq!(
            call_count, 1,
            "expected exactly 1 portmux-port spawn, got {call_count}: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_explicit_app_overrides_pack_id() {
        let dir = tmpdir("p-app");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Port": {
                  "port": {
                    "app": "shared-tor-pool",
                    "isolation": "dest-addr",
                    "field": "port"
                  }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        with_bin(&stub, || {
            apply_pack(&pack, "feather", &cfg, 1, &mut base).unwrap()
        });
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(
            calls.contains("--app shared-tor-pool"),
            "expected explicit --app override in stub log, got: {calls}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_drop_if_removes_key_when_local_tor_used() {
        let dir = tmpdir("p-drop");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Host": {
                  "port": {
                    "isolation": "dest-addr",
                    "field": "host",
                    "drop_if": "feather_use_local_tor"
                  }
                }
              }
            }"#,
        )
        .unwrap();
        // Ubuntu preset has feather_use_local_tor=true; the host key
        // must be dropped and `portmux-port` never called.
        let cfg = SeedConfig::for_preset(SeedPreset::Ubuntu);
        let mut base = Map::new();
        base.insert("socks5Host".into(), json!("stale"));
        with_bin(&stub, || {
            apply_pack(&pack, "feather", &cfg, 1, &mut base).unwrap()
        });
        assert!(base.get("socks5Host").is_none());
        let lines = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            lines.is_empty(),
            "expected no portmux-port spawns when drop_if fires, got: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn port_verb_validate_rejects_as_string_on_non_port_field() {
        let pack: std::result::Result<SeedPack, _> = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Host": {
                  "port": {
                    "isolation": "dest-addr",
                    "field": "host",
                    "as_string": true
                  }
                }
              }
            }"#,
        );
        let pack = pack.expect("schema-valid, semantic-invalid pack still parses");
        let mut lib = SeedPackLibrary::default();
        lib.packs.insert("feather".into(), pack);
        let err = lib.validate().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("as_string only applies"), "got: {msg}");
    }

    #[test]
    fn port_verb_supports_arbitrary_instance_indices() {
        // The whole reason this verb exists: Feather#4, #5, ... each
        // get their own port. Today's array-shape caps at 3.
        let dir = tmpdir("p-many");
        let log = dir.join("calls.log");
        let stub = write_stub(&dir, &log);

        let pack: SeedPack = serde_json::from_str(
            r#"{
              "config_path": "x.json",
              "preset": {
                "socks5Port": {
                  "port": { "isolation": "dest-addr", "field": "port" }
                }
              }
            }"#,
        )
        .unwrap();
        let cfg = SeedConfig::for_preset(SeedPreset::Whonix);
        let mut ports = Vec::new();
        for n in 1..=12u32 {
            let mut base = Map::new();
            with_bin(&stub, || {
                apply_pack(&pack, "feather", &cfg, n, &mut base).unwrap()
            });
            ports.push(base["socks5Port"].as_u64().unwrap());
        }
        // Stub returns 9160+instance — verify each call produced a
        // distinct port and no panic / cap was hit.
        assert_eq!(ports, (1..=12u64).map(|n| 9160 + n).collect::<Vec<_>>());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
