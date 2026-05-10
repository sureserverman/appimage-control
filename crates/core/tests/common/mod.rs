//! Shared fixtures for `appimage_control_core` integration tests (fake HTTP, paths).
#![allow(dead_code)] // Each integration test binary links only part of this module.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

/// Path to the repo `data/catalog.json` when running tests from `crates/core`.
pub fn shipped_catalog_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/catalog.json")
}

/// Path to the repo `data/seeds/` directory when running tests from `crates/core`.
pub fn shipped_seeds_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/seeds")
}

/// Bind an ephemeral port, serve one HTTP/1.1 `200` response with `body`, then close.
/// Returns a base URL such as `http://127.0.0.1:PORT/` suitable for `reqwest` GET.
pub fn serve_bytes_once(body: &[u8], content_type: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind localhost");
    let addr = listener.local_addr().expect("local_addr");
    let body = body.to_vec();
    let ct = content_type.to_string();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 2048];
        let _ = stream.read(&mut buf);
        let header = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            ct,
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(&body);
    });
    thread::sleep(Duration::from_millis(10));
    format!("http://{addr}/")
}

/// Placeholder “AppImage”: executable shell that exits 0 (good enough for paths that exec it).
pub fn write_executable_sh(path: &Path, script_body: &str) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    fs::write(path, script_body).expect("write script");
    let mut perms = fs::metadata(path).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).expect("chmod");
}
