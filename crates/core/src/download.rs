use std::fs;
use std::io::{self, Write};
use std::path::Path;

use crate::error::{Error, Result};

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Download `url` to `dest` atomically (`.part` then rename). Overwrites existing `dest`.
///
/// When the server sends a `Content-Length` header the number of bytes written
/// is compared against it after the transfer completes. A mismatch (e.g. a
/// dropped connection that delivered a truncated body) causes the function to
/// return an error and remove the `.partdownload` file; the final `dest` path
/// is never written.
pub fn download_to_file(url: &str, dest: &Path) -> Result<()> {
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

    let client = reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| Error::DownloadFailed(e.to_string()))?;

    let mut resp = client
        .get(url)
        .send()
        .map_err(|e| Error::DownloadFailed(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(Error::DownloadFailed(format!(
            "HTTP {} from {}",
            resp.status(),
            url
        )));
    }

    // Capture Content-Length before we consume the response body.
    let content_length: Option<u64> = resp.content_length();

    let mut f = fs::File::create(&part)?;

    // Copy body, counting bytes written. If `io::copy` itself errors (e.g. the
    // server closes the connection before delivering the full body), we still
    // need to clean up the `.partdownload` file before propagating the error.
    let written = match io::copy(&mut resp, &mut f) {
        Ok(n) => n,
        Err(e) => {
            drop(f);
            let _ = fs::remove_file(&part);
            return Err(Error::DownloadFailed(e.to_string()));
        }
    };
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
