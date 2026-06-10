//! Multi-instance catalog installs: one AppImage copy per `apps/<dir>/<n>/` (default `dir` =
//! sanitized catalog id; optional [`CatalogApp::instance_subdir`] for short paths, e.g. Electrum).

use std::collections::HashSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, CatalogApp, InstanceStrategy};
use crate::desktop::{
    build_instance_desktop_entry, desktop_filename_for_instance, sanitize_app_id, DESKTOP_PREFIX,
};
use crate::download::download_to_file;
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::icons::write_instance_icon_png;
use crate::paths::AppPaths;
use semver::Version;
use std::io::Read;

/// Upper bound for instances per catalog app (1-based indices).
pub const MAX_INSTANCES_PER_APP: u32 = 20;

/// Parse `app-id` or `app-id:N` (instance count). Used by CLI and test installers.
pub fn parse_app_instance_spec(spec: &str) -> Result<(String, u32)> {
    if let Some((id, n)) = spec.rsplit_once(':') {
        let count: u32 = n.parse().map_err(|_| {
            Error::InvalidPath(format!(
                "invalid instance count in {spec:?} (use app-id or app-id:N)"
            ))
        })?;
        return Ok((id.to_string(), count));
    }
    Ok((spec.to_string(), 1))
}

fn title_case_first_segment(id: &str) -> String {
    let first = id.split('-').next().unwrap_or(id);
    let mut c = first.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn expand_placeholders(
    template: &str,
    app: &CatalogApp,
    instance_index: u32,
    wm_class: &str,
    alt_instance_id: &str,
) -> String {
    let id_title = title_case_first_segment(&app.id);
    template
        .replace("{index}", &instance_index.to_string())
        .replace("{id}", &app.id)
        .replace("{Id}", &id_title)
        .replace("{wm_class}", wm_class)
        .replace("{alt_instance_id}", alt_instance_id)
}

pub fn instance_exec_suffix(app: &CatalogApp, instance_index: u32) -> String {
    let wm_class = instance_startup_wm_class(&app.id, instance_index);
    let alt_instance_id = instance_index.to_string();
    let args_tmpl_raw = match &app.instance_exec_args_template {
        None => "",
        Some(s) if s.trim().is_empty() => "",
        Some(s) => s.as_str(),
    };
    let strategy_args: &'static str = match app.instance_strategy {
        InstanceStrategy::None => "",
        InstanceStrategy::ElectronClass => "--no-sandbox --ozone-platform=x11 --class={wm_class}",
        InstanceStrategy::ElectronClassIsolated => {
            "--no-sandbox --ozone-platform=x11 --class={wm_class}"
        }
        InstanceStrategy::JoplinAltInstance => {
            // Joplin's wrapper rejects `--class`. Force Ozone/X11 so windows are XWayland (native
            // Wayland ignores `xprop`; GNOME then matches `StartupWMClass=` + `Icon=` from our .desktop).
            "--no-sandbox --ozone-platform=x11"
        }
        InstanceStrategy::CryptomatorExtract
        | InstanceStrategy::QtX11
        | InstanceStrategy::KeepassxcIsolated => "",
    };
    let args_tmpl = if !args_tmpl_raw.is_empty() {
        args_tmpl_raw
    } else {
        strategy_args
    };
    let name_tmpl = app
        .instance_name_template
        .as_deref()
        .unwrap_or("{Id}{index}");
    let instance_name =
        expand_placeholders(name_tmpl, app, instance_index, &wm_class, &alt_instance_id);
    expand_placeholders(args_tmpl, app, instance_index, &wm_class, &alt_instance_id)
        .replace("{instance_name}", &instance_name)
}

/// Per-instance **distinct** WM class string for `StartupWMClass=` and `{wm_class}` in exec templates.
///
/// The `i{instance_index}` suffix guarantees two instances of the same catalog app never share a
/// value. For the running window to report that class (so shells match `StartupWMClass`), add
/// toolkit-specific flags in the catalog (e.g. Electron/Chromium `--class={wm_class}`).
pub fn instance_startup_wm_class(catalog_id: &str, instance_index: u32) -> String {
    format!(
        "io.github.appimagecontrol.{}.i{}",
        sanitize_app_id(catalog_id),
        instance_index
    )
}

/// `StartupWMClass=` / launcher `WMCLASS`: per-instance [`instance_startup_wm_class`] for
/// Cryptomator; Electron (`--class` + `--ozone-platform=x11` + launcher unsets `WAYLAND_DISPLAY`);
/// Joplin (same launcher env, no `--class`); QtX11 (`QT_QPA_PLATFORM=xcb` + unset `WAYLAND_DISPLAY`);
/// otherwise catalog `wayland_app_id` when set, else per-instance.
///
/// A shared catalog `wayland_app_id` for every instance would collapse the dock to one icon; we
/// only apply it for [`InstanceStrategy::None`]. Cryptomator, Electron, Joplin, and QtX11 launchers
/// rewrite `WM_CLASS` via `xprop` on XWayland windows.
pub fn instance_startup_wm_class_for_desktop(app: &CatalogApp, instance_index: u32) -> String {
    let per_instance_class = instance_startup_wm_class(&app.id, instance_index);
    match app.instance_strategy {
        InstanceStrategy::CryptomatorExtract
        | InstanceStrategy::ElectronClass
        | InstanceStrategy::ElectronClassIsolated
        | InstanceStrategy::JoplinAltInstance
        | InstanceStrategy::QtX11
        | InstanceStrategy::KeepassxcIsolated => per_instance_class,
        InstanceStrategy::None => app
            .wayland_app_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(per_instance_class.as_str())
            .to_string(),
    }
}

pub fn instance_desktop_display_name(app: &CatalogApp, instance_index: u32) -> String {
    let tmpl = app
        .desktop_name_template
        .as_deref()
        .unwrap_or("{display_name} (#{index})");
    tmpl.replace("{display_name}", &app.display_name_resolved())
        .replace("{index}", &instance_index.to_string())
}

fn stable_instance_app_id(catalog_id: &str, instance_index: u32) -> String {
    format!("{}-{}", sanitize_app_id(catalog_id), instance_index)
}

/// True if `instance_dir` has a launchable AppImage: the catalog-expected name, or any `*.AppImage`
/// (covers catalog URL/filename changes without losing track of installs).
fn instance_dir_has_usable_appimage(instance_dir: &Path, expected_filename: &str) -> bool {
    if instance_dir.join(expected_filename).is_file() {
        return true;
    }
    let Ok(rd) = fs::read_dir(instance_dir) else {
        return false;
    };
    for ent in rd.flatten() {
        let p = ent.path();
        if !p.is_file() {
            continue;
        }
        if p.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("AppImage"))
        {
            return true;
        }
    }
    false
}

/// List 1-based instance indices under `apps/<id>/<n>/` that contain a usable AppImage.
pub fn list_instance_indices(paths: &AppPaths, app: &CatalogApp) -> Result<Vec<u32>> {
    let root = paths.apps_root_for_app(app);
    let filename = app.resolved_appimage_filename();
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut v = Vec::new();
    for ent in fs::read_dir(&root)? {
        let ent = ent?;
        let name = ent.file_name();
        let Some(s) = name.to_str() else { continue };
        let Ok(n) = s.parse::<u32>() else { continue };
        if n == 0 || n > MAX_INSTANCES_PER_APP {
            continue;
        }
        if instance_dir_has_usable_appimage(&ent.path(), &filename) {
            v.push(n);
        }
    }
    v.sort_unstable();
    Ok(v)
}

/// Parse `appimage-control-<catalogId>-<n>.desktop` filenames in the user applications dir.
fn discover_instance_indices_from_desktops(paths: &AppPaths, app: &CatalogApp) -> Result<Vec<u32>> {
    let dir = paths.applications_dir();
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let prefix = format!("{}{}-", DESKTOP_PREFIX, sanitize_app_id(&app.id));
    let suffix = ".desktop";
    let mut out = Vec::new();
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let name = ent.file_name();
        let Some(s) = name.to_str() else { continue };
        if !s.starts_with(&prefix) || !s.ends_with(suffix) {
            continue;
        }
        let mid = &s[prefix.len()..s.len() - suffix.len()];
        if let Ok(n) = mid.parse::<u32>() {
            if (1..=MAX_INSTANCES_PER_APP).contains(&n) {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// Union of instance dirs (with an AppImage) and managed `.desktop` entries.
///
/// Used for UI counts, reconcile, and repair. We intentionally **do not** infer instances from
/// stray PNGs under `hicolor` — leftover `appimage-control-*-N.png` tiles caused false multi-instance
/// installs when the catalog AppImage filename changed and dir scanning looked empty.
pub fn list_managed_instance_indices(paths: &AppPaths, app: &CatalogApp) -> Result<Vec<u32>> {
    let mut v = list_instance_indices(paths, app)?;
    v.extend(discover_instance_indices_from_desktops(paths, app)?);
    v.sort_unstable();
    v.dedup();
    Ok(v)
}

pub fn instance_count(paths: &AppPaths, app: &CatalogApp) -> Result<u32> {
    Ok(list_managed_instance_indices(paths, app)?.len() as u32)
}

/// Version recorded at install time for the **lowest-number instance that has an AppImage** only.
/// For UI and `--list`, prefer [`read_installed_version_fleet_summary`].
pub fn read_installed_version(paths: &AppPaths, app: &CatalogApp) -> Result<Option<String>> {
    let indices = list_instance_indices(paths, app)?;
    let Some(&i) = indices.first() else {
        return Ok(None);
    };
    let p = paths
        .instance_dir_for(app, i)
        .join(crate::version_info::INSTANCE_VERSION_FILENAME);
    match fs::read_to_string(&p) {
        Ok(s) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                Ok(Some(t.to_string()))
            }
        }
        Err(_) => Ok(None),
    }
}

/// Aggregate of `.appimage-control-version` across every **managed** instance (same indices as the
/// spin / reconcile UI). When instances disagree, returns a semver span `low–high` if all values
/// parse; otherwise comma-separated distinct strings.
pub fn read_installed_version_fleet_summary(
    paths: &AppPaths,
    app: &CatalogApp,
) -> Result<Option<String>> {
    let mut indices = list_managed_instance_indices(paths, app)?;
    indices.sort_unstable();
    let mut vers: Vec<String> = indices
        .iter()
        .filter_map(|&i| read_instance_recorded_version(paths, app, i))
        .collect();
    vers.sort();
    vers.dedup();
    if vers.is_empty() {
        return Ok(None);
    }
    if vers.len() == 1 {
        return Ok(Some(vers[0].clone()));
    }
    let parsed: Vec<Version> = vers
        .iter()
        .filter_map(|s| Version::parse(s.trim()).ok())
        .collect();
    if parsed.len() == vers.len() {
        let lo = parsed.iter().min().expect("non-empty");
        let hi = parsed.iter().max().expect("non-empty");
        if lo == hi {
            return Ok(Some(lo.to_string()));
        }
        return Ok(Some(format!("{lo}–{hi}")));
    }
    Ok(Some(vers.join(", ")))
}

/// Human-friendly installed-version label and optional per-instance details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetInstalledDisplay {
    /// e.g. `installed 1.18.0`, `installed 1.17.17-1.18.0`, `installed unknown`.
    pub label: String,
    /// Per-instance details (`#n: value`) for tooltip/CLI expansion when helpful.
    pub details: Vec<String>,
}

fn short_instance_binary_hash(path: &Path) -> Option<String> {
    let mut f = fs::File::open(path).ok()?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let hex = hasher.finalize().to_hex().to_string();
    Some(hex.chars().take(10).collect())
}

/// Display model for installed versions across managed instances.
///
/// - All versions equal: `installed X`
/// - Mixed semver: `installed lo-hi`
/// - Missing/inconclusive metadata: `installed unknown` (with hash details when possible)
pub fn installed_version_fleet_display(
    paths: &AppPaths,
    app: &CatalogApp,
) -> Result<FleetInstalledDisplay> {
    let mut indices = list_managed_instance_indices(paths, app)?;
    indices.sort_unstable();
    if indices.is_empty() {
        return Ok(FleetInstalledDisplay {
            label: "installed unknown".to_string(),
            details: Vec::new(),
        });
    }

    let filename = app.resolved_appimage_filename();
    let mut versions: Vec<String> = Vec::new();
    let mut details: Vec<String> = Vec::new();
    let mut had_missing = false;

    for &i in &indices {
        let ver = read_instance_recorded_version(paths, app, i);
        match ver {
            Some(v) => {
                details.push(format!("#{i}: {v}"));
                versions.push(v);
            }
            None => {
                had_missing = true;
                let bin = paths.instance_dir_for(app, i).join(&filename);
                if let Some(h) = short_instance_binary_hash(&bin) {
                    details.push(format!("#{i}: hash {h}"));
                } else {
                    details.push(format!("#{i}: unknown"));
                }
            }
        }
    }

    versions.sort();
    versions.dedup();

    let label = if versions.is_empty() {
        "installed unknown".to_string()
    } else if versions.len() == 1 && !had_missing {
        format!("installed {}", versions[0])
    } else {
        let parsed: Vec<Version> = versions
            .iter()
            .filter_map(|s| Version::parse(s.trim()).ok())
            .collect();
        if parsed.len() == versions.len() {
            let lo = parsed.iter().min().expect("non-empty");
            let hi = parsed.iter().max().expect("non-empty");
            if lo == hi && !had_missing {
                format!("installed {lo}")
            } else {
                format!("installed {lo}-{hi}")
            }
        } else if versions.len() == 1 {
            format!("installed {}", versions[0])
        } else {
            format!("installed {}-{}", versions[0], versions[versions.len() - 1])
        }
    };

    Ok(FleetInstalledDisplay { label, details })
}

/// True when managed instances have more than one distinct non-empty recorded version.
pub fn installed_version_fleet_has_distinct_recorded_versions(
    paths: &AppPaths,
    app: &CatalogApp,
) -> Result<bool> {
    let indices = list_managed_instance_indices(paths, app)?;
    let mut vers: Vec<String> = indices
        .iter()
        .filter_map(|&i| read_instance_recorded_version(paths, app, i))
        .collect();
    vers.sort();
    vers.dedup();
    Ok(vers.len() > 1)
}

/// Highest semver among managed instances' `.appimage-control-version` lines (normalized).
pub fn read_installed_version_fleet_max_semver(
    paths: &AppPaths,
    app: &CatalogApp,
) -> Result<Option<Version>> {
    let indices = list_managed_instance_indices(paths, app)?;
    let mut best: Option<Version> = None;
    for &i in &indices {
        if let Some(ref s) = read_instance_recorded_version(paths, app, i) {
            let n = crate::version_info::normalize_github_release_tag(s);
            if let Ok(v) = Version::parse(&n) {
                best = Some(match best {
                    None => v,
                    Some(b) => b.max(v),
                });
            }
        }
    }
    Ok(best)
}

