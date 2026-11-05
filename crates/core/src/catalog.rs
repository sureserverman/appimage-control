//! Catalog: apps with optional download URL, multi-instance exec templates, reset hints.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::desktop::sanitize_app_id;
use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum InstanceStrategy {
    /// No special runtime flags beyond `instance_exec_args_template`.
    #[default]
    None,
    /// Electron apps that accept Chromium `--class` (Session, Standard Notes, …): launcher unsets
    /// `WAYLAND_DISPLAY`, sets Ozone/X11, passes `--no-sandbox --ozone-platform=x11 --class={wm_class}`,
    /// `xprop` WM_CLASS; `Icon=` is absolute path to the 256×256 PNG (128×128 also installed).
    ElectronClass,
    /// Joplin: portable dirs per instance; launcher unsets `WAYLAND_DISPLAY`, sets Ozone/X11, passes `--ozone-platform=x11`, `xprop` WM_CLASS; `Icon=` is absolute path to 256×256 PNG (128×128 also installed); embedded PNG if fetch fails.
    JoplinAltInstance,
    /// Cryptomator (jpackage): extract `--appimage-extract` and patch `lib/app/Cryptomator.cfg` so each
    /// instance uses its own IPC socket and config under the portable `.config`/`.home` dirs (the
    /// embedded AppImage cfg always points at the real `user.home`, so a second launch exits as IPC client).
    CryptomatorExtract,
    /// Qt apps (e.g. Feather): launcher unsets `WAYLAND_DISPLAY`, sets `QT_QPA_PLATFORM=xcb`, then
    /// `xprop` WM_CLASS; `Icon=` basename matches `StartupWMClass=` (256×256 absolute path like Electron).
    QtX11,
    /// KeePassXC: bypass single-instance lock by isolating Qt `TempLocation` via `TMPDIR`, and
    /// isolate config/state via `HOME` + `XDG_*` into the per-instance `.home`/`.config` dirs.
    KeepassxcIsolated,
    /// Electron apps (Jan, AnythingLLM, Mattermost, LM Studio): like `electron_class` but also
    /// isolates `HOME`/`XDG_*`/`TMPDIR` so multiple instances can run concurrently.
    ElectronClassIsolated,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub apps: Vec<CatalogApp>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CatalogApp {
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    /// Direct URL to the x86_64 (amd64) `.AppImage` (used on `x86_64` / `x86` hosts).
    #[serde(default)]
    pub download_url: Option<String>,
    /// Optional URL to the aarch64 `.AppImage` (used on `aarch64` hosts).
    #[serde(default)]
    pub download_url_aarch64: Option<String>,
    /// Filename inside each instance dir (default: last segment of URL or `{id}.AppImage`).
    #[serde(default)]
    pub appimage_filename: Option<String>,
    /// Optional per-app runtime strategy for launching multiple concurrent instances.
    ///
    /// This is preferred over injecting raw flags into `instance_exec_args_template` for apps that
    /// have their own instance-lock/IPC mechanisms (e.g. Joplin), for Electron/Chromium `--class`,
    /// or for Qt on GNOME Wayland (`qt_x11`: XCB + `xprop` for per-instance dock icons).
    #[serde(default)]
    pub instance_strategy: InstanceStrategy,
    /// Extra arguments after the AppImage path, e.g. `--name {instance_name}` (Feather-style).
    /// Placeholders: `{instance_name}`, `{index}`, `{id}`, `{Id}`, `{wm_class}` (per-instance unique string, same as `StartupWMClass=`; e.g. `--class={wm_class}` on Electron when it starts reliably).
    /// Skip `--class` if the AppImage exits immediately (seen with some Electron builds). Multi-instance Electron apps should use [`InstanceStrategy::ElectronClassIsolated`] (empty template) instead of relying on a shared profile.
    #[serde(default)]
    pub instance_exec_args_template: Option<String>,
    /// Value passed into `instance_exec_args_template` as `{instance_name}` (default `{Id}{index}`).
    #[serde(default)]
    pub instance_name_template: Option<String>,
    /// `.desktop` Name= field (default `{display_name} (#{index})`).
    #[serde(default)]
    pub desktop_name_template: Option<String>,
    /// Relative segments under `~/.config`, `~/.cache`, `~/.local/share` to remove on reset (legacy single-dir).
    #[serde(default)]
    pub reset_config_dirs: Vec<String>,
    #[serde(default)]
    pub reset_cache_dirs: Vec<String>,
    #[serde(default)]
    pub reset_data_dirs: Vec<String>,
    /// Direct URL to a small PNG (or other image format GDK can load). If unset, the app tries GitHub repo owner avatar, then a homepage favicon.
    #[serde(default)]
    pub icon_url: Option<String>,
    /// Optional override for Wayland `app_id` / shell matching.
    ///
    /// On Wayland, GNOME (and other shells) match running windows to `.desktop` entries primarily
    /// using the Wayland `xdg_toplevel.set_app_id()` value. For many apps (notably Electron), this
    /// value is not controllable from the outside at runtime and may not match our per-instance
    /// generated WM class.
    ///
    /// When set, AppImage Control uses this for `StartupWMClass=` only for [`InstanceStrategy::None`].
    /// Electron uses per-instance `StartupWMClass` with Chromium `--class`; Joplin uses per-instance
    /// class with X11 + `xprop` in the launcher; Cryptomator and QtX11 use `xprop` after forcing X11.
    #[serde(default)]
    pub wayland_app_id: Option<String>,
    /// When `false`, omit the ` %U` suffix on `Exec=` so the desktop does not pass URL/file args.
    /// Electrum treats unknown argv as CLI subcommands and fails on `file://…` from the session.
    #[serde(default)]
    pub desktop_pass_file_urls: Option<bool>,
    /// Directory name under `apps/` (default: sanitized catalog [`CatalogApp::id`]).
    ///
    /// Electrum’s daemon Unix socket lives under the portable `*.AppImage.home` tree; the full path
    /// must stay within the kernel `AF_UNIX` path limit (~108 bytes). Use a short slug (e.g. `ebtc`)
    /// plus a short [`CatalogApp::appimage_filename`] (e.g. `eb.AppImage`) for those apps.
    #[serde(default)]
    pub instance_subdir: Option<String>,
    /// Linux: run the AppImage under `dbus-run-session` so Tauri `tauri_plugin_single_instance` can
    /// register `{app-identifier}.SingleInstance` on a **private** session bus. Without this, every
    /// instance shares the user session bus and the second process hits `NameTaken` and exits.
    #[serde(default)]
    pub isolate_dbus_session: Option<bool>,
    /// Maximum installed instances for this app (spin upper bound and [`crate::instances::reconcile_instance_count`]).
    /// Omitted or higher than the global cap uses [`crate::instances::MAX_INSTANCES_PER_APP`].
    #[serde(default)]
    pub max_instances: Option<u32>,
    /// Shown when the user hits the instance limit (e.g. GTK spin “+” at max). If unset, a generic
    /// message is built from [`CatalogApp::display_name_resolved`].
    #[serde(default)]
    pub max_instances_tooltip: Option<String>,
    /// Optional grouping label for the GUI launcher grid (e.g. `"Wallets"`, `"Messengers"`).
    /// Apps without one are grouped under [`CatalogApp::category_resolved`]'s `"Other"` fallback.
    #[serde(default)]
    pub category: Option<String>,
}

fn nonempty_url(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|u| !u.is_empty())
}

