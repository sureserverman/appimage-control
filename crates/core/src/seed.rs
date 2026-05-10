use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

use crate::catalog::{Catalog, CatalogApp};
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::instances::list_instance_indices;
use crate::paths::AppPaths;
use crate::seed_pack::{apply_pack, SeedPack, SeedPackLibrary};

const SEED_TIMEOUT: Duration = Duration::from_secs(20);
const SEED_POLL_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedPreset {
    Ubuntu,
    Whonix,
}

impl SeedPreset {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ubuntu" => Some(Self::Ubuntu),
            "whonix" => Some(Self::Whonix),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ubuntu => "ubuntu",
            Self::Whonix => "whonix",
        }
    }

    pub fn auto_default() -> Self {
        std::env::var("APPIMAGE_CONTROL_SEED_PRESET")
            .ok()
            .and_then(|v| Self::parse(&v))
            .unwrap_or(Self::Ubuntu)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedConfig {
    pub preset: SeedPreset,
    pub socks5_host: String,
    pub feather_use_local_tor: bool,
    pub feather_ports: [u16; 3],
    pub electrum_btc_port: u16,
    pub electrum_ltc_port: u16,
}

/// Typed view of one [`SeedConfig`] field, returned by
/// [`SeedConfig::field`]. Used by the seed-pack interpreter to resolve
/// `preset.from`, `preset.format`, and `rotation.from_preset_array`
/// references against the active preset without each verb caring about
/// the concrete type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresetField<'a> {
    Bool(bool),
    Str(&'a str),
    U16(u16),
    U16Array(&'a [u16]),
}

impl SeedConfig {
    pub fn for_preset(preset: SeedPreset) -> Self {
        match preset {
            SeedPreset::Ubuntu => Self {
                preset,
                socks5_host: "127.0.0.1".into(),
                feather_use_local_tor: true,
                feather_ports: [9052, 9053, 9070],
                electrum_btc_port: 9054,
                electrum_ltc_port: 9055,
            },
            SeedPreset::Whonix => Self {
                preset,
                socks5_host: "10.152.152.10".into(),
                feather_use_local_tor: false,
                feather_ports: [9052, 9053, 9070],
                electrum_btc_port: 9054,
                electrum_ltc_port: 9055,
            },
        }
    }

    /// Look up a SeedConfig field by name. Single source of truth for
    /// the legal preset-field set referenced by seed-pack `preset.from`,
    /// `preset.format` placeholders, and `rotation.from_preset_array`.
    /// Returns `None` for unknown names so pack validation can flag
    /// typos.
    pub fn field(&self, name: &str) -> Option<PresetField<'_>> {
        match name {
            "socks5_host" => Some(PresetField::Str(&self.socks5_host)),
            "feather_use_local_tor" => Some(PresetField::Bool(self.feather_use_local_tor)),
            "feather_ports" => Some(PresetField::U16Array(&self.feather_ports)),
            "electrum_btc_port" => Some(PresetField::U16(self.electrum_btc_port)),
            "electrum_ltc_port" => Some(PresetField::U16(self.electrum_ltc_port)),
            _ => None,
        }
    }
}

/// Marker that an app id has a registered seed pack. Returned by
/// [`SupportedSeedApp::from_app_id`] so callers (notably the GUI) can
/// keep using the existing `is_some()` check without learning about the
/// seed-pack library directly.
///
/// Now data-driven: an app is "supported" when `data/seeds/<id>.json`
/// exists in the active library (currently: the embedded slice baked
/// in by `build.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupportedSeedApp {
    app_id: String,
}

impl SupportedSeedApp {
    pub fn from_app_id(app_id: &str) -> Option<Self> {
        recipe_for_app_id(app_id).map(|_| Self {
            app_id: app_id.to_string(),
        })
    }

    pub fn from_catalog_app(app: &CatalogApp) -> Option<Self> {
        Self::from_app_id(&app.id)
    }

