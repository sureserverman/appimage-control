//! Smoke test: subprocess side effects without external services.

use std::fs;
use std::process::Command;

#[test]
fn shell_child_creates_expected_file() {
    let tmp = tempfile::tempdir().unwrap();
    let marker = tmp.path().join("ran.txt");
    let path_str = marker.to_string_lossy();
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("printf ok > '{}'", path_str))
        .status()
        .expect("spawn sh");
    assert!(status.success());
    assert_eq!(fs::read_to_string(&marker).unwrap(), "ok");
}
