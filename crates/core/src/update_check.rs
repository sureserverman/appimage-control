//! Which installed apps have a newer release. Shared by the GUI's "Update" badge and the
//! `--check-updates` login check. Reads versions only: nothing here downloads or installs.

use std::cmp::Ordering;

use semver::{Prerelease, Version};

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
    /// The newer release, as a plain version (no `v` or `pkg@` prefix).
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

/// `app` as an [`AvailableUpdate`] when it is installed and `latest` is newer than at least one
/// instance's recorded version, so a lagging instance in a mixed fleet counts.
///
/// `None` when nothing is installed, `latest` is unknown, no instance recorded a version (there
/// is nothing to compare, and a login notice that can never clear is worse than none), the
/// install cannot be read, or either side is not a version this can order (`latest`,
/// `nightly`). A doubtful case is never reported.
pub fn update_available_for(
    paths: &AppPaths,
    app: &CatalogApp,
    latest: Option<&str>,
) -> Option<AvailableUpdate> {
    let latest = normalize_github_release_tag(latest?.trim());
    let latest_key = version_key(&latest)?;
    let indices = list_managed_instance_indices(paths, app).ok()?;
    let behind = indices
        .iter()
        .filter_map(|&i| read_instance_recorded_version(paths, app, i))
        .filter_map(|v| version_key(&normalize_github_release_tag(&v)))
        .any(|installed| compare_keys(&latest_key, &installed) == Ordering::Greater);
    if !behind {
        return None;
    }
    let label = installed_version_fleet_display(paths, app).ok()?.label;
    Some(AvailableUpdate {
        app_id: app.id.clone(),
        display_name: app.display_name_resolved(),
        installed: label
            .strip_prefix("installed ")
            .unwrap_or(&label)
            .to_string(),
        latest,
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

/// An orderable version: its numeric parts plus a semver pre-release (empty for a release).
/// Semver (build metadata ignored) or, for tags such as `2024.05` or `2.1`, dot-separated
/// numbers. Anything else is `None`.
fn version_key(s: &str) -> Option<(Vec<u64>, Prerelease)> {
    let s = s.trim();
    if let Ok(v) = Version::parse(s) {
        return Some((vec![v.major, v.minor, v.patch], v.pre));
    }
    let parts = s
        .split('.')
        .map(|p| {
            if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
                p.parse::<u64>().ok()
            } else {
                None
            }
        })
        .collect::<Option<Vec<u64>>>()?;
    Some((parts, Prerelease::EMPTY))
}

/// Compare two [`version_key`]s: numbers first (missing parts count as 0), then a release
/// beats any pre-release of the same numbers.
fn compare_keys(a: &(Vec<u64>, Prerelease), b: &(Vec<u64>, Prerelease)) -> Ordering {
    let n = a.0.len().max(b.0.len());
    let at = |v: &Vec<u64>, i: usize| v.get(i).copied().unwrap_or(0);
    for i in 0..n {
        match at(&a.0, i).cmp(&at(&b.0, i)) {
            Ordering::Equal => {}
            o => return o,
        }
    }
    match (a.1.is_empty(), b.1.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => a.1.cmp(&b.1),
    }
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
    fn update_check_mixed_fleet_counts_a_lagging_instance() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("1.17.17"));
        install(&paths, &a, 2, Some("1.18.0"));
        let u = update_available_for(&paths, &a, Some("1.18.0")).unwrap();
        assert_eq!(u.installed, "1.17.17-1.18.0");
        assert_eq!(update_available_for(&paths, &a, Some("1.17.17")), None);
        // An instance with no recorded version is skipped, not counted as behind.
        install(&paths, &a, 3, None);
        assert_eq!(update_available_for(&paths, &a, Some("1.17.17")), None);
    }

    #[test]
    fn update_check_no_recorded_version_is_not_listed() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, None);
        assert_eq!(update_available_for(&paths, &a, Some("v3.0.0")), None);
    }

    #[test]
    fn update_check_non_semver_versions_compare_by_number() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("2024.05"));
        assert_eq!(update_available_for(&paths, &a, Some("2024.05")), None);
        assert_eq!(update_available_for(&paths, &a, Some("v2024.05")), None);
        assert_eq!(update_available_for(&paths, &a, Some("2024.04")), None);
        assert_eq!(
            update_available_for(&paths, &a, Some("2024.06"))
                .unwrap()
                .latest,
            "2024.06"
        );
        // A two-part tag is not mistaken for part of a longer version.
        let (_t2, paths2) = home();
        install(&paths2, &a, 1, Some("1.2.1"));
        assert!(update_available_for(&paths2, &a, Some("2.1")).is_some());
        assert_eq!(update_available_for(&paths2, &a, Some("1.2")), None);
        // A downgrade is never an update.
        let (_t3, paths3) = home();
        install(&paths3, &a, 1, Some("2.0.0"));
        assert_eq!(update_available_for(&paths3, &a, Some("1.9")), None);
        // Tags that are not versions are never reported.
        for tag in ["latest", "nightly", "v2.x", "2.0.0-"] {
            assert_eq!(update_available_for(&paths3, &a, Some(tag)), None, "{tag}");
        }
    }

    #[test]
    fn update_check_scoped_tags_build_metadata_and_prereleases() {
        let (_t, paths) = home();
        let a = app("demo", GH);
        install(&paths, &a, 1, Some("3.201.21"));
        assert_eq!(
            update_available_for(&paths, &a, Some("@standardnotes/desktop@3.201.21")),
            None
        );
        let u = update_available_for(&paths, &a, Some("@standardnotes/desktop@3.201.22")).unwrap();
        assert_eq!(u.latest, "3.201.22");
        assert_eq!(
            update_available_for(&paths, &a, Some("3.201.21+build5")),
            None
        );
        assert_eq!(
            update_available_for(&paths, &a, Some("3.201.22-rc.1"))
                .unwrap()
                .latest,
            "3.201.22-rc.1"
        );
        let (_t2, paths2) = home();
        install(&paths2, &a, 1, Some("3.0.0-rc.1"));
        assert!(update_available_for(&paths2, &a, Some("3.0.0")).is_some());
        assert_eq!(
            update_available_for(&paths2, &a, Some("3.0.0-beta.1")),
            None
        );
    }

    #[test]
    fn update_check_pinned_non_github_app() {
        let (_t, paths) = home();
        let p = app("p", "https://example.org/dl/p-2.0.0-x86_64.AppImage");
        install(&paths, &p, 1, Some("1.0.0"));
        let latest = latest_known_version(&p, None);
        let u = update_available_for(&paths, &p, latest.as_deref()).unwrap();
        assert_eq!(
            (u.installed.as_str(), u.latest.as_str()),
            ("1.0.0", "2.0.0")
        );
        let (_t2, paths2) = home();
        install(&paths2, &p, 1, Some("2.0.0"));
        assert_eq!(update_available_for(&paths2, &p, latest.as_deref()), None);
        let (_t3, paths3) = home();
        install(&paths3, &p, 1, None);
        assert_eq!(update_available_for(&paths3, &p, latest.as_deref()), None);
    }

    #[test]
    fn update_check_latest_known_versions_keep_catalog_order() {
        let catalog = Catalog {
            apps: vec![
                app("g", GH),
                app("p", "https://example.org/dl/p-2.0.0-x86_64.AppImage"),
                app("h", GH),
            ],
        };
        let got = latest_known_versions(
            &catalog,
            &[Some("v1.0.0".into()), Some("9.9.9".into()), None],
        );
        assert_eq!(
            got,
            vec![Some("v1.0.0".to_string()), Some("2.0.0".to_string()), None]
        );
        // A short tag list leaves the rest unknown instead of shifting.
        let got = latest_known_versions(&catalog, &[]);
        assert_eq!(got, vec![None, Some("2.0.0".to_string()), None]);
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
