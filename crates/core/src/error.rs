use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("home directory could not be determined")]
    NoHomeDir,

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("desktop file parse error: {0}")]
    DesktopParse(String),

    #[error("managed desktop entry not found: {0}")]
    DesktopNotFound(String),

    #[error("AppImage path missing from desktop entry")]
    MissingExec,

    #[error("source does not exist: {0}")]
    SourceMissing(PathBuf),

    #[error("source is not a file: {0}")]
    NotAFile(PathBuf),

    #[error("filename must end with .AppImage (case-insensitive)")]
    NotAppImageExtension,

    #[error("install target already exists: {0}")]
    TargetExists(PathBuf),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("update failed: {0}")]
    UpdateFailed(String),

    #[error("catalog entry has no download_url")]
    NoDownloadUrl,

    #[error("catalog entry has no AppImage download URL for this CPU architecture ({0})")]
    NoDownloadUrlForArch(&'static str),

    #[error("download failed: {0}")]
    DownloadFailed(String),

    #[error("{id}: cannot exceed {max} instance(s) for this app")]
    TooManyInstances { id: String, max: u32 },

    #[error("backup error: {0}")]
    Backup(String),

    #[error("restore preflight refused: {0}")]
    RestoreRefused(String),

    #[error("seed-pack signature verification failed: {0}")]
    SeedPackSignature(String),
}

pub type Result<T> = std::result::Result<T, Error>;
