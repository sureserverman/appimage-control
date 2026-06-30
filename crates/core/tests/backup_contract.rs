use appimage_control_core::{
    collect_backup_inventory, restore_backup_archive, AppPaths, BackupManifest, Error,
    BACKUP_FORMAT_VERSION,
};

#[test]
fn backup_inventory_includes_managed_desktops_and_icons() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();

    let apps = paths.applications_dir();
    std::fs::create_dir_all(&apps).unwrap();
    std::fs::write(apps.join("appimage-control-foo-1.desktop"), b"x").unwrap();
    std::fs::write(apps.join("other.desktop"), b"x").unwrap();

    let icons = paths.user_hicolor_apps_dir(256);
    std::fs::create_dir_all(&icons).unwrap();
    std::fs::write(icons.join("appimage-control-foo-1.png"), b"x").unwrap();
    std::fs::write(icons.join("other.png"), b"x").unwrap();

    let inv = collect_backup_inventory(&paths).unwrap();
    let inv_s: Vec<String> = inv.iter().map(|e| e.logical_path.clone()).collect();
    assert!(inv_s
        .iter()
        .any(|p| p.ends_with("applications/appimage-control-foo-1.desktop")));
    assert!(inv_s
        .iter()
        .any(|p| p.contains("appimage-control-foo-1.png")));
}

#[test]
fn manifest_version_contract_is_explicit() {
    let m = BackupManifest {
        format_version: BACKUP_FORMAT_VERSION,
        created_at_unix: 0,
        app_ids: vec![],
        backup_entries: vec![],
        entry_count: 0,
    };
    assert_eq!(m.format_version, 1);
}

#[test]
fn restore_refuses_without_force_when_managed_state_exists() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    // Real managed state: an installed instance file, not just an empty dir.
    // (BL-003 fix, 2026-06-30: empty managed dirs — e.g. purge's leftover
    // cache/ — no longer count as state; an actual file still does.)
    std::fs::create_dir_all(paths.manager_data_dir().join("apps/session/1")).unwrap();
    std::fs::write(
        paths
            .manager_data_dir()
            .join("apps/session/1/session.AppImage"),
        b"binary",
    )
    .unwrap();

    let archive = tmp.path().join("dummy.tar.zst");
    std::fs::write(&archive, b"dummy").unwrap();
    let err = restore_backup_archive(&paths, &archive, false).unwrap_err();
    match err {
        Error::RestoreRefused(msg) => assert!(msg.contains("managed state already exists")),
        other => panic!("unexpected error: {other}"),
    }
}
