use std::fs;

use appimage_control_core::{create_backup_archive, AppPaths, BackupOptions, Catalog};

fn fake_catalog(tmp: &tempfile::TempDir) -> Catalog {
    let p = tmp.path().join("catalog.json");
    fs::write(
        &p,
        r#"{
  "apps": [
    {
      "id": "session",
      "display_name": "Session",
      "download_url": "https://example.invalid/session.AppImage",
      "appimage_filename": "session.AppImage"
    }
  ]
}"#,
    )
    .expect("write catalog");
    Catalog::load_from_path(&p).expect("load catalog")
}

#[test]
fn backup_skips_unmanaged_desktop_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("write appimage");
    let apps = paths.applications_dir();
    fs::create_dir_all(&apps).expect("apps dir");
    fs::write(apps.join("appimage-control-session-1.desktop"), b"x").expect("managed desktop");
    fs::write(apps.join("random.desktop"), b"x").expect("unmanaged desktop");
    let out = tmp.path().join("b.tar.zst");
    let cat = fake_catalog(&tmp);
    create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("backup");

    let f = fs::File::open(&out).expect("open");
    let dec = zstd::stream::Decoder::new(f).expect("decoder");
    let mut ar = tar::Archive::new(dec);
    let mut names = Vec::new();
    for e in ar.entries().expect("entries") {
        let e = e.expect("entry");
        names.push(e.path().expect("path").to_string_lossy().to_string());
    }
    assert!(names
        .iter()
        .any(|n| n.ends_with("appimage-control-session-1.desktop")));
    assert!(!names.iter().any(|n| n.ends_with("random.desktop")));
}

#[test]
fn backup_inventory_tolerates_missing_optional_icons() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("write appimage");
    let out = tmp.path().join("b2.tar.zst");
    let cat = fake_catalog(&tmp);
    let rep = create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("backup");
    assert!(rep.entry_count >= 1);
}

#[cfg(unix)]
#[test]
fn backup_atomic_creation_removes_partial_temp_on_error() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    let block = tmp.path().join("not-a-dir");
    fs::write(&block, b"x").expect("write blocker");
    let out = block.join("broken.tar.zst");
    let cat = fake_catalog(&tmp);
    let err = create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).unwrap_err();
    let msg = err.to_string();
    assert!(!out.exists(), "final archive should not exist on failure");
    let tmp_name = out.with_extension("zst.tmp");
    assert!(!tmp_name.exists(), "temporary archive should be cleaned up");
    assert!(!msg.is_empty());
}
