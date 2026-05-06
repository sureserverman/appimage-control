use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

pub const MANAGED_KEY: &str = "X-AppImage-Control-Managed";
pub const APP_ID_KEY: &str = "X-AppImage-Control-AppId";
pub const CATALOG_ID_KEY: &str = "X-AppImage-Control-CatalogId";
pub const INSTANCE_INDEX_KEY: &str = "X-AppImage-Control-InstanceIndex";
pub const DESKTOP_PREFIX: &str = "appimage-control-";

/// Build `.desktop` contents for a managed AppImage.
pub fn build_desktop_entry(
    display_name: &str,
    app_id: &str,
    exec_path: &Path,
    icon_path: Option<&Path>,
) -> String {
    let exec = exec_path.to_string_lossy().replace('\n', "");
    let mut s = String::new();
    s.push_str("[Desktop Entry]\n");
    s.push_str("Type=Application\n");
    s.push_str(&format!("Name={}\n", escape_desktop_value(display_name)));
    s.push_str(&format!("Exec={} %U\n", exec));
    if let Some(icon) = icon_path {
        let i = icon.to_string_lossy().replace('\n', "");
        s.push_str(&format!("Icon={}\n", i));
    }
    s.push_str("Terminal=false\n");
    s.push_str("Categories=Utility;\n");
    s.push_str(&format!("{}=true\n", MANAGED_KEY));
    s.push_str(&format!(
        "{}={}\n",
        APP_ID_KEY,
        escape_desktop_value(app_id)
    ));
    s
}

/// `.desktop` for a catalog instance (separate dir + optional `--name` like Feather).
/// `startup_wm_class` is used for `StartupWMClass=` and should match `{wm_class}` in exec templates when set.
///
/// `icon_file` (absolute path to a PNG) takes precedence over `icon_name` (freedesktop icon name).
/// For GNOME, prefer `icon_name` with PNGs under `~/.local/share/icons/hicolor/.../apps/` — absolute
/// `Icon=` paths can yield a generic gear in the shell/dock even when the file exists.
///
/// `pass_file_urls`: when true, append ` %U` to `Exec=`; set false for apps like Electrum that treat
/// extra argv as subcommands.
#[allow(clippy::too_many_arguments)]
pub fn build_instance_desktop_entry(
    display_name: &str,
    stable_app_id: &str,
    catalog_id: &str,
    instance_index: u32,
    appimage_path: &Path,
    extra_exec_args: &str,
    pass_file_urls: bool,
    icon_name: Option<&str>,
    icon_file: Option<&Path>,
    startup_wm_class: &str,
) -> String {
    let path_str = appimage_path.to_string_lossy();
    let quoted_exec = if path_str.contains(' ') || path_str.contains('\t') {
        format!(
            "\"{}\"",
            path_str.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        path_str.into_owned()
    };
    let url_suffix = if pass_file_urls { " %U" } else { "" };
    let exec_line = if extra_exec_args.trim().is_empty() {
        format!("{}{}", quoted_exec, url_suffix)
    } else {
        format!("{} {}{}", quoted_exec, extra_exec_args.trim(), url_suffix)
    };

    let mut s = String::new();
    s.push_str("[Desktop Entry]\n");
    s.push_str("Type=Application\n");
    s.push_str(&format!("Name={}\n", escape_desktop_value(display_name)));
    s.push_str(&format!("Exec={}\n", exec_line));
    if let Some(path) = icon_file {
        let i = path.to_string_lossy().replace('\n', "");
        s.push_str(&format!("Icon={}\n", escape_desktop_value(&i)));
    } else if let Some(icon) = icon_name {
        let i = icon.replace('\n', "");
        s.push_str(&format!("Icon={}\n", escape_desktop_value(&i)));
    }
    s.push_str(&format!(
        "StartupWMClass={}\n",
        escape_desktop_value(startup_wm_class)
    ));
    s.push_str("Terminal=false\n");
    s.push_str("Categories=Utility;\n");
    s.push_str(&format!("{}=true\n", MANAGED_KEY));
    s.push_str(&format!(
        "{}={}\n",
        APP_ID_KEY,
        escape_desktop_value(stable_app_id)
    ));
    s.push_str(&format!(
        "{}={}\n",
        CATALOG_ID_KEY,
        escape_desktop_value(catalog_id)
    ));
    s.push_str(&format!("{}={}\n", INSTANCE_INDEX_KEY, instance_index));
    s
}

pub fn desktop_filename_for_instance(catalog_id: &str, instance_index: u32) -> String {
    format!(
        "{}{}-{}.desktop",
        DESKTOP_PREFIX,
        sanitize_app_id(catalog_id),
        instance_index
    )
}

fn escape_desktop_value(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Parse a minimal subset of a `.desktop` file (first `[Desktop Entry]` group).
pub fn parse_desktop_file(path: &Path) -> Result<DesktopEntry> {
    let raw = fs::read_to_string(path)?;
    parse_desktop_str(&raw)
}

pub fn parse_desktop_str(raw: &str) -> Result<DesktopEntry> {
    let mut in_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut startup_wm_class = None;
    let mut app_id = None;
    let mut managed = false;

    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "Name" => name = Some(unescape_desktop_value(v.trim())),
            "Exec" => exec = Some(strip_exec_args(v.trim())),
            "Icon" => icon = Some(v.trim().to_string()),
            "StartupWMClass" => startup_wm_class = Some(unescape_desktop_value(v.trim())),
            APP_ID_KEY => app_id = Some(v.trim().to_string()),
            MANAGED_KEY => managed = parse_bool(v.trim()),
            _ => {}
        }
    }

    Ok(DesktopEntry {
        name,
        exec,
        icon,
        startup_wm_class,
        app_id,
        managed,
    })
}

