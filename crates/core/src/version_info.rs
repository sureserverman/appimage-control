//! Version strings from download URLs, GitHub `releases/latest`, and per-instance metadata.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use regex::Regex;
use semver::Version;

use crate::catalog::{Catalog, CatalogApp};

/// Three-state outcome of comparing a remote release tag against an installed version string.
///
/// `Unknown` is returned when either side cannot be parsed as semver (e.g. `latest`, `unstable`,
/// `v2024.1`, date-style tags).  Callers that would normally skip an update because “remote is
/// not newer” **must not** claim “up to date” when the result is `Unknown` — they should surface
/// the remote tag with a “(check manually)” marker instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionComparison {
    /// `remote` parsed as semver and is strictly greater than `recorded`.
    Newer,
    /// Both parsed as semver and `remote` is not strictly greater (equal or older).
    NotNewer,
    /// At least one side could not be parsed as semver; comparison is inconclusive.
    Unknown,
}

/// Three-state comparison of a remote release tag against a recorded installed version.
///
/// Returns [`VersionComparison::Unknown`] when either string fails to parse as semver, so callers
/// can distinguish “definitely not newer” from “cannot determine” and avoid falsely reporting
/// “up to date” for non-semver tags such as `latest`, `unstable`, or date-based releases.
///
/// Semver-vs-semver behavior is identical to the original bool function.
pub fn remote_release_comparison(remote: &str, recorded: &str) -> VersionComparison {
    match (
        Version::parse(recorded.trim()),
        Version::parse(remote.trim()),
    ) {
        (Ok(v_rec), Ok(v_rem)) => {
            if v_rem > v_rec {
                VersionComparison::Newer
            } else {
                VersionComparison::NotNewer
            }
        }
        _ => VersionComparison::Unknown,
    }
}

/// True when both `remote` and `recorded` parse as [`semver::Version`] and `remote` is strictly greater.
///
/// Used so “Update installed” does not skip when the embedded catalog URL is satisfied but GitHub
/// `releases/latest` reports a newer tag than the install.
///
/// **Note:** returns `false` for non-semver tags on either side.  Use [`remote_release_comparison`]
/// when you need to distinguish “not newer” from “unparseable” (e.g. to avoid silently hiding
/// updates for `latest`, date-style, or other non-semver tags).
pub fn remote_release_newer_than_installed(remote: &str, recorded: &str) -> bool {
    matches!(
        remote_release_comparison(remote, recorded),
        VersionComparison::Newer
    )
}

/// GitHub `releases/latest` tag plus a chosen `.AppImage` asset URL (when the API lists one we can select).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GithubReleaseHint {
    pub tag: String,
    pub appimage_download_url: Option<String>,
}

/// True when `hint` includes a download URL and its semver tag is strictly greater than both
/// `pinned` and `recorded` (when each parses). Used to pull a newer AppImage than the catalog URL.
pub fn should_use_github_latest_asset(
    hint: &GithubReleaseHint,
    pinned: Option<&str>,
    recorded: Option<&str>,
) -> bool {
    if hint.appimage_download_url.is_none() {
        return false;
    }
    let Ok(hv) = Version::parse(hint.tag.trim()) else {
        return false;
    };
    let mut floor: Option<Version> = None;
    for s in [pinned.map(str::trim), recorded.map(str::trim)]
        .into_iter()
        .flatten()
    {
        if let Ok(v) = Version::parse(s) {
            floor = match floor {
                None => Some(v),
                Some(f) => Some(f.max(v)),
            };
        }
    }
    let Some(floor) = floor else {
        return false;
    };
    hv > floor
}

pub(crate) fn github_asset_prefix_hint_from_catalog_url(catalog_url: &str) -> Option<String> {
    let path = catalog_url.split('?').next()?;
    let file = path.rsplit('/').next()?;
    let stem = file
        .strip_suffix(".AppImage")
        .or_else(|| file.strip_suffix(".appimage"))?;
    let re = version_token_regex();
    let last = re.find_iter(stem).last()?;
    let idx = last.start();
    if idx == 0 {
        return None;
    }
    let prefix = stem[..idx].trim_end_matches('-');
    if prefix.is_empty() {
        None
    } else {
        Some(prefix.to_string())
    }
}

