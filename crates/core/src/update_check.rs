//! Which installed apps have a newer release. Shared by the GUI's "Update" badge and the
//! `--check-updates` login check. Reads versions only: nothing here downloads or installs.

use semver::Version;

use crate::catalog::{Catalog, CatalogApp};
use crate::instances::{
    installed_version_fleet_display, list_managed_instance_indices, read_instance_recorded_version,
};
use crate::paths::AppPaths;
use crate::version_info::{
    catalog_pinned_version, github_owner_repo_from_url, normalize_github_release_tag,
};

/// One installed app whose newer release is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub app_id: String,
    pub display_name: String,
    /// What is installed, as the card shows it (`2.8.1`, `1.0.0-2.0.0`, `unknown`).
    pub installed: String,
    /// The newer release, without a leading `v`.
    pub latest: String,
}

/// The newest release known for `app` without a further lookup: the GitHub `releases/latest`
/// tag for an app downloaded from GitHub (`None` when that lookup failed), else the version in
/// the catalog's own download URL.
pub fn latest_known_version(app: &CatalogApp, github_tag: Option<&str>) -> Option<String> {
    let url = app.resolved_download_url().ok()?;
    if github_owner_repo_from_url(url).is_some() {
        github_tag
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    } else {
        catalog_pinned_version(app)
    }
}

/// For each catalog app (same order as `catalog.apps`), [`latest_known_version`] given the
/// GitHub tags from [`crate::fetch_remote_latest_versions`].
pub fn latest_known_versions(
    catalog: &Catalog,
    github_tags: &[Option<String>],
) -> Vec<Option<String>> {
    catalog
        .apps
        .iter()
        .enumerate()
        .map(|(i, app)| latest_known_version(app, github_tags.get(i).and_then(|t| t.as_deref())))
        .collect()
}

/// `app` as an [`AvailableUpdate`] when it is installed and `latest` is newer than what is
/// installed. `None` for an app with no instances or no known latest version.
pub fn update_available_for(
    paths: &AppPaths,
    app: &CatalogApp,
    latest: Option<&str>,
) -> Option<AvailableUpdate> {
    let latest = latest.map(str::trim).filter(|l| !l.is_empty())?;
    let indices = list_managed_instance_indices(paths, app).ok()?;
    if indices.is_empty() {
        return None;
    }
    let recorded: Vec<String> = indices
        .iter()
        .filter_map(|&i| read_instance_recorded_version(paths, app, i))
        .collect();
    let label = installed_version_fleet_display(paths, app).ok()?.label;
    let all_recorded = recorded.len() == indices.len();
    if !remote_is_newer(latest, &recorded, all_recorded, &label) {
        return None;
    }
    Some(AvailableUpdate {
        app_id: app.id.clone(),
        display_name: app.display_name_resolved(),
        installed: label
            .strip_prefix("installed ")
            .unwrap_or(&label)
            .to_string(),
        latest: latest.trim_start_matches('v').to_string(),
    })
}

/// Every installed app with a newer release. `latest[i]` belongs to `catalog.apps[i]`; an app
/// whose entry is `None` (offline, rate-limited, unknown) is left out, never reported.
pub fn updates_available(
    paths: &AppPaths,
    catalog: &Catalog,
    latest: &[Option<String>],
) -> Vec<AvailableUpdate> {
    catalog
        .apps
        .iter()
        .enumerate()
        .filter_map(|(i, app)| {
            update_available_for(paths, app, latest.get(i).and_then(|l| l.as_deref()))
        })
        .collect()
}

/// When every instance records a semver version and `remote` is semver: newer than the highest
/// installed one. Otherwise the older best-effort rule (see [`label_differs`]).
fn remote_is_newer(remote: &str, recorded: &[String], all_recorded: bool, label: &str) -> bool {
    let parse = |s: &str| Version::parse(&normalize_github_release_tag(s)).ok();
    if all_recorded && !recorded.is_empty() {
        if let (Some(r), Some(installed)) = (
            parse(remote),
            recorded
                .iter()
                .map(|s| parse(s))
                .collect::<Option<Vec<_>>>(),
        ) {
            return installed.iter().all(|v| r > *v);
        }
    }
    label_differs(remote, label)
}

