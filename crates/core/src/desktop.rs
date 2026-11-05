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
    let exec = quote_exec_arg(&exec_path.to_string_lossy());
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
    let quoted_exec = quote_exec_arg(&appimage_path.to_string_lossy());
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

/// Characters that force an `Exec` argument into double quotes (Desktop Entry spec, "exec-variables").
const EXEC_RESERVED: &[char] = &[
    ' ', '\t', '\n', '"', '\'', '\\', '>', '<', '~', '|', '&', ';', '$', '*', '?', '#', '(', ')',
    '`',
];

/// How a path holding `%` is started, as written in the file (both escape layers applied):
/// `sh -c 'exec "$0" "$@"' <path> …`, using double quotes because the spec allows only those.
const PERCENT_PATH_RUNNER: &str = r#"sh -c "exec \\"\\$0\\" \\"\\$@\\"""#;

/// The same prefix after the string-value layer is unescaped (what [`strip_exec_args`] sees).
const PERCENT_PATH_RUNNER_UNESCAPED: &str = r#"sh -c "exec \"\$0\" \"\$@\"" "#;

/// Turn a path into one `Exec=` argument, as written in the file.
///
/// Applies the quoting rule (double quotes when a reserved character or `%` is present; inside
/// them `"`, `` ` ``, `$` and `\` get a backslash; `%` becomes `%%`) and then the string-value
/// escape layer on top (every `\` doubled, newline as `\n`). A plain path is returned unchanged.
/// [`unquote_exec_arg`] inverts this exactly. A carriage return is dropped (it cannot be
/// represented safely on one line).
///
/// A path holding `%` is run through [`PERCENT_PATH_RUNNER`]: GLib (GNOME's launcher loader)
/// checks that the `Exec=` program exists *before* it turns `%%` back into `%`, so a `%` path as
/// the program never loads. With `sh` first the check passes, and `exec "$0" "$@"` runs the exact
/// file (`env` would not do: it reads a path holding `=` as a variable assignment).
/// [`strip_exec_args`] skips that prefix when parsing.
fn quote_exec_arg(path: &str) -> String {
    let path = path.replace('\r', "");
    if !path.contains(|c| EXEC_RESERVED.contains(&c) || c == '%') {
        return path;
    }
    let mut out = String::with_capacity(path.len() + 2);
    out.push('"');
    for c in path.chars() {
        match c {
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            '\\' => out.push_str("\\\\\\\\"),
            '%' => out.push_str("%%"),
            '\n' => out.push_str("\\n"),
            _ => out.push(c),
        }
    }
    out.push('"');
    if path.contains('%') {
        format!("{PERCENT_PATH_RUNNER} {out}")
    } else {
        out
    }
}

