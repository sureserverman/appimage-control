use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::desktop::parse_desktop_file;
use crate::error::{Error, Result};
use crate::paths::AppPaths;

/// Replace an installed AppImage with a new file (atomic rename). Keeps `.desktop` and app id.
pub fn update_from_file(
    paths: &AppPaths,
    desktop_path: &Path,
    new_appimage: &Path,
) -> Result<PathBuf> {
    if !new_appimage.exists() || !new_appimage.is_file() {
        return Err(Error::NotAFile(new_appimage.to_path_buf()));
    }
    let ext = new_appimage
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("appimage"))
        .unwrap_or(false);
    if !ext {
        return Err(Error::NotAppImageExtension);
    }

    let parsed = parse_desktop_file(desktop_path)?;
    if !parsed.managed {
        return Err(Error::DesktopParse("not a managed desktop entry".into()));
    }
    let current = parsed.appimage_path()?;
    if !current.starts_with(paths.managed_files_dir()) {
        return Err(Error::InvalidPath(
            "managed AppImage is outside manager directory".into(),
        ));
    }

    let dir = current
        .parent()
        .ok_or_else(|| Error::InvalidPath("no parent".into()))?;
    fs::create_dir_all(dir)?;
    let tmp_name = format!(
        ".{}.new.{}",
        current
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("appimage"),
        uuid::Uuid::new_v4()
    );
    let tmp_path = dir.join(tmp_name);
    fs::copy(new_appimage, &tmp_path)?;
    #[cfg(unix)]
    {
        let mut perms = fs::metadata(&tmp_path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&tmp_path, perms)?;
    }

    // Replace atomically
    fs::rename(&tmp_path, &current)?;
    Ok(current)
}

/// Run `appimageupdatetool` if available; returns stdout+stderr combined.
pub fn run_appimageupdatetool(appimage_path: &Path) -> Result<String> {
    let out = Command::new("appimageupdatetool")
        .arg(appimage_path)
        .output()
        .map_err(|e| Error::UpdateFailed(e.to_string()))?;
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    if out.status.success() {
        Ok(text)
    } else {
        Err(Error::UpdateFailed(text))
    }
}

pub fn appimageupdatetool_available() -> bool {
    Command::new("appimageupdatetool")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;

    use super::*;
    use crate::install::install_appimage;
    use crate::install::InstallOptions;
    use crate::paths::AppPaths;

    #[test]
    fn update_replaces_binary() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let src = tmp.child("Old.AppImage");
        src.write_binary(b"v1").unwrap();
        let inst = install_appimage(&paths, src.path(), &InstallOptions::default()).unwrap();

        let new_file = tmp.child("New.AppImage");
        new_file.write_binary(b"v2").unwrap();

        update_from_file(&paths, &inst.desktop_path, new_file.path()).unwrap();
        let bytes = fs::read(&inst.appimage_path).unwrap();
        assert_eq!(bytes, b"v2");
        assert!(inst.desktop_path.exists());
    }
}