impl CatalogApp {
    /// Whether `.desktop` `Exec=` should end with ` %U` (freedesktop URL list). Default: true.
    pub fn desktop_pass_file_urls(&self) -> bool {
        self.desktop_pass_file_urls.unwrap_or(true)
    }

    /// `apps/<this>/<n>/` on disk (sanitized); defaults to sanitized [`CatalogApp::id`].
    pub fn instance_filesystem_dirname(&self) -> String {
        self.instance_subdir
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(sanitize_app_id)
            .unwrap_or_else(|| sanitize_app_id(&self.id))
    }

    pub fn display_name_resolved(&self) -> String {
        self.display_name
            .clone()
            .unwrap_or_else(|| self.id.replace('-', " "))
    }

    /// Grouping label for the GUI launcher grid; apps with no catalog `category`
    /// fall under `"Other"` so every app still lands in exactly one section.
    pub fn category_resolved(&self) -> String {
        self.category
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| "Other".to_string())
    }

    /// True if the catalog lists at least one download URL (any architecture).
    pub fn has_any_download_url(&self) -> bool {
        nonempty_url(&self.download_url).is_some()
            || nonempty_url(&self.download_url_aarch64).is_some()
    }

    /// Picks `download_url` vs `download_url_aarch64` based on `std::env::consts::ARCH`.
    pub fn resolved_download_url(&self) -> crate::error::Result<&str> {
        match std::env::consts::ARCH {
            "aarch64" => nonempty_url(&self.download_url_aarch64).ok_or(
                crate::error::Error::NoDownloadUrlForArch(std::env::consts::ARCH),
            ),
            "x86_64" | "x86" => {
                nonempty_url(&self.download_url).ok_or(crate::error::Error::NoDownloadUrl)
            }
            _ => nonempty_url(&self.download_url)
                .or_else(|| nonempty_url(&self.download_url_aarch64))
                .ok_or(crate::error::Error::NoDownloadUrlForArch(
                    std::env::consts::ARCH,
                )),
        }
    }

    pub fn resolved_appimage_filename(&self) -> String {
        if let Some(ref f) = self.appimage_filename {
            return f.clone();
        }
        self.resolved_download_url()
            .ok()
            .and_then(|u| {
                let path = u.split('?').next().unwrap_or(u);
                Path::new(path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(String::from)
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("{}.AppImage", self.id))
    }

    pub fn download_url_required(&self) -> crate::error::Result<&str> {
        self.resolved_download_url()
    }

    /// Upper bound for instance count (catalog `max_instances`, capped by the global maximum).
    pub fn max_instances_allowed(&self) -> u32 {
        let cap = crate::instances::MAX_INSTANCES_PER_APP;
        match self.max_instances {
            None => cap,
            Some(0) => cap,
            Some(n) => n.min(cap),
        }
    }

    /// Tooltip text when this app’s instance limit is below the global default (hover + “+” at max).
    pub fn instance_limit_tooltip_message(&self) -> Option<String> {
        let max = self.max_instances_allowed();
        if max >= crate::instances::MAX_INSTANCES_PER_APP {
            return None;
        }
        if let Some(ref t) = self.max_instances_tooltip {
            return Some(t.clone());
        }
        Some(if max == 1 {
            format!(
                "You can't use more than one instance of {}.",
                self.display_name_resolved()
            )
        } else {
            format!(
                "You can't use more than {max} instances of {}.",
                self.display_name_resolved()
            )
        })
    }
}

/// Walk `start` and its ancestors (bounded depth); return the first successfully parsed
/// `data/catalog.json` on that chain. Started only from the executable's directory, so a build
/// under `target/debug/` finds the checkout's catalog.
/// JSON shipped with the workspace (also baked into Flatpak). Used only when no catalog file is
/// found on disk so a misconfigured `.desktop` (`Exec=appimage-control` on the host) or an empty
/// `APPIMAGE_CONTROL_CATALOG` still yields a usable catalog.
const SHIPPED_CATALOG_JSON: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/catalog.json"
));

