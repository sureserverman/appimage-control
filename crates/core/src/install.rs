use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::desktop::{app_id_from_appimage_path, build_desktop_entry, desktop_filename_for_app_id};
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::paths::AppPaths;

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// If set, copy this icon path into manager icons dir and reference it from `.desktop`.
    pub icon_source: Option<PathBuf>,
    /// Override display name (default: derived from filename).
    pub display_name: Option<String>,
    /// Override app id (default: from filename).
    pub app_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct InstallResult {
    pub app_id: String,
    pub appimage_path: PathBuf,
    pub desktop_path: PathBuf,
}

fn ensure_appimage_extension(path: &Path) -> Result<()> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("appimage"))
        .unwrap_or(false);
    if ext {
        Ok(())
    } else {
        Err(Error::NotAppImageExtension)
    }
}

/// Copy `source` AppImage into managed dir, chmod +x, write `.desktop`, optionally refresh desktop DB.
pub fn install_appimage(
    paths: &AppPaths,
    source: &Path,
    options: &InstallOptions,
) -> Result<InstallResult> {
    if !source.exists() {
        return Err(Error::SourceMissing(source.to_path_buf()));
    }
    if !source.is_file() {
        return Err(Error::NotAFile(source.to_path_buf()));
    }
    ensure_appimage_extension(source)?;

    let file_name = source
        .file_name()
        .ok_or_else(|| Error::InvalidPath(source.display().to_string()))?;
    let dest_image = paths.managed_files_dir().join(file_name);
    if dest_image.exists() {
        return Err(Error::TargetExists(dest_image));
    }

    fs::create_dir_all(paths.managed_files_dir())?;
    fs::create_dir_all(paths.applications_dir())?;

    fs::copy(source, &dest_image)?;
    let mut perms = fs::metadata(&dest_image)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dest_image, perms)?;

    let app_id = options
        .app_id
        .clone()
        .unwrap_or_else(|| app_id_from_appimage_path(source));
    let display_name = options
        .display_name
        .clone()
        .unwrap_or_else(|| human_name_from_filename(file_name.to_string_lossy().as_ref()));

    let icon_dest = if let Some(ref icon_src) = options.icon_source {
        fs::create_dir_all(paths.icons_dir())?;
        let ext = icon_src
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let icon_dest = paths.icons_dir().join(format!("{}.{}", app_id, ext));
        fs::copy(icon_src, &icon_dest)?;
        Some(icon_dest)
    } else {
        None
    };

    let desktop_body =
        build_desktop_entry(&display_name, &app_id, &dest_image, icon_dest.as_deref());
    let desktop_name = desktop_filename_for_app_id(&app_id);
    let desktop_path = paths.applications_dir().join(desktop_name);
    fs::write(&desktop_path, desktop_body)?;

    update_desktop_database_best_effort(paths);

    Ok(InstallResult {
        app_id,
        appimage_path: dest_image,
        desktop_path,
    })
}

fn human_name_from_filename(name: &str) -> String {
    let base = name.rsplit_once('.').map(|(b, _)| b).unwrap_or(name);
    base.replace(['-', '_'], " ")
}

fn update_desktop_database_best_effort(paths: &AppPaths) {
    let _ = host_command("update-desktop-database")
        .arg(paths.applications_dir())
        .status();
}

/// Remove managed AppImage, its `.desktop`, and optional managed icon.
pub fn uninstall_app(paths: &AppPaths, desktop_path: &Path) -> Result<()> {
    let entry = crate::desktop::parse_desktop_file(desktop_path)?;
    if !entry.managed {
        return Err(Error::DesktopParse(
            "not an AppImage Control managed desktop file".into(),
        ));
    }
    let img = entry.appimage_path()?;
    if img.starts_with(paths.managed_files_dir()) && img.exists() {
        let _ = fs::remove_file(&img);
    }
    if let Some(icon) = entry.icon {
        let p = PathBuf::from(&icon);
        if p.starts_with(paths.icons_dir()) && p.exists() {
            let _ = fs::remove_file(&p);
        }
    }
    fs::remove_file(desktop_path)?;
    update_desktop_database_best_effort(paths);
    Ok(())
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;

    use super::*;
    use crate::paths::AppPaths;

    #[test]
    fn install_sets_executable() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let home = tmp.path();
        let paths = AppPaths::new(home.to_path_buf()).unwrap();
        let src = tmp.child("Test.AppImage");
        src.write_binary(b"fake").unwrap();

        let res = install_appimage(&paths, src.path(), &InstallOptions::default()).unwrap();
        assert!(res.appimage_path.exists());
        let mode = fs::metadata(&res.appimage_path)
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0);
        assert!(res.desktop_path.exists());
    }
}