/// Label for the catalog row **Latest** column: semver-wise maximum of GitHub `releases/latest` tag
/// (when known), the catalog URL pin, and every instance's recorded version — so a stale embedded
/// catalog pin does not hide that installs already record a newer release (e.g. Session 1.18.0).
pub fn catalog_row_latest_version_label(
    paths: &AppPaths,
    app: &CatalogApp,
    remote_tag: Option<&str>,
) -> Result<String> {
    let pin = crate::version_info::catalog_pinned_version(app);
    let fleet_max = read_installed_version_fleet_max_semver(paths, app)?;

    let mut best: Option<Version> = None;
    for s in remote_tag.iter().copied().chain(pin.as_deref()) {
        let n = crate::version_info::normalize_github_release_tag(s);
        if let Ok(v) = Version::parse(&n) {
            best = Some(match best {
                None => v,
                Some(b) => b.max(v),
            });
        }
    }
    if let Some(v) = fleet_max {
        best = Some(match best {
            None => v,
            Some(b) => b.max(v),
        });
    }

    if let Some(v) = best {
        return Ok(v.to_string());
    }

    Ok(remote_tag
        .map(str::to_string)
        .or(pin)
        .or_else(|| {
            let indices = list_managed_instance_indices(paths, app).ok()?;
            indices
                .iter()
                .find_map(|&i| read_instance_recorded_version(paths, app, i))
        })
        .unwrap_or_else(|| "—".to_string()))
}

/// Per-instance `#n: version` list for tooltips when multiple instances exist.
pub fn read_installed_version_fleet_per_instance_breakdown(
    paths: &AppPaths,
    app: &CatalogApp,
) -> Result<Option<String>> {
    let mut indices = list_managed_instance_indices(paths, app)?;
    indices.sort_unstable();
    if indices.is_empty() {
        return Ok(None);
    }
    let parts: Vec<String> = indices
        .iter()
        .map(|&i| {
            let v = read_instance_recorded_version(paths, app, i)
                .unwrap_or_else(|| "unknown".to_string());
            format!("#{i}: {v}")
        })
        .collect();
    Ok(Some(parts.join(", ")))
}

/// `.appimage-control-version` for a specific instance (not the first instance only).
pub fn read_instance_recorded_version(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
) -> Option<String> {
    let p = paths
        .instance_dir_for(app, instance_index)
        .join(crate::version_info::INSTANCE_VERSION_FILENAME);
    fs::read_to_string(&p).ok().and_then(|s| {
        let t = s.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

fn inferred_catalog_cache_release_for_reconcile(
    app: &CatalogApp,
    cache_dir: &Path,
    filename: &str,
) -> Option<String> {
    crate::version_info::read_catalog_cache_release_for_install(cache_dir)
        .or_else(|| crate::version_info::version_from_appimage_filename(filename))
        .or_else(|| {
            let pin = crate::version_info::catalog_pinned_version(app)?;
            if crate::version_info::catalog_cache_fresh_for_pinned(filename, pin.as_str()) {
                Some(pin)
            } else {
                None
            }
        })
}

/// When the instance AppImage has the **same byte size** as the catalog cache blob and we can infer
/// a release label from cache metadata (or a fresh catalog pin / filename semver), rewrite the
/// per-instance `.appimage-control-version` file if it is missing or disagrees — so the manager
/// matches reality when the binary was already replaced from cache but metadata lagged.
///
/// Returns `true` when the version file was written.
pub fn reconcile_instance_version_metadata_if_binary_matches_cache(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    on_progress: &mut dyn FnMut(&str),
) -> Result<bool> {
    let filename = app.resolved_appimage_filename();
    let dir = paths.instance_dir_for(app, instance_index);
    let dest_bin = dir.join(&filename);
    if !dest_bin.is_file() {
        return Ok(false);
    }
    let dest_meta = fs::metadata(&dest_bin)?;
    if dest_meta.len() == 0 {
        return Ok(false);
    }
    let cache_dir = paths.catalog_cache_dir(&app.id);
    let cache_blob = cache_dir.join(&filename);
    if !cache_blob.is_file() {
        return Ok(false);
    }
    let cache_meta = fs::metadata(&cache_blob)?;
    if cache_meta.len() != dest_meta.len() {
        return Ok(false);
    }
    let Some(cache_ver) = inferred_catalog_cache_release_for_reconcile(app, &cache_dir, &filename)
    else {
        return Ok(false);
    };
    if let Some(ref r) = read_instance_recorded_version(paths, app, instance_index) {
        if crate::version_info::recorded_release_matches_tag(r, &cache_ver) {
            return Ok(false);
        }
    }
    let normalized = crate::version_info::normalize_github_release_tag(&cache_ver);
    let meta_path = dir.join(crate::version_info::INSTANCE_VERSION_FILENAME);
    fs::write(&meta_path, format!("{normalized}\n"))?;
    on_progress(&format!(
        "{} instance {}: reconciled install metadata to {} (AppImage size matches catalog cache).",
        app.id, instance_index, normalized
    ));
    Ok(true)
}

/// Runs [`reconcile_instance_version_metadata_if_binary_matches_cache`] for every managed instance
/// of every catalog app.
pub fn reconcile_catalog_install_version_metadata(
    paths: &AppPaths,
    catalog: &Catalog,
    on_progress: &mut dyn FnMut(&str),
) -> Result<u32> {
    let mut fixed: u32 = 0;
    for app in &catalog.apps {
        let indices = list_managed_instance_indices(paths, app)?;
        for i in indices {
            if reconcile_instance_version_metadata_if_binary_matches_cache(
                paths,
                app,
                i,
                on_progress,
            )? {
                fixed += 1;
            }
        }
    }
    Ok(fixed)
}

/// Deletes `~/.local/share/appimages/cache/` (per-catalog downloaded AppImages) and recreates an empty `cache/` dir.
pub fn clean_catalog_download_cache(paths: &AppPaths) -> Result<()> {
    let cache_root = paths.manager_data_dir().join("cache");
    if cache_root.exists() {
        fs::remove_dir_all(&cache_root)?;
    }
    fs::create_dir_all(&cache_root)?;
    Ok(())
}

/// Removes managed `appimage-control-*.desktop` files in the user applications directory.
pub fn sweep_appimage_control_desktop_files(paths: &AppPaths) -> Result<()> {
    let apps_dir = paths.applications_dir();
    if apps_dir.is_dir() {
        for ent in fs::read_dir(&apps_dir)? {
            let ent = ent?;
            let n = ent.file_name().to_string_lossy().into_owned();
            if n.starts_with(DESKTOP_PREFIX) && n.ends_with(".desktop") {
                let _ = fs::remove_file(ent.path());
            }
        }
    }
    Ok(())
}

/// Removes `appimage-control-*.png` tiles under common hicolor sizes (matches install layout).
pub fn sweep_appimage_control_hicolor_png_files(paths: &AppPaths) -> Result<()> {
    for size in [32u32, 48, 64, 128, 256] {
        let dir = paths.user_hicolor_apps_dir(size);
        if !dir.is_dir() {
            continue;
        }
        for ent in fs::read_dir(&dir)? {
            let ent = ent?;
            let n = ent.file_name().to_string_lossy().into_owned();
            if n.starts_with(DESKTOP_PREFIX) && n.ends_with(".png") {
                let _ = fs::remove_file(ent.path());
            }
        }
    }
    Ok(())
}

fn prune_empty_catalog_app_roots(paths: &AppPaths, catalog: &Catalog) -> Result<()> {
    for app in &catalog.apps {
        let root = paths.apps_root_for_app(app);
        if !root.is_dir() {
            continue;
        }
        let mut it = fs::read_dir(&root)?;
        if it.next().is_none() {
            fs::remove_dir_all(&root)?;
        }
    }
    let apps_root = paths.manager_data_dir().join("apps");
    if apps_root.is_dir() {
        let mut it = fs::read_dir(&apps_root)?;
        if it.next().is_none() {
            fs::remove_dir_all(&apps_root)?;
        }
    }
    Ok(())
}

/// Removes every catalog app instance (same teardown as setting each app to **0** instances) and prunes
/// empty `apps/<id>/` trees. **Does not** delete `~/.local/share/appimages/cache/` (downloaded catalog
/// AppImages stay on disk). Also sweeps managed `.desktop` files and `appimage-control-*.png` hicolor
/// tiles so stray launcher entries are cleared.
pub fn clean_installed_catalog_apps_keep_cache(
    paths: &AppPaths,
    catalog: &Catalog,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    for app in &catalog.apps {
        on_progress(&format!("Removing all instances of {} …", app.id));
        reconcile_instance_count(paths, app, 0, on_progress)?;
    }
    prune_empty_catalog_app_roots(paths, catalog)?;
    sweep_appimage_control_desktop_files(paths)?;
    sweep_appimage_control_hicolor_png_files(paths)?;
    update_desktop_database_best_effort(paths);
    update_icon_cache_best_effort(paths);
    on_progress("Clean finished (cache preserved).");
    Ok(())
}

/// Removes **all** AppImage Control catalog installs, download cache, standalone `files/`, manager `icons/`,
/// managed `.desktop` files, and `appimage-control-*.png` tiles under common hicolor sizes.
///
/// When `catalog` is `Some`, each app is reconciled to **0** instances first (proper teardown including
/// portal FUSE best-effort). When `catalog` is `None`, only filesystem cleanup runs (still deletes
/// `apps/`, `cache/`, etc.).
pub fn purge_appimage_control_state(
    paths: &AppPaths,
    catalog: Option<&Catalog>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    if let Some(catalog) = catalog {
        for app in &catalog.apps {
            on_progress(&format!("Removing all instances of {} …", app.id));
            reconcile_instance_count(paths, app, 0, on_progress)?;
        }
    }

    let root = paths.manager_data_dir();
    for sub in ["apps", "cache", "files", "icons"] {
        let p = root.join(sub);
        if p.exists() {
            on_progress(&format!("Removing {} …", p.display()));
            fs::remove_dir_all(&p)?;
        }
    }
    fs::create_dir_all(root.join("cache"))?;

    sweep_appimage_control_desktop_files(paths)?;
    sweep_appimage_control_hicolor_png_files(paths)?;

    update_desktop_database_best_effort(paths);
    update_icon_cache_best_effort(paths);
    on_progress("Purge finished.");
    Ok(())
}

/// Cached AppImage path (download if missing or if cache file is older than the catalog-pinned version).
pub fn ensure_cached_appimage(
    paths: &AppPaths,
    app: &CatalogApp,
    on_progress: &mut dyn FnMut(&str),
) -> Result<std::path::PathBuf> {
    let url = app.download_url_required()?;
    let filename = app.resolved_appimage_filename();
    let dir = paths.catalog_cache_dir(&app.id);
    fs::create_dir_all(&dir)?;
    let dest = dir.join(&filename);
    let pinned = crate::version_info::catalog_pinned_version(app);

    if dest.is_file() {
        let meta = fs::metadata(&dest)?;
        if meta.len() > 0 {
            match pinned.as_deref() {
                None => {
                    on_progress(&format!("Using cached {}", filename));
                    return Ok(dest);
                }
                Some(p) => {
                    if crate::version_info::catalog_cache_fresh_for_pinned(&filename, p) {
                        on_progress(&format!("Using cached {}", filename));
                        return Ok(dest);
                    }
                }
            }
            on_progress(&format!(
                "Cache {} is outdated for this catalog; replacing …",
                filename
            ));
            let _ = fs::remove_file(&dest);
            let _ = fs::remove_file(dir.join(crate::version_info::GITHUB_CACHE_TAG_FILENAME));
        }
    }

    on_progress(&format!("Downloading {} …", url));
    download_to_file(url, &dest)?;
    let mut perms = fs::metadata(&dest)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest, perms)?;
    // Blob is now from the catalog URL; drop any GitHub-latest sidecar so it cannot override the
    // new `.appimage-control-version` during install / migration reconcile.
    let _ = fs::remove_file(dir.join(crate::version_info::GITHUB_CACHE_TAG_FILENAME));
    if let Some(pv) = crate::version_info::catalog_pinned_version(app) {
        crate::version_info::write_catalog_cache_recorded_version(&dir, &pv)?;
    } else if let Some(v) = crate::version_info::version_from_download_url(url) {
        crate::version_info::write_catalog_cache_recorded_version(&dir, &v)?;
    }
    Ok(dest)
}

fn copy_or_hardlink(src: &Path, dst: &Path) -> Result<()> {
    // Build a unique temp name in the same directory as `dst` so that the final
    // `fs::rename` is always on the same filesystem (and therefore atomic).
    let dst_dir = dst.parent().ok_or_else(|| {
        Error::InvalidPath(format!("dst has no parent: {}", dst.display()))
    })?;
    let tmp = dst_dir.join(format!(
        ".{}.new.{}",
        dst.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("appimage"),
        uuid::Uuid::new_v4()
    ));

    // Attempt hard-link first (zero-copy on same filesystem). Fall back to a
    // real copy. Either way, if the operation fails we clean up the temp file
    // and leave `dst` untouched.
    let result = if fs::hard_link(src, &tmp).is_ok() {
        Ok(())
    } else {
        fs::copy(src, &tmp).map(|_| ())
    };

    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }

    // Atomic swap: replaces `dst` if it already exists.
    if let Err(e) = fs::rename(&tmp, dst) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }

    Ok(())
}

fn cryptomator_apprun_path(instance_dir: &Path) -> std::path::PathBuf {
    instance_dir.join("squashfs-root/AppRun")
}

/// Extract the Cryptomator AppImage (`--appimage-extract`) and patch `squashfs-root/lib/app/Cryptomator.cfg`
/// so each instance uses its own IPC socket and data under the portable `*.config` / `*.home` dirs.
///
/// The stock jpackage launcher only reads cfg from the squashfs; it does not merge `~/.local/cryptomator/Cryptomator.cfg`,
/// and the embedded cfg resolves `@{userhome}` to the real home, so a second instance always attaches to the first.
pub fn ensure_cryptomator_instance_ready(
    instance_dir: &Path,
    app: &CatalogApp,
    reextract: bool,
) -> Result<()> {
    if app.instance_strategy != InstanceStrategy::CryptomatorExtract {
        return Ok(());
    }
    let filename = app.resolved_appimage_filename();
    let dest_bin = instance_dir.join(&filename);
    if !dest_bin.is_file() {
        return Err(Error::InvalidPath(format!(
            "Cryptomator AppImage missing: {}",
            dest_bin.display()
        )));
    }

    let portable_config = instance_dir.join(format!("{}.config", filename));
    let portable_home = instance_dir.join(format!("{}.home", filename));
    fs::create_dir_all(&portable_config)?;
    fs::create_dir_all(&portable_home)?;
    fs::create_dir_all(portable_home.join(".local/share/Cryptomator/logs"))?;
    fs::create_dir_all(portable_home.join(".local/share/Cryptomator/mnt"))?;
    fs::create_dir_all(portable_home.join(".Cryptomator"))?;
    fs::create_dir_all(portable_config.join("Cryptomator"))?;

    let apprun = cryptomator_apprun_path(instance_dir);
    let squash = instance_dir.join("squashfs-root");
    if reextract || !apprun.is_file() {
        if squash.is_dir() {
            fs::remove_dir_all(&squash)?;
        }
        let status = host_command(&dest_bin)
            .arg("--appimage-extract")
            .current_dir(instance_dir)
            .status()
            .map_err(|e| {
                Error::InvalidPath(format!(
                    "cryptomator --appimage-extract failed to start: {}",
                    e
                ))
            })?;
        if !status.success() {
            return Err(Error::InvalidPath(format!(
                "cryptomator --appimage-extract failed with status {}",
                status
            )));
        }
        if !apprun.is_file() {
            return Err(Error::InvalidPath(
                "squashfs-root/AppRun missing after Cryptomator AppImage extract".into(),
            ));
        }
    }

    let portable_config = portable_config.canonicalize()?;
    let portable_home = portable_home.canonicalize()?;
    patch_cryptomator_cfg(instance_dir, &portable_config, &portable_home)?;
    Ok(())
}