fn pick_github_release_appimage_url(
    assets: &[serde_json::Value],
    catalog_download_url: Option<&str>,
) -> Option<String> {
    let hint_prefix = catalog_download_url.and_then(github_asset_prefix_hint_from_catalog_url);
    let arch_needles: &[&str] = match std::env::consts::ARCH {
        "x86_64" => &["x86_64", "amd64", "x64"],
        "aarch64" => &["aarch64", "arm64"],
        _ => &[],
    };

    let mut best: Option<(i32, String)> = None;
    for a in assets {
        let name = a.get("name").and_then(|x| x.as_str())?;
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".appimage") {
            continue;
        }
        if lower.contains("windows") || lower.contains("darwin") || lower.contains(".exe") {
            continue;
        }
        let url = a
            .get("browser_download_url")
            .and_then(|x| x.as_str())?
            .to_string();

        let mut score: i32 = 0;
        if !arch_needles.is_empty() {
            let mut arch_hit = false;
            for n in arch_needles {
                if lower.contains(&n.to_ascii_lowercase()) {
                    arch_hit = true;
                    score += 20;
                }
            }
            if !(arch_hit || lower.contains("linux") || lower.contains("portable")) {
                continue;
            }
        }
        if lower.contains("linux") {
            score += 3;
        }
        if let Some(ref hp) = hint_prefix {
            let hpl = hp.to_ascii_lowercase();
            let nl = name.to_ascii_lowercase();
            if nl.starts_with(&hpl) {
                score += 50;
            } else if nl.contains(&hpl) {
                score += 25;
            }
        }

        let better = match &best {
            None => true,
            Some((s, _)) => score > *s,
        };
        if better {
            best = Some((score, url));
        }
    }

    if best.is_some() {
        return best.map(|(_, u)| u);
    }

    // Fallback: first Linux-looking AppImage (no arch match on exotic tags).
    for a in assets {
        let name = a.get("name").and_then(|x| x.as_str())?;
        let lower = name.to_ascii_lowercase();
        if !lower.ends_with(".appimage") {
            continue;
        }
        if lower.contains("windows") || lower.contains("darwin") {
            continue;
        }
        let url = a
            .get("browser_download_url")
            .and_then(|x| x.as_str())?
            .to_string();
        return Some(url);
    }
    None
}

fn fetch_github_latest_release_hint(
    owner: &str,
    repo: &str,
    catalog_download_url: Option<&str>,
) -> Option<GithubReleaseHint> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(concat!("appimage-control/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let api = format!(
        "https://api.github.com/repos/{}/{}/releases/latest",
        owner, repo
    );
    let resp = client.get(api).send().ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let v: serde_json::Value = resp.json().ok()?;
    let tag_raw = v.get("tag_name").and_then(|t| t.as_str())?;
    let tag = normalize_version_tag(tag_raw);
    let assets = v.get("assets").and_then(|a| a.as_array())?;
    let picked = pick_github_release_appimage_url(assets.as_slice(), catalog_download_url);
    Some(GithubReleaseHint {
        tag,
        appimage_download_url: picked,
    })
}

/// For each catalog app: GitHub `releases/latest` tag and best-effort `.AppImage` asset URL.
pub fn fetch_github_release_hints(catalog: &Catalog) -> Vec<Option<GithubReleaseHint>> {
    catalog
        .apps
        .iter()
        .map(|app| {
            let url = app.resolved_download_url().ok()?;
            let (owner, repo) = github_owner_repo_from_url(url)?;
            fetch_github_latest_release_hint(&owner, &repo, Some(url))
        })
        .collect()
}

/// Latest GitHub release tag per app (same order as [`fetch_github_release_hints`] tags only).
pub fn fetch_remote_latest_versions(catalog: &Catalog) -> Vec<Option<String>> {
    fetch_github_release_hints(catalog)
        .into_iter()
        .map(|h| h.map(|x| x.tag))
        .collect()
}

/// Written next to the AppImage so we know what was installed without probing the binary.
pub const INSTANCE_VERSION_FILENAME: &str = ".appimage-control-version";

/// One-line GitHub release tag for the AppImage file in the same per-app catalog cache directory.
pub const GITHUB_CACHE_TAG_FILENAME: &str = ".appimage-control-github-tag";

