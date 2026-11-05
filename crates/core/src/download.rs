use std::fs;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use reqwest::redirect::Policy;

use crate::error::{Error, Result};

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

const MAX_REDIRECTS: usize = 10;
const CLIENT_TIMEOUT: Duration = Duration::from_secs(30);

/// True for `https`, or plain `http` to a loopback host (127.0.0.0/8, ::1, localhost).
fn scheme_host_allowed(url: &reqwest::Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => url.host_str().is_some_and(|h| {
            let h = h.trim_start_matches('[').trim_end_matches(']');
            h.eq_ignore_ascii_case("localhost")
                || h.parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }),
        _ => false,
    }
}

/// The one HTTP client builder for the core crate: fixed user agent, 30 s connect and
/// overall timeouts, and a redirect policy that only follows hops to https (or loopback
/// http), at most 10.
pub(crate) fn http_client() -> reqwest::Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CLIENT_TIMEOUT)
        .timeout(CLIENT_TIMEOUT)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() >= MAX_REDIRECTS {
                attempt.error("too many redirects")
            } else if !scheme_host_allowed(attempt.url()) {
                let msg = format!("refusing redirect to non-https URL {}", attempt.url());
                attempt.error(msg)
            } else {
                attempt.follow()
            }
        }))
        .build()
}

/// Render an error with its full `source()` chain joined by ": ".
fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cur = c.source();
    }
    out
}

fn limit_text(cap: u64) -> String {
    const GIB: u64 = 1024 * 1024 * 1024;
    if cap >= GIB && cap.is_multiple_of(GIB) {
        format!("{} GiB", cap / GIB)
    } else {
        format!("{cap} bytes")
    }
}

/// Download `url` to `dest` atomically (`.part` then rename). Overwrites existing `dest`.
///
/// When the server sends a `Content-Length` header the number of bytes written
/// is compared against it after the transfer completes. A mismatch (e.g. a
/// dropped connection that delivered a truncated body) causes the function to
/// return an error and remove the `.partdownload` file; the final `dest` path
/// is never written.
pub fn download_to_file(url: &str, dest: &Path) -> Result<()> {
    download_to_file_capped(url, dest, MAX_DOWNLOAD_BYTES)
}

/// Upper bound on a single download (4 GiB).
pub const MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub(crate) fn download_to_file_capped(url: &str, dest: &Path, cap: u64) -> Result<()> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|e| Error::DownloadFailed(format!("invalid download URL {url}: {e}")))?;
    if !scheme_host_allowed(&parsed) {
        return Err(Error::DownloadFailed(format!(
            "refusing non-https download URL {url}"
        )));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let fname = dest
        .file_name()
        .ok_or_else(|| Error::InvalidPath(dest.display().to_string()))?;
    let part = dest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{}.partdownload", fname.to_string_lossy()));

    let client = http_client().map_err(|e| Error::DownloadFailed(error_chain(&e)))?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| Error::DownloadFailed(error_chain(&e)))?;

    if !resp.status().is_success() {
        return Err(Error::DownloadFailed(format!(
            "HTTP {} from {}",
            resp.status(),
            url
        )));
    }

    // Capture Content-Length before we consume the response body.
    let content_length: Option<u64> = resp.content_length();
    if content_length.is_some_and(|n| n > cap) {
        return Err(Error::DownloadFailed(format!(
            "download exceeds the {} limit",
            limit_text(cap)
        )));
    }

    let mut f = fs::File::create(&part)?;

    // Copy body, counting bytes written. If `io::copy` itself errors (e.g. the
    // server closes the connection before delivering the full body), we still
    // need to clean up the `.partdownload` file before propagating the error.
    // Read at most cap + 1 bytes so an over-cap stream is detected without unbounded writes.
    let written = match io::copy(&mut (&mut resp).take(cap.saturating_add(1)), &mut f) {
        Ok(n) => n,
        Err(e) => {
            drop(f);
            let _ = fs::remove_file(&part);
            return Err(Error::DownloadFailed(error_chain(&e)));
        }
    };
    if written > cap {
        drop(f);
        let _ = fs::remove_file(&part);
        return Err(Error::DownloadFailed(format!(
            "download exceeds the {} limit",
            limit_text(cap)
        )));
    }
    f.flush()?;
    // Close the file handle before any rename or remove.
    drop(f);

    // Validate byte count when the server advertised Content-Length.
    if let Some(expected) = content_length {
        if written != expected {
            let _ = fs::remove_file(&part);
            return Err(Error::DownloadFailed(format!(
                "Content-Length mismatch: server said {} bytes, received {}",
                expected, written,
            )));
        }
    }

    fs::rename(&part, dest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// Serve one response: raw `header` then `body_len` bytes of 'x' (None = send no body and hold).
    fn serve_raw(header: String, body_len: usize, hold: bool) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 2048];
            let _ = s.read(&mut buf);
            let _ = s.write_all(header.as_bytes());
            let _ = s.write_all(&vec![b'x'; body_len]);
            if hold {
                thread::sleep(std::time::Duration::from_secs(20));
            }
        });
        format!("http://{addr}/")
    }

    #[test]
    fn streamed_body_over_cap_fails_and_removes_partdownload() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        // No Content-Length: body delimited by connection close.
        let url = serve_raw(
            "HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".into(),
            4096,
            false,
        );
        let err = download_to_file_capped(&url, &dest, 1024).unwrap_err();
        assert!(
            err.to_string()
                .contains("download exceeds the 1024 bytes limit"),
            "{err}"
        );
        assert!(!dest.exists());
        assert!(!tmp.path().join("out.bin.partdownload").exists());
    }

    #[test]
    fn content_length_over_cap_fails_before_reading_body() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out.bin");
        // Advertises 10000 bytes, sends none and holds the socket open: a body read would hang.
        let url = serve_raw(
            "HTTP/1.1 200 OK\r\nContent-Length: 10000\r\n\r\n".into(),
            0,
            true,
        );
        let t = std::time::Instant::now();
        let err = download_to_file_capped(&url, &dest, 1024).unwrap_err();
        assert!(t.elapsed() < std::time::Duration::from_secs(5));
        assert!(err.to_string().contains("download exceeds the"), "{err}");
        assert!(!dest.exists());
        assert!(!tmp.path().join("out.bin.partdownload").exists());
    }

    #[test]
    fn limit_text_for_real_cap_is_4_gib() {
        assert_eq!(limit_text(MAX_DOWNLOAD_BYTES), "4 GiB");
        assert_eq!(limit_text(1024), "1024 bytes");
    }
}