fn patch_cryptomator_cfg(
    instance_dir: &Path,
    portable_config: &Path,
    portable_home: &Path,
) -> Result<()> {
    let cfg_path = instance_dir.join("squashfs-root/lib/app/Cryptomator.cfg");
    if !cfg_path.is_file() {
        return Err(Error::InvalidPath(format!(
            "Cryptomator.cfg missing: {}",
            cfg_path.display()
        )));
    }
    let pc_str = portable_config.to_string_lossy();
    let ph_str = portable_home.to_string_lossy();
    let ph_trim = ph_str.trim_end_matches('/');
    let ph_local = format!("{ph_trim}/.local/share");
    let ph_dot_crypt = format!("{ph_trim}/.Cryptomator");

    let mut content = fs::read_to_string(&cfg_path)?;
    content = content.replace("@{userhome}/.local/share", &ph_local);
    content = content.replace("@{userhome}/.config", &pc_str);
    content = content.replace("@{userhome}/.Cryptomator", &ph_dot_crypt);
    fs::write(&cfg_path, content)?;
    Ok(())
}

/// When `provision_portable_dirs` is false, empty `*.home` / `*.config` are not created (except
/// Cryptomator, which still runs [`ensure_cryptomator_instance_ready`]). Used by portable CLI
/// migration so an empty scaffold does not hide a real destination conflict.
pub fn install_instance_from_cache(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    cached_appimage: &Path,
    provision_portable_dirs: bool,
) -> Result<()> {
    if instance_index == 0 || instance_index > MAX_INSTANCES_PER_APP {
        return Err(Error::InvalidPath(format!(
            "instance index out of range: {}",
            instance_index
        )));
    }
    let dir = paths.instance_dir_for(app, instance_index);
    fs::create_dir_all(&dir)?;
    let filename = app.resolved_appimage_filename();
    let dest_bin = dir.join(&filename);
    copy_or_hardlink(cached_appimage, &dest_bin)?;
    let mut perms = fs::metadata(&dest_bin)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest_bin, perms)?;

    // AppImage portable layout: `<name>.config` and `<name>.home` next to the binary
    // (matches `~/dev/installers/linux` e.g. `feather.AppImage.config`, `feather.AppImage.home`).
    let portable_config = dir.join(format!("{}.config", filename));
    let portable_home = dir.join(format!("{}.home", filename));
    if provision_portable_dirs {
        fs::create_dir_all(&portable_config)?;
        fs::create_dir_all(&portable_home)?;
    }

    // Portable migration skips portable dirs for non-Cryptomator apps; Cryptomator needs extract
    // only after legacy data is moved onto `*.home` — see `run_portable_dir_migration`.
    if provision_portable_dirs
        || !matches!(app.instance_strategy, InstanceStrategy::CryptomatorExtract)
    {
        ensure_cryptomator_instance_ready(&dir, app, true)?;
    }

    let cache_dir = paths.catalog_cache_dir(&app.id);
    let meta = dir.join(crate::version_info::INSTANCE_VERSION_FILENAME);
    if let Some(v) = crate::version_info::read_catalog_cache_release_for_install(&cache_dir) {
        let t = v.trim();
        if !t.is_empty() {
            let _ = fs::write(&meta, format!("{t}\n"));
        }
    } else if let Ok(url) = app.resolved_download_url() {
        if let Some(v) = crate::version_info::version_from_download_url(url) {
            let _ = fs::write(&meta, format!("{}\n", v));
        }
    }

    if let Err(e) = write_instance_desktop_and_icon(paths, app, instance_index, &dest_bin) {
        // Launcher write failed — remove the just-created instance dir so no ghost instance
        // remains (binary present but unlaunhable). Best-effort: if cleanup itself fails we
        // still surface the original launcher error to the caller.
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    Ok(())
}

/// Use an absolute `.desktop` `Icon=` path to the 256×256 hicolor PNG (sharp dock tile on GNOME).
fn use_absolute_desktop_icon_png(app: &CatalogApp) -> bool {
    matches!(
        app.instance_strategy,
        InstanceStrategy::JoplinAltInstance
            | InstanceStrategy::ElectronClass
            | InstanceStrategy::ElectronClassIsolated
            | InstanceStrategy::QtX11
    )
}

/// Writes per-instance launcher script, `.desktop`, and icons. Returns `true` if the launcher script
/// or `.desktop` file content changed (icons may still be refreshed separately).
fn write_instance_desktop_and_icon(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    dest_bin: &Path,
) -> Result<bool> {
    let instance_dir = dest_bin
        .parent()
        .ok_or_else(|| Error::InvalidPath("instance AppImage path has no parent".into()))?;
    let cryptomator_dir = matches!(app.instance_strategy, InstanceStrategy::CryptomatorExtract)
        .then_some(instance_dir);
    let icon_name = write_instance_icon_png(paths, app, instance_index, cryptomator_dir)?;
    // Joplin, Electron, Qt: absolute `Icon=` to the 256×256 PNG (hicolor also has 128×128).
    // Cryptomator keeps theme `Icon=` only.
    let icon_file_owned: Option<PathBuf> = if use_absolute_desktop_icon_png(app) {
        icon_name
            .as_ref()
            .map(|n| paths.user_hicolor_apps_dir(256).join(format!("{}.png", n)))
    } else {
        None
    };
    let icon_file: Option<&Path> = icon_file_owned.as_deref();
    let icon_name_for_desktop: Option<&str> = if icon_file.is_some() {
        None
    } else {
        icon_name.as_deref()
    };
    let startup_wm_class = instance_startup_wm_class_for_desktop(app, instance_index);
    let extra = augment_instance_exec_suffix_for_portable_electron(
        instance_exec_suffix(app, instance_index),
        app,
        dest_bin,
    );
    let display = instance_desktop_display_name(app, instance_index);
    let cryptomator_apprun = match app.instance_strategy {
        InstanceStrategy::CryptomatorExtract => Some(cryptomator_apprun_path(instance_dir)),
        _ => None,
    };
    let (launcher, launcher_changed) = ensure_instance_launcher_script(
        paths,
        app,
        instance_index,
        dest_bin,
        &extra,
        &startup_wm_class,
        cryptomator_apprun.as_deref(),
    )?;
    let body = build_instance_desktop_entry(
        &display,
        &stable_instance_app_id(&app.id, instance_index),
        &app.id,
        instance_index,
        &launcher,
        "",
        app.desktop_pass_file_urls(),
        icon_name_for_desktop,
        icon_file,
        &startup_wm_class,
    );
    let desktop_path = paths
        .applications_dir()
        .join(desktop_filename_for_instance(&app.id, instance_index));
    fs::create_dir_all(paths.applications_dir())?;
    let desktop_changed = match fs::read_to_string(&desktop_path) {
        Ok(existing) => {
            if existing == body {
                false
            } else {
                fs::write(&desktop_path, &body)?;
                true
            }
        }
        Err(_) => {
            fs::write(&desktop_path, &body)?;
            true
        }
    };
    if launcher_changed || desktop_changed {
        update_desktop_database_best_effort(paths);
        update_icon_cache_best_effort(paths);
    }

    Ok(launcher_changed || desktop_changed)
}

/// LM Studio’s Electron shell can ignore `XDG_CONFIG_HOME` for the Chromium profile tree, which would
/// collapse multiple catalog instances onto one Chromium lockfile. Force a distinct
/// `--user-data-dir=…/lmstudio-chromium-profile` under each instance’s portable `.config` dir.
///
/// **Note:** The “quit the llmster daemon first” dialog is **not** a Chromium issue. On startup the app
/// runs `tryFindLocalAPIServer()` — parallel HTTP `GET` to `127.0.0.1:<port>/lms-status` on a fixed
/// set of high ports bundled as `apiServerPorts`. **Any** llmster already listening on one of those
/// localhost ports (another AppImage Control instance, a system LM Studio install, or `lms daemon up`)
/// satisfies that probe and triggers the dialog. Isolating D-Bus does not change loopback TCP; avoiding
/// the dialog requires not having another llmster bound on those ports (LM Studio does not expose a
/// supported env var to move that probe off 127.0.0.1).
fn augment_instance_exec_suffix_for_portable_electron(
    mut extra: String,
    app: &CatalogApp,
    dest_bin: &Path,
) -> String {
    if app.id != "lm-studio" {
        return extra;
    }
    let Some(instance_dir) = dest_bin.parent() else {
        return extra;
    };
    let Some(fname) = dest_bin.file_name().and_then(|n| n.to_str()) else {
        return extra;
    };
    let portable_config = instance_dir.join(format!("{}.config", fname));
    let user_data = portable_config.join("lmstudio-chromium-profile");
    let p = user_data.to_string_lossy();
    if !extra.trim().is_empty() {
        extra.push(' ');
    }
    extra.push_str("--user-data-dir=");
    extra.push_str(p.as_ref());
    extra
}

fn ensure_instance_launcher_script(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    appimage_path: &Path,
    extra_exec_args: &str,
    startup_wm_class: &str,
    cryptomator_apprun: Option<&Path>,
) -> Result<(std::path::PathBuf, bool)> {
    let dir = paths.instance_dir_for(app, instance_index);
    fs::create_dir_all(&dir)?;
    let launcher = dir.join("appimage-control-launch.sh");

    let mut content = String::new();
    content.push_str("#!/bin/bash\nset -e\n");
    // Capture login home before any strategy overrides `HOME` (portable/isolated layouts). X11
    // clients default to `$HOME/.Xauthority`; without this, isolated Electron on XWayland cannot
    // open a display and exits immediately.
    content.push_str("APPIMAGE_CONTROL_REAL_HOME=\"${HOME}\"\n");
    // `.desktop` launches often use a minimal PATH; prepend user-local bindirs so apps find
    // helpers installed under ~/.local/bin or ~/.cargo/bin.
    content.push_str(
        "export PATH=\"${APPIMAGE_CONTROL_REAL_HOME}/.local/bin:${APPIMAGE_CONTROL_REAL_HOME}/.cargo/bin:/usr/local/bin:/usr/bin:/bin:${PATH}\"\n",
    );
    // `.desktop` and `gio launch` often omit `XDG_RUNTIME_DIR`; without it we cannot find Mutter’s
    // XWayland cookie under /run/user/<uid>/ and Electron exits before showing a window.
    content.push_str("export XDG_RUNTIME_DIR=\"${XDG_RUNTIME_DIR:-/run/user/$(id -u)}\"\n");
    content.push_str(
        "if [[ -z \"${DBUS_SESSION_BUS_ADDRESS:-}\" && -S \"${XDG_RUNTIME_DIR}/bus\" ]]; then\n  export DBUS_SESSION_BUS_ADDRESS=\"unix:path=${XDG_RUNTIME_DIR}/bus\"\nfi\n",
    );
    // GNOME/Mutter on Wayland often stores the XWayland cookie only under $XDG_RUNTIME_DIR, not in
    // ~/.Xauthority. `.desktop` launches may omit XAUTHORITY; after we override HOME, the default
    // $HOME/.Xauthority path is wrong — resolve before isolated strategies run.
    content.push_str(
        r#"appimage_control_ensure_xauthority() {
  if [[ -n "${XAUTHORITY:-}" && -f "$XAUTHORITY" ]]; then
    return 0
  fi
  if [[ -f "$APPIMAGE_CONTROL_REAL_HOME/.Xauthority" ]]; then
    export XAUTHORITY="$APPIMAGE_CONTROL_REAL_HOME/.Xauthority"
    return 0
  fi
  if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
    shopt -s nullglob
    local _xf
    for _xf in "$XDG_RUNTIME_DIR"/xauth_* "$XDG_RUNTIME_DIR"/.mutter-Xwaylandauth.*; do
      if [[ -f "$_xf" ]]; then
        export XAUTHORITY="$_xf"
        shopt -u nullglob
        return 0
      fi
    done
    shopt -u nullglob
  fi
  export XAUTHORITY="$APPIMAGE_CONTROL_REAL_HOME/.Xauthority"
}
"#,
    );
    content.push_str("APPIMAGE=");
    content.push_str(&shell_quote_path(appimage_path));
    content.push('\n');
    if let Some(ap) = cryptomator_apprun {
        content.push_str("APPRUN=");
        content.push_str(&shell_quote_path(ap));
        content.push('\n');
    }
    content.push_str("WMCLASS=");
    content.push_str(&shell_quote_str(startup_wm_class));
    content.push('\n');
    content.push_str("EXTRA_ARGS=");
    content.push_str(&shell_quote_str(extra_exec_args.trim()));
    content.push('\n');

    // Launch and (best-effort) rewrite WM_CLASS via xprop on XWayland/X11. Cryptomator’s JVM
    // otherwise uses native Wayland (xprop does not apply) and reports upstream WM_CLASS; we force
    // X11/XWayland so each instance can keep a distinct StartupWMClass + icon match.
    if cryptomator_apprun.is_some() {
        content.push_str(
            r#"
export GDK_BACKEND=x11
export JDK_JAVA_OPTIONS="-Djdk.awt.useWayland=false${JDK_JAVA_OPTIONS:+ ${JDK_JAVA_OPTIONS}}"
"$APPRUN" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
        );
    } else if matches!(
        app.instance_strategy,
        InstanceStrategy::JoplinAltInstance | InstanceStrategy::ElectronClass
    ) {
        content.push_str(
            r#"
# Force XWayland: native Wayland ignores `xprop`; Electron keeps a fixed app_id → GNOME gear.
unset WAYLAND_DISPLAY
export GDK_BACKEND=x11
export ELECTRON_OZONE_PLATFORM_HINT=x11
"$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
        );
    } else if app.instance_strategy == InstanceStrategy::ElectronClassIsolated {
        content.push_str(
            r#"
# Electron single-instance locks and profiles are typically tied to the app user data dir; isolate
# HOME/XDG per instance so multiple can run concurrently.
APP_BASENAME="$(basename "$APPIMAGE")"
PORTABLE_CONFIG="$(dirname "$APPIMAGE")/${APP_BASENAME}.config"
PORTABLE_HOME="$(dirname "$APPIMAGE")/${APP_BASENAME}.home"

export HOME="$PORTABLE_HOME"
export XDG_CONFIG_HOME="$PORTABLE_CONFIG"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CACHE_HOME="$HOME/.cache"
# AppImage FUSE mounts and Chromium temp trees under TMPDIR; use a short /tmp path (not only the
# portable home tree) to avoid rare mount/extract failures on nested paths.
_AIC_TMP="$(mktemp -d /tmp/appimage-control-isolated.XXXXXX)"
export TMPDIR="$_AIC_TMP"
trap 'rm -rf "$_AIC_TMP" 2>/dev/null || true' EXIT

mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME"

appimage_control_ensure_xauthority

# Per-instance runtime dir (GTK, portals, temp sockets). Session D-Bus for the *user* was resolved
# above before HOME override; DBUS_SESSION_BUS_ADDRESS is unchanged here.
export XDG_RUNTIME_DIR="$PORTABLE_HOME/.xdg-runtime"
mkdir -p "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

unset WAYLAND_DISPLAY
export GDK_BACKEND=x11
export ELECTRON_OZONE_PLATFORM_HINT=x11

"#,
        );
        if app.isolate_dbus_session.unwrap_or(false) {
            content.push_str(
                r#"# Tauri apps (e.g. Jan) use the user session D-Bus for `tauri_plugin_single_instance`
# (well-known bus name). A second catalog instance collides unless each run uses a private session
# bus. Drop inherited session-bus env so the AppImage runtime cannot hand the user bus to a child
# before dbus-run-session installs its own DBUS_SESSION_BUS_ADDRESS.
#
# LM Studio: this D-Bus isolation does **not** address the “quit the llmster daemon first” dialog.
# That check is driven by HTTP probes to 127.0.0.1 (see bundled `apiServerPorts` / `lms-status`), so
# another llmster on this machine still triggers it. Per-instance `--user-data-dir` above is what
# separates Chromium; loopback conflicts need no other llmster on those ports.
unset DBUS_SESSION_BUS_ADDRESS DBUS_STARTER_ADDRESS DBUS_STARTER_BUS_TYPE 2>/dev/null || true
if ! command -v dbus-run-session >/dev/null 2>&1; then
  echo "appimage-control: dbus-run-session not found (install dbus-dbus or dbus-x11)." >&2
  exit 1
fi
dbus-run-session -- "$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
            );
        } else {
            content.push_str(
                r#""$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
            );
        }
    } else if app.instance_strategy == InstanceStrategy::QtX11 {
        content.push_str(
            r#"
# Qt on native Wayland ignores `xprop`; force XCB so per-instance WM_CLASS sticks (dock + icons).
unset WAYLAND_DISPLAY
export QT_QPA_PLATFORM=xcb
"#,
        );
        if app.id == "feather" {
            content.push_str(
                r#"export TMPDIR="$(dirname "$APPIMAGE")/.feather-tmp"
mkdir -p "$TMPDIR"
"#,
            );
        }
        content.push_str(
            r#""$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
        );
    } else if app.instance_strategy == InstanceStrategy::KeepassxcIsolated {
        // KeePassXC uses Qt single-instance locks/sockets under QStandardPaths::TempLocation. When
        // launched from multiple instance dirs, isolate TMPDIR (and HOME/XDG) so instances are independent.
        content.push_str(
            r#"
APP_BASENAME="$(basename "$APPIMAGE")"
PORTABLE_CONFIG="$(dirname "$APPIMAGE")/${APP_BASENAME}.config"
PORTABLE_HOME="$(dirname "$APPIMAGE")/${APP_BASENAME}.home"

export HOME="$PORTABLE_HOME"
export XDG_CONFIG_HOME="$PORTABLE_CONFIG"
export XDG_DATA_HOME="$HOME/.local/share"
export XDG_CACHE_HOME="$HOME/.cache"
export TMPDIR="$HOME/.tmp"

mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$TMPDIR"

appimage_control_ensure_xauthority

"$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
        );
    } else {
        content.push_str(
            r#"
"$APPIMAGE" $EXTRA_ARGS "$@" &
APP_PID=$!
"#,
        );
    }
    content.push_str(
        r#"
if command -v xdotool >/dev/null 2>&1 && command -v xprop >/dev/null 2>&1; then
  collect_descendants() {
    local p="$1"
    echo "$p"
    local c
    for c in $(pgrep -P "$p" 2>/dev/null || true); do
      collect_descendants "$c"
    done
  }
  while kill -0 "$APP_PID" 2>/dev/null; do
    for pid in $(collect_descendants "$APP_PID" | sort -u); do
      WIN_IDS=$(xdotool search --pid "$pid" 2>/dev/null || true)
      for WIN_ID in $WIN_IDS; do
        xprop -id "$WIN_ID" -f WM_CLASS 8s -set WM_CLASS "$WMCLASS" "$WMCLASS" >/dev/null 2>&1 || true
      done
    done
    sleep 0.1
  done
fi
wait "$APP_PID" 2>/dev/null || true
"#,
    );

    let launcher_changed = match fs::read_to_string(&launcher) {
        Ok(existing) => existing != content,
        Err(_) => true,
    };
    if launcher_changed {
        fs::write(&launcher, &content)?;
        let mut perms = fs::metadata(&launcher)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&launcher, perms)?;
    }
    Ok((launcher, launcher_changed))
}