fn parse_bool(v: &str) -> bool {
    matches!(v.to_lowercase().as_str(), "true" | "1" | "yes")
}

/// Exec often has `%U` etc.; keep only the command path.
fn strip_exec_args(exec: &str) -> String {
    exec.split_whitespace().next().unwrap_or(exec).to_string()
}

fn unescape_desktop_value(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('\\') => out.push('\\'),
                Some(x) => {
                    out.push('\\');
                    out.push(x);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[derive(Debug, Clone, Default)]
pub struct DesktopEntry {
    pub name: Option<String>,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub startup_wm_class: Option<String>,
    pub app_id: Option<String>,
    pub managed: bool,
}

impl DesktopEntry {
    pub fn appimage_path(&self) -> Result<PathBuf> {
        let exec = self.exec.as_ref().ok_or(Error::MissingExec)?;
        Ok(PathBuf::from(exec))
    }
}

pub fn desktop_filename_for_app_id(app_id: &str) -> String {
    format!("{}{}.desktop", DESKTOP_PREFIX, sanitize_app_id(app_id))
}

pub(crate) fn sanitize_app_id(app_id: &str) -> String {
    let mut s: String = app_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    s.trim_matches('-').to_string()
}

/// Derive default app id from AppImage filename (without extension).
pub fn app_id_from_appimage_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| sanitize_app_id(&s.to_lowercase()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "appimage".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_build_parse() {
        let exec = PathBuf::from("/opt/Foo.AppImage");
        let body = build_desktop_entry("Foo App", "foo-app", &exec, None);
        let parsed = parse_desktop_str(&body).unwrap();
        assert_eq!(parsed.name.as_deref(), Some("Foo App"));
        assert_eq!(parsed.exec.as_deref(), Some("/opt/Foo.AppImage"));
        assert!(parsed.managed);
        assert_eq!(parsed.app_id.as_deref(), Some("foo-app"));
    }

    #[test]
    fn instance_desktop_icon_file_overrides_theme_name() {
        let exec = PathBuf::from("/tmp/appimage-control-launch.sh");
        let icon_png = PathBuf::from(
            "/home/u/.local/share/icons/hicolor/256x256/apps/appimage-control-z-1.png",
        );
        let body = build_instance_desktop_entry(
            "Z (#1)",
            "z-1",
            "z",
            1,
            &exec,
            "",
            true,
            Some("appimage-control-z-1"),
            Some(&icon_png),
            "io.github.appimagecontrol.z.i1",
        );
        assert!(body.contains(&format!("Icon={}", icon_png.display())));
        assert!(!body.contains("Icon=appimage-control-z-1\n"));
    }

    #[test]
    fn instance_desktop_exec_without_file_urls_has_no_percent_u() {
        let exec = PathBuf::from("/tmp/launch.sh");
        let body = build_instance_desktop_entry(
            "Electrum (#1)",
            "electrum-btc-1",
            "electrum-btc",
            1,
            &exec,
            "",
            false,
            Some("appimage-control-electrum-btc-1"),
            None,
            "io.github.appimagecontrol.electrum-btc.i1",
        );
        assert!(
            body.contains("Exec=/tmp/launch.sh\n"),
            "expected Exec without %U, got:\n{}",
            body
        );
        assert!(!body.contains("%U"));
    }
}
