use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

use crate::catalog::{Catalog, CatalogApp};
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::instances::list_instance_indices;
use crate::paths::AppPaths;

const SEED_TIMEOUT: Duration = Duration::from_secs(20);
const SEED_POLL_INTERVAL: Duration = Duration::from_millis(250);
const FEATHER_SKINS: [&str; 3] = ["Breeze/Light", "Breeze/Dark", "Breeze/Light"];
const BTC_PINNED_SERVER: &str =
    "***REDACTED-ONION-ENDPOINT***";
const BTC_BOOKMARKED_SERVERS: &[&str] = &[
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
    "***REDACTED-ONION-ENDPOINT***",
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedSeedApp {
    Feather,
    ElectrumBtc,
    ElectrumLtc,
}

impl SupportedSeedApp {
    pub fn from_app_id(app_id: &str) -> Option<Self> {
        match app_id {
            "feather" => Some(Self::Feather),
            "electrum-btc" => Some(Self::ElectrumBtc),
            "electrum-ltc" => Some(Self::ElectrumLtc),
            _ => None,
        }
    }

    pub fn from_catalog_app(app: &CatalogApp) -> Option<Self> {
        Self::from_app_id(&app.id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeedTarget {
    pub app_id: String,
    pub instance_index: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SeedPatchKind {
    Feather,
    ElectrumBtc,
    ElectrumLtc,
}

/// Explicit recipe model for supported seeded apps.
///
/// A recipe captures exactly the app-specific pieces of the unified seeding pipeline:
/// the supported app id, the expected config path relative to the instance dir, the
/// first-run launch arguments, and which patcher to apply after the config appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedRecipe {
    pub app: SupportedSeedApp,
    pub config_path_doc: &'static str,
    pub first_run_args: &'static [&'static str],
    patch: SeedPatchKind,
}

impl SeedRecipe {
    pub fn config_path_for(self, filename: &str) -> PathBuf {
        match self.patch {
            SeedPatchKind::Feather => {
                PathBuf::from(format!("{filename}.config/feather/settings.json"))
            }
            SeedPatchKind::ElectrumBtc => {
                PathBuf::from(format!("{filename}.home/.electrum/config"))
            }
            SeedPatchKind::ElectrumLtc => {
                PathBuf::from(format!("{filename}.home/.electrum-ltc/config"))
            }
        }
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

pub fn recipe_for_supported_app(app: &CatalogApp) -> Option<SeedRecipe> {
    match SupportedSeedApp::from_catalog_app(app)? {
        SupportedSeedApp::Feather => Some(SeedRecipe {
            app: SupportedSeedApp::Feather,
            config_path_doc: "feather.AppImage.config/feather/settings.json",
            first_run_args: &[],
            patch: SeedPatchKind::Feather,
        }),
        SupportedSeedApp::ElectrumBtc => Some(SeedRecipe {
            app: SupportedSeedApp::ElectrumBtc,
            config_path_doc: "<AppImage>.home/.electrum/config",
            first_run_args: &["--offline"],
            patch: SeedPatchKind::ElectrumBtc,
        }),
        SupportedSeedApp::ElectrumLtc => Some(SeedRecipe {
            app: SupportedSeedApp::ElectrumLtc,
            config_path_doc: "<AppImage>.home/.electrum-ltc/config",
            first_run_args: &["--offline"],
            patch: SeedPatchKind::ElectrumLtc,
        }),
    }
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

    let target = SeedTarget {
        app_id: app.id.clone(),
        instance_index,
    };
    let config = SeedConfig::for_preset(preset);
    let expected = dir.join(recipe.config_path_for(&filename));
    on_progress(&format!(
        "{} instance {}: seeding settings with {} preset …",
        app.id,
        instance_index,
        preset.as_str()
    ));
    seed_until_config(&appimage, recipe.first_run_args, &expected)?;
    patch_seeded_config(recipe, &expected, &target, &config)?;
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
            if SupportedSeedApp::from_app_id(app_id).is_none() {
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

fn patch_seeded_config(
    recipe: SeedRecipe,
    config_path: &Path,
    target: &SeedTarget,
    config: &SeedConfig,
) -> Result<()> {
    match recipe.patch {
        SeedPatchKind::Feather => patch_feather_config(config_path, target.instance_index, config),
        SeedPatchKind::ElectrumBtc => patch_electrum_btc_config(config_path, config),
        SeedPatchKind::ElectrumLtc => patch_electrum_ltc_config(config_path, config),
    }
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

fn feather_skin(instance_index: u32) -> &'static str {
    let slot = ((instance_index.saturating_sub(1)) % 3) as usize;
    FEATHER_SKINS[slot]
}

fn patch_feather_config(path: &Path, instance_index: u32, cfg: &SeedConfig) -> Result<()> {
    let mut map = read_json_object(path)?;
    map.insert("proxy".into(), json!(1));
    map.insert("skin".into(), json!(feather_skin(instance_index)));
    map.insert(
        "socks5Port".into(),
        json!(cfg.feather_ports[((instance_index.saturating_sub(1)) % 3) as usize].to_string()),
    );
    map.insert("torOnlyAllowOnion".into(), json!(true));
    map.insert("torPrivacyLevel".into(), json!(2));
    map.insert("useLocalTor".into(), json!(cfg.feather_use_local_tor));
    if cfg.feather_use_local_tor {
        map.remove("socks5Host");
    } else {
        map.insert("socks5Host".into(), json!(cfg.socks5_host));
    }
    write_json_object(path, map)
}

fn patch_electrum_btc_config(path: &Path, cfg: &SeedConfig) -> Result<()> {
    let mut map = read_json_object(path)?;
    map.insert(
        "network_bookmarked_servers".into(),
        Value::Array(
            BTC_BOOKMARKED_SERVERS
                .iter()
                .map(|s| Value::String((*s).to_string()))
                .collect(),
        ),
    );
    map.insert("oneserver".into(), json!(true));
    map.insert("server".into(), json!(BTC_PINNED_SERVER));
    map.insert(
        "proxy".into(),
        json!(format!(
            "socks5:{}:{}",
            cfg.socks5_host, cfg.electrum_btc_port
        )),
    );
    write_json_object(path, map)
}

fn patch_electrum_ltc_config(path: &Path, cfg: &SeedConfig) -> Result<()> {
    let mut map = read_json_object(path)?;
    map.insert(
        "proxy".into(),
        json!(format!(
            "socks5:{}:{}",
            cfg.socks5_host, cfg.electrum_ltc_port
        )),
    );
    write_json_object(path, map)
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

    #[test]
    fn feather_recipe_patch_matches_installer_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("settings.json");
        fs::write(&p, "{}").unwrap();
        patch_feather_config(&p, 2, &SeedConfig::for_preset(SeedPreset::Whonix)).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
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
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("config");
        fs::write(&p, "{}").unwrap();
        patch_electrum_btc_config(&p, &SeedConfig::for_preset(SeedPreset::Ubuntu)).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["oneserver"], true);
        assert_eq!(v["server"], BTC_PINNED_SERVER);
        assert_eq!(v["proxy"], "socks5:127.0.0.1:9054");
        assert_eq!(v["network_bookmarked_servers"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn electrum_ltc_recipe_patch_matches_installer_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("config");
        fs::write(&p, "{}").unwrap();
        patch_electrum_ltc_config(&p, &SeedConfig::for_preset(SeedPreset::Whonix)).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&p).unwrap()).unwrap();
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
