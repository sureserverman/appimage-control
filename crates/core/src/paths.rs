use std::path::{Component, Path, PathBuf};

use crate::catalog::CatalogApp;
use crate::desktop::DESKTOP_PREFIX;
use crate::error::{Error, Result};

/// `true` when `rel` is a relative path that stays under whatever directory it is joined to:
/// not absolute, no `..` component, and at least one normal component (so `""` and `.` do not
/// name the base directory itself). Catalog reset hints and seed-pack `config_path` both use
/// this one rule.
pub(crate) fn is_safe_relative_path(rel: &str) -> bool {
    let p = Path::new(rel);
    if p.is_absolute() {
        return false;
    }
    let mut has_normal = false;
    for comp in p.components() {
        match comp {
            Component::Normal(_) => has_normal = true,
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    has_normal
}

/// Filesystem layout for managed AppImages (override `home` in tests).
#[derive(Debug, Clone)]
pub struct AppPaths {
    home: PathBuf,
}

impl AppPaths {
    /// User profile root (e.g. `$HOME`).
    pub fn new(home: PathBuf) -> Result<Self> {
        if home.as_os_str().is_empty() {
            return Err(Error::InvalidPath("empty home".into()));
        }
        Ok(Self { home })
    }

    pub fn from_env() -> Result<Self> {
        let home = dirs::home_dir().ok_or(Error::NoHomeDir)?;
        Self::new(home)
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Root for all manager-owned data: `~/.local/share/appimages`.
    pub fn manager_data_dir(&self) -> PathBuf {
        self.home.join(".local/share/appimages")
    }

    /// Standalone AppImage copies from `install_appimage` (not catalog instance dirs).
    pub fn managed_files_dir(&self) -> PathBuf {
        self.manager_data_dir().join("files")
    }

    /// Per-catalog downloaded AppImage: `.../cache/<catalog_id>/`.
    pub fn catalog_cache_dir(&self, catalog_id: &str) -> PathBuf {
        self.manager_data_dir().join("cache").join(catalog_id)
    }

    /// Instance roots: `.../apps/<catalog_id>/<1-based-index>/` (tests and callers without a [`CatalogApp`]).
    pub fn instance_dir(&self, catalog_id: &str, instance_index: u32) -> PathBuf {
        self.manager_data_dir()
            .join("apps")
            .join(catalog_id)
            .join(instance_index.to_string())
    }

    /// Per [`CatalogApp::instance_filesystem_dirname`] (supports short slugs for Electrum, etc.).
    pub fn instance_dir_for(&self, app: &CatalogApp, instance_index: u32) -> PathBuf {
        self.manager_data_dir()
            .join("apps")
            .join(app.instance_filesystem_dirname())
            .join(instance_index.to_string())
    }

    pub fn apps_root_for(&self, catalog_id: &str) -> PathBuf {
        self.manager_data_dir().join("apps").join(catalog_id)
    }

    pub fn apps_root_for_app(&self, app: &CatalogApp) -> PathBuf {
        self.manager_data_dir()
            .join("apps")
            .join(app.instance_filesystem_dirname())
    }

    /// Optional icons we reference from .desktop: `~/.local/share/appimages/icons`.
    pub fn icons_dir(&self) -> PathBuf {
        self.manager_data_dir().join("icons")
    }

    /// XDG applications directory for .desktop files.
    pub fn applications_dir(&self) -> PathBuf {
        self.home.join(".local/share/applications")
    }

    pub fn config_dir(&self) -> PathBuf {
        self.home.join(".config")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.home.join(".cache")
    }

    pub fn local_share_dir(&self) -> PathBuf {
        self.home.join(".local/share")
    }

    /// Freedesktop hicolor N×N app icons: `~/.local/share/icons/hicolor/{N}x{N}/apps`.
    pub fn user_hicolor_apps_dir(&self, size: u32) -> PathBuf {
        self.home
            .join(format!(".local/share/icons/hicolor/{0}x{0}/apps", size))
    }

    /// Manager-owned desktop entries under `~/.local/share/applications`.
    pub fn managed_desktop_entries(&self) -> Result<Vec<PathBuf>> {
        let dir = self.applications_dir();
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for ent in std::fs::read_dir(dir)? {
            let ent = ent?;
            let p = ent.path();
            let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if name.starts_with(DESKTOP_PREFIX) && name.ends_with(".desktop") {
                out.push(p);
            }
        }
        out.sort();
        Ok(out)
    }

    /// Manager-owned icon files we generate under the user's hicolor tree.
    pub fn managed_hicolor_icon_files(&self) -> Result<Vec<PathBuf>> {
        let hicolor = self.home.join(".local/share/icons/hicolor");
        if !hicolor.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for size_dir in std::fs::read_dir(&hicolor)? {
            let size_dir = size_dir?;
            let apps = size_dir.path().join("apps");
            if !apps.is_dir() {
                continue;
            }
            for ent in std::fs::read_dir(apps)? {
                let ent = ent?;
                let p = ent.path();
                let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.starts_with("appimage-control-") && name.ends_with(".png") {
                    out.push(p);
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_under_fake_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let p = AppPaths::new(home.clone()).unwrap();
        assert_eq!(p.home(), home.as_path());
        assert!(p
            .managed_files_dir()
            .ends_with(".local/share/appimages/files"));
        assert!(p.applications_dir().ends_with(".local/share/applications"));
    }

    #[test]
    fn finds_managed_desktop_and_icons() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let p = AppPaths::new(home.clone()).unwrap();
        let apps = p.applications_dir();
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("appimage-control-test-1.desktop"), b"x").unwrap();
        std::fs::write(apps.join("other.desktop"), b"x").unwrap();

        let icons = p.user_hicolor_apps_dir(256);
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(icons.join("appimage-control-test-1.png"), b"x").unwrap();
        std::fs::write(icons.join("random.png"), b"x").unwrap();

        let d = p.managed_desktop_entries().unwrap();
        assert_eq!(d.len(), 1);
        let i = p.managed_hicolor_icon_files().unwrap();
        assert_eq!(i.len(), 1);
    }
}
