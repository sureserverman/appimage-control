use std::fs;

use appimage_control_core::{
    create_backup_archive, restore_backup_archive, AppPaths, BackupOptions, Catalog,
};

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

fn seed_managed_state(paths: &AppPaths) {
    fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("write appimage");
    fs::create_dir_all(paths.applications_dir()).expect("apps dir");
    fs::write(
        paths
            .applications_dir()
            .join("appimage-control-session-1.desktop"),
        b"[Desktop Entry]\nName=Session\n",
    )
    .expect("write desktop");
}

#[test]
fn restore_into_empty_state() {
    let src = tempfile::tempdir().expect("src tempdir");
    let src_paths = AppPaths::new(src.path().to_path_buf()).expect("src paths");
    seed_managed_state(&src_paths);
    let cat = fake_catalog(&src);
    let archive = src.path().join("backup.tar.zst");
    create_backup_archive(&src_paths, &cat, &archive, &BackupOptions::default()).expect("backup");

    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    restore_backup_archive(&dst_paths, &archive, false).expect("restore");

    assert!(dst_paths
        .manager_data_dir()
        .join("apps/session/1/session.AppImage")
        .is_file());
    assert!(dst_paths
        .applications_dir()
        .join("appimage-control-session-1.desktop")
        .is_file());
}

#[test]
fn restore_refusal_when_state_exists_without_force() {
    let src = tempfile::tempdir().expect("src tempdir");
    let src_paths = AppPaths::new(src.path().to_path_buf()).expect("src paths");
    seed_managed_state(&src_paths);
    let cat = fake_catalog(&src);
    let archive = src.path().join("backup.tar.zst");
    create_backup_archive(&src_paths, &cat, &archive, &BackupOptions::default()).expect("backup");

    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    seed_managed_state(&dst_paths);
    let err = restore_backup_archive(&dst_paths, &archive, false).unwrap_err();
    assert!(err.to_string().contains("managed state already exists"));
}

#[test]
fn forced_restore_replaces_managed_state_only() {
    let src = tempfile::tempdir().expect("src tempdir");
    let src_paths = AppPaths::new(src.path().to_path_buf()).expect("src paths");
    seed_managed_state(&src_paths);
    fs::write(
        src_paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"new-binary",
    )
    .expect("write new");
    let cat = fake_catalog(&src);
    let archive = src.path().join("backup.tar.zst");
    create_backup_archive(&src_paths, &cat, &archive, &BackupOptions::default()).expect("backup");

    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    seed_managed_state(&dst_paths);
    let unmanaged = dst.path().join("outside.txt");
    fs::write(&unmanaged, b"keep").expect("write unmanaged");

    restore_backup_archive(&dst_paths, &archive, true).expect("forced restore");
    let bytes = fs::read(
        dst_paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
    )
    .expect("read restored");
    assert_eq!(bytes, b"new-binary");
    assert_eq!(fs::read(&unmanaged).expect("read unmanaged"), b"keep");
}

#[test]
fn invalid_archive_rejected_before_writes() {
    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    let bad = dst.path().join("bad.tar.zst");
    fs::write(&bad, b"not-zstd").expect("write bad");
    let err = restore_backup_archive(&dst_paths, &bad, false).unwrap_err();
    assert!(!dst_paths.manager_data_dir().join("apps").exists());
    assert!(
        err.to_string().contains("zstd decode init failed") || err.to_string().contains("IO error")
    );
}

/// A truncated (corrupt) archive must not destroy pre-existing managed state.
/// Specifically: with `force=true`, the old state must remain fully intact when
/// extraction fails before the swap phase is reached.
#[test]
fn truncated_archive_leaves_existing_state_intact() {
    // Build source state and archive.
    let src = tempfile::tempdir().expect("src tempdir");
    let src_paths = AppPaths::new(src.path().to_path_buf()).expect("src paths");
    seed_managed_state(&src_paths);
    let cat = fake_catalog(&src);
    let archive = src.path().join("backup.tar.zst");
    create_backup_archive(&src_paths, &cat, &archive, &BackupOptions::default()).expect("backup");

    // Truncate the archive to 60% of its original length.
    {
        let orig_len = fs::metadata(&archive).expect("meta").len();
        let truncated_len = (orig_len as f64 * 0.6) as u64;
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&archive)
            .expect("open for truncate");
        f.set_len(truncated_len).expect("truncate");
    }

    // Set up a destination that already has managed state, including a desktop file.
    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    fs::create_dir_all(dst_paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        dst_paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"original-binary",
    )
    .expect("write original appimage");
    fs::create_dir_all(dst_paths.applications_dir()).expect("apps dir");
    let managed_desktop = dst_paths
        .applications_dir()
        .join("appimage-control-session-1.desktop");
    fs::write(&managed_desktop, b"[Desktop Entry]\nName=OriginalSession\n")
        .expect("write managed desktop");

    // Restore with force=true must fail (archive is corrupt).
    let result = restore_backup_archive(&dst_paths, &archive, true);
    assert!(result.is_err(), "expected Err from truncated archive, got Ok");

    // Pre-existing managed state must be fully intact.
    let appimage = dst_paths
        .manager_data_dir()
        .join("apps/session/1/session.AppImage");
    assert!(
        appimage.is_file(),
        "AppImage should still exist after failed restore"
    );
    assert_eq!(
        fs::read(&appimage).expect("read appimage"),
        b"original-binary",
        "AppImage content must be the original, not overwritten"
    );
    assert!(
        managed_desktop.is_file(),
        "managed .desktop file should still exist after failed restore"
    );
    assert_eq!(
        fs::read(&managed_desktop).expect("read desktop"),
        b"[Desktop Entry]\nName=OriginalSession\n",
        ".desktop content must be the original"
    );
}

