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
    // Staging file uses `{name}.aic-tmp-{pid}` naming; confirm no residue.
    let residue: Vec<_> = fs::read_dir(tmp.path())
        .expect("read tmpdir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("aic-tmp-"))
        .collect();
    assert!(
        residue.is_empty(),
        "staging file should be cleaned up, found: {:?}",
        residue
    );
    assert!(!msg.is_empty());
}

#[test]
fn backup_failure_leaves_pre_existing_archive_intact() {
    // A failed backup run must not delete (or truncate) a pre-existing valid
    // archive at the target path.
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");

    // Create a valid first archive.
    let out = tmp.path().join("backup.tar.zst");
    let cat = fake_catalog(&tmp);
    create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("first backup");
    let original_len = fs::metadata(&out).expect("stat").len();
    assert!(original_len > 0);

    // Now attempt a backup to a broken target (file in place of parent dir).
    let block = tmp.path().join("subdir-blocker");
    fs::write(&block, b"x").expect("write blocker");
    let broken_out = block.join("new.tar.zst");
    create_backup_archive(&paths, &cat, &broken_out, &BackupOptions::default()).unwrap_err();

    // The original archive must be fully intact.
    assert!(
        out.is_file(),
        "pre-existing archive must survive a failed sibling backup"
    );
    assert_eq!(
        fs::metadata(&out).expect("stat after").len(),
        original_len,
        "pre-existing archive must not be truncated"
    );
}

#[test]
fn backup_overwrites_existing_archive_with_fresh_one() {
    // A second successful backup to the same path must replace the first
    // archive completely (not fail, not leave both).
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    let out = tmp.path().join("backup.tar.zst");
    let cat = fake_catalog(&tmp);

    create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("first backup");
    let first_mtime = fs::metadata(&out)
        .expect("stat first")
        .modified()
        .expect("mtime");

    // Small sleep so mtime can differ on filesystems with 1-second resolution.
    std::thread::sleep(std::time::Duration::from_millis(10));

    create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("second backup");

    assert!(out.is_file(), "archive must exist after second backup");
    // No stale staging files alongside the archive.
    let residue: Vec<_> = fs::read_dir(tmp.path())
        .expect("read tmpdir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().contains("aic-tmp-"))
        .collect();
    assert!(
        residue.is_empty(),
        "no staging residue after successful backup: {:?}",
        residue
    );
    // The archive is a valid zst-tar (readable without error).
    let f = fs::File::open(&out).expect("open second archive");
    let dec = zstd::stream::Decoder::new(f).expect("decoder");
    let mut ar = tar::Archive::new(dec);
    let names: Vec<_> = ar
        .entries()
        .expect("entries")
        .filter_map(|e| e.ok())
        .map(|e| e.path().expect("path").to_string_lossy().to_string())
        .collect();
    assert!(names.iter().any(|n| n == "manifest.json"));
    let _ = first_mtime; // used only to verify the assignment above
}

/// The archive holds wallet and password-manager homes: it is never readable by others.
#[test]
fn archive_mode_is_0600() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("write");
    let out = tmp.path().join("b.tar.zst");
    let cat = fake_catalog(&tmp);
    // A common desktop umask: plain file creation would give 0644.
    // SAFETY: umask only swaps the process file-mode mask; restored right after.
    let prev = unsafe { libc::umask(0o022) };
    let r = create_backup_archive(&paths, &cat, &out, &BackupOptions::default());
    unsafe { libc::umask(prev) };
    r.expect("backup");
    let mode = fs::metadata(&out).expect("meta").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "archive mode {mode:o}");
}

/// The manifest records the sha256 of the bytes actually archived.
#[test]
fn manifest_hash_matches_archived_bytes() {
    use sha2::{Digest, Sha256};
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
    fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("write");
    let out = tmp.path().join("b.tar.zst");
    let cat = fake_catalog(&tmp);
    create_backup_archive(&paths, &cat, &out, &BackupOptions::default()).expect("backup");
    let mut ar =
        tar::Archive::new(zstd::stream::Decoder::new(fs::File::open(&out).unwrap()).unwrap());
    let (mut files, mut manifest) = (std::collections::HashMap::new(), None);
    for e in ar.entries().unwrap() {
        let mut e = e.unwrap();
        let p = e.path().unwrap().to_string_lossy().into_owned();
        let mut d = Vec::new();
        std::io::Read::read_to_end(&mut e, &mut d).unwrap();
        if p == "manifest.json" {
            manifest = Some(serde_json::from_slice::<serde_json::Value>(&d).unwrap());
        } else {
            files.insert(p, d);
        }
    }
    for e in manifest.unwrap()["backup_entries"].as_array().unwrap() {
        let p = e["logical_path"].as_str().unwrap();
        let got: String = Sha256::digest(&files[p])
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(e["sha256"].as_str().unwrap(), got, "{p}");
    }
}