/// Take the first argument of an `Exec` value (already string-unescaped): a leading
/// double-quoted argument with its quote escapes, or a bare word; `%%` becomes `%`. Anything
/// after the argument (field codes such as `%U`, extra flags) is dropped.
fn unquote_exec_arg(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars().peekable();
    if chars.peek() == Some(&'"') {
        chars.next();
        while let Some(c) = chars.next() {
            match c {
                '"' => break,
                '\\' => match chars.peek() {
                    Some(&n) if matches!(n, '"' | '`' | '$' | '\\') => {
                        out.push(n);
                        chars.next();
                    }
                    _ => out.push('\\'),
                },
                '%' if chars.peek() == Some(&'%') => {
                    chars.next();
                    out.push('%');
                }
                _ => out.push(c),
            }
        }
    } else {
        while let Some(c) = chars.next() {
            if c.is_whitespace() {
                break;
            }
            if c == '%' && chars.peek() == Some(&'%') {
                chars.next();
            }
            out.push(c);
        }
    }
    out
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
    let value = unescape_desktop_value(exec);
    // A path holding `%` is written behind `PERCENT_PATH_RUNNER` (see `quote_exec_arg`).
    let value = match value
        .trim_start()
        .strip_prefix(PERCENT_PATH_RUNNER_UNESCAPED)
    {
        Some(rest) => rest.trim_start().to_string(),
        None => value,
    };
    let first = unquote_exec_arg(&value);
    if first.is_empty() {
        exec.to_string()
    } else {
        first
    }
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

    fn exec_line(path: &str) -> String {
        let body = build_desktop_entry("N", "n", Path::new(path), None);
        body.lines()
            .find(|l| l.starts_with("Exec="))
            .unwrap()
            .to_string()
    }

    fn instance_body(path: &str) -> String {
        build_instance_desktop_entry(
            "N",
            "n-1",
            "n",
            1,
            Path::new(path),
            "",
            true,
            None,
            None,
            "c",
        )
    }

    fn assert_roundtrip(path: &str, expected_exec: &str) {
        assert_eq!(exec_line(path), expected_exec);
        let parsed =
            parse_desktop_str(&build_desktop_entry("N", "n", Path::new(path), None)).unwrap();
        assert_eq!(parsed.appimage_path().unwrap(), PathBuf::from(path));
        let inst = instance_body(path);
        assert!(inst.contains(&format!("{}\n", expected_exec)), "{}", inst);
        let parsed = parse_desktop_str(&inst).unwrap();
        assert_eq!(parsed.appimage_path().unwrap(), PathBuf::from(path));
    }

    #[test]
    fn exec_plain_path_stays_unquoted() {
        assert_roundtrip("/opt/Foo-1.2_x.AppImage", "Exec=/opt/Foo-1.2_x.AppImage %U");
    }

    #[test]
    fn exec_space_is_quoted() {
        assert_roundtrip("/o/a b.AppImage", r#"Exec="/o/a b.AppImage" %U"#);
    }

    /// GLib checks that an `Exec=` program exists before it turns `%%` back into `%`, so a
    /// path holding `%` as the program never loads on GNOME. Such a path runs through `env`.
    #[test]
    fn exec_percent_is_doubled_and_runs_through_sh() {
        assert_roundtrip(
            "/o/50%.AppImage",
            r#"Exec=sh -c "exec \\"\\$0\\" \\"\\$@\\"" "/o/50%%.AppImage" %U"#,
        );
    }

    #[test]
    fn exec_dollar_is_escaped() {
        assert_roundtrip("/o/a$b", r#"Exec="/o/a\\$b" %U"#);
    }

    #[test]
    fn exec_double_quote_is_escaped() {
        assert_roundtrip("/o/a\"b", r#"Exec="/o/a\\"b" %U"#);
    }

    #[test]
    fn exec_backtick_is_escaped() {
        assert_roundtrip("/o/a`b", r#"Exec="/o/a\\`b" %U"#);
    }

    #[test]
    fn exec_backslash_is_escaped() {
        assert_roundtrip("/o/a\\b", r#"Exec="/o/a\\\\b" %U"#);
    }

    #[test]
    fn exec_all_reserved_combined() {
        assert_roundtrip(
            "/o/a b%c$d\"e`f\\g;h",
            r#"Exec=sh -c "exec \\"\\$0\\" \\"\\$@\\"" "/o/a b%%c\\$d\\"e\\`f\\\\g;h" %U"#,
        );
    }

    #[test]
    fn exec_newline_cannot_inject_a_line() {
        let body = instance_body("/o/a\nExec=evil");
        assert_eq!(body.lines().filter(|l| l.starts_with("Exec=")).count(), 1);
        let parsed = parse_desktop_str(&body).unwrap();
        assert_eq!(
            parsed.appimage_path().unwrap(),
            PathBuf::from("/o/a\nExec=evil")
        );
    }

    #[test]
    fn exec_legacy_launchers_still_parse() {
        let e = parse_desktop_str("[Desktop Entry]\nExec=/opt/x.AppImage %U\n").unwrap();
        assert_eq!(e.exec.as_deref(), Some("/opt/x.AppImage"));
        let e =
            parse_desktop_str("[Desktop Entry]\nExec=\"/opt/a b/x.AppImage\" --flag %F\n").unwrap();
        assert_eq!(e.exec.as_deref(), Some("/opt/a b/x.AppImage"));
        let e = parse_desktop_str("[Desktop Entry]\nExec=/opt/x.AppImage\n").unwrap();
        assert_eq!(e.exec.as_deref(), Some("/opt/x.AppImage"));
    }

    /// `env` would read a path holding `=` as a variable assignment, so a `%` path runs through
    /// `sh -c 'exec "$0" "$@"'`, which takes any path as-is.
    #[test]
    fn exec_percent_and_equals_round_trip() {
        assert_roundtrip(
            "/o/a=b%c.AppImage",
            r#"Exec=sh -c "exec \\"\\$0\\" \\"\\$@\\"" "/o/a=b%%c.AppImage" %U"#,
        );
    }
}
