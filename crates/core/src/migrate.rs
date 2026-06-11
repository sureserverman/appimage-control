//! Migrate legacy per-app data directories from a working directory into catalog instance portable
//! `*.home` / `*.config` trees under `~/.local/share/appimages/apps/`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::catalog::{Catalog, CatalogApp, InstanceStrategy};
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::instances::{
    ensure_cryptomator_instance_ready, list_managed_instance_indices,
    reconcile_instance_count_for_portable_migration, MAX_INSTANCES_PER_APP,
};
use crate::paths::AppPaths;

/// One directory under the scan root that maps to a catalog app instance.
#[derive(Debug, Clone)]
pub struct MigrationCandidate {
    pub dir_name: String,
    pub source_path: PathBuf,
    pub catalog_app_index: usize,
    pub instance_index: u32,
}

/// User/script choice when the portable `.home` destination already has data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationResolution {
    Replace,
    Renumber,
    Skip,
}

/// Describes a clashing destination `.home` tree before migration runs.
#[derive(Debug, Clone)]
pub struct MigrationConflict {
    pub source_path: PathBuf,
    pub catalog_app_id: String,
    pub requested_instance: u32,
    pub conflicting_home: PathBuf,
}

/// Scan `cwd` for immediate child directories that match catalog [`CatalogApp::instance_filesystem_dirname`]
/// naming (see module docs in the plan: `slug` → instance 1, `slug` + digits → instance `digits`).
pub fn discover_migration_candidates(cwd: &Path, catalog: &Catalog) -> Vec<MigrationCandidate> {
    let mut apps: Vec<(usize, &CatalogApp)> = catalog.apps.iter().enumerate().collect();
    apps.sort_by_key(|(_, a)| std::cmp::Reverse(a.instance_filesystem_dirname().len()));

    let Ok(rd) = fs::read_dir(cwd) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for ent in rd.flatten() {
        let Ok(ft) = ent.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        let name = ent.file_name();
        let Some(d) = name.to_str() else {
            continue;
        };
        if d.starts_with('.') {
            continue;
        }

        let source_path = ent.path();
        for (idx, app) in &apps {
            let slug = app.instance_filesystem_dirname();
            if !d.starts_with(slug.as_str()) {
                continue;
            }
            let rest = &d[slug.len()..];
            let instance_index = if rest.is_empty() {
                if d == slug.as_str() {
                    1
                } else {
                    continue;
                }
            } else if rest.chars().all(|c| c.is_ascii_digit()) {
                let Ok(n) = rest.parse::<u32>() else {
                    continue;
                };
                if !(1..=MAX_INSTANCES_PER_APP).contains(&n) {
                    continue;
                }
                n
            } else {
                continue;
            };

            out.push(MigrationCandidate {
                dir_name: d.to_string(),
                source_path,
                catalog_app_index: *idx,
                instance_index,
            });
            break;
        }
    }

    out.sort_by(|a, b| {
        a.catalog_app_index
            .cmp(&b.catalog_app_index)
            .then_with(|| a.instance_index.cmp(&b.instance_index))
            .then_with(|| a.source_path.cmp(&b.source_path))
    });
    out.dedup_by(|a, b| a.source_path == b.source_path);
    out
}

fn portable_home_path(paths: &AppPaths, app: &CatalogApp, instance_index: u32) -> PathBuf {
    let dir = paths.instance_dir_for(app, instance_index);
    let fname = app.resolved_appimage_filename();
    dir.join(format!("{fname}.home"))
}

fn portable_config_path(paths: &AppPaths, app: &CatalogApp, instance_index: u32) -> PathBuf {
    let dir = paths.instance_dir_for(app, instance_index);
    let fname = app.resolved_appimage_filename();
    dir.join(format!("{fname}.config"))
}

