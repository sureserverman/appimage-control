use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::{Error, Result};

const USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Download `url` to `dest` atomically (`.part` then rename). Overwrites existing `dest`.
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

    let mut f = fs::File::create(&part)?;
    resp.copy_to(&mut f)
        .map_err(|e| Error::DownloadFailed(e.to_string()))?;
    f.flush()?;

    fs::rename(&part, dest)?;
    Ok(())
}