fn shell_quote_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\"'\"'");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn shell_quote_path(p: &Path) -> String {
    shell_quote_str(&p.to_string_lossy())
}

/// True when size and modification time match (both `modified()` Ok). Used to detect no-op `appimageupdatetool` runs.
pub fn appimage_file_identity_unchanged(
    before: &std::fs::Metadata,
    after: &std::fs::Metadata,
) -> bool {
    if before.len() != after.len() {
        return false;
    }
    match (before.modified(), after.modified()) {
        (Ok(tb), Ok(ta)) => tb == ta,
        _ => false,
    }
}

/// Result of trying to refresh an instance AppImage from update tooling or catalog URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceUpdateOutcome {
    /// Binary on disk changed (appimageupdatetool or re-download).
    Updated,
    /// After update attempts, size and mtime match the starting file (best-effort).
    Unchanged,
    /// Recorded version matches the catalog URL and GitHub latest (if known) is not semver-newer; no tool or network fetch.
    SkippedUpToDate,
}

struct InstanceTargetCacheSkipCtx<'a> {
    paths: &'a AppPaths,
    app: &'a CatalogApp,
    instance_index: u32,
    dest_bin: &'a Path,
    filename: &'a str,
    pin: &'a Option<String>,
    rec: &'a Option<String>,
    use_gh: bool,
    remote_hint: Option<&'a crate::version_info::GithubReleaseHint>,
}

fn try_skip_instance_already_matches_target_cache(
    ctx: &InstanceTargetCacheSkipCtx<'_>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<Option<InstanceUpdateOutcome>> {
    let dest_meta = fs::metadata(ctx.dest_bin)?;
    if dest_meta.len() == 0 {
        return Ok(None);
    }
    let cache_dir = ctx.paths.catalog_cache_dir(&ctx.app.id);
    let cache_blob = cache_dir.join(ctx.filename);
    if !cache_blob.is_file() {
        return Ok(None);
    }
    let cache_meta = fs::metadata(&cache_blob)?;
    if cache_meta.len() != dest_meta.len() {
        return Ok(None);
    }

    if ctx.use_gh {
        let Some(h) = ctx.remote_hint else {
            return Ok(None);
        };
        if !crate::version_info::github_catalog_cache_looks_fresh(&cache_dir, ctx.filename, &h.tag)
        {
            return Ok(None);
        }
        let Some(ref rec_s) = ctx.rec else {
            return Ok(None);
        };
        if !crate::version_info::recorded_release_matches_tag(rec_s, &h.tag) {
            return Ok(None);
        }
        on_progress(&format!(
            "{} instance {}: already matches cached GitHub release {}; skipping.",
            ctx.app.id, ctx.instance_index, h.tag
        ));
        return Ok(Some(InstanceUpdateOutcome::SkippedUpToDate));
    }

    let fresh = match ctx.pin.as_deref() {
        Some(p) => crate::version_info::catalog_cache_fresh_for_pinned(ctx.filename, p),
        None => true,
    };
    if !fresh {
        return Ok(None);
    }
    let Some(cache_rel) = crate::version_info::read_catalog_cache_release_for_install(&cache_dir)
    else {
        return Ok(None);
    };
    let Some(ref rec_s) = ctx.rec else {
        return Ok(None);
    };
    if !crate::version_info::recorded_release_matches_tag(rec_s, &cache_rel) {
        return Ok(None);
    }
    if let Some(ref p) = ctx.pin {
        if !crate::version_info::catalog_pin_matches_recorded(p, cache_rel.as_str()) {
            return Ok(None);
        }
        if !crate::version_info::catalog_pin_matches_recorded(p, rec_s) {
            return Ok(None);
        }
    }
    on_progress(&format!(
        "{} instance {}: already matches catalog cache; skipping.",
        ctx.app.id, ctx.instance_index
    ));
    Ok(Some(InstanceUpdateOutcome::SkippedUpToDate))
}

/// Try `appimageupdatetool` on the instance binary; on failure or absence, refresh from the catalog
/// cache or from a GitHub `releases/latest` AppImage asset when it is semver-newer than the catalog pin.
///
/// `remote_hint`: same-index entry from [`crate::version_info::fetch_github_release_hints`].
pub fn update_instance_appimage(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    remote_hint: Option<&crate::version_info::GithubReleaseHint>,
    on_progress: &mut dyn FnMut(&str),
) -> Result<InstanceUpdateOutcome> {
    update_instance_appimage_with_shared_cache(
        paths,
        app,
        instance_index,
        remote_hint,
        on_progress,
        &mut None,
    )
}

fn update_instance_appimage_with_shared_cache(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    remote_hint: Option<&crate::version_info::GithubReleaseHint>,
    on_progress: &mut dyn FnMut(&str),
    shared_catalog_blob: &mut Option<PathBuf>,
) -> Result<InstanceUpdateOutcome> {
    app.resolved_download_url()?;
    let dir = paths.instance_dir_for(app, instance_index);
    let filename = app.resolved_appimage_filename();
    let dest_bin = dir.join(&filename);
    if !dest_bin.is_file() {
        return Err(Error::InvalidPath(format!(
            "instance AppImage missing: {}",
            dest_bin.display()
        )));
    }

    let _ = reconcile_instance_version_metadata_if_binary_matches_cache(
        paths,
        app,
        instance_index,
        on_progress,
    )?;

    let pin = crate::version_info::catalog_pinned_version(app);
    let rec = read_instance_recorded_version(paths, app, instance_index);

    if let (Some(ref pinned), Some(ref recorded)) = (&pin, &rec) {
        if crate::version_info::catalog_pin_matches_recorded(pinned, recorded) {
            let pull_gh = remote_hint.is_some_and(|h| {
                crate::version_info::should_use_github_latest_asset(
                    h,
                    Some(pinned.as_str()),
                    Some(recorded.as_str()),
                )
            });
            let tag_newer_no_asset = remote_hint.is_some_and(|h| {
                h.appimage_download_url.is_none()
                    && crate::version_info::remote_release_newer_than_installed(&h.tag, recorded)
            });
            if !pull_gh && !tag_newer_no_asset {
                on_progress(&format!(
                    "{} instance {}: already at catalog version {}; skipping.",
                    app.id, instance_index, pinned
                ));
                return Ok(InstanceUpdateOutcome::SkippedUpToDate);
            }
            if pull_gh {
                if let Some(h) = remote_hint {
                    on_progress(&format!(
                        "{} instance {}: GitHub release {} is newer than catalog ({}); will try updater then direct download if needed.",
                        app.id, instance_index, h.tag, pinned
                    ));
                }
            } else if tag_newer_no_asset {
                if let Some(h) = remote_hint {
                    on_progress(&format!(
                        "{} instance {}: GitHub tag {} is newer than install but no AppImage asset was selected; trying appimageupdatetool …",
                        app.id, instance_index, h.tag
                    ));
                }
            }
        }
    }

    let use_gh = remote_hint.is_some_and(|h| {
        crate::version_info::should_use_github_latest_asset(h, pin.as_deref(), rec.as_deref())
    });

    let cache_skip_ctx = InstanceTargetCacheSkipCtx {
        paths,
        app,
        instance_index,
        dest_bin: &dest_bin,
        filename: &filename,
        pin: &pin,
        rec: &rec,
        use_gh,
        remote_hint,
    };
    if let Some(skip) =
        try_skip_instance_already_matches_target_cache(&cache_skip_ctx, on_progress)?
    {
        return Ok(skip);
    }

    let meta_at_start = fs::metadata(&dest_bin)?;

    if crate::update::appimageupdatetool_available() {
        on_progress(&format!(
            "{} instance {}: running appimageupdatetool …",
            app.id, instance_index
        ));
        match crate::update::run_appimageupdatetool(&dest_bin) {
            Ok(out) => {
                let trimmed = out.trim();
                if !trimmed.is_empty() {
                    for line in trimmed.lines() {
                        on_progress(line);
                    }
                }
                let meta_after_tool = fs::metadata(&dest_bin)?;
                if appimage_file_identity_unchanged(&meta_at_start, &meta_after_tool) {
                    on_progress(&format!(
                        "{} instance {}: appimageupdatetool did not change the on-disk file; fetching build …",
                        app.id, instance_index
                    ));
                } else {
                    on_progress(&format!(
                        "{} instance {}: appimageupdatetool finished (binary replaced)",
                        app.id, instance_index
                    ));
                    refresh_instance_version_metadata(paths, app, instance_index, None)?;
                    ensure_cryptomator_instance_ready(&dir, app, true)?;
                    let _ = write_instance_desktop_and_icon(paths, app, instance_index, &dest_bin)?;
                    return Ok(InstanceUpdateOutcome::Updated);
                }
            }
            Err(e) => {
                on_progress(&format!(
                    "{} instance {}: appimageupdatetool failed ({}), re-downloading …",
                    app.id, instance_index, e
                ));
            }
        }
    }

    let tmp_name = format!(".{}.new.{}", filename, uuid::Uuid::new_v4());
    let tmp_path = dir.join(tmp_name);

    // Inner closure so that any `?` propagation is caught here and we can
    // clean up the temp file before returning the error to the caller.
    // This guarantees `dest_bin` is never left missing or truncated on failure.
    let stage_result = (|| -> Result<()> {
        if use_gh {
            let hint = remote_hint.ok_or_else(|| {
                Error::InvalidPath("internal: GitHub upgrade without release hint".into())
            })?;
            let gh_url = hint.appimage_download_url.as_ref().ok_or_else(|| {
                Error::InvalidPath("internal: GitHub upgrade without asset URL".into())
            })?;
            let cache_dir = paths.catalog_cache_dir(&app.id);
            let cache_path = cache_dir.join(&filename);
            if crate::version_info::github_catalog_cache_looks_fresh(
                &cache_dir,
                &filename,
                &hint.tag,
            ) {
                on_progress(&format!(
                    "{} instance {}: using cached GitHub release {} …",
                    app.id, instance_index, hint.tag
                ));
                fs::copy(&cache_path, &tmp_path)?;
                // Backfill markers so the next instance does not re-download after a partial metadata layout.
                crate::version_info::write_github_cache_tag_sidecar(&cache_dir, &hint.tag)?;
                crate::version_info::write_catalog_cache_recorded_version(&cache_dir, &hint.tag)?;
            } else {
                on_progress(&format!(
                    "{} instance {}: downloading GitHub release {} …",
                    app.id, instance_index, hint.tag
                ));
                download_to_file(gh_url, &tmp_path)?;
            }
        } else {
            let cached = if let Some(ref p) = shared_catalog_blob {
                p.clone()
            } else {
                let p = ensure_cached_appimage(paths, app, on_progress)?;
                *shared_catalog_blob = Some(p.clone());
                p
            };
            on_progress(&format!(
                "{} instance {}: applying build from catalog cache …",
                app.id, instance_index
            ));
            fs::copy(&cached, &tmp_path)?;
        }

        let mut perms = fs::metadata(&tmp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp_path, perms)?;
        fs::rename(&tmp_path, &dest_bin)?;
        Ok(())
    })();

    if let Err(e) = stage_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    let gh_ver = if use_gh {
        remote_hint.map(|h| h.tag.as_str())
    } else {
        None
    };
    refresh_instance_version_metadata(paths, app, instance_index, gh_ver)?;

    if use_gh {
        let cache_dir = paths.catalog_cache_dir(&app.id);
        fs::create_dir_all(&cache_dir)?;
        let cache_path = cache_dir.join(&filename);
        let _ = fs::copy(&dest_bin, &cache_path);
        if let Some(h) = remote_hint {
            crate::version_info::write_github_cache_tag_sidecar(&cache_dir, &h.tag)?;
            crate::version_info::write_catalog_cache_recorded_version(&cache_dir, &h.tag)?;
        }
    }

    ensure_cryptomator_instance_ready(&dir, app, true)?;
    let _ = write_instance_desktop_and_icon(paths, app, instance_index, &dest_bin)?;
    let meta_at_end = fs::metadata(&dest_bin)?;
    if appimage_file_identity_unchanged(&meta_at_start, &meta_at_end) {
        Ok(InstanceUpdateOutcome::Unchanged)
    } else {
        Ok(InstanceUpdateOutcome::Updated)
    }
}