/// Highest instance index under `apps/<id>/<n>/` whose portable `*.home` tree already has data.
/// Counts layouts that [`list_managed_instance_indices`] can miss (e.g. data without a usable AppImage).
fn max_instance_index_nonempty_portable_home(paths: &AppPaths, app: &CatalogApp) -> Result<u32> {
    let root = paths.apps_root_for_app(app);
    if !root.is_dir() {
        return Ok(0);
    }
    let fname = app.resolved_appimage_filename();
    let mut max = 0u32;
    for ent in fs::read_dir(&root)? {
        let ent = ent?;
        let name = ent.file_name();
        let Some(s) = name.to_str() else {
            continue;
        };
        let Ok(n) = s.parse::<u32>() else {
            continue;
        };
        if n == 0 || n > MAX_INSTANCES_PER_APP {
            continue;
        }
        let home = ent.path().join(format!("{fname}.home"));
        if dir_is_nonempty(&home) {
            max = max.max(n);
        }
    }
    Ok(max)
}

fn dir_is_nonempty(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    if path.is_file() {
        return fs::metadata(path).map(|m| m.len() > 0).unwrap_or(true);
    }
    fs::read_dir(path)
        .map(|mut it| it.next().is_some())
        .unwrap_or(true)
}

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

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    if src.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)?;
        return Ok(());
    }
    fs::create_dir_all(dst)?;
    for ent in fs::read_dir(src)? {
        let ent = ent?;
        let fp = ent.path();
        let name = ent.file_name();
        let dest = dst.join(&name);
        if fp.is_dir() {
            copy_tree(&fp, &dest)?;
        } else {
            fs::copy(&fp, &dest)?;
        }
    }
    Ok(())
}

fn try_rename_or_copy_tree(src: &Path, dst: &Path) -> Result<()> {
    match fs::rename(src, dst) {
        Ok(()) => Ok(()),
        Err(e) => {
            let cross = e.raw_os_error() == Some(18); // EXDEV
            if cross {
                move_tree_via_staged_copy(src, dst)
            } else {
                Err(e.into())
            }
        }
    }
}

/// Cross-device move: copy `src` to a staging sibling of `dst`, rename the
/// completed copy into place, and only then delete `src`. A failure at any
/// step leaves `src` untouched and no partial `dst` (or staging residue)
/// behind, so a re-run never mistakes a half-copied tree for a migrated home.
fn move_tree_via_staged_copy(src: &Path, dst: &Path) -> Result<()> {
    let staging = staging_sibling_path(dst)?;
    remove_path_best_effort(&staging);
    if let Err(copy_err) = copy_tree(src, &staging) {
        remove_path_best_effort(&staging);
        return Err(copy_err);
    }
    if let Err(rename_err) = fs::rename(&staging, dst) {
        remove_path_best_effort(&staging);
        return Err(rename_err.into());
    }
    if src.is_dir() {
        fs::remove_dir_all(src)?;
    } else {
        let _ = fs::remove_file(src);
    }
    Ok(())
}

fn staging_sibling_path(dst: &Path) -> Result<PathBuf> {
    let Some(name) = dst.file_name().and_then(|n| n.to_str()) else {
        return Err(Error::InvalidPath(format!(
            "migrate: destination has no usable file name: {}",
            dst.display()
        )));
    };
    Ok(dst.with_file_name(format!(".{name}.aic-staging-{}", std::process::id())))
}

fn remove_path_best_effort(p: &Path) {
    if p.is_dir() {
        let _ = fs::remove_dir_all(p);
    } else {
        let _ = fs::remove_file(p);
    }
}

/// Next 1-based instance index for `[n]ew instance` / `MigrationResolution::Renumber`: the slot
/// **after** the highest occupied index (managed AppImage or nonempty portable `*.home`).
///
/// We do **not** fill gaps (e.g. empty `*.home` on instance 2 while 1 and 3 are in use); that led to
/// migrating into instance 2 when the user already had three instances and expected a fourth.
fn next_instance_index_after_occupied(paths: &AppPaths, app: &CatalogApp) -> Result<Option<u32>> {
    let cap = app.max_instances_allowed().min(MAX_INSTANCES_PER_APP);
    let managed_max = list_managed_instance_indices(paths, app)?
        .into_iter()
        .max()
        .unwrap_or(0);
    let home_data_max = max_instance_index_nonempty_portable_home(paths, app)?;
    let occupied = managed_max.max(home_data_max);
    let next = occupied.saturating_add(1);
    if next > cap || next > MAX_INSTANCES_PER_APP {
        return Ok(None);
    }
    Ok(Some(next))
}

