//! CLI golden-style checks via the real `appimage-control` binary (stderr/stdout strings).
//! Complements in-process `run_cli_ops` tests in `src/cli.rs`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn app_exe() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_appimage_control")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_appimage-control").map(PathBuf::from))
        .unwrap_or_else(|| {
            // `cargo test` usually sets `CARGO_BIN_EXE_*`; fall back for other runners.
            let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../../target/{profile}/appimage-control"))
        })
}

fn write_session_catalog(path: &Path) {
    fs::write(
        path,
        r#"{
  "apps": [
    {
      "id": "session",
      "display_name": "Session",
      "download_url": "http://127.0.0.1:9/unused.AppImage",
      "appimage_filename": "session.AppImage",
      "instance_strategy": "electron_class"
    }
  ]
}"#,
    )
    .expect("catalog");
}

/// Ensures instance 1 exists (cache + `+session`), then marks `*.home` as occupied for migrate conflict.
fn install_session_via_cli(tmp: &Path, home: &Path, catalog: &Path) {
    let cache = home.join(".local/share/appimages/cache/session");
    fs::create_dir_all(&cache).expect("cache");
    fs::write(cache.join("session.AppImage"), b"fake").expect("cache bin");

    let st = Command::new(app_exe().as_os_str())
        .env("HOME", home)
        .env("APPIMAGE_CONTROL_CATALOG", catalog)
        .arg("+session")
        .current_dir(tmp)
        .status()
        .expect("spawn +session");
    assert!(st.success(), "+session failed");

    let home_tree = home.join(".local/share/appimages/apps/session/1/session.AppImage.home");
    fs::create_dir_all(&home_tree).expect("home tree");
    fs::write(home_tree.join("occupied.txt"), b"1").expect("occupy home");
}

#[test]
fn migrate_y_without_on_conflict_aborts_when_destination_occupied() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);

    install_session_via_cli(tmp.path(), &home, &catalog);

    let mig_root = tmp.path().join("migrate_cwd");
    fs::create_dir_all(mig_root.join("session")).expect("legacy session dir");

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .current_dir(&mig_root)
        .args(["--migrate", "-y"])
        .output()
        .expect("spawn migrate");

    assert!(
        !out.status.success(),
        "expected migrate to fail, got stdout={} stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("migrate: destination already exists")
            && err.contains("--on-conflict="),
        "unexpected stderr: {err}"
    );
}

#[test]
fn migrate_y_on_conflict_skip_exits_success_when_destination_occupied() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);

    install_session_via_cli(tmp.path(), &home, &catalog);

    let mig_root = tmp.path().join("migrate_cwd");
    fs::create_dir_all(mig_root.join("session")).expect("legacy session dir");

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .current_dir(&mig_root)
        .args(["--migrate", "-y", "--on-conflict=skip"])
        .output()
        .expect("spawn migrate");

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("Skipped") || combined.contains("skipped"),
        "expected skip progress, got {combined}"
    );
}

#[test]
fn seed_settings_prints_summary_line() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    fs::write(
        &catalog,
        r#"{
  "apps": [
    {
      "id": "feather",
      "display_name": "Feather Wallet",
      "download_url": "http://127.0.0.1:9/x.AppImage",
      "appimage_filename": "feather.AppImage"
    }
  ]
}"#,
    )
    .expect("catalog");

    let dir = home.join(".local/share/appimages/apps/feather/1");
    fs::create_dir_all(&dir).expect("dir");
    let appimage = dir.join("feather.AppImage");
    let cfg = dir.join("feather.AppImage.config/feather/settings.json");
    use std::os::unix::fs::PermissionsExt;
    fs::write(
        &appimage,
        format!(
            "#!/bin/sh\nset -eu\nmkdir -p \"{}\"\nprintf '{{}}' > \"{}\"\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
            cfg.parent().expect("parent").display(),
            cfg.display()
        ),
    )
    .expect("script");
    let mut perms = fs::metadata(&appimage).expect("meta").permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&appimage, perms).expect("chmod");

    let bin_dir = home.join("bin");
    fs::create_dir_all(&bin_dir).expect("bin");
    let xvfb = bin_dir.join("xvfb-run");
    fs::write(
        &xvfb,
        "#!/bin/sh\nshift\nshift\nexec \"$@\"\n",
    )
    .expect("xvfb stub");
    let mut xp = fs::metadata(&xvfb).expect("xvfb meta").permissions();
    xp.set_mode(0o755);
    fs::set_permissions(&xvfb, xp).expect("chmod xvfb");

    let mut path = bin_dir.as_os_str().to_os_string();
    path.push(":");
    if let Ok(p) = std::env::var("PATH") {
        path.push(p);
    }

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .env("PATH", &path)
        .args([
            "--seed-settings",
            "feather",
            "--seed-preset",
            "ubuntu",
        ])
        .output()
        .expect("seed");

    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Seed settings summary:") && err.contains("seeded"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn backup_prints_created_line() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);
    let archive = tmp.path().join("bk.tar.zst");

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .args(["--backup", archive.to_str().expect("utf8")])
        .output()
        .expect("backup");

    assert!(out.status.success(), "stderr={}", String::from_utf8_lossy(&out.stderr));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Backup created:") && err.contains("entries"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn restore_refuses_stderr_matches_preflight() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);
    fs::create_dir_all(home.join(".local/share/appimages/apps")).expect("managed");

    let dummy = tmp.path().join("dummy.tar.zst");
    fs::write(&dummy, b"not-a-real-archive").expect("dummy");

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .args(["--restore-backup", dummy.to_str().expect("utf8")])
        .output()
        .expect("restore");

    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("restore preflight refused") || err.contains("managed state already exists"),
        "unexpected stderr: {err}"
    );
}