/// A valid archive can still be restored after the extract-then-swap refactor.
/// This complements the existing restore_into_empty_state test by verifying
/// the forced-overwrite path with the new ordering as well.
#[test]
fn valid_archive_forced_restore_succeeds() {
    let src = tempfile::tempdir().expect("src tempdir");
    let src_paths = AppPaths::new(src.path().to_path_buf()).expect("src paths");
    fs::create_dir_all(src_paths.manager_data_dir().join("apps/session/1")).expect("mkdir");
    fs::write(
        src_paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"fresh-binary",
    )
    .expect("write appimage");
    fs::create_dir_all(src_paths.applications_dir()).expect("apps dir");
    fs::write(
        src_paths
            .applications_dir()
            .join("appimage-control-session-1.desktop"),
        b"[Desktop Entry]\nName=Session\n",
    )
    .expect("write desktop");
    let cat = fake_catalog(&src);
    let archive = src.path().join("backup.tar.zst");
    create_backup_archive(&src_paths, &cat, &archive, &BackupOptions::default()).expect("backup");

    let dst = tempfile::tempdir().expect("dst tempdir");
    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    // Pre-seed stale state to verify it gets replaced.
    seed_managed_state(&dst_paths);

    restore_backup_archive(&dst_paths, &archive, true).expect("forced restore of valid archive");

    assert_eq!(
        fs::read(
            dst_paths
                .manager_data_dir()
                .join("apps/session/1/session.AppImage")
        )
        .expect("read restored"),
        b"fresh-binary"
    );
    assert!(
        dst_paths
            .applications_dir()
            .join("appimage-control-session-1.desktop")
            .is_file()
    );
}

#[test]
fn path_traversal_rejected() {
    let dst = tempfile::tempdir().expect("dst tempdir");
    let bad = dst.path().join("traversal.tar.zst");
    let file = fs::File::create(&bad).expect("create archive");
    let enc = zstd::stream::Encoder::new(file, 3).expect("enc");
    let mut tar = tar::Builder::new(enc);
    let manifest = br#"{"format_version":1,"created_at_unix":0,"app_ids":[],"backup_entries":[{"logical_path":"../escape","source_path":"/tmp/x","size_bytes":1,"sha256":null}],"entry_count":1}"#;
    let mut mh = tar::Header::new_gnu();
    mh.set_size(manifest.len() as u64);
    mh.set_mode(0o644);
    mh.set_cksum();
    tar.append_data(&mut mh, "manifest.json", &manifest[..])
        .expect("append manifest");
    let snapshot = br#"{"apps":[]}"#;
    let mut sh = tar::Header::new_gnu();
    sh.set_size(snapshot.len() as u64);
    sh.set_mode(0o644);
    sh.set_cksum();
    tar.append_data(&mut sh, "catalog-snapshot.json", &snapshot[..])
        .expect("append snapshot");
    let payload = b"x";
    let mut ph = tar::Header::new_gnu();
    ph.set_size(payload.len() as u64);
    ph.set_mode(0o644);
    ph.set_cksum();
    tar.append_data(&mut ph, "manager-data/safe", &payload[..])
        .expect("append safe payload");
    let enc = tar.into_inner().expect("into enc");
    enc.finish().expect("finish enc");

    let dst_paths = AppPaths::new(dst.path().to_path_buf()).expect("dst paths");
    let err = restore_backup_archive(&dst_paths, &bad, false).unwrap_err();
    assert!(err.to_string().contains("unsafe component"));
}