/// Ensures each catalog app has enough instances for the given max target indices, then moves each
/// `source_path` onto `*.home` (and sibling `*.config` in the same source parent if present — not in v1;
/// only the matched directory is moved as the home tree).
///
/// `fixed_on_conflict`: when set, every collision uses this resolution (for `--on-conflict=` scripts).
/// When `abort_on_unresolved_conflict` is true and `fixed_on_conflict` is `None`, the first nonempty
/// destination returns an error instead of calling `resolve_conflict` (non-interactive / `-y` without
/// a fixed strategy).
pub fn run_portable_dir_migration(
    paths: &AppPaths,
    catalog: &Catalog,
    candidates: &[MigrationCandidate],
    fixed_on_conflict: Option<MigrationResolution>,
    abort_on_unresolved_conflict: bool,
    mut resolve_conflict: impl FnMut(MigrationConflict) -> MigrationResolution,
    on_progress: &mut dyn FnMut(&str),
) -> Result<()> {
    if candidates.is_empty() {
        on_progress("No migration directories matched the catalog in this folder.");
        return Ok(());
    }

    let mut max_instance: HashMap<usize, u32> = HashMap::new();
    for c in candidates {
        let e = max_instance.entry(c.catalog_app_index).or_insert(0);
        *e = (*e).max(c.instance_index);
    }

    for (&idx, &candidate_max) in &max_instance {
        let app = &catalog.apps[idx];
        let managed_max = list_managed_instance_indices(paths, app)?
            .into_iter()
            .max()
            .unwrap_or(0);
        let home_data_max = max_instance_index_nonempty_portable_home(paths, app)?;
        let installed_max = managed_max.max(home_data_max);
        let target = candidate_max.max(installed_max);
        on_progress(&format!(
            "Reconciling {} to {} instance(s) (max of migration folder targets and existing installs) …",
            app.id, target
        ));
        reconcile_instance_count_for_portable_migration(paths, app, target, on_progress)?;
    }

    for c in candidates {
        let app = &catalog.apps[c.catalog_app_index];
        let mut target_instance = c.instance_index;
        let mut home_dest = portable_home_path(paths, app, target_instance);

        if dir_is_nonempty(&home_dest) {
            let conflict = MigrationConflict {
                source_path: c.source_path.clone(),
                catalog_app_id: app.id.clone(),
                requested_instance: target_instance,
                conflicting_home: home_dest.clone(),
            };
            let choice = if let Some(r) = fixed_on_conflict {
                r
            } else if abort_on_unresolved_conflict {
                return Err(Error::InvalidPath(format!(
                    "migrate: destination already exists ({}). Re-run with --on-conflict=replace|renumber|skip",
                    home_dest.display()
                )));
            } else {
                resolve_conflict(conflict)
            };
            match choice {
                MigrationResolution::Skip => {
                    on_progress(&format!(
                        "Skipped {} (destination exists: {})",
                        c.dir_name,
                        home_dest.display()
                    ));
                    continue;
                }
                MigrationResolution::Replace => {
                    on_progress(&format!("Replacing existing {} …", home_dest.display()));
                    let inst_dir = paths.instance_dir_for(app, target_instance);
                    best_effort_unmount_portal_doc(&inst_dir, app);
                    if home_dest.exists() {
                        fs::remove_dir_all(&home_dest)?;
                    }
                    let cfg = portable_config_path(paths, app, target_instance);
                    if dir_is_nonempty(&cfg) {
                        best_effort_unmount_portal_doc(&inst_dir, app);
                        fs::remove_dir_all(&cfg)?;
                    }
                }
                MigrationResolution::Renumber => {
                    let Some(new_m) = next_instance_index_after_occupied(paths, app)? else {
                        return Err(Error::InvalidPath(format!(
                            "{}: no instance slot after the occupied range (catalog limit reached)",
                            app.id
                        )));
                    };
                    on_progress(&format!(
                        "Renumbering migration for {} from instance {} to {} …",
                        app.id, target_instance, new_m
                    ));
                    let managed_max = list_managed_instance_indices(paths, app)?
                        .into_iter()
                        .max()
                        .unwrap_or(0);
                    let home_data_max = max_instance_index_nonempty_portable_home(paths, app)?;
                    let installed_max = managed_max.max(home_data_max);
                    let max_needed = max_instance
                        .get(&c.catalog_app_index)
                        .copied()
                        .unwrap_or(0)
                        .max(new_m)
                        .max(installed_max);
                    reconcile_instance_count_for_portable_migration(
                        paths,
                        app,
                        max_needed,
                        on_progress,
                    )?;
                    target_instance = new_m;
                    home_dest = portable_home_path(paths, app, target_instance);
                    if dir_is_nonempty(&home_dest) {
                        return Err(Error::InvalidPath(format!(
                            "internal: home {} still occupied after renumber",
                            home_dest.display()
                        )));
                    }
                }
            }
        }

        on_progress(&format!(
            "Moving {} → {} (instance {}) …",
            c.source_path.display(),
            home_dest.display(),
            target_instance
        ));
        try_rename_or_copy_tree(&c.source_path, &home_dest)?;

        let inst_dir = paths.instance_dir_for(app, target_instance);
        if matches!(app.instance_strategy, InstanceStrategy::CryptomatorExtract) {
            ensure_cryptomator_instance_ready(&inst_dir, app, true)?;
        }
        let cfg_dest = portable_config_path(paths, app, target_instance);
        if !cfg_dest.is_dir() {
            fs::create_dir_all(&cfg_dest)?;
        }
    }

    on_progress("Migration finished.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogApp;
    use crate::catalog::InstanceStrategy;
    use crate::instances::{
        list_instance_indices, reconcile_instance_count,
        reconcile_instance_count_for_portable_migration,
    };

    fn fake_session() -> CatalogApp {
        CatalogApp {
            id: "session".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x".into()),
            download_url_aarch64: None,
            appimage_filename: Some("session.AppImage".into()),
            instance_strategy: InstanceStrategy::ElectronClass,
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

    fn fake_feather() -> CatalogApp {
        CatalogApp {
            id: "feather".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x".into()),
            download_url_aarch64: None,
            appimage_filename: Some("feather.AppImage".into()),
            instance_strategy: InstanceStrategy::QtX11,
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

    fn fake_se_app() -> CatalogApp {
        CatalogApp {
            id: "se".into(),
            display_name: None,
            homepage: None,
            download_url: Some("http://example.invalid/x".into()),
            download_url_aarch64: None,
            appimage_filename: Some("se.AppImage".into()),
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
        }
    }

    #[test]
    fn discover_session_and_session2() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join("session")).unwrap();
        fs::create_dir_all(cwd.join("session2")).unwrap();
        fs::create_dir_all(cwd.join(".hidden")).unwrap();
        fs::create_dir_all(cwd.join("other")).unwrap();

        let cat = Catalog {
            apps: vec![fake_session()],
        };
        let v = discover_migration_candidates(cwd, &cat);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].instance_index, 1);
        assert_eq!(v[1].instance_index, 2);
    }

    #[test]
    fn longest_slug_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        fs::create_dir_all(cwd.join("session")).unwrap();

        let cat = Catalog {
            apps: vec![fake_se_app(), fake_session()],
        };
        let v = discover_migration_candidates(cwd, &cat);
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].catalog_app_index, 1);
        assert_eq!(v[0].instance_index, 1);
    }

    #[test]
    fn migrate_replace_via_resolver() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let paths = AppPaths::new(home).unwrap();
        let app = fake_session();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("session");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 1, &mut |_| {}).unwrap();

        let legacy = tmp.path().join("legacyhome");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("marker.txt"), b"x").unwrap();

        let old_home = portable_home_path(&paths, &app, 1);
        fs::write(old_home.join("old.txt"), b"old").unwrap();

        let c = MigrationCandidate {
            dir_name: "legacyhome".into(),
            source_path: legacy.clone(),
            catalog_app_index: 0,
            instance_index: 1,
        };

        run_portable_dir_migration(
            &paths,
            &cat,
            &[c],
            Some(MigrationResolution::Replace),
            false,
            |_| unreachable!(),
            &mut |_| {},
        )
        .unwrap();

        assert!(portable_home_path(&paths, &app, 1)
            .join("marker.txt")
            .is_file());
        assert!(!legacy.exists());
    }

    #[test]
    fn migrate_aborts_when_unresolved_conflict_and_flag_set() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_session();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("session");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 1, &mut |_| {}).unwrap();

        let h1 = portable_home_path(&paths, &app, 1);
        fs::write(h1.join("occupied.txt"), b"1").unwrap();

        let legacy = tmp.path().join("session");
        fs::create_dir_all(&legacy).unwrap();

        let c = MigrationCandidate {
            dir_name: "session".into(),
            source_path: legacy,
            catalog_app_index: 0,
            instance_index: 1,
        };

        let r = run_portable_dir_migration(
            &paths,
            &cat,
            &[c],
            None,
            true,
            |_| MigrationResolution::Replace,
            &mut |_| {},
        );
        assert!(r.is_err());
    }

    #[test]
    fn migrate_does_not_remove_higher_instances_when_only_lower_slot_migrates() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_feather();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 3, &mut |_| {}).unwrap();
        assert_eq!(list_instance_indices(&paths, &app).unwrap(), vec![1, 2, 3]);

        let cwd = tmp.path().join("wd");
        fs::create_dir_all(&cwd).unwrap();
        let legacy = cwd.join("feather1");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("migrated.txt"), b"1").unwrap();

        let candidates = discover_migration_candidates(&cwd, &cat);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].instance_index, 1);

        run_portable_dir_migration(
            &paths,
            &cat,
            &candidates,
            Some(MigrationResolution::Replace),
            false,
            |_| unreachable!(),
            &mut |_| {},
        )
        .unwrap();

        assert_eq!(list_instance_indices(&paths, &app).unwrap(), vec![1, 2, 3]);
        assert!(portable_home_path(&paths, &app, 1)
            .join("migrated.txt")
            .is_file());
    }

    #[test]
    fn migration_reconcile_does_not_create_portable_home() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_session();
        let cache = paths.catalog_cache_dir("session");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"fake").unwrap();
        reconcile_instance_count_for_portable_migration(&paths, &app, 1, &mut |_| {}).unwrap();
        let home = portable_home_path(&paths, &app, 1);
        assert!(
            !home.exists(),
            "empty *.home must not exist before migrate move or TTY conflict is skipped"
        );
    }

    #[test]
    fn migrate_renumber_uses_next_index_after_fleet_not_gap() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_feather();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("feather");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("feather.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 3, &mut |_| {}).unwrap();
        for i in 1..=3u32 {
            let h = portable_home_path(&paths, &app, i);
            fs::write(h.join("marker.txt"), format!("{i}")).unwrap();
        }

        let cwd = tmp.path().join("wd");
        fs::create_dir_all(&cwd).unwrap();
        let legacy = cwd.join("feather1");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("migrated.txt"), b"m").unwrap();

        let candidates = discover_migration_candidates(&cwd, &cat);
        run_portable_dir_migration(
            &paths,
            &cat,
            &candidates,
            Some(MigrationResolution::Renumber),
            false,
            |_| unreachable!(),
            &mut |_| {},
        )
        .unwrap();

        assert!(
            portable_home_path(&paths, &app, 4)
                .join("migrated.txt")
                .is_file(),
            "Renumber should add a new instance after 1..=3, not fill a gap at 2"
        );
        assert_eq!(
            list_instance_indices(&paths, &app).unwrap(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn migrate_renumber_when_resolver_says_renumber() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_session();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("session");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 1, &mut |_| {}).unwrap();

        let h1 = portable_home_path(&paths, &app, 1);
        fs::write(h1.join("occupied.txt"), b"1").unwrap();

        let legacy = tmp.path().join("session");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("new.txt"), b"n").unwrap();

        let c = MigrationCandidate {
            dir_name: "session".into(),
            source_path: legacy.clone(),
            catalog_app_index: 0,
            instance_index: 1,
        };

        run_portable_dir_migration(
            &paths,
            &cat,
            &[c],
            Some(MigrationResolution::Renumber),
            false,
            |_| unreachable!(),
            &mut |_| {},
        )
        .unwrap();

        let h2 = portable_home_path(&paths, &app, 2);
        assert!(h2.join("new.txt").is_file());
        assert!(h1.join("occupied.txt").is_file());
        assert!(!legacy.exists());
    }

    fn is_root() -> bool {
        unsafe extern "C" {
            fn getuid() -> u32;
        }
        (unsafe { getuid() }) == 0
    }

    #[test]
    fn migration_move_failure_leaves_source_intact() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return; // permission-based failure injection needs a non-root uid
        }
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let app = fake_session();
        let cat = Catalog {
            apps: vec![app.clone()],
        };

        let cache = paths.catalog_cache_dir("session");
        fs::create_dir_all(&cache).unwrap();
        fs::write(cache.join("session.AppImage"), b"fake").unwrap();
        reconcile_instance_count(&paths, &app, 1, &mut |_| {}).unwrap();

        let legacy = tmp.path().join("session");
        fs::create_dir_all(&legacy).unwrap();
        fs::write(legacy.join("precious.txt"), b"keep me").unwrap();

        // Make the instance dir read-only so the rename into *.home fails.
        let inst_dir = paths.instance_dir_for(&app, 1);
        let home_dest = portable_home_path(&paths, &app, 1);
        if home_dest.exists() {
            fs::remove_dir_all(&home_dest).unwrap();
        }
        let mut ro = fs::metadata(&inst_dir).unwrap().permissions();
        ro.set_mode(0o555);
        fs::set_permissions(&inst_dir, ro).unwrap();

        let c = MigrationCandidate {
            dir_name: "session".into(),
            source_path: legacy.clone(),
            catalog_app_index: 0,
            instance_index: 1,
        };
        let r = run_portable_dir_migration(
            &paths,
            &cat,
            &[c],
            Some(MigrationResolution::Replace),
            false,
            |_| unreachable!(),
            &mut |_| {},
        );

        let mut rw = fs::metadata(&inst_dir).unwrap().permissions();
        rw.set_mode(0o755);
        fs::set_permissions(&inst_dir, rw).unwrap();

        assert!(r.is_err(), "move into read-only instance dir must fail");
        assert!(
            legacy.join("precious.txt").is_file(),
            "failed migration must leave the source dir intact"
        );
    }

    #[test]
    fn staged_copy_failure_keeps_source_no_partial_dest() {
        use std::os::unix::fs::PermissionsExt;
        if is_root() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/data.txt"), b"payload").unwrap();

        let dest_parent = tmp.path().join("dest_parent");
        fs::create_dir_all(&dest_parent).unwrap();
        let mut ro = fs::metadata(&dest_parent).unwrap().permissions();
        ro.set_mode(0o555);
        fs::set_permissions(&dest_parent, ro).unwrap();

        let dst = dest_parent.join("moved.home");
        let r = move_tree_via_staged_copy(&src, &dst);

        let mut rw = fs::metadata(&dest_parent).unwrap().permissions();
        rw.set_mode(0o755);
        fs::set_permissions(&dest_parent, rw).unwrap();

        assert!(r.is_err(), "staged copy into read-only parent must fail");
        assert!(src.join("sub/data.txt").is_file(), "source must survive");
        assert!(!dst.exists(), "no partial destination may remain");
        let residue: Vec<_> = fs::read_dir(&dest_parent).unwrap().collect();
        assert!(residue.is_empty(), "no staging residue may remain");
    }

    #[test]
    fn staged_copy_success_moves_tree_and_removes_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("sub/data.txt"), b"payload").unwrap();
        let dst = tmp.path().join("moved.home");

        move_tree_via_staged_copy(&src, &dst).unwrap();

        assert_eq!(fs::read(dst.join("sub/data.txt")).unwrap(), b"payload");
        assert!(!src.exists(), "source removed after completed move");
        let residue: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("aic-staging"))
            .collect();
        assert!(residue.is_empty(), "no staging residue after success");
    }
}
