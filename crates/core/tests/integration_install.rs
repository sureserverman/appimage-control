use std::fs;

use appimage_control_core::{
    install_appimage, install_batch, list_installed, uninstall_app, AppPaths, InstallOptions,
};

#[test]
fn install_list_uninstall_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let paths = AppPaths::new(home).unwrap();

    let src = tmp.path().join("MyApp.AppImage");
    fs::write(&src, b"appimage-placeholder").unwrap();

    let res = install_appimage(&paths, &src, &InstallOptions::default()).unwrap();
    assert!(res.appimage_path.exists());
    assert!(res.desktop_path.exists());

    let list = list_installed(&paths).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "myapp");
    assert_eq!(list[0].display_name, "MyApp");

    uninstall_app(&paths, &res.desktop_path).unwrap();
    assert!(!res.appimage_path.exists());
    assert!(!res.desktop_path.exists());
    let list = list_installed(&paths).unwrap();
    assert!(list.is_empty());
}

#[test]
fn batch_second_failure_when_duplicate_name() {
    let tmp = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(tmp.path().to_path_buf()).unwrap();
    let a = tmp.path().join("Same.AppImage");
    let b = tmp.path().join("other/Same.AppImage");
    fs::create_dir_all(b.parent().unwrap()).unwrap();
    fs::write(&a, b"1").unwrap();
    fs::write(&b, b"2").unwrap();

    let results = install_batch(&paths, &[a.clone(), b], &InstallOptions::default());
    assert!(results[0].is_ok());
    assert!(results[1].is_err());
}
