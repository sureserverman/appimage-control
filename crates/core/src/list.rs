use std::fs;
use std::path::Path;

use crate::desktop::{parse_desktop_file, DESKTOP_PREFIX};
use crate::error::{Error, Result};
use crate::model::InstalledApp;
use crate::paths::AppPaths;

/// List all installations managed by this app (by scanning `applications/` for our prefix).
pub fn list_installed(paths: &AppPaths) -> Result<Vec<InstalledApp>> {
    let apps_dir = paths.applications_dir();
    if !apps_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut out = Vec::new();
    for ent in fs::read_dir(&apps_dir)? {
        let ent = ent?;
        let path = ent.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if !name.starts_with(DESKTOP_PREFIX) || !name.ends_with(".desktop") {
            continue;
        }
        if let Ok(app) = installed_app_from_desktop(&path) {
            out.push(app);
        }
    }
    out.sort_by(|a, b| a.display_name.cmp(&b.display_name));
    Ok(out)
}

fn installed_app_from_desktop(desktop_path: &Path) -> Result<InstalledApp> {
    let parsed = parse_desktop_file(desktop_path)?;
    if !parsed.managed {
        return Err(Error::DesktopParse("unmanaged desktop file".into()));
    }
    let appimage_path = parsed.appimage_path()?;
    let id = parsed
        .app_id
        .clone()
        .unwrap_or_else(|| crate::desktop::app_id_from_appimage_path(&appimage_path));
    let display_name = parsed.name.unwrap_or_else(|| id.replace('-', " "));
    let icon_path = parsed
        .icon
        .as_deref()
        .map(Path::new)
        .map(|p| p.to_path_buf());

    Ok(InstalledApp {
        id,
        display_name,
        appimage_path,
        desktop_file_path: desktop_path.to_path_buf(),
        icon_path,
    })
}
