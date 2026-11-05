use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::desktop::parse_desktop_file;
use crate::error::{Error, Result};
use crate::host_cmd::host_command;
use crate::paths::AppPaths;

/// Sweep leftover `.{name}.new.{uuid}`-style temp files in `instance_dir` that are older than
/// `older_than`. Mirrors the [`crate::seed::sweep_stale_pid_files`] pattern.
///
/// Called before each update run so a crashed prior update does not leave temp files accumulating
/// indefinitely. The 1-hour cutoff matches the seed-sweep: a legitimate in-progress update finishes
/// in seconds, so any file older than an hour is definitely abandoned.
pub fn sweep_stale_update_temps(instance_dir: &Path, older_than: SystemTime) {
    let Ok(entries) = fs::read_dir(instance_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        // Match `.{anything}.new.{anything}` — the pattern used by both
        // `copy_or_hardlink` in instances.rs and `update_from_file` here.
        if !name_str.starts_with('.') {
            continue;
        }
        // Must contain ".new." (not just start with a dot).
        if !name_str.contains(".new.") {
            continue;
        }
        if let Ok(meta) = entry.metadata() {
            if let Ok(mtime) = meta.modified() {
                if mtime < older_than {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }
    }
}

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
    if !crate::install::is_contained_in(&current, &paths.managed_files_dir()) {
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
    let out = host_command("appimageupdatetool")
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
    host_command("appimageupdatetool")
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

    /// A backdated `.x.new.deadbeef` temp must be removed; a fresh one must be kept.
    #[test]
    fn sweep_stale_update_temps_removes_old_keeps_fresh() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();

        // Stale temp file — backdated past the cutoff.
        let stale = dir.join(".session.new.deadbeef");
        std::fs::write(&stale, b"partial").unwrap();
        std::process::Command::new("touch")
            .args(["-d", "@1000000", stale.to_str().unwrap()])
            .status()
            .unwrap();

        // Fresh temp file — mtime is now, will be after the cutoff.
        let fresh = dir.join(".session.new.cafecafe");
        std::fs::write(&fresh, b"in-progress").unwrap();

        // Unrelated dot-file that must not be touched.
        let unrelated = dir.join(".appimage-control-seed-host-pid.42");
        std::fs::write(&unrelated, b"pid").unwrap();

        let cutoff = SystemTime::now() - std::time::Duration::from_secs(3600);
        sweep_stale_update_temps(dir, cutoff);

        assert!(!stale.exists(), "stale temp must be removed");
        assert!(fresh.exists(), "fresh temp must be preserved");
        assert!(unrelated.exists(), "unrelated dot-file must not be touched");
    }

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

    #[test]
    fn update_refuses_dotdot_exec() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        fs::create_dir_all(paths.applications_dir()).unwrap();
        fs::create_dir_all(paths.managed_files_dir()).unwrap();
        let new_file = tmp.child("New.AppImage");
        new_file.write_binary(b"v2").unwrap();

        let victim = paths
            .managed_files_dir()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("victim.AppImage");
        fs::write(&victim, b"precious").unwrap();
        let evil_exec = paths.managed_files_dir().join("../../victim.AppImage");
        let body = crate::desktop::build_desktop_entry("evil", "evil", &evil_exec, None);
        let desktop = paths
            .applications_dir()
            .join(crate::desktop::desktop_filename_for_app_id("evil"));
        fs::write(&desktop, body).unwrap();

        let res = update_from_file(&paths, &desktop, new_file.path());
        assert!(res.is_err(), "escaping launcher must be refused");
        assert_eq!(fs::read(&victim).unwrap(), b"precious");

        // Symlink escape.
        let link = paths.managed_files_dir().join("link.AppImage");
        std::os::unix::fs::symlink(&victim, &link).unwrap();
        let body = crate::desktop::build_desktop_entry("evil2", "evil2", &link, None);
        let desktop2 = paths
            .applications_dir()
            .join(crate::desktop::desktop_filename_for_app_id("evil2"));
        fs::write(&desktop2, body).unwrap();
        assert!(update_from_file(&paths, &desktop2, new_file.path()).is_err());
        assert_eq!(fs::read(&victim).unwrap(), b"precious");

        // A normal launcher still updates.
        let src = tmp.child("Old.AppImage");
        src.write_binary(b"v1").unwrap();
        let inst = install_appimage(&paths, src.path(), &InstallOptions::default()).unwrap();
        update_from_file(&paths, &inst.desktop_path, new_file.path()).unwrap();
        assert_eq!(fs::read(&inst.appimage_path).unwrap(), b"v2");
    }
}
