//! Everything the per-app detail page shows, read from disk in one place: each instance's
//! folder, AppImage, launcher, recorded version and size, where updates come from, and how the
//! cached download was verified. Reads only; never downloads or changes anything.

use std::fs;
use std::path::PathBuf;

use crate::catalog::CatalogApp;
use crate::desktop::desktop_filename_for_instance;
use crate::instances::{
    instance_appimage_path, list_managed_instance_indices, read_instance_recorded_version,
};
use crate::integrity::{catalog_pin, read_cache_sidecar, CacheSidecar};
use crate::paths::AppPaths;
use crate::version_info::github_owner_repo_from_url;

/// One installed instance as found on disk. A missing piece is marked, never dropped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceDetails {
    pub index: u32,
    /// `apps/<dir>/<n>/`.
    pub dir: PathBuf,
    pub dir_present: bool,
    /// The catalog-named AppImage inside `dir`.
    pub appimage: PathBuf,
    pub appimage_present: bool,
    /// The instance's `.desktop` launcher in the user applications dir.
    pub launcher: PathBuf,
    pub launcher_present: bool,
    /// `.appimage-control-version`, when recorded.
    pub version: Option<String>,
    /// AppImage size in bytes, when the file is there.
    pub size: Option<u64>,
}

/// Where a newer release of this app is found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateSource {
    /// A GitHub release asset: newer releases are read from the repo's releases.
    GithubReleases { owner: String, repo: String },
    /// A fixed URL whose sha256 the catalog publishes.
    PinnedUrl { url: String },
    /// A fixed URL with no published sha256.
    UnpinnedUrl { url: String },
    /// The catalog has no download for this system.
    NoDownload,
}

/// How the cached download was accepted, from the cache sidecar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verification {
    /// Matched the sha256 in the catalog.
    CatalogHash,
    /// Matched the sha256 GitHub publishes for the release asset.
    GithubDigest,
    /// No published sha256; installed because the user allowed it.
    UnverifiedAllowed,
    /// The sidecar exists but cannot be read or names an unknown label.
    Unreadable,
    /// No sidecar: nothing cached yet, or cached before verification existed.
    NotChecked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDetails {
    pub app_id: String,
    pub display_name: String,
    pub instances: Vec<InstanceDetails>,
    pub source: UpdateSource,
    /// How the download in the cache was accepted. This is the cache's state: an instance
    /// copied from an earlier download may have been accepted differently.
    pub cache_verification: Verification,
    /// Set when the install folders could not be read; `instances` is then incomplete and must
    /// not be shown as "nothing installed".
    pub listing_error: Option<String>,
}

/// Read `app`'s details from disk. An app with no instances has an empty list, not an error; a
/// folder that cannot be read sets [`AppDetails::listing_error`].
pub fn app_details(paths: &AppPaths, app: &CatalogApp) -> AppDetails {
    let filename = app.resolved_appimage_filename();
    let (indices, listing_error) = match list_managed_instance_indices(paths, app) {
        Ok(v) => (v, None),
        Err(e) => (Vec::new(), Some(e.to_string())),
    };
    let instances = indices
        .into_iter()
        .map(|index| {
            let dir = paths.instance_dir_for(app, index);
            let appimage =
                instance_appimage_path(&dir, &filename).unwrap_or_else(|| dir.join(&filename));
            let launcher = paths
                .applications_dir()
                .join(desktop_filename_for_instance(&app.id, index));
            let size = fs::metadata(&appimage)
                .ok()
                .filter(|m| m.is_file())
                .map(|m| m.len());
            InstanceDetails {
                index,
                dir_present: dir.is_dir(),
                appimage_present: appimage.is_file(),
                launcher_present: launcher.is_file(),
                version: read_instance_recorded_version(paths, app, index),
                size,
                dir,
                appimage,
                launcher,
            }
        })
        .collect();
    AppDetails {
        app_id: app.id.clone(),
        display_name: app.display_name_resolved(),
        instances,
        source: update_source(app, std::env::consts::ARCH),
        cache_verification: verification_from_sidecar(&read_cache_sidecar(
            &paths.catalog_cache_dir(&app.id),
        )),
        listing_error,
    }
}