    pub fn app_id(&self) -> &str {
        &self.app_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedTarget {
    pub app_id: String,
    pub instance_index: u32,
}

/// Pack-backed recipe for one supported seedable app. Constructed by
/// [`recipe_for_supported_app`] from a [`SeedPack`] in the active
/// [`SeedPackLibrary`]; carries the human-facing config-path doc string
/// (with `{appimage}` substituted), the first-run AppImage arguments,
/// and the underlying pack used by [`apply_pack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedRecipe {
    pub app_id: String,
    pub config_path_doc: String,
    pub first_run_args: Vec<String>,
    pack: SeedPack,
}

impl SeedRecipe {
    pub fn config_path_for(&self, filename: &str) -> PathBuf {
        PathBuf::from(self.pack.config_path.replace("{appimage}", filename))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedOutcome {
    Seeded,
    SkippedUnsupported,
    SkippedMissingInstance,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SeedReport {
    pub attempted: u32,
    pub seeded: u32,
    pub skipped_unsupported: u32,
    pub skipped_missing: u32,
    pub failed: u32,
}

/// Resolve a [`CatalogApp`] to its [`SeedRecipe`] from the active
/// [`SeedPackLibrary`] (discovered via the same fallback chain
/// `Catalog::discover_load` uses for `catalog.json`: explicit dir →
/// `APPIMAGE_CONTROL_SEEDS_DIR` env → cwd ancestor walk → exe-parent
/// ancestor walk → `/app/share/...` → `/usr/share/...` → embedded).
pub fn recipe_for_supported_app(app: &CatalogApp) -> Option<SeedRecipe> {
    recipe_for_app_id(&app.id)
}

fn recipe_for_app_id(app_id: &str) -> Option<SeedRecipe> {
    let lib = active_seed_library().ok()?;
    let pack = lib.get(app_id)?.clone();
    Some(SeedRecipe {
        app_id: app_id.to_string(),
        config_path_doc: pack.config_path.replace("{appimage}", "<AppImage>"),
        first_run_args: pack.first_run_args.clone(),
        pack,
    })
}

/// Centralised seed-pack library lookup. `discover_load` reaches the
/// disk-override / Flatpak / embedded chain; on any failure we fall
/// back to the embedded slice so a malformed disk pack can't break
/// every "is this app seedable" probe.
fn active_seed_library() -> Result<SeedPackLibrary> {
    SeedPackLibrary::discover_load(None).or_else(|_| SeedPackLibrary::embedded())
}

pub fn seed_instance_settings(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    preset: SeedPreset,
    on_progress: &mut dyn FnMut(&str),
) -> Result<SeedOutcome> {
    let Some(recipe) = recipe_for_supported_app(app) else {
        on_progress(&format!(
            "{} instance {}: seeding skipped (unsupported app)",
            app.id, instance_index
        ));
        return Ok(SeedOutcome::SkippedUnsupported);
    };
    seed_instance_settings_with_recipe(paths, app, instance_index, recipe, preset, on_progress)
}

pub fn seed_instance_settings_with_recipe(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    recipe: SeedRecipe,
    preset: SeedPreset,
    on_progress: &mut dyn FnMut(&str),
) -> Result<SeedOutcome> {
    let dir = paths.instance_dir_for(app, instance_index);
    let filename = app.resolved_appimage_filename();
    let appimage = dir.join(&filename);
    if !appimage.is_file() {
        on_progress(&format!(
            "{} instance {}: seeding skipped (missing instance AppImage)",
            app.id, instance_index
        ));
        return Ok(SeedOutcome::SkippedMissingInstance);
    }

    let config = SeedConfig::for_preset(preset);
    let expected = dir.join(recipe.config_path_for(&filename));
    on_progress(&format!(
        "{} instance {}: seeding settings with {} preset …",
        app.id,
        instance_index,
        preset.as_str()
    ));
    let first_run_args: Vec<&str> = recipe.first_run_args.iter().map(String::as_str).collect();
    seed_until_config(&appimage, &first_run_args, &expected)?;
    let mut map = read_json_object(&expected)?;
    apply_pack(&recipe.pack, &config, instance_index, &mut map)?;
    write_json_object(&expected, map)?;
    on_progress(&format!(
        "{} instance {}: settings seeded ({})",
        app.id,
        instance_index,
        preset.as_str()
    ));
    Ok(SeedOutcome::Seeded)
}

pub fn seed_supported_installed_apps(
    paths: &AppPaths,
    catalog: &Catalog,
    preset: SeedPreset,
    selected_app_ids: Option<&[String]>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<SeedReport> {
    let selected_set: Option<HashSet<&str>> =
        selected_app_ids.map(|v| v.iter().map(|s| s.as_str()).collect::<HashSet<_>>());
    let mut report = SeedReport::default();

    if let Some(selected) = &selected_set {
        for app_id in selected {
            if recipe_for_app_id(app_id).is_none() {
                report.skipped_unsupported += 1;
                on_progress(&format!("{app_id}: seeding skipped (unsupported app)"));
            }
        }
    }

    for app in &catalog.apps {
        if let Some(selected) = &selected_set {
            if !selected.contains(app.id.as_str()) {
                continue;
            }
        }
        if recipe_for_supported_app(app).is_none() {
            continue;
        }
        let indices = list_instance_indices(paths, app)?;
        if indices.is_empty() {
            if selected_set.is_some() {
                report.skipped_missing += 1;
                on_progress(&format!(
                    "{}: seeding skipped (no installed instances)",
                    app.id
                ));
            }
            continue;
        }
        for index in indices {
            report.attempted += 1;
            match seed_instance_settings(paths, app, index, preset, on_progress) {
                Ok(SeedOutcome::Seeded) => report.seeded += 1,
                Ok(SeedOutcome::SkippedUnsupported) => report.skipped_unsupported += 1,
                Ok(SeedOutcome::SkippedMissingInstance) => report.skipped_missing += 1,
                Err(e) => {
                    report.failed += 1;
                    on_progress(&format!(
                        "{} instance {}: seeding failed: {}",
                        app.id, index, e
                    ));
                }
            }
        }
    }

    Ok(report)
}

pub fn seed_new_instances_after_reconcile(
    paths: &AppPaths,
    app: &CatalogApp,
    new_indices: &[u32],
    preset: SeedPreset,
    on_progress: &mut dyn FnMut(&str),
) -> Result<SeedReport> {
    let mut report = SeedReport::default();
    for &index in new_indices {
        report.attempted += 1;
        match seed_instance_settings(paths, app, index, preset, on_progress) {
            Ok(SeedOutcome::Seeded) => report.seeded += 1,
            Ok(SeedOutcome::SkippedUnsupported) => report.skipped_unsupported += 1,
            Ok(SeedOutcome::SkippedMissingInstance) => report.skipped_missing += 1,
            Err(e) => {
                report.failed += 1;
                on_progress(&format!(
                    "{} instance {}: seeding failed: {}",
                    app.id, index, e
                ));
            }
        }
    }
    Ok(report)
}

fn seed_until_config(appimage: &Path, first_run_args: &[&str], expected: &Path) -> Result<()> {
    // We need to take down xvfb-run *and* its descendants (Xvfb, the AppImage
    // FUSE mount, the actual app binary which has exec'd away from
    // <appimage> to /tmp/.mount_*/). Killing only the AppImage-path-named
    // process leaves Xvfb + the unmounted app binary alive, which holds Qt's
    // `qtsingleapp-*` lock and makes the next instance bail out immediately.
    //
    // Strategy: start the seed in a fresh session via `setsid`, write the
    // session-leader PID to a small file under the instance dir, and on
    // teardown send `kill -KILL -<pgid>` to that PID — that signals every
    // process in the group at once. Works the same native or via
    // `flatpak-spawn --host` because we capture the *host*-side session
    // leader's PID, not the in-sandbox proxy's.
    let pid_file = appimage
        .parent()
        .ok_or_else(|| Error::InvalidPath("seed: AppImage has no parent dir".into()))?
        .join(".appimage-control-seed-host-pid");
    let _ = fs::remove_file(&pid_file);

    let pid_file_q = sh_single_quote(&pid_file.to_string_lossy());
    let appimage_q = sh_single_quote(&appimage.to_string_lossy());
    let extras_q = first_run_args
        .iter()
        .map(|a| sh_single_quote(a))
        .collect::<Vec<_>>()
        .join(" ");
    let script = format!(
        "echo $$ > {pid_file_q}; \
         exec xvfb-run -a '--server-args=-screen 0 1280x1024x24' {appimage_q} {extras_q}"
    );

    // `--fork` forces setsid to fork even if we already happen to be a
    // group leader (defensive — the helper-spawned child usually isn't, but
    // the host helper's exact PG semantics aren't documented). `--wait`
    // makes setsid block until sh (and therefore xvfb-run + Xvfb + the
    // AppImage tree) exits. Without `--wait` setsid returns as soon as the
    // fork is done and our `child.try_wait()` would fire immediately,
    // mis-reporting "seeded app exited before creating config".
    let mut child = host_command("setsid")
        .args(["--fork", "--wait", "sh", "-c", script.as_str()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::InvalidPath(
                    "required dependency not found: setsid (provided by util-linux)".into(),
                )
            } else {
                Error::Io(e)
            }
        })?;

    let started = Instant::now();
    let mut child_exited = false;
    while started.elapsed() < SEED_TIMEOUT {
        if expected.exists() {
            terminate_seed_child(&mut child, &pid_file);
            return Ok(());
        }
        if child.try_wait()?.is_some() {
            child_exited = true;
            break;
        }
        thread::sleep(SEED_POLL_INTERVAL);
    }

    terminate_seed_child(&mut child, &pid_file);
    if expected.exists() {
        return Ok(());
    }
    if child_exited {
        return Err(Error::InvalidPath(format!(
            "seeded app exited before creating config: {}",
            expected.display()
        )));
    }
    Err(Error::InvalidPath(format!(
        "timed out waiting for seeded config: {}",
        expected.display()
    )))
}

/// Quote `s` for a POSIX shell single-quoted context (`'…'`). Embedded
/// single quotes are emitted as `'\''`. Always safe — never interprets
/// the input as a glob, expansion, or command.
fn sh_single_quote(s: &str) -> String {
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

fn terminate_seed_child(child: &mut Child, pid_file: &Path) {
    // The host-side session leader PID was written by our `sh` wrapper before
    // it `exec`d xvfb-run. Reading it gives us the PGID of the entire
    // xvfb-run + Xvfb + AppImage tree. Missing file = wrapper never got far
    // enough to write it; fall back to just killing our direct child.
    let host_pgid = fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|p| *p > 1);

    if let Some(pgid) = host_pgid {
        // procps-ng `kill` mis-parses `kill -KILL -<n>` (it returns exit 0
        // but does not signal the group). Use the explicit `-s SIG --
        // -<pgid>` form so that `--` terminates option parsing and the
        // negative PID is unambiguously a process group.
        let neg = format!("-{pgid}");
        let _ = host_command("kill")
            .args(["-s", "TERM", "--", neg.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        thread::sleep(Duration::from_millis(500));
        let _ = host_command("kill")
            .args(["-s", "KILL", "--", neg.as_str()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    let _ = child.kill();
    let _ = child.wait();
    // Short grace period so the kernel reaps the group and Feather/Electrum
    // release any /tmp lockfiles before the caller starts the next instance.
    thread::sleep(Duration::from_millis(300));
    let _ = fs::remove_file(pid_file);
}

fn read_json_object(path: &Path) -> Result<Map<String, Value>> {
    let raw = fs::read_to_string(path)?;
    let value: Value = serde_json::from_str(&raw)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(Error::InvalidPath(format!(
            "expected JSON object in {}",
            path.display()
        ))),
    }
}

fn write_json_object(path: &Path, map: Map<String, Value>) -> Result<()> {
    let s = serde_json::to_string_pretty(&Value::Object(map))?;
    fs::write(path, format!("{s}\n"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn write_exec(path: &Path, body: &str) {
        fs::write(path, body).unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    fn with_fake_xvfb<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
        let _g = env_lock().lock().unwrap();
        let old = std::env::var_os("PATH");
        let bin = dir.join("bin");
        fs::create_dir_all(&bin).unwrap();
        write_exec(
            &bin.join("xvfb-run"),
            r#"#!/bin/sh
shift
shift
exec "$@"
"#,
        );
        let mut path = bin.as_os_str().to_os_string();
        path.push(":");
        if let Some(oldv) = &old {
            path.push(oldv);
        }
        std::env::set_var("PATH", &path);
        let out = f();
        match old {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        out
    }

    fn make_seedable_instance(
        tmp: &tempfile::TempDir,
        app: &CatalogApp,
        instance_index: u32,
        rel_cfg: &str,
        offline_marker: bool,
    ) -> (AppPaths, PathBuf, PathBuf) {
        let paths = AppPaths::new(tmp.path().join("home")).unwrap();
        let dir = paths.instance_dir_for(app, instance_index);
        fs::create_dir_all(&dir).unwrap();
        let appimage = dir.join(app.resolved_appimage_filename());
        let cfg = dir.join(rel_cfg);
        let cfg_dir = cfg.parent().unwrap();
        let script = format!(
            "#!/bin/sh\nset -eu\n{}mkdir -p \"{}\"\nprintf '{{}}' > \"{}\"\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
            if offline_marker {
                "if [ \"$1\" != \"--offline\" ]; then exit 44; fi\n"
            } else {
                ""
            },
            cfg_dir.display(),
            cfg.display()
        );
        write_exec(&appimage, &script);
        (paths, appimage, cfg)
    }

    fn supported_app(id: &str, filename: &str) -> CatalogApp {
        CatalogApp {
            id: id.into(),
            display_name: Some(id.into()),
            homepage: None,
            download_url: Some(format!("https://example.invalid/{filename}")),
            download_url_aarch64: None,
            appimage_filename: Some(filename.into()),
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        }
    }

    #[test]
    fn preset_field_accessor_covers_every_legal_name() {
        for preset in [SeedPreset::Ubuntu, SeedPreset::Whonix] {
            let cfg = SeedConfig::for_preset(preset);
            assert_eq!(
                cfg.field("socks5_host"),
                Some(PresetField::Str(cfg.socks5_host.as_str()))
            );
            assert_eq!(
                cfg.field("feather_use_local_tor"),
                Some(PresetField::Bool(cfg.feather_use_local_tor))
            );
            assert_eq!(
                cfg.field("feather_ports"),
                Some(PresetField::U16Array(&cfg.feather_ports))
            );
            assert_eq!(
                cfg.field("electrum_btc_port"),
                Some(PresetField::U16(cfg.electrum_btc_port))
            );
            assert_eq!(
                cfg.field("electrum_ltc_port"),
                Some(PresetField::U16(cfg.electrum_ltc_port))
            );
            assert_eq!(cfg.field("nope"), None);
            assert_eq!(cfg.field(""), None);
        }
    }

    #[test]
    fn presets_match_installer_defaults() {
        let u = SeedConfig::for_preset(SeedPreset::Ubuntu);
        assert_eq!(u.socks5_host, "127.0.0.1");
        assert!(u.feather_use_local_tor);
        assert_eq!(u.feather_ports, [9052, 9053, 9070]);
        assert_eq!(u.electrum_btc_port, 9054);
        assert_eq!(u.electrum_ltc_port, 9055);

        let w = SeedConfig::for_preset(SeedPreset::Whonix);
        assert_eq!(w.socks5_host, "10.152.152.10");
        assert!(!w.feather_use_local_tor);
        assert_eq!(w.feather_ports, [9052, 9053, 9070]);
        assert_eq!(w.electrum_btc_port, 9054);
        assert_eq!(w.electrum_ltc_port, 9055);
    }

    fn apply_pack_to_empty_map(app_id: &str, idx: u32, preset: SeedPreset) -> Value {
        let recipe = recipe_for_app_id(app_id).expect("pack for app must exist");
        let cfg = SeedConfig::for_preset(preset);
        let mut map = Map::new();
        apply_pack(&recipe.pack, &cfg, idx, &mut map).unwrap();
        Value::Object(map)
    }

    #[test]
    fn feather_recipe_patch_matches_installer_defaults() {
        let v = apply_pack_to_empty_map("feather", 2, SeedPreset::Whonix);
        assert_eq!(v["proxy"], 1);
        assert_eq!(v["skin"], "Breeze/Dark");
        assert_eq!(v["socks5Port"], "9053");
        assert_eq!(v["torOnlyAllowOnion"], true);
        assert_eq!(v["torPrivacyLevel"], 2);
        assert_eq!(v["useLocalTor"], false);
        assert_eq!(v["socks5Host"], "10.152.152.10");
    }

    #[test]
    fn electrum_btc_recipe_patch_matches_installer_defaults() {
        let v = apply_pack_to_empty_map("electrum-btc", 1, SeedPreset::Ubuntu);
        assert_eq!(v["oneserver"], true);
        assert_eq!(
            v["server"],
            "***REDACTED-ONION-ENDPOINT***"
        );
        assert_eq!(v["proxy"], "socks5:127.0.0.1:9054");
        assert_eq!(v["network_bookmarked_servers"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn electrum_ltc_recipe_patch_matches_installer_defaults() {
        let v = apply_pack_to_empty_map("electrum-ltc", 1, SeedPreset::Whonix);
        assert_eq!(v["proxy"], "socks5:10.152.152.10:9055");
    }

    #[test]
    fn seed_instance_settings_with_shared_pipeline_for_all_supported_apps() {
        with_fake_xvfb(tempfile::tempdir().unwrap().path(), || {
            let tmp = tempfile::tempdir().unwrap();
            let feather = supported_app("feather", "feather.AppImage");
            let (paths_f, _, cfg_f) = make_seedable_instance(
                &tmp,
                &feather,
                1,
                "feather.AppImage.config/feather/settings.json",
                false,
            );
            let mut lines = Vec::new();
            let out = seed_instance_settings(&paths_f, &feather, 1, SeedPreset::Ubuntu, &mut |s| {
                lines.push(s.to_string())
            })
            .unwrap();
            assert_eq!(out, SeedOutcome::Seeded);
            let f: Value = serde_json::from_str(&fs::read_to_string(&cfg_f).unwrap()).unwrap();
            assert_eq!(f["socks5Port"], "9052");

            let btc = supported_app("electrum-btc", "eb.AppImage");
            let (paths_b, _, cfg_b) =
                make_seedable_instance(&tmp, &btc, 1, "eb.AppImage.home/.electrum/config", true);
            seed_instance_settings(&paths_b, &btc, 1, SeedPreset::Ubuntu, &mut |_| {}).unwrap();
            let b: Value = serde_json::from_str(&fs::read_to_string(&cfg_b).unwrap()).unwrap();
            assert_eq!(b["proxy"], "socks5:127.0.0.1:9054");

            let ltc = supported_app("electrum-ltc", "el.AppImage");
            let (paths_l, _, cfg_l) = make_seedable_instance(
                &tmp,
                &ltc,
                1,
                "el.AppImage.home/.electrum-ltc/config",
                true,
            );
            seed_instance_settings(&paths_l, &ltc, 1, SeedPreset::Whonix, &mut |_| {}).unwrap();
            let l: Value = serde_json::from_str(&fs::read_to_string(&cfg_l).unwrap()).unwrap();
            assert_eq!(l["proxy"], "socks5:10.152.152.10:9055");
            assert!(lines.iter().any(|s| s.contains("settings seeded")));
        });
    }

    #[test]
    fn seed_supported_installed_apps_is_idempotent_and_reports_unsupported() {
        with_fake_xvfb(tempfile::tempdir().unwrap().path(), || {
            let tmp = tempfile::tempdir().unwrap();
            let feather = supported_app("feather", "feather.AppImage");
            let (paths, _, cfg) = make_seedable_instance(
                &tmp,
                &feather,
                1,
                "feather.AppImage.config/feather/settings.json",
                false,
            );
            let catalog = Catalog {
                apps: vec![
                    feather.clone(),
                    supported_app("mattermost", "Mattermost.AppImage"),
                ],
            };
            let selected = vec!["feather".to_string(), "mattermost".to_string()];
            let r1 = seed_supported_installed_apps(
                &paths,
                &catalog,
                SeedPreset::Ubuntu,
                Some(&selected),
                &mut |_| {},
            )
            .unwrap();
            let r2 = seed_supported_installed_apps(
                &paths,
                &catalog,
                SeedPreset::Ubuntu,
                Some(&["feather".to_string()]),
                &mut |_| {},
            )
            .unwrap();
            let v: Value = serde_json::from_str(&fs::read_to_string(&cfg).unwrap()).unwrap();
            assert_eq!(v["proxy"], 1);
            assert_eq!(r1.seeded, 1);
            assert_eq!(r1.skipped_unsupported, 1);
            assert_eq!(r2.seeded, 1);
        });
    }

    #[test]
    fn seed_instance_settings_skips_unsupported() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().join("home")).unwrap();
        let app = supported_app("mattermost", "Mattermost.AppImage");
        let out = seed_instance_settings(&paths, &app, 1, SeedPreset::Ubuntu, &mut |_| {}).unwrap();
        assert_eq!(out, SeedOutcome::SkippedUnsupported);
    }

}
