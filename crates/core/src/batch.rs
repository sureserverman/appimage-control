use std::path::PathBuf;

use crate::error::Error;
use crate::install::{install_appimage, InstallOptions, InstallResult};
use crate::paths::AppPaths;

/// Install multiple AppImages in order; each path gets an independent `Result`.
pub fn install_batch(
    paths: &AppPaths,
    sources: &[PathBuf],
    options: &InstallOptions,
) -> Vec<std::result::Result<InstallResult, Error>> {
    let mut results = Vec::with_capacity(sources.len());
    for src in sources {
        results.push(install_appimage(paths, src.as_path(), options));
    }
    results
}

#[cfg(test)]
mod tests {
    use assert_fs::prelude::*;

    use super::*;
    use crate::paths::AppPaths;

    #[test]
    fn batch_reports_per_file() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
        let a = tmp.child("A.AppImage");
        let b = tmp.child("B.AppImage");
        a.write_binary(b"x").unwrap();
        b.write_binary(b"y").unwrap();

        let res = install_batch(
            &paths,
            &[a.path().to_path_buf(), b.path().to_path_buf()],
            &InstallOptions::default(),
        );
        assert_eq!(res.len(), 2);
        assert!(res[0].is_ok());
        assert!(res[1].is_ok());
    }
}