/// Where `app`'s updates come from on `arch`.
pub fn update_source(app: &CatalogApp, arch: &str) -> UpdateSource {
    let Some(url) = app.download_url_for_arch(arch) else {
        return UpdateSource::NoDownload;
    };
    if let Some((owner, repo)) = github_owner_repo_from_url(url) {
        return UpdateSource::GithubReleases { owner, repo };
    }
    if catalog_pin(app, arch).is_some() {
        UpdateSource::PinnedUrl {
            url: url.to_string(),
        }
    } else {
        UpdateSource::UnpinnedUrl {
            url: url.to_string(),
        }
    }
}

/// Map the cache sidecar to what the detail page says about verification.
pub fn verification_from_sidecar(sidecar: &CacheSidecar) -> Verification {
    match sidecar {
        CacheSidecar::Absent => Verification::NotChecked,
        CacheSidecar::Corrupt => Verification::Unreadable,
        CacheSidecar::Present { label, .. } => match label.as_str() {
            "verified catalog" => Verification::CatalogHash,
            "verified github-digest" => Verification::GithubDigest,
            "unverified user-allowed" => Verification::UnverifiedAllowed,
            _ => Verification::Unreadable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{parse_catalog_json, InstanceStrategy};
    use crate::instances::install_instance_from_cache;
    use crate::integrity::{write_cache_sidecar, CACHE_SHA256_FILENAME};

    const HEX: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn app(id: &str, url: Option<&str>, sha: Option<&str>) -> CatalogApp {
        CatalogApp {
            id: id.to_string(),
            display_name: Some(format!("{id} Display")),
            homepage: None,
            download_url: url.map(str::to_string),
            download_url_aarch64: url.map(str::to_string),
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
            sha256: sha.map(str::to_string),
            sha256_aarch64: sha.map(str::to_string),
        }
    }

    /// Install `n` fixture instances from a cached blob of `bytes` bytes.
    fn install(paths: &AppPaths, app: &CatalogApp, n: u32, bytes: usize) {
        let cache = paths.catalog_cache_dir(&app.id);
        fs::create_dir_all(&cache).unwrap();
        let cached = cache.join("demo.AppImage");
        fs::write(&cached, vec![b'x'; bytes]).unwrap();
        for i in 1..=n {
            install_instance_from_cache(paths, app, i, &cached, true).unwrap();
        }
    }

    #[test]
    fn app_details_lists_each_instance() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = app("demo", Some("https://example.org/demo.AppImage"), None);
        install(&paths, &a, 2, 1234);
        fs::write(
            paths
                .instance_dir_for(&a, 2)
                .join(crate::version_info::INSTANCE_VERSION_FILENAME),
            b"2.8.1\n",
        )
        .unwrap();
        let d = app_details(&paths, &a);
        assert_eq!(d.app_id, "demo");
        assert_eq!(d.display_name, "demo Display");
        assert_eq!(d.instances.len(), 2);
        let i2 = &d.instances[1];
        assert_eq!(i2.index, 2);
        assert_eq!(i2.dir, paths.instance_dir_for(&a, 2));
        assert_eq!(
            i2.appimage,
            paths.instance_dir_for(&a, 2).join("demo.AppImage")
        );
        assert!(i2.dir_present && i2.appimage_present && i2.launcher_present);
        assert_eq!(i2.version.as_deref(), Some("2.8.1"));
        assert_eq!(i2.size, Some(1234));
        assert_eq!(d.instances[0].index, 1);
    }

    #[test]
    fn app_details_marks_a_missing_appimage() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = app("demo", Some("https://example.org/demo.AppImage"), None);
        install(&paths, &a, 2, 10);
        fs::remove_file(paths.instance_dir_for(&a, 2).join("demo.AppImage")).unwrap();
        let d = app_details(&paths, &a);
        assert_eq!(d.instances.len(), 2, "the broken instance is listed");
        assert!(!d.instances[1].appimage_present);
        assert_eq!(d.instances[1].size, None);
        assert!(d.instances[1].launcher_present);
        assert!(d.instances[0].appimage_present);
    }

    #[test]
    fn app_details_finds_an_appimage_installed_under_an_older_name() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let mut a = app("demo", Some("https://example.org/demo.AppImage"), None);
        install(&paths, &a, 1, 77);
        a.appimage_filename = Some("demo-2.0.0.AppImage".into());
        let d = app_details(&paths, &a);
        let i1 = &d.instances[0];
        assert!(
            i1.appimage_present,
            "the older file is the instance's AppImage"
        );
        assert_eq!(
            i1.appimage,
            paths.instance_dir_for(&a, 1).join("demo.AppImage")
        );
        assert_eq!(i1.size, Some(77));
    }

    #[test]
    fn app_details_unreadable_install_folder_is_reported() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = app("demo", Some("https://example.org/demo.AppImage"), None);
        install(&paths, &a, 1, 10);
        let root = paths.apps_root_for_app(&a);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
        let readable = fs::read_dir(&root).is_ok(); // true when the tests run as root
        let d = app_details(&paths, &a);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(d.listing_error.is_some(), "{d:?}");
        }
        assert_eq!(app_details(&paths, &a).listing_error, None);
    }

    #[test]
    fn app_details_no_instances_is_an_empty_list() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = app("demo", Some("https://example.org/demo.AppImage"), None);
        let d = app_details(&paths, &a);
        assert!(d.instances.is_empty());
        assert_eq!(d.cache_verification, Verification::NotChecked);
    }

    #[test]
    fn app_details_update_source_per_url_kind() {
        let gh = app(
            "g",
            Some("https://github.com/keepassxreboot/keepassxc/releases/download/2.7.9/K.AppImage"),
            None,
        );
        assert_eq!(
            update_source(&gh, "x86_64"),
            UpdateSource::GithubReleases {
                owner: "keepassxreboot".into(),
                repo: "keepassxc".into()
            }
        );
        let pinned = app("p", Some("https://example.org/p-2.0.0.AppImage"), Some(HEX));
        assert_eq!(
            update_source(&pinned, "x86_64"),
            UpdateSource::PinnedUrl {
                url: "https://example.org/p-2.0.0.AppImage".into()
            }
        );
        let unpinned = app("u", Some("https://example.org/u.AppImage"), None);
        assert_eq!(
            update_source(&unpinned, "x86_64"),
            UpdateSource::UnpinnedUrl {
                url: "https://example.org/u.AppImage".into()
            }
        );
        assert_eq!(
            update_source(&app("n", None, None), "x86_64"),
            UpdateSource::NoDownload
        );
        // Another arch takes whichever URL is set, as installs do.
        let mut arm_only = app("r", None, None);
        arm_only.download_url_aarch64 = Some("https://example.org/r.AppImage".into());
        assert_eq!(
            update_source(&arm_only, "riscv64"),
            UpdateSource::UnpinnedUrl {
                url: "https://example.org/r.AppImage".into()
            }
        );
    }

    #[test]
    fn app_details_verification_per_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = app("demo", Some("https://example.org/demo.AppImage"), None);
        let cache = paths.catalog_cache_dir("demo");
        fs::create_dir_all(&cache).unwrap();
        assert_eq!(
            app_details(&paths, &a).cache_verification,
            Verification::NotChecked
        );
        for (label, want) in [
            ("verified catalog", Verification::CatalogHash),
            ("verified github-digest", Verification::GithubDigest),
            ("unverified user-allowed", Verification::UnverifiedAllowed),
        ] {
            write_cache_sidecar(&cache, HEX, label).unwrap();
            assert_eq!(app_details(&paths, &a).cache_verification, want, "{label}");
        }
        fs::write(cache.join(CACHE_SHA256_FILENAME), b"not a sidecar").unwrap();
        assert_eq!(
            app_details(&paths, &a).cache_verification,
            Verification::Unreadable
        );
        // A label nobody writes is not passed off as verified.
        fs::write(
            cache.join(CACHE_SHA256_FILENAME),
            format!("{HEX} verified maybe"),
        )
        .unwrap();
        assert_eq!(
            app_details(&paths, &a).cache_verification,
            Verification::Unreadable
        );
    }

    #[test]
    fn app_details_every_catalog_app() {
        let raw = fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/catalog.json"),
        )
        .unwrap();
        let catalog = parse_catalog_json(&raw).unwrap();
        assert!(!catalog.apps.is_empty());
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        for a in &catalog.apps {
            let d = app_details(&paths, a);
            assert_eq!(d.app_id, a.id);
            assert!(
                d.instances.is_empty(),
                "{}: empty home has no instances",
                a.id
            );
            if std::env::consts::ARCH == "x86_64" {
                assert_ne!(
                    d.source,
                    UpdateSource::NoDownload,
                    "{}: every shipped app has an x86_64 download",
                    a.id
                );
            }
            assert_eq!(d.cache_verification, Verification::NotChecked, "{}", a.id);
        }
    }
}
