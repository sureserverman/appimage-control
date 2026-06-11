use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::paths::AppPaths;

#[derive(Debug, Clone)]
pub struct ResetPreview {
    pub paths: Vec<PathBuf>,
}

/// Returns `true` when a catalog-supplied relative path segment is safe to join onto a base
/// directory.
///
/// A path is rejected when it:
/// - Is absolute (starts with `/`), or
/// - Contains any `..` component (path traversal escape).
///
/// Empty strings pass through and produce the base directory itself (`.exists()` will be false
/// in practice because the base dirs only exist when apps are installed, not for empty strings).
fn is_safe_reset_rel(rel: &str) -> bool {
    let p = Path::new(rel);
    // Reject absolute paths outright.
    if p.is_absolute() {
        return false;
    }
    // Reject any component that is `..`.
    for comp in p.components() {
        if comp == Component::ParentDir {
            return false;
        }
    }
    true
}

/// Resolve directories that would be deleted for an app id using optional catalog hints.
///
/// Catalog-supplied relative path segments are validated with [`is_safe_reset_rel`]; any segment
/// containing `..` or an absolute path is skipped with a warning to stderr.
pub fn preview_reset_paths(
    paths: &AppPaths,
    catalog: Option<&Catalog>,
    app_id: &str,
) -> ResetPreview {
    let mut out = Vec::new();
    if let Some(cat) = catalog {
        if let Some(entry) = cat.apps.iter().find(|a| a.id == app_id) {
            for rel in &entry.reset_config_dirs {
                if !is_safe_reset_rel(rel) {
                    eprintln!(
                        "appimage-control: warning: reset_config_dirs entry {:?} for app '{}' contains '..' or is absolute; skipping",
                        rel, app_id
                    );
                    continue;
                }
                let p = paths.config_dir().join(rel);
                if p.exists() {
                    out.push(p);
                }
            }
            for rel in &entry.reset_cache_dirs {
                if !is_safe_reset_rel(rel) {
                    eprintln!(
                        "appimage-control: warning: reset_cache_dirs entry {:?} for app '{}' contains '..' or is absolute; skipping",
                        rel, app_id
                    );
                    continue;
                }
                let p = paths.cache_dir().join(rel);
                if p.exists() {
                    out.push(p);
                }
            }
            for rel in &entry.reset_data_dirs {
                if !is_safe_reset_rel(rel) {
                    eprintln!(
                        "appimage-control: warning: reset_data_dirs entry {:?} for app '{}' contains '..' or is absolute; skipping",
                        rel, app_id
                    );
                    continue;
                }
                let p = paths.local_share_dir().join(rel);
                if p.exists() {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out.dedup();
    ResetPreview { paths: out }
}

/// Remove files/directories from the preview.
pub fn execute_reset(preview: &ResetPreview) -> Result<()> {
    if preview.paths.is_empty() {
        return Err(Error::InvalidPath(
            "no paths matched catalog hints for this app".into(),
        ));
    }
    delete_paths(&preview.paths)
}

/// Delete paths chosen explicitly by the user (folders or files).
pub fn delete_paths(paths: &[PathBuf]) -> Result<()> {
    for p in paths {
        if !p.exists() {
            continue;
        }
        if p.is_dir() {
            fs::remove_dir_all(p)?;
        } else {
            fs::remove_file(p)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;

    fn make_catalog_app_with_reset(
        id: &str,
        config_dirs: Vec<&str>,
        cache_dirs: Vec<&str>,
        data_dirs: Vec<&str>,
    ) -> crate::catalog::CatalogApp {
        crate::catalog::CatalogApp {
            id: id.into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: config_dirs.into_iter().map(String::from).collect(),
            reset_cache_dirs: cache_dirs.into_iter().map(String::from).collect(),
            reset_data_dirs: data_dirs.into_iter().map(String::from).collect(),
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
    fn reset_removes_hinted_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let cfg_dir = paths.config_dir().join("feather");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(cfg_dir.join("x"), b"1").unwrap();

        let cat = Catalog {
            apps: vec![make_catalog_app_with_reset(
                "feather",
                vec!["feather"],
                vec![],
                vec![],
            )],
        };
        let prev = preview_reset_paths(&paths, Some(&cat), "feather");
        assert_eq!(prev.paths.len(), 1);
        execute_reset(&prev).unwrap();
        assert!(!cfg_dir.exists());
    }

    /// A catalog app with `reset_config_dirs: ["../escape"]` must NOT produce any path in the
    /// preview — the `..` component must be rejected.
    #[test]
    fn reset_rejects_dotdot_escape_in_config_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();

        // Plant a file at the would-be escape destination so the test is not vacuous.
        let escape_target = paths.config_dir().join("../escape");
        // Normalise so we can create it if the parent exists.
        if let Some(parent) = escape_target.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&escape_target, b"secret");

        let cat = Catalog {
            apps: vec![make_catalog_app_with_reset(
                "evil-app",
                vec!["../escape"],
                vec![],
                vec![],
            )],
        };
        let prev = preview_reset_paths(&paths, Some(&cat), "evil-app");
        // The escape path must not appear in the preview.
        assert!(
            prev.paths.is_empty(),
            "expected no paths in preview for dotdot escape, got: {:?}",
            prev.paths
        );
    }

    /// An absolute path in `reset_config_dirs` must also be rejected.
    #[test]
    fn reset_rejects_absolute_path_in_config_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();

        let cat = Catalog {
            apps: vec![make_catalog_app_with_reset(
                "evil-abs",
                vec!["/etc/passwd"],
                vec![],
                vec![],
            )],
        };
        let prev = preview_reset_paths(&paths, Some(&cat), "evil-abs");
        assert!(
            prev.paths.is_empty(),
            "expected no paths for absolute reset dir, got: {:?}",
            prev.paths
        );
    }
}
