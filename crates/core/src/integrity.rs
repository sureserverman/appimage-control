//! Where the expected SHA-256 of a catalog download comes from.
//!
//! Order: a hash pinned in the catalog wins; otherwise, for GitHub release assets, the `digest`
//! GitHub publishes for the asset; otherwise nothing. Any failure to learn a digest is reported
//! as [`ExpectedDigest::Unavailable`] and never as a digest.

use crate::catalog::CatalogApp;
use crate::version_info::{github_api_base, GithubAsset};

/// The digest a downloaded file should have, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedDigest {
    /// Pinned in `catalog.json` (64 lower-case hex).
    Catalog(String),
    /// Published by the GitHub release-asset API (64 lower-case hex).
    GithubDigest(String),
    /// No trustworthy digest is known; the string says why.
    Unavailable(String),
}

/// True when `s` is exactly 64 lower-case hex characters.
pub(crate) fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            if !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Encode as one URL path segment (everything but unreserved characters is escaped, so `/` too).
fn percent_encode_segment(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `https://github.com/<owner>/<repo>/releases/download/<tag...>/<asset>` ->
/// `(owner, repo, tag, asset)`, tag and asset percent-decoded.
fn parse_github_asset_url(url: &str) -> Option<(String, String, String, String)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let rest = rest.split(['?', '#']).next()?;
    let mut parts = rest.splitn(5, '/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next()? != "releases" || parts.next()? != "download" {
        return None;
    }
    let (tag, asset) = parts.next()?.rsplit_once('/')?;
    // Owner and repo go into the API path as-is: allow only GitHub's own name characters, so the
    // lookup provably targets the repository the download comes from.
    let gh_name = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !gh_name(owner) || !gh_name(repo) || tag.is_empty() || asset.is_empty() {
        return None;
    }
    // A release asset name never holds `/`; one that decodes to it is not a real asset URL.
    let asset = percent_decode(asset).filter(|a| !a.contains('/'))?;
    Some((
        owner.to_string(),
        repo.to_string(),
        percent_decode(tag)?,
        asset,
    ))
}

fn github_asset_digest(
    owner: &str,
    repo: &str,
    tag: &str,
    asset: &str,
) -> Result<Option<String>, String> {
    let client = crate::download::http_client().map_err(|e| format!("http client: {e}"))?;
    let api = format!(
        "{}/repos/{}/{}/releases/tags/{}",
        github_api_base(),
        owner,
        repo,
        percent_encode_segment(tag)
    );
    let resp = client
        .get(api)
        .send()
        .map_err(|e| format!("github api request failed: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("github api returned HTTP {status}"));
    }
    let v: serde_json::Value = resp
        .json()
        .map_err(|e| format!("github api response is not valid JSON: {e}"))?;
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "github api response has no assets list".to_string())?;
    let found = assets
        .iter()
        .filter_map(|a| serde_json::from_value::<GithubAsset>(a.clone()).ok())
        .find(|a| a.name == asset)
        .ok_or_else(|| format!("asset {asset:?} not in release {tag:?}"))?;
    let Some(d) = found.digest else {
        return Ok(None);
    };
    match d.strip_prefix("sha256:") {
        Some(hex) if is_sha256_hex(hex) => Ok(Some(hex.to_string())),
        _ => Err(format!(
            "github digest {d:?} is not sha256:<64 lower-case hex>"
        )),
    }
}