/// Loads `path` only if it parses and lists at least one app. Empty `{"apps":[]}` files are ignored
/// so a stray `…/data/catalog.json` on an ancestor chain (e.g. under `$HOME`) cannot shadow the
/// real Flatpak or shipped catalog.
///
/// When the file exists but fails to parse, a warning is printed to stderr naming the path and
/// the parse error so the operator knows why a candidate was skipped.
fn try_nonempty_catalog_file(path: &Path) -> Option<Catalog> {
    if !path.is_file() {
        return None;
    }
    match Catalog::load_from_path(path) {
        Err(e) => {
            eprintln!(
                "appimage-control: warning: catalog candidate {} failed to parse ({}); skipping",
                path.display(),
                e
            );
            None
        }
        Ok(c) if c.apps.is_empty() => None,
        Ok(c) => Some(c),
    }
}

fn try_load_catalog_from_data_json_ancestors(start: &Path) -> Option<Catalog> {
    const MAX_DEPTH: usize = 32;
    let mut dir = start;
    for _ in 0..MAX_DEPTH {
        let candidate = dir.join("data").join("catalog.json");
        if let Some(c) = try_nonempty_catalog_file(&candidate) {
            return Some(c);
        }
        dir = dir.parent()?;
    }
    None
}

/// `true` when `s` is one plain file or directory name: non-empty, not `.` or `..`, and made of
/// `A-Za-z0-9._-` only. Catalog `id` and `appimage_filename` become path parts, so anything else
/// (a `/`, a `..`, a space) could steer a write outside the app's own directory.
fn is_plain_name(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Deserialize catalog JSON and validate it. Every catalog source (an explicit file,
/// `APPIMAGE_CONTROL_CATALOG`, the `data/catalog.json` walks, the system paths and the embedded
/// copy) goes through here, so no unvalidated catalog reaches the rest of the app.
pub fn parse_catalog_json(raw: &str) -> Result<Catalog> {
    let c: Catalog = serde_json::from_str(raw)?;
    c.validate()?;
    Ok(c)
}

/// The embedded catalog's load path, separate so a test can feed it a bad string.
fn parse_embedded_catalog(raw: &str) -> Result<Catalog> {
    parse_catalog_json(raw)
}

impl Catalog {
    /// Refuse a catalog whose fields would be used as unsafe path parts: an `id` or
    /// `appimage_filename` that is not one plain name, or a `reset_*_dirs` entry that is empty,
    /// `.`, absolute or contains `..`. The error names the app and the field.
    pub fn validate(&self) -> Result<()> {
        let bad = |id: &str, field: &str, value: &str, why: &str| {
            Error::InvalidCatalog(format!("app {id:?}: field `{field}` = {value:?} {why}"))
        };
        let mut dirs: HashMap<String, &str> = HashMap::new();
        for app in &self.apps {
            // Fields written into launchers and `.desktop` files: a newline there could add an
            // `Exec=` line or a command to the launcher script.
            for (field, value) in [
                ("display_name", app.display_name.as_deref()),
                (
                    "instance_exec_args_template",
                    app.instance_exec_args_template.as_deref(),
                ),
                (
                    "instance_name_template",
                    app.instance_name_template.as_deref(),
                ),
                (
                    "desktop_name_template",
                    app.desktop_name_template.as_deref(),
                ),
                ("wayland_app_id", app.wayland_app_id.as_deref()),
            ] {
                if let Some(v) = value {
                    if v.chars().any(char::is_control) {
                        return Err(bad(
                            &app.id,
                            field,
                            v,
                            "must not contain control characters",
                        ));
                    }
                }
            }
            // Each app owns `apps/<dir>/`: it must be a real name and not shared, or one app's
            // reset or uninstall would remove another's instances.
            let dir = app.instance_filesystem_dirname();
            let dir_field = if app
                .instance_subdir
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
            {
                "instance_subdir"
            } else {
                "id"
            };
            if dir.is_empty() {
                return Err(bad(
                    &app.id,
                    dir_field,
                    app.instance_subdir.as_deref().unwrap_or(&app.id),
                    "gives an empty instance directory name",
                ));
            }
            if let Some(other) = dirs.insert(dir.clone(), &app.id) {
                return Err(bad(
                    &app.id,
                    dir_field,
                    app.instance_subdir.as_deref().unwrap_or(&app.id),
                    &format!("gives instance directory `{dir}`, already used by app {other:?}"),
                ));
            }
            if !is_plain_name(&app.id) {
                return Err(bad(
                    &app.id,
                    "id",
                    &app.id,
                    "must be one plain name (A-Za-z0-9._-, not . or ..)",
                ));
            }
            if let Some(f) = &app.appimage_filename {
                if !is_plain_name(f) {
                    return Err(bad(
                        &app.id,
                        "appimage_filename",
                        f,
                        "must be one plain name (A-Za-z0-9._-, not . or ..)",
                    ));
                }
            }
            for (field, entries) in [
                ("reset_config_dirs", &app.reset_config_dirs),
                ("reset_cache_dirs", &app.reset_cache_dirs),
                ("reset_data_dirs", &app.reset_data_dirs),
            ] {
                for rel in entries {
                    if !crate::reset::is_safe_reset_rel(rel) {
                        return Err(bad(
                            &app.id,
                            field,
                            rel,
                            "must be a relative path with a name in it and no `..`",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn load_from_path(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)?;
        parse_catalog_json(&raw)
    }

    /// Resolve catalog JSON: `explicit` path (must load if present), else non-empty
    /// `APPIMAGE_CONTROL_CATALOG` when it points to a readable file, then — only for a build
    /// running from a cargo `target/` dir — `data/catalog.json` on the executable’s directory or
    /// any ancestor (how dev and test builds find the repo's catalog); never the current
    /// directory, and never from an installed binary (the GUI starts in `$HOME`),
    /// then `/app/share/appimage-control/catalog.json` (Flatpak) and `/usr/share/appimage-control/catalog.json`,
    /// then the catalog JSON that was compiled into this crate (same as repo `data/catalog.json`).
    pub fn discover_load(explicit: Option<&Path>) -> crate::error::Result<Self> {
        if let Some(p) = explicit {
            if !p.is_file() {
                return Err(crate::error::Error::InvalidPath(format!(
                    "catalog is not a file: {}",
                    p.display()
                )));
            }
            return Self::load_from_path(p);
        }
        if let Ok(s) = std::env::var("APPIMAGE_CONTROL_CATALOG") {
            let s = s.trim();
            if !s.is_empty() {
                let p = PathBuf::from(s);
                if p.is_file() {
                    match Self::load_from_path(&p) {
                        Ok(c) => return Ok(c),
                        Err(e) => {
                            eprintln!(
                                "appimage-control: warning: APPIMAGE_CONTROL_CATALOG={} failed to parse ({}); falling through to next source",
                                p.display(),
                                e
                            );
                        }
                    }
                } else {
                    eprintln!(
                        "appimage-control: warning: APPIMAGE_CONTROL_CATALOG={} is not a readable file; falling through to next source",
                        p.display()
                    );
                }
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent().and_then(crate::paths::dev_build_dir) {
                if let Some(c) = try_load_catalog_from_data_json_ancestors(dir) {
                    return Ok(c);
                }
            }
        }
        for p in [
            PathBuf::from("/app/share/appimage-control/catalog.json"),
            PathBuf::from("/usr/share/appimage-control/catalog.json"),
        ] {
            if let Some(c) = try_nonempty_catalog_file(&p) {
                return Ok(c);
            }
        }
        parse_embedded_catalog(SHIPPED_CATALOG_JSON).map_err(|e| {
            crate::error::Error::InvalidPath(format!(
                "built-in catalog failed to parse (report as a bug): {e}"
            ))
        })
    }

    pub fn by_id(&self) -> HashMap<String, &CatalogApp> {
        self.apps.iter().map(|a| (a.id.clone(), a)).collect()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::{Mutex, OnceLock};

    use super::{Catalog, CatalogApp, InstanceStrategy};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn app_urls(amd: Option<&str>, arm: Option<&str>) -> CatalogApp {
        CatalogApp {
            id: "t".into(),
            display_name: None,
            homepage: None,
            download_url: amd.map(String::from),
            download_url_aarch64: arm.map(String::from),
            appimage_filename: Some("t.AppImage".into()),
            instance_strategy: InstanceStrategy::None,
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
            category: None,
        }
    }

    #[test]
    fn electrum_compact_layout_keeps_daemon_socket_path_short() {
        let app = CatalogApp {
            id: "electrum-btc".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: Some("eb.AppImage".into()),
            instance_strategy: InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: Some("ebtc".into()),
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
            category: None,
        };
        let paths = crate::paths::AppPaths::new(std::path::PathBuf::from("/home/user")).unwrap();
        let sock = paths
            .instance_dir_for(&app, 1)
            .join("eb.AppImage.home")
            .join(".electrum")
            .join("daemon_rpc_socket");
        assert!(
            sock.as_os_str().len() <= 108,
            "AF_UNIX path too long ({}): {}",
            sock.as_os_str().len(),
            sock.display()
        );
    }

    #[test]
    fn resolved_url_matches_host_amd64_catalog() {
        if !matches!(std::env::consts::ARCH, "x86_64" | "x86") {
            return;
        }
        let a = app_urls(Some("https://x.example/a.AppImage"), None);
        assert_eq!(
            a.resolved_download_url().unwrap(),
            "https://x.example/a.AppImage"
        );
    }

    #[test]
    fn aarch64_url_used_on_aarch64() {
        if std::env::consts::ARCH != "aarch64" {
            return;
        }
        let a = app_urls(
            Some("https://x.example/x86.AppImage"),
            Some("https://x.example/arm.AppImage"),
        );
        assert_eq!(
            a.resolved_download_url().unwrap(),
            "https://x.example/arm.AppImage"
        );
    }

    #[test]
    fn x86_requires_download_url_not_only_arm() {
        if !matches!(std::env::consts::ARCH, "x86_64" | "x86") {
            return;
        }
        let a = app_urls(None, Some("https://x.example/arm.AppImage"));
        assert!(a.resolved_download_url().is_err());
    }

    #[test]
    fn max_instances_allowed_respects_catalog_and_global_cap() {
        let mut a = app_urls(Some("https://x.example/a.AppImage"), None);
        assert_eq!(
            a.max_instances_allowed(),
            crate::instances::MAX_INSTANCES_PER_APP
        );
        a.max_instances = Some(1);
        assert_eq!(a.max_instances_allowed(), 1);
        a.max_instances = Some(999);
        assert_eq!(
            a.max_instances_allowed(),
            crate::instances::MAX_INSTANCES_PER_APP
        );
    }

    #[test]
    fn discover_load_explicit_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("catalog.json");
        fs::write(&p, r#"{"apps":[]}"#).unwrap();
        let c = Catalog::discover_load(Some(p.as_path())).unwrap();
        assert!(c.apps.is_empty());
    }

    #[test]
    fn shipped_catalog_embed_constant_parses() {
        let c: Catalog =
            super::parse_catalog_json(super::SHIPPED_CATALOG_JSON).expect("embed parse");
        assert!(!c.apps.is_empty(), "shipped catalog should list apps");
    }

    #[test]
    fn ancestor_walk_finds_nonempty_data_catalog_json() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("crates/gui/src");
        fs::create_dir_all(&deep).unwrap();
        let catalog_path = tmp.path().join("data/catalog.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(
            &catalog_path,
            r#"{"apps":[{"id":"stub-app","download_url":"https://example.com/x.AppImage","appimage_filename":"x.AppImage"}]}"#,
        )
        .unwrap();
        let c = super::try_load_catalog_from_data_json_ancestors(&deep).expect("catalog");
        assert_eq!(c.apps.len(), 1);
        assert_eq!(c.apps[0].id, "stub-app");
    }

    #[test]
    fn ancestor_walk_skips_empty_apps_catalog_json() {
        let tmp = tempfile::tempdir().unwrap();
        let deep = tmp.path().join("a/b");
        fs::create_dir_all(&deep).unwrap();
        let catalog_path = tmp.path().join("data/catalog.json");
        fs::create_dir_all(catalog_path.parent().unwrap()).unwrap();
        fs::write(&catalog_path, r#"{"apps":[]}"#).unwrap();
        assert!(super::try_load_catalog_from_data_json_ancestors(&deep).is_none());
    }

    #[test]
    fn instance_limit_tooltip_prefers_catalog_string() {
        let mut a = app_urls(Some("https://x.example/a.AppImage"), None);
        a.max_instances = Some(1);
        a.max_instances_tooltip = Some("Custom limit message.".into());
        assert_eq!(
            a.instance_limit_tooltip_message().as_deref(),
            Some("Custom limit message.")
        );
    }

    /// When `APPIMAGE_CONTROL_CATALOG` points to a malformed file, `discover_load` must still
    /// fall through to the next source (the embedded catalog) and return a non-empty catalog.
    /// We code-review the `eprintln!` path; asserting the fallback behaviour is sufficient.
    #[test]
    fn discover_load_falls_through_when_env_catalog_is_malformed() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let bad = tmp.path().join("bad-catalog.json");
        fs::write(&bad, b"this is not valid json {{{{").unwrap();

        let prev = std::env::var("APPIMAGE_CONTROL_CATALOG").ok();
        std::env::set_var("APPIMAGE_CONTROL_CATALOG", bad.to_str().unwrap());

        let result = Catalog::discover_load(None);

        match prev {
            Some(v) => std::env::set_var("APPIMAGE_CONTROL_CATALOG", v),
            None => std::env::remove_var("APPIMAGE_CONTROL_CATALOG"),
        }

        // Must fall through to the embedded catalog, which is always non-empty.
        let catalog = result.expect("should fall through to embedded catalog");
        assert!(
            !catalog.apps.is_empty(),
            "expected embedded catalog apps after fallthrough"
        );
    }

    /// When `APPIMAGE_CONTROL_CATALOG` points to a non-existent file, `discover_load` must also
    /// fall through to the next source.
    #[test]
    fn discover_load_falls_through_when_env_catalog_is_missing() {
        let _g = env_lock().lock().unwrap();
        let tmp = tempfile::tempdir().unwrap();
        let absent = tmp.path().join("no-such-file.json");
        // Do NOT create the file — it does not exist.

        let prev = std::env::var("APPIMAGE_CONTROL_CATALOG").ok();
        std::env::set_var("APPIMAGE_CONTROL_CATALOG", absent.to_str().unwrap());

        let result = Catalog::discover_load(None);

        match prev {
            Some(v) => std::env::set_var("APPIMAGE_CONTROL_CATALOG", v),
            None => std::env::remove_var("APPIMAGE_CONTROL_CATALOG"),
        }

        let catalog = result.expect("should fall through to embedded catalog");
        assert!(!catalog.apps.is_empty());
    }

    // --- Task 2.1: every catalog load path validates its fields ---------------------------

    /// A catalog JSON with one good app and one app built from `extra` (a JSON object body).
    fn catalog_json_with(extra: &str) -> String {
        format!(
            r#"{{"apps":[{{"id":"okapp","download_url":"https://example.com/a.AppImage"}},{{{extra}}}]}}"#
        )
    }

    fn bad_name_values() -> [&'static str; 7] {
        ["../x", "/abs", "a/b", "", ".", "..", "x y"]
    }

    #[test]
    fn validate_refuses_bad_id_naming_the_field() {
        for bad in bad_name_values() {
            let json =
                catalog_json_with(&format!(r#""id":{}"#, serde_json::to_string(bad).unwrap()));
            let err = super::parse_catalog_json(&json)
                .expect_err(&format!("id {bad:?} must be refused"))
                .to_string();
            assert!(
                err.contains("`id`"),
                "id {bad:?}: error must name the field: {err}"
            );
        }
    }

    #[test]
    fn validate_refuses_bad_appimage_filename_naming_app_and_field() {
        for bad in bad_name_values() {
            let json = catalog_json_with(&format!(
                r#""id":"evilapp","appimage_filename":{}"#,
                serde_json::to_string(bad).unwrap()
            ));
            let err = super::parse_catalog_json(&json)
                .expect_err(&format!("appimage_filename {bad:?} must be refused"))
                .to_string();
            assert!(
                err.contains("evilapp") && err.contains("`appimage_filename`"),
                "appimage_filename {bad:?}: error must name app and field: {err}"
            );
        }
    }

    #[test]
    fn validate_refuses_bad_reset_entries_naming_the_field() {
        for field in ["reset_config_dirs", "reset_cache_dirs", "reset_data_dirs"] {
            for bad in ["", ".", "/etc", "a/../.."] {
                let json = catalog_json_with(&format!(
                    r#""id":"evilapp","{field}":[{}]"#,
                    serde_json::to_string(bad).unwrap()
                ));
                let err = super::parse_catalog_json(&json)
                    .expect_err(&format!("{field} {bad:?} must be refused"))
                    .to_string();
                assert!(
                    err.contains("evilapp") && err.contains(&format!("`{field}`")),
                    "{field} {bad:?}: error must name app and field: {err}"
                );
            }
        }
    }

    #[test]
    fn validate_accepts_shipped_and_embedded_catalogs() {
        let embedded = super::parse_catalog_json(super::SHIPPED_CATALOG_JSON).expect("embedded");
        assert!(!embedded.apps.is_empty());
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/catalog.json");
        Catalog::load_from_path(&file).expect("shipped data/catalog.json validates");
    }

    #[test]
    fn load_path_file_refuses_bad_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("catalog.json");
        fs::write(
            &p,
            catalog_json_with(r#""id":"evilapp","appimage_filename":"../x""#),
        )
        .unwrap();
        let err = Catalog::load_from_path(&p)
            .expect_err("bad file must be refused")
            .to_string();
        assert!(err.contains("`appimage_filename`"), "{err}");
        assert!(Catalog::discover_load(Some(p.as_path())).is_err());
    }

    #[test]
    fn load_path_env_refuses_bad_catalog_and_falls_through() {
        let _g = env_lock().lock().unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bad.json");
        fs::write(
            &p,
            r#"{"apps":[{"id":"env-decoy-ok","download_url":"https://example.com/a.AppImage"},{"id":"evilapp","reset_config_dirs":[""]}]}"#,
        )
        .unwrap();
        let prev = std::env::var("APPIMAGE_CONTROL_CATALOG").ok();
        std::env::set_var("APPIMAGE_CONTROL_CATALOG", p.to_str().unwrap());
        let result = Catalog::discover_load(None);
        match prev {
            Some(v) => std::env::set_var("APPIMAGE_CONTROL_CATALOG", v),
            None => std::env::remove_var("APPIMAGE_CONTROL_CATALOG"),
        }
        let c = result.expect("falls through to another source");
        assert!(
            !c.apps.iter().any(|a| a.id == "env-decoy-ok"),
            "an invalid APPIMAGE_CONTROL_CATALOG must not be used"
        );
    }

    #[test]
    fn load_path_embedded_refuses_bad_catalog() {
        let bad = catalog_json_with(r#""id":"evilapp","reset_data_dirs":["."]"#);
        let err = super::parse_embedded_catalog(&bad)
            .expect_err("embedded parse must validate")
            .to_string();
        assert!(err.contains("`reset_data_dirs`"), "{err}");
    }

    #[test]
    fn validate_refuses_empty_or_colliding_instance_dirs() {
        // An id or instance_subdir that sanitizes to "" would put instances in `apps/` itself.
        for extra in [
            r#""id":"evilapp","instance_subdir":"..""#,
            r#""id":"evilapp","instance_subdir":"-""#,
            r#""id":"...""#,
            r#""id":"-""#,
        ] {
            let err = super::parse_catalog_json(&catalog_json_with(extra))
                .expect_err(&format!("{extra} must be refused"))
                .to_string();
            assert!(err.contains("instance directory"), "{extra}: {err}");
        }
        // Two apps sharing one instance dir would delete each other's instances.
        for (a, b) in [
            (r#""id":"a.b""#, r#""id":"a-b""#),
            (
                r#""id":"x1","instance_subdir":"same""#,
                r#""id":"x2","instance_subdir":"same""#,
            ),
            (r#""id":"dup""#, r#""id":"dup""#),
        ] {
            let json = format!(r#"{{"apps":[{{{a}}},{{{b}}}]}}"#);
            let err = super::parse_catalog_json(&json)
                .expect_err(&format!("{a} + {b} must be refused"))
                .to_string();
            assert!(err.contains("instance directory"), "{a} + {b}: {err}");
        }
    }

    #[test]
    fn validate_refuses_control_characters_in_written_fields() {
        for field in [
            "display_name",
            "instance_exec_args_template",
            "instance_name_template",
            "desktop_name_template",
            "wayland_app_id",
        ] {
            let json =
                catalog_json_with(&format!(r#""id":"evilapp","{field}":"ok\nExec=/bin/sh""#));
            let err = super::parse_catalog_json(&json)
                .expect_err(&format!("newline in {field} must be refused"))
                .to_string();
            assert!(
                err.contains("evilapp") && err.contains(&format!("`{field}`")),
                "{field}: {err}"
            );
        }
    }
}
