//! Cross-repo check: seeding against the real `portmux-port configure-app`.
//!
//! Runs only when `APPIMAGE_CONTROL_PORTMUX_PORT_BIN` names a real binary,
//! and reports itself skipped otherwise. `TORRC` must point at a temp file, so
//! the host's tor config is never touched.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use appimage_control_core::{
    seed_instance_settings, AppPaths, CatalogApp, InstanceStrategy, SeedOutcome, SeedPreset,
    ENV_PORTMUX_PORT_BIN, ENV_SEEDS_DIR,
};

fn write_exec(path: &std::path::Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn seeding_proxies_a_wallet_through_the_real_binary() {
    if std::env::var_os(ENV_PORTMUX_PORT_BIN).is_none() {
        eprintln!("skipping: {ENV_PORTMUX_PORT_BIN} is not set");
        return;
    }
    let torrc = std::env::var_os("TORRC").expect("TORRC must point at a temp file");
    let torrc = std::path::PathBuf::from(torrc);

    let tmp = tempfile::tempdir().unwrap();
    let filename = "feather.AppImage";
    let rel_cfg = format!("{filename}.config/feather/settings.json");

    let seeds = tmp.path().join("seeds");
    fs::create_dir_all(&seeds).unwrap();
    fs::write(
        seeds.join("feather.json"),
        format!(
            r#"{{"config_path":"{rel_cfg}","static":{{"torPrivacyLevel":2}},"proxy_setup":{{"app":"feather"}}}}"#
        ),
    )
    .unwrap();
    std::env::set_var(ENV_SEEDS_DIR, &seeds);

    // Only this test runs in this binary, so setting these is race-free.
    std::env::set_var("PORTMUX_DISABLE_RELOAD", "1");
    std::env::set_var("PORTMUX_DROPIN_LOCK", tmp.path().join("dropin.lock"));
    std::env::set_var(
        "PORTMUX_SOCKS_APP_STORE",
        tmp.path().join("socks-apps.json"),
    );

    let bin = tmp.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    write_exec(
        &bin.join("xvfb-run"),
        "#!/bin/sh\nshift\nshift\nexec \"$@\"\n",
    );
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::set_var("PATH", format!("{}:{path}", bin.display()));

    let app = CatalogApp {
        id: "feather".into(),
        display_name: None,
        homepage: None,
        download_url: None,
        download_url_aarch64: None,
        appimage_filename: Some(filename.into()),
        instance_strategy: InstanceStrategy::None,
        instance_exec_args_template: None,
        instance_name_template: None,
        desktop_name_template: None,
        reset_config_dirs: vec![],
        reset_cache_dirs: vec![],
        reset_data_dirs: vec![],
        icon_url: None,
        wayland_app_id: None,
        desktop_pass_file_urls: None,
        instance_subdir: None,
        isolate_dbus_session: None,
        max_instances: None,
        max_instances_tooltip: None,
        category: None,
    };
    let paths = AppPaths::new(tmp.path().join("home")).unwrap();
    let dir = paths.instance_dir_for(&app, 4);
    fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join(&rel_cfg);
    write_exec(
        &dir.join(filename),
        &format!(
            "#!/bin/sh\nmkdir -p '{}'\nprintf '{{\"keep\":\"me\"}}' > '{}'\nwhile :; do sleep 1; done\n",
            cfg.parent().unwrap().display(),
            cfg.display()
        ),
    );

    let mut lines = Vec::new();
    let out = seed_instance_settings(&paths, &app, 4, SeedPreset::Ubuntu, &mut |s| {
        lines.push(s.to_string())
    })
    .unwrap_or_else(|e| panic!("seeding failed: {e}\n{lines:?}"));
    assert_eq!(out, SeedOutcome::Seeded);

    let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(v["keep"], "me", "first-run key lost: {v}");
    assert_eq!(v["torPrivacyLevel"], 2, "pack key lost: {v}");
    let port = v["socks5Port"].as_str().expect("portmux wrote no port");
    assert!(port.parse::<u16>().is_ok(), "not a port: {port}");

    let dropin = torrc.parent().unwrap().join("torrc.d/50-portmux.conf");
    let dropin = fs::read_to_string(&dropin).unwrap();
    assert!(
        dropin.contains("appimage-control-feather-4"),
        "desktop id not stamped: {dropin}"
    );
    assert!(dropin.contains(port), "port not in the drop-in: {dropin}");
}
