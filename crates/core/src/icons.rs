//! Resolve and fetch small raster icons for catalog apps (HTTP).

use std::fs;
use std::path::Path;

use crate::catalog::{CatalogApp, InstanceStrategy};
use crate::desktop::sanitize_app_id;
use crate::error::Result;
use crate::paths::AppPaths;
use crate::version_info::github_owner_repo_from_url;

const MAX_ICON_BYTES: usize = 4 * 1024 * 1024;

/// Bundled Joplin tile (same asset as `crates/gui/assets/catalog-icons/joplin.png`); always used for `joplin` installs.
static JOPLIN_EMBEDDED_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gui/assets/catalog-icons/joplin.png"
));

/// Official Electrum-LTC branding (`installers/linux/files/electrum-ltc.png`); avoids wrong favicon/GitHub avatars.
static ELECTRUM_LTC_EMBEDDED_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gui/assets/catalog-icons/electrum-ltc.png"
));

/// Feather dock tiles: cycle 1 → 2 → 3 → 1… per instance index.
///
/// These are the “proper” per-instance icons (copied from `~/dev/installers/linux/files/feather{1,2,3}.png`).
static FEATHER_SLOT_1_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gui/assets/catalog-icons/feather1.png"
));
static FEATHER_SLOT_2_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gui/assets/catalog-icons/feather2.png"
));
static FEATHER_SLOT_3_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../gui/assets/catalog-icons/feather3.png"
));

fn http_client() -> Option<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(concat!("appimage-control/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()
}

/// Best-effort host from `https://example.org/path` → `example.org`.
fn homepage_host(homepage: &str) -> Option<String> {
    let s = homepage.trim();
    let rest = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))?;
    let host = rest.split('/').next()?.split(':').next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

/// Small PNG favicon via Google’s favicon service (no extra crates).
fn google_favicon_url_for_homepage(homepage: &str) -> Option<String> {
    let host = homepage_host(homepage)?;
    Some(format!(
        "https://www.google.com/s2/favicons?domain={}&sz=64",
        host
    ))
}

/// True when the homepage host is GitHub itself (favicon is useless; use repo API instead).
fn homepage_host_is_github(homepage: &str) -> bool {
    match homepage_host(homepage) {
        Some(h) => h == "github.com" || h.ends_with(".github.com") || h.ends_with(".github.io"),
        None => false,
    }
}

fn fetch_github_repo_owner_avatar_url(owner: &str, repo: &str) -> Option<String> {
    let client = http_client()?;
    let api = format!("https://api.github.com/repos/{}/{}", owner, repo);
    let resp = client.get(api).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().ok()?;
    v.get("owner")?
        .get("avatar_url")?
        .as_str()
        .map(|s| s.to_string())
}