/// Normalize GitHub release tags for display/comparison (`v` prefix, scoped npm-style tags).
pub fn normalize_github_release_tag(s: &str) -> String {
    normalize_version_tag(s)
}

/// True when two tag strings refer to the same normalized release label.
pub fn github_release_tags_equivalent(a: &str, b: &str) -> bool {
    normalize_github_release_tag(a) == normalize_github_release_tag(b)
}

/// First line of [`INSTANCE_VERSION_FILENAME`] in a per-app catalog cache directory (if present).
pub fn read_catalog_cache_recorded_version(cache_dir: &Path) -> Option<String> {
    let p = cache_dir.join(INSTANCE_VERSION_FILENAME);
    let s = fs::read_to_string(&p).ok()?;
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

fn read_github_cache_tag_sidecar_line(cache_dir: &Path) -> Option<String> {
    let p = cache_dir.join(GITHUB_CACHE_TAG_FILENAME);
    let s = fs::read_to_string(&p).ok()?;
    let line = s.lines().next()?.trim();
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

/// Release label for the cached AppImage used when provisioning instances.
///
/// Reads [`.appimage-control-version`](INSTANCE_VERSION_FILENAME) and
/// [`.appimage-control-github-tag`](GITHUB_CACHE_TAG_FILENAME). When both disagree, prefers the
/// **newer semver** when both parse; otherwise prefers the catalog version file over a loose tag
/// sidecar (stale GitHub tags are cleared on catalog re-download, but manual layouts can still race).
pub fn read_catalog_cache_release_for_install(cache_dir: &Path) -> Option<String> {
    let inst = read_catalog_cache_recorded_version(cache_dir);
    let gh = read_github_cache_tag_sidecar_line(cache_dir);
    match (&inst, &gh) {
        (None, None) => None,
        (Some(a), None) => Some(a.clone()),
        (None, Some(b)) => Some(b.clone()),
        (Some(a), Some(b)) => {
            if github_release_tags_equivalent(a, b) {
                return Some(a.clone());
            }
            let na = normalize_github_release_tag(a);
            let nb = normalize_github_release_tag(b);
            match (Version::parse(&na), Version::parse(&nb)) {
                (Ok(va), Ok(vb)) => Some(if va >= vb { na } else { nb }),
                (Ok(_), Err(_)) => Some(na),
                (Err(_), Ok(_)) => Some(nb),
                _ => Some(a.clone()),
            }
        }
    }
}

/// Records which release the cached AppImage in `cache_dir` represents (catalog pin or GitHub tag).
pub fn write_catalog_cache_recorded_version(
    cache_dir: &Path,
    version: &str,
) -> crate::error::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let p = cache_dir.join(INSTANCE_VERSION_FILENAME);
    let normalized = normalize_github_release_tag(version);
    fs::write(&p, format!("{normalized}\n"))?;
    Ok(())
}

/// True when `recorded` and `tag` denote the same release (normalized strings or equal semver).
pub fn recorded_release_matches_tag(recorded: &str, tag: &str) -> bool {
    if github_release_tags_equivalent(recorded, tag) {
        return true;
    }
    let a = normalize_github_release_tag(tag);
    let b = normalize_github_release_tag(recorded);
    match (Version::parse(&a), Version::parse(&b)) {
        (Ok(va), Ok(vb)) => va == vb,
        _ => false,
    }
}

/// True when `cache_dir/filename` exists and we can prove it matches `hint_tag`: recorded version
/// file (same as instance [`.appimage-control-version`](INSTANCE_VERSION_FILENAME)), GitHub tag
/// sidecar, or a semver embedded in the basename.
pub fn github_catalog_cache_looks_fresh(
    cache_dir: &Path,
    appimage_filename: &str,
    hint_tag: &str,
) -> bool {
    let img = cache_dir.join(appimage_filename);
    if !img.is_file() {
        return false;
    }
    if fs::metadata(&img)
        .ok()
        .map(|m| m.len() == 0)
        .unwrap_or(true)
    {
        return false;
    }
    if let Some(ref v) = read_catalog_cache_recorded_version(cache_dir) {
        if recorded_release_matches_tag(v, hint_tag) {
            return true;
        }
    }
    let tag_path = cache_dir.join(GITHUB_CACHE_TAG_FILENAME);
    if let Ok(recorded) = fs::read_to_string(&tag_path) {
        let line = recorded.lines().next().unwrap_or("").trim();
        if !line.is_empty() && recorded_release_matches_tag(line, hint_tag) {
            return true;
        }
    }
    if let Some(fv) = version_from_appimage_filename(appimage_filename) {
        if recorded_release_matches_tag(&fv, hint_tag) {
            return true;
        }
    }
    false
}

/// Persist which GitHub release tag the cached AppImage in `cache_dir` corresponds to.
pub fn write_github_cache_tag_sidecar(cache_dir: &Path, tag: &str) -> crate::error::Result<()> {
    fs::create_dir_all(cache_dir)?;
    let p = cache_dir.join(GITHUB_CACHE_TAG_FILENAME);
    let normalized = normalize_github_release_tag(tag);
    fs::write(&p, format!("{normalized}\n"))?;
    Ok(())
}

fn version_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\d+\.\d+\.\d+(?:\.\d+)?|\d+\.\d+").expect("version regex"))
}