fn refresh_instance_version_metadata(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    version_override: Option<&str>,
) -> Result<()> {
    let v = if let Some(v) = version_override {
        Some(v.to_string())
    } else {
        let Ok(url) = app.resolved_download_url() else {
            return Ok(());
        };
        crate::version_info::version_from_download_url(url)
    };
    if let Some(v) = v {
        let meta = paths
            .instance_dir_for(app, instance_index)
            .join(crate::version_info::INSTANCE_VERSION_FILENAME);
        let _ = fs::write(meta, format!("{}\n", v));
    }
    Ok(())
}

/// Aggregate counts from [`update_all_installed_instances`].
#[derive(Debug, Default, Clone, Copy)]
pub struct UpdateAllReport {
    pub updated: u32,
    pub unchanged: u32,
    /// Instances that matched the catalog URL version before any tool or download ran.
    pub skipped_up_to_date: u32,
    pub errors: u32,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ReconcileReport {
    pub kept: Vec<u32>,
    pub removed: Vec<u32>,
    pub added: Vec<u32>,
}

/// For each catalog app with at least one installed instance and a resolvable download URL, update every instance.
///
/// `remote_hints`: optional slice parallel to `catalog.apps` (same indices) from
/// [`crate::version_info::fetch_github_release_hints`]. When `None`, GitHub is not consulted.
///
/// `on_fraction`: optional callback after each instance finishes: `(fraction, done, total)` where
/// `done`/`total` count instances (`total == 0` means nothing to update). Also called with
/// `(0.0, 0, total)` before work and `(1.0, 0, 0)` when there is nothing to update.
pub fn update_all_installed_instances(
    paths: &AppPaths,
    catalog: &Catalog,
    remote_hints: Option<&[Option<crate::version_info::GithubReleaseHint>]>,
    on_progress: &mut dyn FnMut(&str),
    mut on_fraction: Option<&mut dyn FnMut(f64, u32, u32)>,
) -> Result<UpdateAllReport> {
    let mut work: Vec<(usize, u32)> = Vec::new();
    for (app_idx, app) in catalog.apps.iter().enumerate() {
        if app.resolved_download_url().is_err() {
            continue;
        }
        let indices = list_instance_indices(paths, app)?;
        if indices.is_empty() {
            continue;
        }
        for i in indices {
            work.push((app_idx, i));
        }
    }

    let mut report = UpdateAllReport::default();

    if work.is_empty() {
        if let Some(f) = &mut on_fraction {
            f(1.0, 0, 0);
        }
        return Ok(report);
    }

    let total = work.len();
    let total_u32 = total as u32;
    if let Some(f) = &mut on_fraction {
        f(0.0, 0, total_u32);
    }
    let mut shared_catalog_blob: Option<PathBuf> = None;
    let mut prev_app_idx: Option<usize> = None;

    for (done, (app_idx, i)) in work.iter().enumerate() {
        if prev_app_idx != Some(*app_idx) {
            shared_catalog_blob = None;
            prev_app_idx = Some(*app_idx);
        }
        let app = &catalog.apps[*app_idx];
        let remote = remote_hints
            .and_then(|s| s.get(*app_idx))
            .and_then(|o| o.as_ref());
        match update_instance_appimage_with_shared_cache(
            paths,
            app,
            *i,
            remote,
            on_progress,
            &mut shared_catalog_blob,
        ) {
            Ok(InstanceUpdateOutcome::Updated) => report.updated += 1,
            Ok(InstanceUpdateOutcome::Unchanged) => report.unchanged += 1,
            Ok(InstanceUpdateOutcome::SkippedUpToDate) => report.skipped_up_to_date += 1,
            Err(e) => {
                report.errors += 1;
                on_progress(&format!("{} instance {}: {}", app.id, i, e));
            }
        }
        if let Some(f) = &mut on_fraction {
            let done_u = (done + 1) as u32;
            f(done_u as f64 / total_u32 as f64, done_u, total_u32);
        }
    }

    if report.updated + report.unchanged + report.skipped_up_to_date + report.errors > 0 {
        on_progress(&format!(
            "Summary: {} updated, {} unchanged, {} skipped (already catalog), {} failed.",
            report.updated, report.unchanged, report.skipped_up_to_date, report.errors
        ));
    }
    Ok(report)
}

/// Best-effort regeneration of per-instance `.desktop` files and icons for managed instances.
///
/// Instance indices come from [`list_managed_instance_indices`] (instance dirs and/or our
/// `appimage-control-<id>-<n>.desktop` files). Stray PNGs under `~/.local/share/icons` are **not**
/// used as install signals.
pub fn repair_instance_launchers(
    paths: &AppPaths,
    catalog: &Catalog,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    for app in &catalog.apps {
        let indices = list_managed_instance_indices(paths, app)?;
        if indices.is_empty() {
            continue;
        }
        let filename = app.resolved_appimage_filename();
        for i in indices {
            let dir = paths.instance_dir_for(app, i);
            let dest_bin = dir.join(&filename);
            if !dest_bin.is_file() {
                // Attempt to recreate the instance dir from cached AppImage if available.
                let cached = paths.catalog_cache_dir(&app.id).join(&filename);
                if cached.is_file() {
                    fs::create_dir_all(&dir)?;
                    copy_or_hardlink(&cached, &dest_bin)?;
                    let mut perms = fs::metadata(&dest_bin)?.permissions();
                    perms.set_mode(0o755);
                    fs::set_permissions(&dest_bin, perms)?;

                    let portable_config = dir.join(format!("{}.config", filename));
                    let portable_home = dir.join(format!("{}.home", filename));
                    let _ = fs::create_dir_all(&portable_config);
                    let _ = fs::create_dir_all(&portable_home);

                    let _ = refresh_instance_version_metadata(paths, app, i, None);
                } else {
                    on_progress(&format!(
                        "{} instance {}: skipped (missing AppImage: {}; cache missing: {})",
                        app.id,
                        i,
                        dest_bin.display(),
                        cached.display()
                    ));
                    continue;
                }
            }
            let _ = reconcile_instance_version_metadata_if_binary_matches_cache(
                paths,
                app,
                i,
                on_progress,
            )?;
            ensure_cryptomator_instance_ready(&dir, app, false)?;
            if write_instance_desktop_and_icon(paths, app, i, &dest_bin)? {
                on_progress(&format!("{} instance {}: launcher repaired", app.id, i));
            }
        }
    }
    Ok(())
}

/// xdg-document-portal may leave a `fuse.portal` mount under the portable instance home; `remove_dir_all`
/// then fails with EPERM until it is unmounted.
fn best_effort_unmount_portal_doc(instance_dir: &Path, app: &CatalogApp) {
    let Some(fname) = app.appimage_filename.as_deref() else {
        return;
    };
    let doc = instance_dir
        .join(format!("{fname}.home"))
        .join(".xdg-runtime/doc");
    if !doc.is_dir() {
        return;
    }
    let _ = host_command("fusermount3")
        .args(["-u", doc.to_string_lossy().as_ref()])
        .status();
}

pub fn remove_instance(paths: &AppPaths, app: &CatalogApp, instance_index: u32) -> Result<()> {
    let desktop = paths
        .applications_dir()
        .join(desktop_filename_for_instance(&app.id, instance_index));
    if desktop.is_file() {
        fs::remove_file(&desktop)?;
    }
    let dir = paths.instance_dir_for(app, instance_index);
    if dir.is_dir() {
        best_effort_unmount_portal_doc(&dir, app);
        fs::remove_dir_all(&dir)?;
    }
    update_desktop_database_best_effort(paths);
    Ok(())
}

/// Set exact instance count to `target` (0..=MAX). Adds/removes 1-based indices.
pub fn reconcile_instance_count(
    paths: &AppPaths,
    app: &CatalogApp,
    target: u32,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    reconcile_instance_count_with_report(paths, app, target, on_progress).map(|_| ())
}

pub fn reconcile_instance_count_with_report(
    paths: &AppPaths,
    app: &CatalogApp,
    target: u32,
    on_progress: &mut dyn FnMut(&str),
) -> Result<ReconcileReport> {
    reconcile_instance_count_inner(paths, app, target, true, on_progress)
}

/// Like [`reconcile_instance_count`] but does not pre-create empty portable `*.home` / `*.config`
/// trees (Cryptomator still provisions its layout). Used by [`crate::migrate::run_portable_dir_migration`].
pub fn reconcile_instance_count_for_portable_migration(
    paths: &AppPaths,
    app: &CatalogApp,
    target: u32,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    reconcile_instance_count_inner(paths, app, target, false, on_progress).map(|_| ())
}

fn reconcile_instance_count_inner(
    paths: &AppPaths,
    app: &CatalogApp,
    target: u32,
    provision_portable_dirs: bool,
    on_progress: &mut dyn FnMut(&str),
) -> Result<ReconcileReport> {
    if target > MAX_INSTANCES_PER_APP {
        return Err(Error::InvalidPath(format!(
            "max {} instances per app",
            MAX_INSTANCES_PER_APP
        )));
    }
    if target > app.max_instances_allowed() {
        return Err(Error::TooManyInstances {
            id: app.id.clone(),
            max: app.max_instances_allowed(),
        });
    }
    if target > 0 {
        app.download_url_required()?;
    }

    let mut current: Vec<u32> = list_managed_instance_indices(paths, app)?;
    let current_set: HashSet<u32> = current.iter().copied().collect();
    let want: HashSet<u32> = (1..=target).collect();

    let mut to_remove: Vec<u32> = current_set.difference(&want).copied().collect();
    to_remove.sort_by(|a, b| b.cmp(a));
    let removed = to_remove.clone();
    for i in to_remove {
        on_progress(&format!("Removing {} instance {} …", app.id, i));
        remove_instance(paths, app, i)?;
    }

    current = list_managed_instance_indices(paths, app)?;
    let current_set: HashSet<u32> = current.iter().copied().collect();
    let mut to_add: Vec<u32> = want.difference(&current_set).copied().collect();
    to_add.sort_unstable();
    let added = to_add.clone();

    if !to_add.is_empty() {
        let cached = ensure_cached_appimage(paths, app, on_progress)?;
        for i in to_add {
            on_progress(&format!("Installing {} instance {} …", app.id, i));
            install_instance_from_cache(paths, app, i, &cached, provision_portable_dirs)?;
        }
    }

    let kept: Vec<u32> = (1..=target)
        .filter(|i| !removed.contains(i) && !added.contains(i))
        .collect();

    Ok(ReconcileReport {
        kept,
        removed,
        added,
    })
}

fn update_desktop_database_best_effort(paths: &AppPaths) {
    let _ = host_command("update-desktop-database")
        .arg(paths.applications_dir())
        .status();
}

fn update_icon_cache_best_effort(paths: &AppPaths) {
    let hicolor = paths.home().join(".local/share/icons/hicolor");
    if hicolor.is_dir() {
        let _ = host_command("gtk-update-icon-cache")
            .args(["-q", "-f", "-t"])
            .arg(&hicolor)
            .status();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Catalog, CatalogApp};
    use crate::desktop::desktop_filename_for_instance;
    use crate::paths::AppPaths;
    use std::process::Command;

    #[test]
    fn appimage_file_identity_unchanged_same_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("bin");
        fs::write(&p, b"x").unwrap();
        let m1 = fs::metadata(&p).unwrap();
        let m2 = fs::metadata(&p).unwrap();
        assert!(appimage_file_identity_unchanged(&m1, &m2));
        fs::write(&p, b"yy").unwrap();
        let m3 = fs::metadata(&p).unwrap();
        assert!(!appimage_file_identity_unchanged(&m1, &m3));
    }

    #[test]
    fn zulip_catalog_uses_absolute_desktop_icon_via_electron_class_isolated() {
        let app = CatalogApp {
            id: "zulip".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClassIsolated,
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
        };
        assert!(super::use_absolute_desktop_icon_png(&app));
    }

    #[test]
    fn generic_none_strategy_skips_absolute_desktop_icon() {
        let app = CatalogApp {
            id: "some-other-app".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
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
        };
        assert!(!super::use_absolute_desktop_icon_png(&app));
    }