/// Best-effort rule for tags that are not semver: the remote tag counts as newer unless the
/// installed label already contains it. Equal strings are never an update.
fn label_differs(remote: &str, installed_label: &str) -> bool {
    let r = remote.trim().trim_start_matches('v');
    let i = installed_label.trim();
    !r.is_empty() && !i.is_empty() && r != i && !i.contains(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{parse_catalog_json, InstanceStrategy};
    use std::fs;

    fn app(id: &str, url: &str) -> CatalogApp {
        CatalogApp {
            id: id.to_string(),
            display_name: Some(format!("{id} Display")),
            homepage: None,
            download_url: Some(url.into()),
            download_url_aarch64: Some(url.into()),
            appimage_filename: Some("demo.AppImage".into()),
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
            sha256: None,
            sha256_aarch64: None,
        }
    }

    /// A fixture instance: the AppImage file plus, when given, its recorded version.
    fn install(paths: &AppPaths, app: &CatalogApp, index: u32, version: Option<&str>) {
        let dir = paths.instance_dir_for(app, index);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(app.resolved_appimage_filename()), b"x").unwrap();
        if let Some(v) = version {
            fs::write(
                dir.join(crate::version_info::INSTANCE_VERSION_FILENAME),
                format!("{v}\n"),
            )
            .unwrap();
        }
    }

    fn home() -> (tempfile::TempDir, AppPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        (tmp, paths)
    }

    const GH: &str = "https://github.com/o/r/releases/download/v2.8.1/demo.AppImage";

    #[test]
    fn update_check_newer_remote_is_listed() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("2.8.1"));
        let u = update_available_for(&paths, &a, Some("v2.9.0")).unwrap();
        assert_eq!(u.app_id, "demo");
        assert_eq!(u.display_name, "demo Display");
        assert_eq!(u.installed, "2.8.1");
        assert_eq!(u.latest, "2.9.0");
    }

    #[test]
    fn update_check_equal_older_or_unknown_remote_is_not_listed() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("2.8.1"));
        assert_eq!(update_available_for(&paths, &a, Some("v2.8.1")), None);
        assert_eq!(update_available_for(&paths, &a, Some("2.8.0")), None);
        assert_eq!(
            update_available_for(&paths, &a, None),
            None,
            "failed lookup"
        );
        assert_eq!(update_available_for(&paths, &a, Some("  ")), None);
    }

    #[test]
    fn update_check_uninstalled_app_is_not_listed() {
        let (_t, paths) = home();
        assert_eq!(
            update_available_for(&paths, &app("demo", GH), Some("9.0.0")),
            None
        );
    }

    #[test]
    fn update_check_mixed_fleet_compares_with_the_newest_instance() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("1.17.17"));
        install(&paths, &a, 2, Some("1.18.0"));
        assert_eq!(update_available_for(&paths, &a, Some("1.18.0")), None);
        let u = update_available_for(&paths, &a, Some("1.18.1")).unwrap();
        assert_eq!(u.installed, "1.17.17-1.18.0");
    }

    /// The cases the GUI's old `version_is_newer` handled, for tags that are not semver.
    #[test]
    fn update_check_non_semver_tags_use_the_label_rule() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("2024.05"));
        assert_eq!(update_available_for(&paths, &a, Some("2024.05")), None);
        assert_eq!(update_available_for(&paths, &a, Some("v2024.05")), None);
        assert!(update_available_for(&paths, &a, Some("2024.06")).is_some());
        // No recorded version: the label is "installed unknown", so any tag differs.
        let (_t2, paths2) = home();
        install(&paths2, &a, 1, None);
        let u = update_available_for(&paths2, &a, Some("v3.0.0")).unwrap();
        assert_eq!(u.installed, "unknown");
        assert!(!label_differs("", "installed 1.0"));
        assert!(!label_differs("v1.0", "installed 1.0"));
        assert!(label_differs("1.1", "installed 1.0"));
    }

    #[test]
    fn update_check_latest_known_version_per_source() {
        let gh = app("g", GH);
        assert_eq!(
            latest_known_version(&gh, Some("v3.0.0")).as_deref(),
            Some("v3.0.0")
        );
        assert_eq!(
            latest_known_version(&gh, None),
            None,
            "failed lookup stays unknown"
        );
        let pinned = app("p", "https://example.org/dl/p-2.0.0-x86_64.AppImage");
        assert_eq!(
            latest_known_version(&pinned, None).as_deref(),
            Some("2.0.0")
        );
        assert_eq!(
            latest_known_version(&pinned, Some("9.9.9")).as_deref(),
            Some("2.0.0"),
            "a non-GitHub app ignores GitHub tags"
        );
    }

    #[test]
    fn update_check_every_catalog_app() {
        let raw = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/catalog.json"),
        )
        .unwrap();
        let catalog = parse_catalog_json(&raw).unwrap();
        assert!(catalog.apps.len() > 1);
        let (_t, paths) = home();
        let mut remote = Vec::new();
        let mut want = Vec::new();
        for (i, a) in catalog.apps.iter().enumerate() {
            install(&paths, a, 1, Some("1.0.0"));
            if i % 2 == 0 {
                remote.push(Some("v2.0.0".to_string()));
                want.push(a.id.clone());
            } else if i % 4 == 1 {
                remote.push(Some("1.0.0".to_string()));
            } else {
                remote.push(None);
            }
        }
        let got: Vec<String> = updates_available(&paths, &catalog, &remote)
            .into_iter()
            .map(|u| u.app_id)
            .collect();
        assert_eq!(got, want);
    }
}