/// Returns a URL to a small raster icon (PNG), or `None` if unknown.
pub fn resolve_icon_image_url(app: &CatalogApp) -> Option<String> {
    if let Some(ref u) = app.icon_url {
        let t = u.trim();
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    // Product-site favicon before GitHub org avatar: org logos often differ from app branding
    // (e.g. Session: getsession.org vs session-foundation on GitHub).
    if let Some(ref hp) = app.homepage {
        if !homepage_host_is_github(hp) {
            if let Some(url) = google_favicon_url_for_homepage(hp) {
                return Some(url);
            }
        }
    }
    if let Ok(dl) = app.resolved_download_url() {
        if let Some((o, r)) = github_owner_repo_from_url(dl) {
            if let Some(av) = fetch_github_repo_owner_avatar_url(&o, &r) {
                return Some(av);
            }
        }
    }
    if let Some(ref hp) = app.homepage {
        if let Some((o, r)) = github_owner_repo_from_url(hp) {
            if let Some(av) = fetch_github_repo_owner_avatar_url(&o, &r) {
                return Some(av);
            }
        }
    }
    None
}

/// GET bytes; rejects responses larger than [`MAX_ICON_BYTES`].
pub fn fetch_url_bytes(url: &str) -> Option<Vec<u8>> {
    let client = http_client()?;
    let resp = client.get(url).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let upper = resp.content_length().unwrap_or(0) as usize;
    if upper > MAX_ICON_BYTES {
        return None;
    }
    let bytes = resp.bytes().ok()?;
    if bytes.len() > MAX_ICON_BYTES {
        return None;
    }
    Some(bytes.to_vec())
}

/// Theme icon name (no extension) for instance launchers.
pub fn instance_icon_name(catalog_id: &str, instance_index: u32) -> String {
    format!(
        "appimage-control-{}-{}",
        sanitize_app_id(catalog_id),
        instance_index
    )
}

/// Theme icon basename for `Icon=` and hicolor `apps/<name>.png`.
///
/// Joplin, [`InstanceStrategy::ElectronClass`], [`InstanceStrategy::ElectronClassIsolated`], and
/// [`InstanceStrategy::QtX11`] use the **same**
/// string as `StartupWMClass=` (`io.github.appimagecontrol…`) so GNOME can resolve the dock icon
/// with XWayland + `xprop`.
pub fn instance_icon_theme_name(app: &CatalogApp, instance_index: u32) -> String {
    if matches!(
        app.instance_strategy,
        InstanceStrategy::JoplinAltInstance
            | InstanceStrategy::ElectronClass
            | InstanceStrategy::ElectronClassIsolated
            | InstanceStrategy::QtX11
    ) {
        format!(
            "io.github.appimagecontrol.{}.i{}",
            sanitize_app_id(&app.id),
            instance_index
        )
    } else {
        instance_icon_name(&app.id, instance_index)
    }
}

/// Read bundled raster icon from an extracted Cryptomator AppImage (`squashfs-root/...`).
fn cryptomator_bundled_icon_bytes(instance_dir: &Path) -> Option<Vec<u8>> {
    let hicolor = instance_dir.join("squashfs-root/usr/share/icons/hicolor");
    if !hicolor.is_dir() {
        return None;
    }
    for size in [256u32, 512, 128, 64, 48] {
        let candidate = hicolor.join(format!(
            "{size}x{size}/apps/org.cryptomator.Cryptomator.png"
        ));
        if candidate.is_file() {
            return fs::read(candidate).ok();
        }
    }
    None
}

/// Fetch icon bytes and write PNGs under the user hicolor theme at **128×128** and **256×256** only.
/// Returns the **icon name** (no extension) for `.desktop` `Icon=` (theme name), unless the caller
/// uses an absolute path via [`crate::desktop::build_instance_desktop_entry`].
///
/// For Cryptomator, pass `cryptomator_instance_dir` when the AppImage has been extracted so if HTTP
/// fetch fails, bytes are taken from `squashfs-root/usr/share/icons/hicolor/.../org.cryptomator.Cryptomator.png`.
pub fn write_instance_icon_png(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_index: u32,
    cryptomator_instance_dir: Option<&Path>,
) -> Result<Option<String>> {
    let bytes = if app.id == "feather" && app.instance_strategy == InstanceStrategy::QtX11 {
        // 1,4,7.. → slot 1; 2,5,8.. → slot 2; 3,6,9.. → slot 3.
        let slot = ((instance_index.saturating_sub(1)) % 3) + 1;
        match slot {
            1 => FEATHER_SLOT_1_PNG.to_vec(),
            2 => FEATHER_SLOT_2_PNG.to_vec(),
            _ => FEATHER_SLOT_3_PNG.to_vec(),
        }
    } else if app.id == "electrum-ltc" {
        ELECTRUM_LTC_EMBEDDED_PNG.to_vec()
    } else if app.id == "joplin" {
        // Always use the bundled 256×256 Linux icon — favicon/GitHub fallbacks are tiny and look blurry scaled up.
        JOPLIN_EMBEDDED_PNG.to_vec()
    } else {
        let from_net = resolve_icon_image_url(app).and_then(|u| fetch_url_bytes(&u));
        if let Some(b) = from_net {
            b
        } else if app.id == "cryptomator" {
            let Some(dir) = cryptomator_instance_dir else {
                return Ok(None);
            };
            let Some(b) = cryptomator_bundled_icon_bytes(dir) else {
                return Ok(None);
            };
            b
        } else if app.id == "session" {
            // When `icon_url` fetch fails (offline / tests), still install a tile so `.desktop` gets `Icon=`.
            JOPLIN_EMBEDDED_PNG.to_vec()
        } else {
            return Ok(None);
        }
    };
    let icon_name = instance_icon_theme_name(app, instance_index);
    let scaled_128 = scale_png_to_square(&bytes, 128).unwrap_or_else(|| bytes.clone());
    let scaled_256 = scale_png_to_square(&bytes, 256).unwrap_or_else(|| bytes.clone());
    // Only 128×128 and 256×256 under hicolor (no 48×48): sharper dock/shell picks.
    let joplin_native_256 = app.id == "joplin";
    const HICOLOR_SIZES: &[u32] = &[128, 256];
    for &size in HICOLOR_SIZES {
        let dir = paths.user_hicolor_apps_dir(size);
        fs::create_dir_all(&dir)?;
        let dest = dir.join(format!("{}.png", icon_name));
        let out_bytes: &[u8] = if size == 256 && joplin_native_256 {
            &bytes
        } else if size == 256 {
            &scaled_256
        } else {
            &scaled_128
        };
        fs::write(&dest, out_bytes)?;
    }
    // Legacy `appimage-control-<id>-N.png` for discovery / old .desktop references.
    if matches!(
        app.instance_strategy,
        InstanceStrategy::JoplinAltInstance
            | InstanceStrategy::ElectronClass
            | InstanceStrategy::ElectronClassIsolated
            | InstanceStrategy::QtX11
    ) {
        let legacy = instance_icon_name(&app.id, instance_index);
        if legacy != icon_name {
            for &size in HICOLOR_SIZES {
                let dir = paths.user_hicolor_apps_dir(size);
                let dest = dir.join(format!("{}.png", legacy));
                let out_bytes: &[u8] = if size == 256 && joplin_native_256 {
                    &bytes
                } else if size == 256 {
                    &scaled_256
                } else {
                    &scaled_128
                };
                fs::write(&dest, out_bytes)?;
            }
        }
    }
    Ok(Some(icon_name))
}

fn scale_png_to_square(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    use image::codecs::png::PngEncoder;
    use image::imageops::FilterType;
    use image::{ColorType, ImageEncoder};

    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    let resized = image::imageops::resize(&rgba, size, size, FilterType::Lanczos3);

    let mut out = Vec::new();
    let enc = PngEncoder::new(&mut out);
    enc.write_image(resized.as_raw(), size, size, ColorType::Rgba8.into())
        .ok()?;
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn homepage_host_parses_https() {
        assert_eq!(
            homepage_host("https://electrum.org/foo").as_deref(),
            Some("electrum.org")
        );
    }

    #[test]
    fn resolve_prefers_homepage_favicon_before_github_org_avatar() {
        let app = crate::catalog::CatalogApp {
            id: "session".into(),
            display_name: None,
            homepage: Some("https://getsession.org/".into()),
            download_url: Some(
                "https://github.com/session-foundation/session-desktop/releases/download/v1/y.AppImage"
                    .into(),
            ),
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        let url = resolve_icon_image_url(&app).expect("url");
        assert!(
            url.contains("getsession.org") && url.contains("favicons"),
            "expected google favicon for getsession.org, got {}",
            url
        );
    }

    #[test]
    fn resolve_prefers_explicit_icon_url() {
        let app = crate::catalog::CatalogApp {
            id: "x".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: crate::catalog::InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: Some("https://example.com/icon.png".into()),
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        assert_eq!(
            resolve_icon_image_url(&app).as_deref(),
            Some("https://example.com/icon.png")
        );
    }

    #[test]
    fn electron_icon_theme_name_matches_startup_wm_class_token() {
        let app = crate::catalog::CatalogApp {
            id: "session".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: InstanceStrategy::ElectronClass,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        assert_eq!(
            instance_icon_theme_name(&app, 1),
            "io.github.appimagecontrol.session.i1"
        );
    }

    #[test]
    fn electron_isolated_icon_theme_name_matches_startup_wm_class_token() {
        let app = crate::catalog::CatalogApp {
            id: "jan".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: InstanceStrategy::ElectronClassIsolated,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        assert_eq!(
            instance_icon_theme_name(&app, 2),
            "io.github.appimagecontrol.jan.i2"
        );
    }

    #[test]
    fn joplin_icon_theme_name_matches_startup_wm_class_token() {
        let app = crate::catalog::CatalogApp {
            id: "joplin".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: InstanceStrategy::JoplinAltInstance,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        assert_eq!(
            instance_icon_theme_name(&app, 2),
            "io.github.appimagecontrol.joplin.i2"
        );
    }

    #[test]
    fn feather_qt_x11_icon_theme_name_matches_startup_wm_class_token() {
        let app = crate::catalog::CatalogApp {
            id: "feather".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: InstanceStrategy::QtX11,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
        };
        assert_eq!(
            instance_icon_theme_name(&app, 3),
            "io.github.appimagecontrol.feather.i3"
        );
    }

    #[test]
    fn cryptomator_bundled_icon_reads_from_squashfs_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp
            .path()
            .join("squashfs-root/usr/share/icons/hicolor/256x256/apps");
        fs::create_dir_all(&root).unwrap();
        let png = root.join("org.cryptomator.Cryptomator.png");
        fs::write(&png, b"\x89PNG\r\n\x1a\nfake").unwrap();
        let b = super::cryptomator_bundled_icon_bytes(tmp.path()).expect("bundled png");
        assert_eq!(b, b"\x89PNG\r\n\x1a\nfake");
    }
}