    fn versioned_catalog_app(id: &str, fname: &str, url: &str) -> CatalogApp {
        CatalogApp {
            id: id.to_string(),
            display_name: Some(id.to_string()),
            homepage: None,
            download_url: Some(url.into()),
            download_url_aarch64: None,
            appimage_filename: Some(fname.into()),
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
    fn read_installed_version_fleet_summary_semver_span() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "fleetv",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.0.0/session.AppImage",
        );
        let cache = paths.catalog_cache_dir("fleetv");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"x").unwrap();
        let cached = cache.join("session.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        install_instance_from_cache(&paths, &app, 2, &cached, true).unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 1)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.17.17\n",
        )
        .unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 2)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.18.0\n",
        )
        .unwrap();
        assert_eq!(
            read_installed_version_fleet_summary(&paths, &app)
                .unwrap()
                .unwrap(),
            "1.17.17–1.18.0"
        );
        assert_eq!(
            read_installed_version(&paths, &app).unwrap().unwrap(),
            "1.17.17"
        );
    }

    #[test]
    fn installed_version_fleet_display_single_version() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "fleetdisp1",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.18.0/session.AppImage",
        );
        let cache = paths.catalog_cache_dir("fleetdisp1");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"x").unwrap();
        let cached = cache.join("session.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 1)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.18.0\n",
        )
        .unwrap();
        let d = installed_version_fleet_display(&paths, &app).unwrap();
        assert_eq!(d.label, "installed 1.18.0");
        assert_eq!(d.details, vec!["#1: 1.18.0".to_string()]);
    }

    #[test]
    fn installed_version_fleet_display_mixed_range() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "fleetdisp2",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.17.17/session.AppImage",
        );
        let cache = paths.catalog_cache_dir("fleetdisp2");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"x").unwrap();
        let cached = cache.join("session.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        install_instance_from_cache(&paths, &app, 2, &cached, true).unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 1)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.17.17\n",
        )
        .unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 2)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.18.0\n",
        )
        .unwrap();
        let d = installed_version_fleet_display(&paths, &app).unwrap();
        assert_eq!(d.label, "installed 1.17.17-1.18.0");
        assert_eq!(d.details.len(), 2);
    }

    #[test]
    fn installed_version_fleet_display_hash_fallback_when_version_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "fleetdisp3",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.18.0/session.AppImage",
        );
        let cache = paths.catalog_cache_dir("fleetdisp3");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"abcdef").unwrap();
        let cached = cache.join("session.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let _ = fs::remove_file(
            paths
                .instance_dir_for(&app, 1)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
        );
        let d = installed_version_fleet_display(&paths, &app).unwrap();
        assert_eq!(d.label, "installed unknown");
        assert_eq!(d.details.len(), 1);
        assert!(d.details[0].starts_with("#1: hash "));
        let h = d.details[0].trim_start_matches("#1: hash ");
        assert_eq!(h.len(), 10);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn reconcile_fixes_version_file_when_appimage_matches_cache_size() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "metafix",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.17.17/session.AppImage",
        );
        let cache_dir = paths.catalog_cache_dir("metafix");
        fs::create_dir_all(&cache_dir).unwrap();
        let blob = b"payloadbytes";
        fs::write(cache_dir.join("session.AppImage"), blob).unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache_dir, "1.18.0").unwrap();
        let dir = paths.instance_dir_for(&app, 1);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.AppImage"), blob).unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.17.17\n",
        )
        .unwrap();
        let mut lines = Vec::new();
        assert!(reconcile_instance_version_metadata_if_binary_matches_cache(
            &paths,
            &app,
            1,
            &mut |s| lines.push(s.to_string())
        )
        .unwrap());
        assert_eq!(
            read_instance_recorded_version(&paths, &app, 1).as_deref(),
            Some("1.18.0")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("reconciled install metadata")),
            "{lines:?}"
        );
    }

    #[test]
    fn reconcile_noop_when_version_already_matches_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "metafix2",
            "session.AppImage",
            "https://github.com/a/b/releases/download/v1.17.17/session.AppImage",
        );
        let cache_dir = paths.catalog_cache_dir("metafix2");
        fs::create_dir_all(&cache_dir).unwrap();
        let blob = b"x";
        fs::write(cache_dir.join("session.AppImage"), blob).unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache_dir, "1.18.0").unwrap();
        let dir = paths.instance_dir_for(&app, 1);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.AppImage"), blob).unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.18.0\n",
        )
        .unwrap();
        assert!(
            !reconcile_instance_version_metadata_if_binary_matches_cache(
                &paths,
                &app,
                1,
                &mut |_| {}
            )
            .unwrap()
        );
    }

    #[test]
    fn catalog_row_latest_label_includes_fleet_when_ahead_of_catalog_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "pinlag",
            "feather-2.8.1.AppImage",
            "https://github.com/x/y/releases/download/2.8.1/feather-2.8.1.AppImage",
        );
        let cache = paths.catalog_cache_dir("pinlag");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather-2.8.1.AppImage"), b"x").unwrap();
        let cached = cache.join("feather-2.8.1.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        install_instance_from_cache(&paths, &app, 2, &cached, true).unwrap();
        for idx in [1u32, 2u32] {
            fs::write(
                paths
                    .instance_dir_for(&app, idx)
                    .join(crate::version_info::INSTANCE_VERSION_FILENAME),
                b"2.9.0\n",
            )
            .unwrap();
        }
        assert_eq!(
            catalog_row_latest_version_label(&paths, &app, None).unwrap(),
            "2.9.0"
        );
        assert_eq!(
            catalog_row_latest_version_label(&paths, &app, Some("2.8.1")).unwrap(),
            "2.9.0"
        );
        assert_eq!(
            catalog_row_latest_version_label(&paths, &app, Some("3.0.0")).unwrap(),
            "3.0.0"
        );
    }

    #[test]
    fn update_instance_skips_when_recorded_version_matches_catalog() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "skipme",
            "feather-2.8.1.AppImage",
            "https://github.com/x/y/releases/download/2.8.1/feather-2.8.1.AppImage",
        );
        let idx = 1u32;
        let dir = paths.instance_dir_for(&app, idx);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("feather-2.8.1.AppImage");
        fs::write(&bin, b"appimg").unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.1\n",
        )
        .unwrap();

        let out = update_instance_appimage(&paths, &app, idx, None, &mut |_| {}).unwrap();
        assert_eq!(out, InstanceUpdateOutcome::SkippedUpToDate);
        assert_eq!(fs::read(&bin).unwrap(), b"appimg");
    }

    #[test]
    fn update_instance_skips_when_recorded_normalized_matches_catalog_pin() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "skipnorm",
            "feather-2.8.1.AppImage",
            "https://github.com/x/y/releases/download/2.8.1/feather-2.8.1.AppImage",
        );
        let idx = 1u32;
        let dir = paths.instance_dir_for(&app, idx);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("feather-2.8.1.AppImage");
        fs::write(&bin, b"appimg").unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"v2.8.1\n",
        )
        .unwrap();

        let out = update_instance_appimage(&paths, &app, idx, None, &mut |_| {}).unwrap();
        assert_eq!(out, InstanceUpdateOutcome::SkippedUpToDate);
        assert_eq!(fs::read(&bin).unwrap(), b"appimg");
    }

    #[test]
    fn update_instance_skips_via_catalog_cache_when_unversioned_url_matches_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("opaqueurl", "session.AppImage");
        let cache_dir = paths.catalog_cache_dir("opaqueurl");
        fs::create_dir_all(&cache_dir).unwrap();
        let blob = b"samebytes";
        fs::write(cache_dir.join("session.AppImage"), blob).unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache_dir, "1.5.0").unwrap();

        let idx = 1u32;
        let dir = paths.instance_dir_for(&app, idx);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("session.AppImage");
        fs::write(&bin, blob).unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.5.0\n",
        )
        .unwrap();

        let mut lines = Vec::new();
        let out =
            update_instance_appimage(&paths, &app, idx, None, &mut |s| lines.push(s.to_string()))
                .unwrap();
        assert_eq!(out, InstanceUpdateOutcome::SkippedUpToDate);
        assert!(
            lines
                .iter()
                .any(|l| l.contains("already matches catalog cache")),
            "expected cache skip log, got {lines:?}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.contains("applying build from catalog cache")),
            "should not copy from cache when skip applies: {lines:?}"
        );
    }

    #[test]
    fn update_all_reconciles_stale_metadata_when_binary_matches_cache_then_skips() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "twoup",
            "feather-2.8.1.AppImage",
            "https://example.invalid/wont/fetch/feather-2.8.1.AppImage",
        );
        let cat = Catalog {
            apps: vec![app.clone()],
        };
        let cache_dir = paths.catalog_cache_dir("twoup");
        fs::create_dir_all(&cache_dir).unwrap();
        let blob = b"blobtwoup!!";
        fs::write(cache_dir.join("feather-2.8.1.AppImage"), blob).unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache_dir, "2.8.1").unwrap();

        for idx in [1u32, 2u32] {
            let dir = paths.instance_dir_for(&app, idx);
            fs::create_dir_all(&dir).unwrap();
            let bin = dir.join("feather-2.8.1.AppImage");
            fs::write(&bin, blob).unwrap();
        }
        fs::write(
            paths
                .instance_dir_for(&app, 1)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.1\n",
        )
        .unwrap();
        fs::write(
            paths
                .instance_dir_for(&app, 2)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.0\n",
        )
        .unwrap();

        let mut lines = Vec::new();
        let report = update_all_installed_instances(
            &paths,
            &cat,
            None,
            &mut |s| {
                lines.push(s.to_string());
            },
            None,
        )
        .unwrap();
        assert_eq!(
            report.skipped_up_to_date, 2,
            "report={report:?} lines={lines:?}"
        );
        assert_eq!(report.updated, 0);
        assert_eq!(report.unchanged, 0);
        assert_eq!(
            read_instance_recorded_version(&paths, &app, 2).as_deref(),
            Some("2.8.1")
        );
        assert!(
            lines.iter().any(|l| {
                l.contains("twoup instance 2:") && l.contains("reconciled install metadata")
            }),
            "expected metadata reconcile for instance 2: {lines:?}"
        );
        assert!(
            lines.iter().any(|l| {
                l.contains("already matches catalog cache")
                    || l.contains("already at catalog version")
            }),
            "expected skip-without-refresh log: {lines:?}"
        );
    }

    #[test]
    fn update_all_stale_metadata_wrong_binary_size_still_refreshes_from_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "twomismatch",
            "feather-2.8.1.AppImage",
            "https://example.invalid/wont/fetch/feather-2.8.1.AppImage",
        );
        let cat = Catalog {
            apps: vec![app.clone()],
        };
        let cache_dir = paths.catalog_cache_dir("twomismatch");
        fs::create_dir_all(&cache_dir).unwrap();
        let blob = b"blobtwoup!!";
        fs::write(cache_dir.join("feather-2.8.1.AppImage"), blob).unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache_dir, "2.8.1").unwrap();

        let dir1 = paths.instance_dir_for(&app, 1);
        fs::create_dir_all(&dir1).unwrap();
        fs::write(dir1.join("feather-2.8.1.AppImage"), blob).unwrap();
        fs::write(
            dir1.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.1\n",
        )
        .unwrap();

        let dir2 = paths.instance_dir_for(&app, 2);
        fs::create_dir_all(&dir2).unwrap();
        let mut wrong_bin = blob.to_vec();
        wrong_bin.push(b'x');
        fs::write(dir2.join("feather-2.8.1.AppImage"), wrong_bin).unwrap();
        fs::write(
            dir2.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.0\n",
        )
        .unwrap();

        let report = update_all_installed_instances(&paths, &cat, None, &mut |_| {}, None).unwrap();
        assert!(
            report.updated + report.unchanged >= 1,
            "instance 2 should refresh from cache when binary size differs, report={report:?}"
        );
        assert_eq!(
            read_instance_recorded_version(&paths, &app, 2).as_deref(),
            Some("2.8.1")
        );
    }

    #[test]
    fn update_instance_no_skip_when_github_newer_than_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "sessionish",
            "session-1.17.17.AppImage",
            "https://example.invalid/wont/session-1.17.17.AppImage",
        );
        let idx = 1u32;
        let dir = paths.instance_dir_for(&app, idx);
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("session-1.17.17.AppImage");
        fs::write(&bin, b"appimg").unwrap();
        fs::write(
            dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"1.17.17\n",
        )
        .unwrap();

        let gh = crate::version_info::GithubReleaseHint {
            tag: "1.18.0".into(),
            appimage_download_url: None,
        };
        if let Ok(InstanceUpdateOutcome::SkippedUpToDate) =
            update_instance_appimage(&paths, &app, idx, Some(&gh), &mut |_| {})
        {
            panic!("GitHub newer than install must not short-circuit skip");
        }
    }

    #[test]
    fn ensure_cached_removes_stale_file_before_download_attempt() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "stalecat",
            "feather-2.8.0.AppImage",
            "https://example.invalid/wont/fetch/feather-2.8.1.AppImage",
        );
        let cache = paths.catalog_cache_dir("stalecat");
        fs::create_dir_all(&cache).unwrap();
        let dest = cache.join("feather-2.8.0.AppImage");
        fs::write(&dest, b"old").unwrap();
        let r = ensure_cached_appimage(&paths, &app, &mut |_| {});
        assert!(r.is_err());
        assert!(
            !dest.exists(),
            "stale cache file should be removed before failed download"
        );
    }

    fn fake_app(id: &str, fname: &str) -> CatalogApp {
        CatalogApp {
            id: id.to_string(),
            display_name: Some(id.to_string()),
            homepage: None,
            download_url: Some("http://example.invalid/x".into()),
            download_url_aarch64: None,
            appimage_filename: Some(fname.into()),
            instance_strategy: crate::catalog::InstanceStrategy::QtX11,
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
    fn exec_suffix_feather_style() {
        // Placeholder expansion (real Feather catalog has no `--name`; upstream is single-instance).
        let app = CatalogApp {
            id: "feather-wallet".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x".into()),
            download_url_aarch64: None,
            appimage_filename: Some("w.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: Some("--name {instance_name}".into()),
            instance_name_template: Some("{Id}{index}".into()),
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
        };
        assert_eq!(instance_exec_suffix(&app, 1), "--name Feather1");
        assert_eq!(instance_exec_suffix(&app, 2), "--name Feather2");
    }

    #[test]
    fn clean_catalog_download_cache_removes_and_recreates_cache_root() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let cache = paths.manager_data_dir().join("cache").join("x");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("blob"), b"1").unwrap();
        clean_catalog_download_cache(&paths).unwrap();
        let root = paths.manager_data_dir().join("cache");
        assert!(root.is_dir());
        assert!(!root.join("x").exists());
    }

    #[test]
    fn purge_removes_instances_cache_desktops_and_standalone_files() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("purgeapp", "purge.AppImage");
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let files = paths.managed_files_dir();
        fs::create_dir_all(&files).unwrap();
        fs::write(files.join("standalone.AppImage"), b"x").unwrap();

        let cache = paths.catalog_cache_dir("purgeapp");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("purge.AppImage"), b"fake").unwrap();
        let cached = cache.join("purge.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();

        let desktop = paths
            .applications_dir()
            .join(desktop_filename_for_instance("purgeapp", 1));
        assert!(desktop.is_file(), "expected .desktop at {:?}", desktop);

        let hicolor = paths.user_hicolor_apps_dir(256);
        fs::create_dir_all(&hicolor).unwrap();
        fs::write(hicolor.join("appimage-control-purgeapp-1.png"), b"i").unwrap();

        purge_appimage_control_state(&paths, Some(&cat), &mut |_| {}).unwrap();

        assert!(!paths.instance_dir_for(&app, 1).exists());
        assert!(!paths.manager_data_dir().join("apps").exists());
        assert!(paths.manager_data_dir().join("cache").is_dir());
        assert!(!cache.join("purge.AppImage").exists());
        assert!(!desktop.exists());
        assert!(!files.join("standalone.AppImage").exists());
        assert!(!hicolor.join("appimage-control-purgeapp-1.png").exists());
    }

    #[test]
    fn clean_installed_keeps_cache_and_removes_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("keepcache", "k.AppImage");
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let files = paths.managed_files_dir();
        fs::create_dir_all(&files).unwrap();
        fs::write(files.join("standalone.AppImage"), b"keep").unwrap();

        let cache = paths.catalog_cache_dir("keepcache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("k.AppImage"), b"cached").unwrap();
        let cached = cache.join("k.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();

        let desktop = paths
            .applications_dir()
            .join(desktop_filename_for_instance("keepcache", 1));
        assert!(desktop.is_file());

        clean_installed_catalog_apps_keep_cache(&paths, &cat, &mut |_| {}).unwrap();

        assert!(!paths.instance_dir_for(&app, 1).exists());
        assert!(!desktop.exists());
        assert!(cached.is_file(), "cache AppImage should remain");
        assert_eq!(fs::read(&cached).unwrap(), b"cached");
        assert!(files.join("standalone.AppImage").is_file());
    }

    #[test]
    fn parse_app_instance_spec_accepts_optional_count() {
        assert_eq!(parse_app_instance_spec("foo").unwrap(), ("foo".into(), 1));
        assert_eq!(parse_app_instance_spec("foo:3").unwrap(), ("foo".into(), 3));
        assert_eq!(
            parse_app_instance_spec("org-app:2").unwrap(),
            ("org-app".into(), 2)
        );
        assert!(parse_app_instance_spec("foo:x").is_err());
    }

    #[test]
    fn reconcile_rejects_target_above_catalog_max_instances() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let mut app = fake_app("boxed", "b.AppImage");
        app.max_instances = Some(1);
        let r = reconcile_instance_count(&paths, &app, 2, &mut |_| {});
        assert!(matches!(
            r,
            Err(crate::error::Error::TooManyInstances { ref id, max: 1 }) if id == "boxed"
        ));
    }

    #[test]
    fn add_remove_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("feather", "feather.AppImage");
        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();

        let cached = cache.join("feather.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let inst = paths.instance_dir("feather", 1);
        assert!(inst.join("feather.AppImage").exists());
        assert!(inst.join("feather.AppImage.config").is_dir());
        assert!(inst.join("feather.AppImage.home").is_dir());
        let n = list_instance_indices(&paths, &app).unwrap();
        assert_eq!(n, vec![1]);

        remove_instance(&paths, &app, 1).unwrap();
        assert!(list_instance_indices(&paths, &app).unwrap().is_empty());
    }

    #[test]
    fn instance_startup_wm_class_is_unique_per_instance() {
        let id = "feather";
        let mut seen = std::collections::HashSet::new();
        for n in 1u32..=5u32 {
            let wm = instance_startup_wm_class(id, n);
            assert!(
                seen.insert(wm.clone()),
                "duplicate WM class for instance {}: {}",
                n,
                wm
            );
        }
        assert_ne!(
            instance_startup_wm_class(id, 1),
            instance_startup_wm_class(id, 2)
        );
    }

    #[test]
    fn cryptomator_extract_ignores_wayland_app_id_for_distinct_desktop_wm_class() {
        let app = CatalogApp {
            id: "cryptomator".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/c.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("cryptomator.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::CryptomatorExtract,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: Some("org.cryptomator.launcher.Cryptomator$MainApp".into()),
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        let w1 = instance_startup_wm_class_for_desktop(&app, 1);
        let w2 = instance_startup_wm_class_for_desktop(&app, 2);
        assert_eq!(w1, instance_startup_wm_class("cryptomator", 1));
        assert_eq!(w2, instance_startup_wm_class("cryptomator", 2));
        assert_ne!(w1, w2);
    }

    #[test]
    fn wm_class_placeholder_expands_in_exec_suffix() {
        let app = CatalogApp {
            id: "my-app".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("test.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: Some("--class={wm_class}".into()),
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
        };
        let wm = instance_startup_wm_class("my-app", 2);
        assert_eq!(wm, "io.github.appimagecontrol.my-app.i2");
        assert_eq!(instance_exec_suffix(&app, 2), format!("--class={}", wm));
    }

    #[test]
    fn desktop_file_has_startup_wm_class_after_install() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "feather".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("feather.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::QtX11,
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
        };
        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        let cached = cache.join("feather.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let desktop_path = paths
            .applications_dir()
            .join(desktop_filename_for_instance("feather", 1));
        let raw = fs::read_to_string(&desktop_path).unwrap();
        let parsed = crate::desktop::parse_desktop_str(&raw).unwrap();
        let expect_wm = instance_startup_wm_class("feather", 1);
        assert_eq!(parsed.startup_wm_class.as_deref(), Some(expect_wm.as_str()));
        assert!(raw.contains(&format!("StartupWMClass={}", expect_wm)));
        let launcher = paths
            .instance_dir("feather", 1)
            .join("appimage-control-launch.sh");
        let launch = fs::read_to_string(&launcher).unwrap();
        assert!(launch.contains("QT_QPA_PLATFORM=xcb"));
        assert!(launch.contains("unset WAYLAND_DISPLAY"));
        assert!(launch.contains("TMPDIR=") && launch.contains(".feather-tmp"));
    }

    #[test]
    fn redownload_atomic_rename_replaces_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("app.AppImage");
        fs::write(&dest, b"v1").unwrap();
        let tmp = dir.path().join(".app.AppImage.new");
        fs::write(&tmp, b"v2").unwrap();
        let mut perms = fs::metadata(&tmp).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp, perms).unwrap();
        fs::rename(&tmp, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"v2");
    }

    #[test]
    fn keepassxc_isolated_launcher_sets_tmpdir_and_xdg_homes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "keepassxc".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/k.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("KeePassXC.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::KeepassxcIsolated,
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
        };
        let cache = paths.catalog_cache_dir("keepassxc");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("KeePassXC.AppImage"), b"fake").unwrap();
        let cached = cache.join("KeePassXC.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let launcher = paths
            .instance_dir("keepassxc", 1)
            .join("appimage-control-launch.sh");
        let launch = fs::read_to_string(&launcher).unwrap();
        assert!(launch.contains("PORTABLE_CONFIG="));
        assert!(launch.contains("PORTABLE_HOME="));
        assert!(launch.contains("export TMPDIR="));
        assert!(launch.contains("export XDG_CONFIG_HOME="));
        assert!(launch.contains("export XDG_DATA_HOME="));
        assert!(launch.contains("export XDG_CACHE_HOME="));
        assert!(launch.contains("APPIMAGE_CONTROL_REAL_HOME="));
        assert!(launch.contains("appimage_control_ensure_xauthority"));
        assert!(launch.contains("mutter-Xwaylandauth"));
    }

    #[test]
    fn electron_class_isolated_launcher_sets_tmpdir_and_xdg_homes() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "iso-app".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/iso.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Iso.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClassIsolated,
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
        };
        let cache = paths.catalog_cache_dir("iso-app");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("Iso.AppImage"), b"fake").unwrap();
        let cached = cache.join("Iso.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let launcher = paths
            .instance_dir("iso-app", 1)
            .join("appimage-control-launch.sh");
        let launch = fs::read_to_string(&launcher).unwrap();
        assert!(launch.contains("PORTABLE_CONFIG="));
        assert!(launch.contains("PORTABLE_HOME="));
        assert!(launch.contains("export TMPDIR="));
        assert!(launch.contains("mktemp -d /tmp/appimage-control-isolated"));
        assert!(launch.contains("rm -rf \"$_AIC_TMP\" 2>/dev/null || true"));
        assert!(launch.contains("export XDG_CONFIG_HOME="));
        assert!(launch.contains("export XDG_DATA_HOME="));
        assert!(launch.contains("export XDG_CACHE_HOME="));
        assert!(launch.contains("ELECTRON_OZONE_PLATFORM_HINT=x11"));
        assert!(launch.contains("APPIMAGE_CONTROL_REAL_HOME="));
        assert!(launch.contains("export XDG_RUNTIME_DIR="));
        assert!(
            launch.contains("export XDG_RUNTIME_DIR=\"$PORTABLE_HOME/.xdg-runtime\""),
            "isolated apps use a per-instance XDG_RUNTIME_DIR under the portable home"
        );
        assert!(launch.contains("chmod 700 \"$XDG_RUNTIME_DIR\""));
        assert!(launch.contains("DBUS_SESSION_BUS_ADDRESS"));
        assert!(
            !launch.contains("dbus-run-session"),
            "Electron-style isolated apps use the user session bus by default"
        );
        assert!(launch.contains("appimage_control_ensure_xauthority"));
        assert!(launch.contains("mutter-Xwaylandauth"));
        assert!(launch.contains("xauth_*"));
        let ok = Command::new("bash")
            .args(["-n", launcher.to_str().unwrap()])
            .status()
            .expect("bash -n");
        assert!(
            ok.success(),
            "isolated electron launcher must be valid shell"
        );
    }

    #[test]
    fn electron_class_isolated_tauri_uses_dbus_run_session_when_catalog_flag_set() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "jan".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/j.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Jan.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClassIsolated,
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
            isolate_dbus_session: Some(true),
            max_instances: None,
            max_instances_tooltip: None,
        };
        let cache = paths.catalog_cache_dir("jan");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("Jan.AppImage"), b"fake").unwrap();
        let cached = cache.join("Jan.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let launcher = paths
            .instance_dir("jan", 1)
            .join("appimage-control-launch.sh");
        let launch = fs::read_to_string(&launcher).unwrap();
        assert!(launch.contains("dbus-run-session -- \"$APPIMAGE\""));
        assert!(launch.contains("tauri_plugin_single_instance"));
        assert!(launch.contains("unset DBUS_SESSION_BUS_ADDRESS"));
        assert!(
            !launch.contains("--user-data-dir="),
            "Jan must not get LM Studio user-data-dir injection"
        );
        let ok = Command::new("bash")
            .args(["-n", launcher.to_str().unwrap()])
            .status()
            .expect("bash -n");
        assert!(ok.success());
    }

    #[test]
    fn electron_class_isolated_lm_studio_uses_dbus_run_session_when_catalog_flag_set() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "lm-studio".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/lms.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("LMStudio.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClassIsolated,
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
            isolate_dbus_session: Some(true),
            max_instances: None,
            max_instances_tooltip: None,
        };
        let cache = paths.catalog_cache_dir("lm-studio");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("LMStudio.AppImage"), b"fake").unwrap();
        let cached = cache.join("LMStudio.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let launcher = paths
            .instance_dir("lm-studio", 1)
            .join("appimage-control-launch.sh");
        let launch = fs::read_to_string(&launcher).unwrap();
        assert!(launch.contains("dbus-run-session -- \"$APPIMAGE\""));
        assert!(
            launch.contains("lms-status") && launch.contains("apiServerPorts"),
            "launcher should document llmster localhost probe (bundled apiServerPorts / lms-status)"
        );
        assert!(
            launch.contains("--user-data-dir=") && launch.contains("lmstudio-chromium-profile"),
            "LM Studio needs an explicit Chromium profile per instance; expected user-data-dir in:\n{}",
            launch
        );
        assert!(launch.contains("unset DBUS_SESSION_BUS_ADDRESS"));
        let ok = Command::new("bash")
            .args(["-n", launcher.to_str().unwrap()])
            .status()
            .expect("bash -n");
        assert!(ok.success());
    }

    #[test]
    fn install_writes_instance_version_from_cache_not_only_catalog_url() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = versioned_catalog_app(
            "sessmeta",
            "session.AppImage",
            "https://github.com/session-foundation/session-desktop/releases/download/v1.17.17/session-desktop-linux-x86_64-1.17.17.AppImage",
        );
        let cache = paths.catalog_cache_dir("sessmeta");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"x").unwrap();
        crate::version_info::write_catalog_cache_recorded_version(&cache, "1.18.0").unwrap();
        install_instance_from_cache(&paths, &app, 1, &cache.join("session.AppImage"), true)
            .unwrap();
        let meta = paths
            .instance_dir_for(&app, 1)
            .join(crate::version_info::INSTANCE_VERSION_FILENAME);
        assert_eq!(fs::read_to_string(&meta).unwrap().trim(), "1.18.0");
    }

    #[test]
    fn writes_version_metadata_from_catalog_url() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = CatalogApp {
            id: "feather".into(),
            display_name: None,
            homepage: None,
            download_url: Some(
                "https://github.com/x/y/releases/download/2.0.0/feather-2.0.0.AppImage".into(),
            ),
            download_url_aarch64: None,
            appimage_filename: Some("feather.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::QtX11,
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
        };
        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        let cached = cache.join("feather.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();
        let meta = paths
            .instance_dir("feather", 1)
            .join(crate::version_info::INSTANCE_VERSION_FILENAME);
        assert_eq!(fs::read_to_string(meta).unwrap().trim(), "2.0.0");
    }

    #[test]
    fn joplin_strategy_is_no_sandbox_and_ozone_x11() {
        let app = CatalogApp {
            id: "joplin".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/j.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Joplin.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::JoplinAltInstance,
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
        };
        assert_eq!(
            instance_exec_suffix(&app, 1),
            "--no-sandbox --ozone-platform=x11"
        );
        assert_eq!(
            instance_exec_suffix(&app, 3),
            "--no-sandbox --ozone-platform=x11"
        );
    }

    #[test]
    fn electron_class_ignores_wayland_app_id_for_distinct_desktop_wm_class() {
        let app = CatalogApp {
            id: "session".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/s.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("session.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClass,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: Some("Session".into()),
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        let w1 = instance_startup_wm_class_for_desktop(&app, 1);
        let w2 = instance_startup_wm_class_for_desktop(&app, 2);
        assert_eq!(w1, instance_startup_wm_class("session", 1));
        assert_eq!(w2, instance_startup_wm_class("session", 2));
        assert_ne!(w1, w2);
    }

    #[test]
    fn electron_class_strategy_includes_no_sandbox_and_wm_class() {
        let app = CatalogApp {
            id: "session".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/s.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Session.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClass,
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
        };
        let wm = instance_startup_wm_class("session", 2);
        assert_eq!(
            instance_exec_suffix(&app, 2),
            format!("--no-sandbox --ozone-platform=x11 --class={}", wm)
        );
    }

    #[test]
    fn template_overrides_strategy_when_present() {
        let app = CatalogApp {
            id: "feather".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/f.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("feather.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClass,
            instance_exec_args_template: Some("--name {instance_name}".into()),
            instance_name_template: Some("{Id}{index}".into()),
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
        };
        assert_eq!(instance_exec_suffix(&app, 1), "--name Feather1");
    }

    #[test]
    fn cryptomator_cfg_substitution_replaces_userhome_placeholders() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("squashfs-root/lib/app");
        fs::create_dir_all(&root).unwrap();
        let cfg_path = root.join("Cryptomator.cfg");
        fs::write(
            &cfg_path,
            r#"java-options=-Dcryptomator.logDir=@{userhome}/.local/share/Cryptomator/logs
java-options=-Dcryptomator.settingsPath=@{userhome}/.config/Cryptomator/settings.json:@{userhome}/.Cryptomator/settings.json
java-options=-Dcryptomator.ipcSocketPath=@{userhome}/.config/Cryptomator/ipc.socket
"#,
        )
        .unwrap();
        let pc = tmp.path().join("c.config");
        let ph = tmp.path().join("c.home");
        fs::create_dir_all(&pc).unwrap();
        fs::create_dir_all(&ph).unwrap();
        let pc = pc.canonicalize().unwrap();
        let ph = ph.canonicalize().unwrap();
        patch_cryptomator_cfg(tmp.path(), &pc, &ph).unwrap();
        let out = fs::read_to_string(&cfg_path).unwrap();
        let ph_trim = ph.to_string_lossy().trim_end_matches('/').to_string();
        assert!(out.contains(&format!("{}/.local/share/Cryptomator/logs", ph_trim)));
        assert!(out.contains(&format!(
            "{}/Cryptomator/settings.json:",
            pc.to_string_lossy().trim_end_matches('/')
        )));
        assert!(!out.contains("@{userhome}"));
    }

    #[test]
    fn repair_is_quiet_when_launcher_and_desktop_already_match() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("feather", "feather.AppImage");
        let cat = crate::catalog::Catalog {
            apps: vec![app.clone()],
        };
        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        let cached = cache.join("feather.AppImage");
        install_instance_from_cache(&paths, &app, 1, &cached, true).unwrap();

        let mut log = String::new();
        repair_instance_launchers(&paths, &cat, &mut |m| {
            log.push_str(m);
            log.push('\n');
        })
        .unwrap();
        assert!(
            !log.contains("launcher repaired"),
            "repair should not log repaired when nothing drifted (avoids noisy GUI startup): {log}"
        );
    }

    #[test]
    fn repair_recreates_instance_from_desktop_marker_and_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();

        // Simulate: cache exists, instance dir was removed, but our managed .desktop remains.
        let app = CatalogApp {
            id: "joplin".into(),
            display_name: Some("Joplin".into()),
            homepage: None,
            download_url: Some("http://example.invalid/Joplin-1.0.0.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Joplin.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: Some("--no-sandbox".into()),
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
        };
        let cat = crate::catalog::Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("joplin");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("Joplin.AppImage"), b"fake").unwrap();

        let apps_dir = paths.applications_dir();
        fs::create_dir_all(&apps_dir).unwrap();
        fs::write(
            apps_dir.join(desktop_filename_for_instance("joplin", 2)),
            "[Desktop Entry]\nType=Application\nName=Joplin\n",
        )
        .unwrap();

        // Orphan hicolor PNG alone must not imply an instance (regression guard).
        let icon_dir = paths.user_hicolor_apps_dir(256);
        fs::create_dir_all(&icon_dir).unwrap();
        fs::write(
            icon_dir.join("appimage-control-joplin-9.png"),
            b"\x89PNG\r\n\x1a\n",
        )
        .unwrap();

        repair_instance_launchers(&paths, &cat, &mut |_m| {}).unwrap();

        let inst_bin = paths.instance_dir("joplin", 2).join("Joplin.AppImage");
        assert!(inst_bin.is_file());

        let desktop = paths
            .applications_dir()
            .join(desktop_filename_for_instance("joplin", 2));
        assert!(desktop.is_file());
        assert_eq!(
            list_managed_instance_indices(&paths, &app).unwrap(),
            vec![2]
        );
        assert!(!paths.instance_dir("joplin", 9).exists());
    }

    #[test]
    fn orphan_hicolor_png_does_not_spawn_instance_on_repair() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("feather", "feather.AppImage");
        let cat = crate::catalog::Catalog {
            apps: vec![app.clone()],
        };
        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        let icon_dir = paths.user_hicolor_apps_dir(256);
        fs::create_dir_all(&icon_dir).unwrap();
        fs::write(icon_dir.join("appimage-control-feather-3.png"), b"x").unwrap();
        repair_instance_launchers(&paths, &cat, &mut |_m| {}).unwrap();
        assert!(list_managed_instance_indices(&paths, &app)
            .unwrap()
            .is_empty());
        assert!(!paths.instance_dir("feather", 3).exists());
    }

    #[test]
    fn list_instance_indices_finds_any_appimage_when_expected_filename_differs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("feather", "feather-new.AppImage");
        let inst = paths.instance_dir("feather", 1);
        fs::create_dir_all(&inst).unwrap();
        fs::write(inst.join("feather-old.AppImage"), b"fake").unwrap();
        let v = list_instance_indices(&paths, &app).unwrap();
        assert_eq!(v, vec![1]);
    }

    /// End-to-end: Session (Electron) uses `--class=` + Ozone X11 + launcher env; Joplin same env + `xprop` (no `--class`).
    #[test]
    fn session_and_joplin_installed_desktops_distinct_wm_class_and_exec_class() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();

        let session = CatalogApp {
            id: "session".into(),
            display_name: Some("Session".into()),
            homepage: None,
            download_url: Some("http://example.invalid/session.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("session.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::ElectronClass,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: Some("Session".into()),
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        let joplin = CatalogApp {
            id: "joplin".into(),
            display_name: Some("Joplin".into()),
            homepage: None,
            download_url: Some("http://example.invalid/Joplin.AppImage".into()),
            download_url_aarch64: None,
            appimage_filename: Some("Joplin.AppImage".into()),
            instance_strategy: crate::catalog::InstanceStrategy::JoplinAltInstance,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: Some("@joplin/app-desktop".into()),
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };

        for (app, fname) in [(&session, "session.AppImage"), (&joplin, "Joplin.AppImage")] {
            let cache = paths.catalog_cache_dir(&app.id);
            fs::create_dir_all(&cache).unwrap();
            let cached = cache.join(fname);
            fs::write(&cached, b"fake").unwrap();
            for n in [1u32, 2u32] {
                install_instance_from_cache(&paths, app, n, &cached, true).unwrap();
            }
            let wm1 = instance_startup_wm_class(&app.id, 1);
            let wm2 = instance_startup_wm_class(&app.id, 2);
            assert_ne!(wm1, wm2);
            let raw1 = fs::read_to_string(
                paths
                    .applications_dir()
                    .join(desktop_filename_for_instance(&app.id, 1)),
            )
            .unwrap();
            let raw2 = fs::read_to_string(
                paths
                    .applications_dir()
                    .join(desktop_filename_for_instance(&app.id, 2)),
            )
            .unwrap();
            assert!(
                raw1.contains(&format!("StartupWMClass={}", wm1)),
                "desktop1 missing StartupWMClass={}: {}",
                wm1,
                raw1
            );
            assert!(
                raw2.contains(&format!("StartupWMClass={}", wm2)),
                "desktop2 missing StartupWMClass={}: {}",
                wm2,
                raw2
            );
            let launch1 = paths
                .instance_dir_for(app, 1)
                .join("appimage-control-launch.sh");
            let launch2 = paths
                .instance_dir_for(app, 2)
                .join("appimage-control-launch.sh");
            let sh1 = fs::read_to_string(&launch1).unwrap();
            let sh2 = fs::read_to_string(&launch2).unwrap();
            if app.id == "session" {
                assert!(
                    sh1.contains(&format!("--class={}", wm1)),
                    "launcher1 missing --class={}: {}",
                    wm1,
                    sh1
                );
                assert!(
                    sh2.contains(&format!("--class={}", wm2)),
                    "launcher2 missing --class={}: {}",
                    wm2,
                    sh2
                );
                assert!(
                    sh1.contains("export GDK_BACKEND=x11")
                        && sh1.contains("export ELECTRON_OZONE_PLATFORM_HINT=x11"),
                    "Session launcher missing X11 hints: {}",
                    sh1
                );
                assert!(
                    sh1.contains("unset WAYLAND_DISPLAY"),
                    "Session launcher should unset WAYLAND_DISPLAY to force X11: {}",
                    sh1
                );
                assert!(
                    sh1.contains("EXTRA_ARGS=")
                        && sh1.contains("--no-sandbox")
                        && sh1.contains("--ozone-platform=x11")
                        && sh1.contains(&format!("--class={}", wm1)),
                    "Session launcher missing ozone/X11/class args: {}",
                    sh1
                );
                assert!(
                    !raw1.contains("StartupWMClass=Session\n"),
                    "must not use flat catalog wayland_app_id: {}",
                    raw1
                );
                assert!(
                    raw1.contains("Icon=/")
                        && raw1.contains("256x256")
                        && raw1.contains("session.i1.png"),
                    "Session Icon= should be absolute path to 256px PNG:\n{}",
                    raw1
                );
                assert!(
                    raw2.contains("Icon=/")
                        && raw2.contains("256x256")
                        && raw2.contains("session.i2.png"),
                    "Session Icon= should be absolute path to 256px PNG:\n{}",
                    raw2
                );
                let png1 = paths
                    .user_hicolor_apps_dir(256)
                    .join(format!("{}.png", wm1));
                assert!(
                    png1.is_file(),
                    "expected hicolor PNG for Icon={}: {}",
                    wm1,
                    png1.display()
                );
            } else {
                assert!(
                    !sh1.contains("--class="),
                    "Joplin launcher must not pass Chromium --class: {}",
                    sh1
                );
                assert!(
                    !sh2.contains("--class="),
                    "Joplin launcher must not pass Chromium --class: {}",
                    sh2
                );
                assert!(
                    sh1.contains("export GDK_BACKEND=x11")
                        && sh1.contains("export ELECTRON_OZONE_PLATFORM_HINT=x11"),
                    "Joplin launcher missing X11 hints: {}",
                    sh1
                );
                assert!(
                    sh1.contains("unset WAYLAND_DISPLAY"),
                    "Joplin launcher should unset WAYLAND_DISPLAY to force X11: {}",
                    sh1
                );
                assert!(
                    sh1.contains("EXTRA_ARGS=")
                        && sh1.contains("--no-sandbox")
                        && sh1.contains("--ozone-platform=x11"),
                    "Joplin launcher missing ozone X11 args: {}",
                    sh1
                );
                assert!(
                    !raw1.contains("StartupWMClass=@joplin/app-desktop"),
                    "must not use flat catalog wayland_app_id: {}",
                    raw1
                );
                assert!(
                    raw1.contains("Icon=/")
                        && raw1.contains("256x256")
                        && raw1.contains("joplin.i1.png"),
                    "Joplin Icon= should be absolute path to 256px PNG:\n{}",
                    raw1
                );
                assert!(
                    raw2.contains("Icon=/")
                        && raw2.contains("256x256")
                        && raw2.contains("joplin.i2.png"),
                    "Joplin Icon= should be absolute path to 256px PNG:\n{}",
                    raw2
                );
                let png1 = paths
                    .user_hicolor_apps_dir(256)
                    .join(format!("{}.png", wm1));
                assert!(
                    png1.is_file(),
                    "expected hicolor PNG for Icon={}: {}",
                    wm1,
                    png1.display()
                );
            }
        }
    }

    // --- copy_or_hardlink atomicity tests ---

    /// Happy path: copy_or_hardlink with no pre-existing dst writes correct content.
    #[test]
    fn copy_or_hardlink_happy_path_no_preexisting_dst() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.AppImage");
        let dst = dir.path().join("dst.AppImage");
        fs::write(&src, b"v2-content").unwrap();

        copy_or_hardlink(&src, &dst).unwrap();

        assert_eq!(fs::read(&dst).unwrap(), b"v2-content");
    }

    /// Happy path: copy_or_hardlink replaces pre-existing dst atomically — dst content
    /// reflects the new source, not the old value.
    #[test]
    fn copy_or_hardlink_replaces_preexisting_dst_content() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.AppImage");
        let dst = dir.path().join("dst.AppImage");
        fs::write(&dst, b"old-content").unwrap();
        fs::write(&src, b"new-content").unwrap();

        copy_or_hardlink(&src, &dst).unwrap();

        assert_eq!(
            fs::read(&dst).unwrap(),
            b"new-content",
            "dst must contain the new content after successful copy_or_hardlink"
        );
    }

    /// Failure path: when src does not exist, copy_or_hardlink returns Err AND any
    /// pre-existing dst is left intact (not removed or truncated).
    #[test]
    fn copy_or_hardlink_missing_src_leaves_preexisting_dst_intact() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("nonexistent.AppImage");
        let dst = dir.path().join("dst.AppImage");
        fs::write(&dst, b"original").unwrap();

        let result = copy_or_hardlink(&src, &dst);

        assert!(
            result.is_err(),
            "copy_or_hardlink with missing src must return Err"
        );
        assert!(
            dst.is_file(),
            "pre-existing dst must still exist after failed copy_or_hardlink"
        );
        assert_eq!(
            fs::read(&dst).unwrap(),
            b"original",
            "pre-existing dst must be unmodified after failed copy_or_hardlink"
        );
    }

    /// No temp-file residue: after a successful copy_or_hardlink there must be no
    /// `.*.new.*` sibling left in the destination directory.
    #[test]
    fn copy_or_hardlink_no_temp_residue_after_success() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.AppImage");
        let dst = dir.path().join("dst.AppImage");
        fs::write(&src, b"payload").unwrap();

        copy_or_hardlink(&src, &dst).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".new.")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp-file residue expected after success; found: {:?}",
            leftovers
                .iter()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        );
    }

    fn is_root() -> bool {
        // SAFETY: getuid() is always safe to call; it never fails and has no side-effects.
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        (unsafe { getuid() }) == 0
    }

    /// install_instance_from_cache must roll back the instance directory when
    /// write_instance_desktop_and_icon fails (e.g. applications dir is read-only).
    /// After failure: Err is returned, instance dir is absent, no .desktop residue.
    #[test]
    fn install_instance_from_cache_launcher_failure_rolls_back() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return; // permission-based failure injection needs a non-root uid
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("rollback-app", "rollback.AppImage");

        // Pre-create the cache dir with a fake AppImage.
        let cache = paths.catalog_cache_dir("rollback-app");
        fs::create_dir_all(&cache).unwrap();
        let cached = cache.join("rollback.AppImage");
        fs::write(&cached, b"fake appimage").unwrap();

        // Make the applications dir read-only so the .desktop write fails.
        let apps_dir = paths.applications_dir();
        fs::create_dir_all(&apps_dir).unwrap();
        fs::set_permissions(&apps_dir, fs::Permissions::from_mode(0o555)).unwrap();

        let result = install_instance_from_cache(&paths, &app, 1, &cached, true);

        // Restore permissions so tempdir cleanup can proceed.
        fs::set_permissions(&apps_dir, fs::Permissions::from_mode(0o755)).unwrap();

        // The call must have failed.
        assert!(
            result.is_err(),
            "expected Err when launcher write fails, got Ok"
        );

        // The instance directory must not exist (rolled back).
        let instance_dir = paths.instance_dir_for(&app, 1);
        assert!(
            !instance_dir.exists(),
            "instance dir must be absent after rollback, found: {}",
            instance_dir.display()
        );

        // No .desktop residue.
        let desktop_path = apps_dir.join(desktop_filename_for_instance("rollback-app", 1));
        assert!(
            !desktop_path.exists(),
            ".desktop must not exist after rollback"
        );
    }

    /// reconcile_instance_count does not count a failed install in its `added` list.
    #[test]
    fn reconcile_does_not_count_failed_launcher_install_as_added() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return; // permission-based failure injection needs a non-root uid
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_app("reconcile-fail-app", "rfail.AppImage");

        let cache = paths.catalog_cache_dir("reconcile-fail-app");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("rfail.AppImage"), b"fake appimage").unwrap();

        let apps_dir = paths.applications_dir();
        fs::create_dir_all(&apps_dir).unwrap();
        fs::set_permissions(&apps_dir, fs::Permissions::from_mode(0o555)).unwrap();

        let result = reconcile_instance_count_with_report(&paths, &app, 1, &mut |_| {});

        fs::set_permissions(&apps_dir, fs::Permissions::from_mode(0o755)).unwrap();

        // reconcile must propagate the launcher error.
        assert!(
            result.is_err(),
            "expected reconcile to fail when launcher write fails, got Ok"
        );

        // The instance dir must be absent (no ghost instance).
        let instance_dir = paths.instance_dir_for(&app, 1);
        assert!(
            !instance_dir.exists(),
            "instance dir must be absent after reconcile failure"
        );
    }

    /// No temp-file residue: after a failed copy_or_hardlink (missing src) the
    /// destination directory must contain no `.*.new.*` temp files.
    #[test]
    fn copy_or_hardlink_no_temp_residue_after_failure() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("nonexistent.AppImage");
        let dst = dir.path().join("dst.AppImage");

        let _ = copy_or_hardlink(&src, &dst);

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains(".new.")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "no temp-file residue expected after failure; found: {:?}",
            leftovers
                .iter()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        );
    }
}