/// Best-effort version segment from the download URL filename (matches pinned catalog releases).
pub fn version_from_download_url(url: &str) -> Option<String> {
    let path = url.split('?').next()?;
    let file = path.rsplit('/').next()?;
    let base = file
        .strip_suffix(".AppImage")
        .or_else(|| file.strip_suffix(".appimage"))?;
    let re = version_token_regex();
    let mut last = None;
    for m in re.find_iter(base) {
        last = Some(m.as_str().to_string());
    }
    last.map(|s| normalize_version_tag(&s))
}

/// Pinned version implied by the catalog URL for this host architecture.
pub fn catalog_pinned_version(app: &CatalogApp) -> Option<String> {
    app.resolved_download_url()
        .ok()
        .and_then(version_from_download_url)
}

/// True when the catalog URL pin and the instance-recorded version refer to the same release.
pub fn catalog_pin_matches_recorded(pinned: &str, recorded: &str) -> bool {
    if github_release_tags_equivalent(pinned, recorded) {
        return true;
    }
    let np = normalize_github_release_tag(pinned);
    let nr = normalize_github_release_tag(recorded);
    match (Version::parse(&np), Version::parse(&nr)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Version token extracted from an AppImage filename (same rules as [`version_from_download_url`]).
pub fn version_from_appimage_filename(filename: &str) -> Option<String> {
    let url = format!("https://h/{filename}");
    version_from_download_url(&url)
}

/// Whether an existing cache file named `filename` is acceptable for `pinned` without re-downloading.
///
/// When the basename does not embed a semver (e.g. generic `session.AppImage`), returns `true` so
/// we do not treat the cache as stale—we cannot compare filename to `pinned` and would otherwise
/// re-download the same URL forever.
///
/// When a version can be parsed from the filename: returns `true` if it matches `pinned` or is
/// semver-`>=` `pinned` (newer cache than embedded catalog); `false` if semver-`<` `pinned`.
pub fn catalog_cache_fresh_for_pinned(filename: &str, pinned: &str) -> bool {
    let Some(cv) = version_from_appimage_filename(filename) else {
        return true;
    };
    if cv == pinned {
        return true;
    }
    match (Version::parse(&cv), Version::parse(pinned)) {
        (Ok(vc), Ok(vp)) => vc >= vp,
        _ => false,
    }
}

fn normalize_version_tag(s: &str) -> String {
    let t = s.trim().trim_start_matches('v');
    // Monorepo / npm-style tags like `@standardnotes/desktop@3.201.21` — show only the semver tail
    // so "Latest" matches the pinned URL version format.
    if let Some(pos) = t.rfind('@') {
        let tail = t[pos + 1..].trim().trim_start_matches('v');
        if version_token_regex().is_match(tail) {
            return tail.to_string();
        }
    }
    t.to_string()
}

/// `https://github.com/owner/repo/...` → (owner, repo without .git).
pub fn github_owner_repo_from_url(url: &str) -> Option<(String, String)> {
    let rest = url
        .split_once("github.com/")
        .map(|(_, r)| r)
        .or_else(|| url.split_once("github.com:").map(|(_, r)| r))?;
    let mut parts = rest.splitn(3, '/');
    let owner = parts.next()?.trim();
    let repo = parts.next()?.trim();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    let repo = repo.trim_end_matches(".git");
    Some((owner.to_string(), repo.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_version_from_github_style_filenames() {
        let u = "https://github.com/feather-wallet/feather/releases/download/2.8.1/feather-2.8.1.AppImage";
        assert_eq!(version_from_download_url(u).as_deref(), Some("2.8.1"));
    }

    #[test]
    fn parses_session_style() {
        let u = "https://github.com/x/y/releases/download/v1.17.17/session-desktop-linux-x86_64-1.17.17.AppImage";
        assert_eq!(version_from_download_url(u).as_deref(), Some("1.17.17"));
    }

    #[test]
    fn parses_electrum_path() {
        let u = "https://download.electrum.org/4.7.2/electrum-4.7.2-x86_64.AppImage";
        assert_eq!(version_from_download_url(u).as_deref(), Some("4.7.2"));
    }

    #[test]
    fn normalizes_scoped_npm_style_release_tag() {
        assert_eq!(
            super::normalize_version_tag("@standardnotes/desktop@3.201.21"),
            "3.201.21"
        );
        assert_eq!(super::normalize_version_tag("v3.201.21"), "3.201.21");
    }

    #[test]
    fn github_repo_from_release_url() {
        let u =
            "https://github.com/session-foundation/session-desktop/releases/download/v1/x.AppImage";
        assert_eq!(
            github_owner_repo_from_url(u),
            Some((
                "session-foundation".to_string(),
                "session-desktop".to_string()
            ))
        );
    }

    #[test]
    fn version_from_appimage_filename_matches_url_rules() {
        assert_eq!(
            version_from_appimage_filename("feather-2.8.1.AppImage").as_deref(),
            Some("2.8.1")
        );
    }

    #[test]
    fn catalog_cache_fresh_for_pinned_semver_ordering() {
        assert!(super::catalog_cache_fresh_for_pinned(
            "feather-2.8.1.AppImage",
            "2.8.1"
        ));
        assert!(!super::catalog_cache_fresh_for_pinned(
            "feather-2.8.0.AppImage",
            "2.8.1"
        ));
        assert!(super::catalog_cache_fresh_for_pinned(
            "feather-2.9.0.AppImage",
            "2.8.1"
        ));
    }

    #[test]
    fn catalog_cache_fresh_opaque_basename_not_treated_as_stale() {
        assert!(super::catalog_cache_fresh_for_pinned(
            "session.AppImage",
            "1.17.17"
        ));
    }

    #[test]
    fn remote_release_newer_than_installed_semver() {
        assert!(super::remote_release_newer_than_installed(
            "1.18.0", "1.17.17"
        ));
        assert!(!super::remote_release_newer_than_installed(
            "1.17.17", "1.17.17"
        ));
        assert!(!super::remote_release_newer_than_installed(
            "1.17.16", "1.17.17"
        ));
    }

    // --- Three-state VersionComparison tests ---

    #[test]
    fn version_comparison_newer() {
        assert_eq!(
            super::remote_release_comparison("2.0.0", "1.0.0"),
            super::VersionComparison::Newer
        );
    }

    #[test]
    fn version_comparison_not_newer_equal() {
        assert_eq!(
            super::remote_release_comparison("1.0.0", "1.0.0"),
            super::VersionComparison::NotNewer
        );
    }

    #[test]
    fn version_comparison_not_newer_older() {
        assert_eq!(
            super::remote_release_comparison("0.9.0", "1.0.0"),
            super::VersionComparison::NotNewer
        );
    }

    #[test]
    fn version_comparison_unknown_non_semver_remote() {
        // Non-semver remote tag must yield Unknown, not NotNewer.
        assert_eq!(
            super::remote_release_comparison("latest", "1.0.0"),
            super::VersionComparison::Unknown
        );
        assert_eq!(
            super::remote_release_comparison("unstable", "1.0.0"),
            super::VersionComparison::Unknown
        );
        assert_eq!(
            super::remote_release_comparison("v2024.1", "1.0.0"),
            super::VersionComparison::Unknown
        );
    }

    #[test]
    fn version_comparison_unknown_non_semver_installed() {
        assert_eq!(
            super::remote_release_comparison("1.0.0", "not-a-version"),
            super::VersionComparison::Unknown
        );
    }

    #[test]
    fn version_comparison_unknown_both_garbage() {
        assert_eq!(
            super::remote_release_comparison("nightly", "bleeding-edge"),
            super::VersionComparison::Unknown
        );
    }

    /// The legacy bool function must preserve its original semver-vs-semver semantics.
    #[test]
    fn bool_wrapper_still_returns_false_for_non_semver() {
        // old contract: non-semver returns false (not newer)
        assert!(!super::remote_release_newer_than_installed(
            "latest", "1.0.0"
        ));
        assert!(!super::remote_release_newer_than_installed(
            "1.0.0", "bad-ver"
        ));
    }

    #[test]
    fn github_asset_prefix_strips_trailing_version_from_catalog_url() {
        let u = "https://github.com/session-foundation/session-desktop/releases/download/v1.17.17/session-desktop-linux-x86_64-1.17.17.AppImage";
        assert_eq!(
            super::github_asset_prefix_hint_from_catalog_url(u).as_deref(),
            Some("session-desktop-linux-x86_64")
        );
    }

    #[test]
    fn should_use_github_latest_when_url_and_tag_ahead() {
        let h = super::GithubReleaseHint {
            tag: "1.18.0".into(),
            appimage_download_url: Some(
                "https://github.com/a/b/releases/download/v1.18.0/x.AppImage".into(),
            ),
        };
        assert!(super::should_use_github_latest_asset(
            &h,
            Some("1.17.17"),
            Some("1.17.17")
        ));
        assert!(!super::should_use_github_latest_asset(
            &h,
            Some("1.18.0"),
            Some("1.18.0")
        ));
    }

    #[test]
    fn github_catalog_cache_fresh_requires_sidecar_match() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("session.AppImage"), b"x").unwrap();
        assert!(!super::github_catalog_cache_looks_fresh(
            dir,
            "session.AppImage",
            "1.18.0"
        ));
        super::write_github_cache_tag_sidecar(dir, "v1.18.0").unwrap();
        assert!(super::github_catalog_cache_looks_fresh(
            dir,
            "session.AppImage",
            "1.18.0"
        ));
        assert!(!super::github_catalog_cache_looks_fresh(
            dir,
            "session.AppImage",
            "1.19.0"
        ));
    }

    #[test]
    fn cache_release_for_install_newer_semver_wins_when_markers_differ() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join(super::INSTANCE_VERSION_FILENAME), b"1.17.17\n").unwrap();
        super::write_github_cache_tag_sidecar(dir, "1.18.0").unwrap();
        assert_eq!(
            super::read_catalog_cache_release_for_install(dir).as_deref(),
            Some("1.18.0")
        );
    }

    #[test]
    fn cache_release_for_install_prefers_catalog_when_newer_than_github_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join(super::INSTANCE_VERSION_FILENAME), b"1.19.0\n").unwrap();
        super::write_github_cache_tag_sidecar(dir, "1.18.0").unwrap();
        assert_eq!(
            super::read_catalog_cache_release_for_install(dir).as_deref(),
            Some("1.19.0")
        );
    }

    #[test]
    fn catalog_pin_matches_recorded_accepts_v_prefix() {
        assert!(super::catalog_pin_matches_recorded("2.8.1", "v2.8.1"));
        assert!(super::catalog_pin_matches_recorded("v2.8.1", "2.8.1"));
        assert!(!super::catalog_pin_matches_recorded("2.8.1", "2.8.2"));
    }

    #[test]
    fn github_catalog_cache_fresh_accepts_recorded_version_without_github_sidecar() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("session.AppImage"), b"x").unwrap();
        super::write_catalog_cache_recorded_version(dir, "1.18.0").unwrap();
        assert!(super::github_catalog_cache_looks_fresh(
            dir,
            "session.AppImage",
            "v1.18.0"
        ));
        assert!(!super::github_catalog_cache_looks_fresh(
            dir,
            "session.AppImage",
            "1.19.0"
        ));
    }
}