/// Expected SHA-256 of the file at `url` for `arch` (`std::env::consts::ARCH` style).
pub fn expected_digest(app: &CatalogApp, url: &str, arch: &str) -> ExpectedDigest {
    let pinned = if arch == "aarch64" {
        app.sha256_aarch64.as_deref()
    } else {
        app.sha256.as_deref()
    };
    if let Some(h) = pinned {
        if is_sha256_hex(h) {
            return ExpectedDigest::Catalog(h.to_string());
        }
        return ExpectedDigest::Unavailable("catalog sha256 is malformed".to_string());
    }
    let Some((owner, repo, tag, asset)) = parse_github_asset_url(url) else {
        return ExpectedDigest::Unavailable("no catalog sha256 and not a GitHub asset".to_string());
    };
    match github_asset_digest(&owner, &repo, &tag, &asset) {
        Ok(Some(hex)) => ExpectedDigest::GithubDigest(hex),
        Ok(None) => ExpectedDigest::Unavailable("github has no digest for this asset".to_string()),
        Err(why) => ExpectedDigest::Unavailable(why),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse_catalog_json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    const GOOD: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const CATALOG_HASH: &str = "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    const GH: &str = "https://github.com/o/r/releases/download/v1.0/x-1.0.AppImage";

    fn env_lock() -> MutexGuard<'static, ()> {
        static L: OnceLock<Mutex<()>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Loopback API fixture: answers every request with `(status, body)` and records paths.
    fn serve(status: u16, body: String) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen2 = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { return };
                let mut buf = [0u8; 4096];
                let n = s.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                seen2.lock().unwrap().push(path);
                let resp = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = s.write_all(resp.as_bytes());
            }
        });
        (base, seen)
    }

    fn release_json(asset: &str, digest: &str) -> String {
        format!(
            r#"{{"tag_name":"t","assets":[{{"name":"other.AppImage","digest":"sha256:{GOOD}"}},{{"name":"{asset}","digest":{digest}}}]}}"#
        )
    }

    fn good_digest_json() -> String {
        format!("\"sha256:{GOOD}\"")
    }

    fn app(extra: &str) -> CatalogApp {
        let json = format!(r#"{{"apps":[{{"id":"t"{extra}}}]}}"#);
        parse_catalog_json(&json).unwrap().apps.remove(0)
    }

    fn with_base<T>(base: &str, f: impl FnOnce() -> T) -> T {
        let _g = env_lock();
        std::env::set_var("APPIMAGE_CONTROL_GITHUB_API_BASE", base);
        let r = f();
        std::env::remove_var("APPIMAGE_CONTROL_GITHUB_API_BASE");
        r
    }

    #[test]
    fn catalog_sha256_wins_over_github() {
        let (base, seen) = serve(200, release_json("x-1.0.AppImage", &good_digest_json()));
        let a = app(&format!(r#","sha256":"{CATALOG_HASH}""#));
        let got = with_base(&base, || expected_digest(&a, GH, "x86_64"));
        assert_eq!(got, ExpectedDigest::Catalog(CATALOG_HASH.into()));
        assert!(seen.lock().unwrap().is_empty(), "must not query GitHub");
    }

    #[test]
    fn catalog_aarch64_hash_used_only_on_aarch64() {
        let (base, _) = serve(500, String::new());
        let a = app(&format!(r#","sha256_aarch64":"{CATALOG_HASH}""#));
        assert_eq!(
            with_base(&base, || expected_digest(&a, GH, "aarch64")),
            ExpectedDigest::Catalog(CATALOG_HASH.into())
        );
        assert!(matches!(
            with_base(&base, || expected_digest(&a, GH, "x86_64")),
            ExpectedDigest::Unavailable(_)
        ));
    }

    #[test]
    fn pinned_github_url_resolves_via_tags_and_matches_asset_by_name() {
        let (base, seen) = serve(200, release_json("x-1.0.AppImage", &good_digest_json()));
        let got = with_base(&base, || expected_digest(&app(""), GH, "x86_64"));
        assert_eq!(got, ExpectedDigest::GithubDigest(GOOD.into()));
        assert_eq!(*seen.lock().unwrap(), vec!["/repos/o/r/releases/tags/v1.0"]);
    }

    #[test]
    fn standard_notes_tag_is_decoded_then_encoded_as_one_segment() {
        let name = "standard-notes-3.201.21-linux-x86_64.AppImage";
        let (base, seen) = serve(200, release_json(name, &good_digest_json()));
        let url = format!(
            "https://github.com/standardnotes/app/releases/download/%40standardnotes/desktop%403.201.21/{name}"
        );
        let got = with_base(&base, || expected_digest(&app(""), &url, "x86_64"));
        assert_eq!(got, ExpectedDigest::GithubDigest(GOOD.into()));
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["/repos/standardnotes/app/releases/tags/%40standardnotes%2Fdesktop%403.201.21"]
        );
    }

    #[test]
    fn null_digest_is_unavailable() {
        let (base, _) = serve(200, release_json("x-1.0.AppImage", "null"));
        assert_eq!(
            with_base(&base, || expected_digest(&app(""), GH, "x86_64")),
            ExpectedDigest::Unavailable("github has no digest for this asset".into())
        );
    }

    #[test]
    fn api_errors_are_unavailable_never_a_digest() {
        for status in [403u16, 404, 500] {
            let (base, _) = serve(status, release_json("x-1.0.AppImage", &good_digest_json()));
            let got = with_base(&base, || expected_digest(&app(""), GH, "x86_64"));
            assert!(
                matches!(got, ExpectedDigest::Unavailable(_)),
                "HTTP {status}: {got:?}"
            );
        }
    }

    #[test]
    fn connection_error_is_unavailable() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let got = with_base(&base, || expected_digest(&app(""), GH, "x86_64"));
        assert!(matches!(got, ExpectedDigest::Unavailable(_)), "{got:?}");
    }

    #[test]
    fn bad_json_missing_asset_and_malformed_digest_are_unavailable() {
        let cases = [
            "not json".to_string(),
            release_json("different.AppImage", &good_digest_json()),
            release_json("x-1.0.AppImage", "\"sha256:ABC\""),
            release_json("x-1.0.AppImage", &format!("\"md5:{GOOD}\"")),
            release_json(
                "x-1.0.AppImage",
                &format!("\"sha256:{}\"", GOOD.to_uppercase()),
            ),
        ];
        for body in cases {
            let (base, _) = serve(200, body.clone());
            let got = with_base(&base, || expected_digest(&app(""), GH, "x86_64"));
            assert!(
                matches!(got, ExpectedDigest::Unavailable(_)),
                "{body}: {got:?}"
            );
        }
    }

    #[test]
    fn non_github_url_without_catalog_hash_is_unavailable() {
        let got = expected_digest(&app(""), "https://example.com/a.AppImage", "x86_64");
        assert_eq!(
            got,
            ExpectedDigest::Unavailable("no catalog sha256 and not a GitHub asset".into())
        );
    }

    /// The URL parser's reject paths: each yields `Unavailable` and no request reaches the API.
    #[test]
    fn malformed_or_foreign_urls_never_query_and_are_unavailable() {
        let (base, seen) = serve(200, release_json("a", &good_digest_json()));
        let a = app("");
        for url in [
            "http://github.com/o/r/releases/download/t/a",
            "https://GitHub.com/o/r/releases/download/t/a",
            "https://github.com.evil.com/o/r/releases/download/t/a",
            "https://github.com/o/r/releases/download//a",
            "https://github.com/o/r/releases/download/t%zz/a",
            "https://github.com/o/r/releases/download/t%4/a",
            "https://github.com/o/r/releases/download/t%ff/a",
            "https://github.com/o/r/releases/download/t/%+1",
            "https://github.com/o/r/archive/t/a",
            "https://github.com/o%2Fx/r/releases/download/t/a",
            "https://github.com/../r/releases/download/t/a",
        ] {
            let got = with_base(&base, || expected_digest(&a, url, "x86_64"));
            assert!(
                matches!(got, ExpectedDigest::Unavailable(_)),
                "{url}: {got:?}"
            );
        }
        assert!(
            seen.lock().unwrap().is_empty(),
            "{:?}",
            seen.lock().unwrap()
        );
    }

    /// An asset name holding an encoded `/` can never match a release asset.
    #[test]
    fn encoded_slash_in_asset_is_unavailable() {
        let (base, _) = serve(200, release_json("a/b", &good_digest_json()));
        let got = with_base(&base, || {
            expected_digest(
                &app(""),
                "https://github.com/o/r/releases/download/t/a%2Fb",
                "x86_64",
            )
        });
        assert!(matches!(got, ExpectedDigest::Unavailable(_)), "{got:?}");
    }

    /// A query string is ignored: the asset still resolves.
    #[test]
    fn query_string_is_ignored() {
        let (base, _) = serve(200, release_json("a", &good_digest_json()));
        let got = with_base(&base, || {
            expected_digest(
                &app(""),
                "https://github.com/o/r/releases/download/t/a?x=1",
                "x86_64",
            )
        });
        assert_eq!(got, ExpectedDigest::GithubDigest(GOOD.into()));
    }

    /// The API base override only accepts https or loopback http.
    #[test]
    fn api_base_override_refuses_remote_http() {
        let _g = env_lock();
        std::env::set_var("APPIMAGE_CONTROL_GITHUB_API_BASE", "http://evil.example");
        let base = crate::version_info::github_api_base();
        std::env::remove_var("APPIMAGE_CONTROL_GITHUB_API_BASE");
        assert_eq!(base, "https://api.github.com");
    }
}
