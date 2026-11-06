use std::path::{Component, Path, PathBuf};

use crate::catalog::CatalogApp;
use crate::desktop::DESKTOP_PREFIX;
use crate::error::{Error, Result};

/// The directory of a cargo build output (`…/target/debug`, `…/target/release/deps`, …) that
/// the running executable sits in, if it sits in one. Only such a build walks up from its own
/// location to find the checkout's `data/` (catalog and seed packs). An installed binary
/// (`/usr/bin`, `~/.cargo/bin`, `~/.local/bin`) never walks: from there the walk would pass
/// through `$HOME` or `/`, where a planted `data/catalog.json` would win.
///
/// Recognised layouts: `target/{debug,release}`, `target/<triple>/{debug,release}`, each with an
/// optional trailing `deps` (where test binaries live).
pub(crate) fn dev_build_dir(exe_dir: &Path) -> Option<&Path> {
    let names: Vec<&str> = exe_dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => n.to_str(),
            _ => None,
        })
        .collect();
    let mut rest: &[&str] = &names;
    if rest.last() == Some(&"deps") {
        rest = &rest[..rest.len() - 1];
    }
    let profile = |n: &&str| *n == "debug" || *n == "release";
    let is_build = match rest {
        [.., t, p] => *t == "target" && profile(p),
        _ => false,
    } || match rest {
        [.., t, _triple, p] => *t == "target" && profile(p),
        _ => false,
    };
    is_build.then_some(exe_dir)
}

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

/// Base folder for AppImage Control's own configuration (`config.json`): `~/.config` natively.
/// Inside Flatpak `~/.config` is not writable (the sandbox exposes only `xdg-data/appimages`,
/// `applications` and `icons`), so it is `$XDG_CONFIG_HOME`, which Flatpak points at
/// `~/.var/app/<app id>/config`.
pub fn own_config_base(home: &Path) -> PathBuf {
    own_base_from(
        home,
        ".config",
        crate::host_cmd::in_flatpak(),
        std::env::var_os("XDG_CONFIG_HOME"),
    )
}

/// Base folder for AppImage Control's own data (saved seed forms, pack recordings):
/// `~/.local/share` natively, `$XDG_DATA_HOME` (`~/.var/app/<app id>/data`) inside Flatpak.
pub fn own_data_base(home: &Path) -> PathBuf {
    own_base_from(
        home,
        ".local/share",
        crate::host_cmd::in_flatpak(),
        std::env::var_os("XDG_DATA_HOME"),
    )
}

/// [`own_config_base`] / [`own_data_base`] with the sandbox check and variable passed in.
pub(crate) fn own_base_from(
    home: &Path,
    native_rel: &str,
    flatpak: bool,
    xdg: Option<std::ffi::OsString>,
) -> PathBuf {
    if flatpak {
        if let Some(p) = xdg.map(PathBuf::from).filter(|p| p.is_absolute()) {
            return p;
        }
    }
    home.join(native_rel)
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
    fn own_state_bases_move_into_the_sandbox_only_inside_flatpak() {
        let home = Path::new("/home/u");
        let xdg = |p: &str| Some(std::ffi::OsString::from(p));
        assert_eq!(
            own_base_from(home, ".config", false, xdg("/elsewhere")),
            Path::new("/home/u/.config")
        );
        assert_eq!(
            own_base_from(home, ".config", true, xdg("/home/u/.var/app/x/config")),
            Path::new("/home/u/.var/app/x/config")
        );
        assert_eq!(
            own_base_from(home, ".local/share", true, None),
            Path::new("/home/u/.local/share")
        );
        assert_eq!(
            own_base_from(home, ".local/share", true, xdg("relative")),
            Path::new("/home/u/.local/share")
        );
    }

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

    #[test]
    fn only_cargo_build_dirs_walk_for_data() {
        use super::dev_build_dir;
        for dev in [
            "/home/u/dev/aic/target/debug",
            "/home/u/dev/aic/target/release",
            "/home/u/dev/aic/target/debug/deps",
            "/home/u/dev/aic/target/x86_64-unknown-linux-gnu/release",
            "/home/u/dev/aic/target/aarch64-unknown-linux-gnu/debug/deps",
        ] {
            assert!(dev_build_dir(Path::new(dev)).is_some(), "{dev}");
        }
        for installed in [
            "/usr/bin",
            "/home/u/.cargo/bin",
            "/home/u/.local/bin",
            "/app/bin",
            "/home/u/Applications",
            "/home/u/target/bin",
            "/home/u/target",
            "/opt/target/tools/bin",
        ] {
            assert!(dev_build_dir(Path::new(installed)).is_none(), "{installed}");
        }
    }
}
