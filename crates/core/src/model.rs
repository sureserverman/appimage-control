use std::path::PathBuf;

/// A managed AppImage installation discovered from our `.desktop` entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// Stable id (slug / `X-AppImage-Control-AppId`).
    pub id: String,
    pub display_name: String,
    pub appimage_path: PathBuf,
    pub desktop_file_path: PathBuf,
    pub icon_path: Option<PathBuf>,
}
