use std::fs;
use std::path::PathBuf;

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::paths::AppPaths;

#[derive(Debug, Clone)]
pub struct ResetPreview {
    pub paths: Vec<PathBuf>,
}

/// Resolve directories that would be deleted for an app id using optional catalog hints.
pub fn preview_reset_paths(
    paths: &AppPaths,
    catalog: Option<&Catalog>,
    app_id: &str,
) -> ResetPreview {
    let mut out = Vec::new();
    if let Some(cat) = catalog {
        if let Some(entry) = cat.apps.iter().find(|a| a.id == app_id) {
            for rel in &entry.reset_config_dirs {
                let p = paths.config_dir().join(rel);
                if p.exists() {
                    out.push(p);
                }
            }
            for rel in &entry.reset_cache_dirs {
                let p = paths.cache_dir().join(rel);
                if p.exists() {
                    out.push(p);
                }
            }
            for rel in &entry.reset_data_dirs {
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

    #[test]
    fn reset_removes_hinted_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let cfg_dir = paths.config_dir().join("feather");
        fs::create_dir_all(&cfg_dir).unwrap();
        fs::write(cfg_dir.join("x"), b"1").unwrap();

        let cat = Catalog {
            apps: vec![crate::catalog::CatalogApp {
                id: "feather".into(),
                display_name: None,
                homepage: None,
                download_url: None,
                download_url_aarch64: None,
                appimage_filename: None,
                instance_strategy: crate::catalog::InstanceStrategy::None,
                instance_exec_args_template: None,
                instance_name_template: None,
                desktop_name_template: None,
                reset_config_dirs: vec!["feather".into()],
                reset_cache_dirs: vec![],
                reset_data_dirs: vec![],
                icon_url: None,
                wayland_app_id: None,
                desktop_pass_file_urls: None,
                instance_subdir: None,
                isolate_dbus_session: None,
                max_instances: None,
                max_instances_tooltip: None,
            }],
        };
        let prev = preview_reset_paths(&paths, Some(&cat), "feather");
        assert_eq!(prev.paths.len(), 1);
        execute_reset(&prev).unwrap();
        assert!(!cfg_dir.exists());
    }
}
