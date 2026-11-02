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
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/{profile}/appimage-control"))
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
        err.contains("migrate: destination already exists") && err.contains("--on-conflict="),
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
    fs::write(&xvfb, "#!/bin/sh\nshift\nshift\nexec \"$@\"\n").expect("xvfb stub");
    let mut xp = fs::metadata(&xvfb).expect("xvfb meta").permissions();
    xp.set_mode(0o755);
    fs::set_permissions(&xvfb, xp).expect("chmod xvfb");

    // Fake portmux-port so the seed pre-flight and proxy setup don't depend
    // on an installed helper. Answers the probe (`configure-app --help`) and
    // writes a proxy key into --config, as the real command does.
    let port_helper = bin_dir.join("port-helper");
    fs::write(
        &port_helper,
        "#!/bin/sh\n\
         [ \"$1\" = configure-app ] || exit 1\n\
         [ \"$2\" = --help ] && exit 0\n\
         CFG=\n\
         prev=\n\
         for a in \"$@\"; do [ \"$prev\" = --config ] && CFG=\"$a\"; prev=\"$a\"; done\n\
         case \"$*\" in\n  \
           *--write-only*) ;;\n  \
           *) printf '{\"proxy\":1,\"socks5Port\":\"9168\"}' > \"$CFG\" ;;\n\
         esac\n\
         printf '{\"version\":2}\\n'\n",
    )
    .expect("port-helper stub");
    let mut pp = fs::metadata(&port_helper).expect("ph meta").permissions();
    pp.set_mode(0o755);
    fs::set_permissions(&port_helper, pp).expect("chmod port-helper");

    let mut path = bin_dir.as_os_str().to_os_string();
    path.push(":");
    if let Ok(p) = std::env::var("PATH") {
        path.push(p);
    }

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .env("PATH", &path)
        .env(
            appimage_control_core::ENV_PORT_HELPER_BIN,
            port_helper.as_os_str(),
        )
        .args(["--seed-settings", "feather"])
        .output()
        .expect("seed");

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Seed settings summary:") && err.contains("seeded"),
        "unexpected stderr: {err}"
    );
}

/// Runs `+feather --seed-new` with `helper` named as the portmux-port binary
/// and returns (exit ok, stderr, whether the instance is installed). Each run
/// is its own process because the helper probe is cached per process.
fn seed_new_with_helper(tmp: &Path, helper: &Path) -> (bool, String, bool) {
    let home = tmp.join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.join("catalog.json");
    fs::write(
        &catalog,
        r#"{ "apps": [ {
            "id": "feather",
            "display_name": "Feather Wallet",
            "download_url": "http://127.0.0.1:9/x.AppImage",
            "appimage_filename": "feather.AppImage"
        } ] }"#,
    )
    .expect("catalog");
    let cache = home.join(".local/share/appimages/cache/feather");
    fs::create_dir_all(&cache).expect("cache");
    fs::write(cache.join("feather.AppImage"), b"fake").expect("cached");
    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .env(appimage_control_core::ENV_PORT_HELPER_BIN, helper)
        .args(["+feather", "--seed-new"])
        .output()
        .expect("seed-new");
    let installed = home.join(".local/share/appimages/apps/feather/1").exists();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        installed,
    )
}

#[test]
fn seed_new_missing_helper_exits_failure_with_hint_instance_installed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let (ok, err, installed) = seed_new_with_helper(tmp.path(), Path::new("/nonexistent/helper"));
    assert!(!ok, "missing helper must exit non-zero: {err}");
    assert!(err.contains("sudo apt install portmux"), "hint: {err}");
    assert!(installed, "the instance stays installed: {err}");
}

#[test]
fn seed_new_old_helper_exits_failure_with_hint_instance_installed() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tmp");
    // A portmux older than configure-app: top-level --help works, the
    // subcommand does not exist.
    let helper = tmp.path().join("old-helper");
    fs::write(
        &helper,
        "#!/bin/sh\n[ \"$1\" = --help ] && exit 0\nexit 1\n",
    )
    .expect("stub");
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).expect("chmod");
    let (ok, err, installed) = seed_new_with_helper(tmp.path(), &helper);
    assert!(!ok, "old helper must exit non-zero: {err}");
    assert!(err.contains("sudo apt install portmux"), "hint: {err}");
    assert!(installed, "the instance stays installed: {err}");
}

/// The recorder only reads a pack for its defaults, so it carries on without
/// one, but it says that the pack was refused rather than acting as if none
/// exists.
#[test]
fn record_baseline_names_a_refused_pack() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);
    let seeds = tmp.path().join("seeds");
    fs::create_dir_all(&seeds).expect("seeds");
    fs::write(
        seeds.join("feather.json"),
        r#"{ "config_path": "c.json", "preset": { "a": { "from": "socks5_host" } } }"#,
    )
    .expect("pack");
    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .env(appimage_control_core::ENV_SEEDS_DIR, &seeds)
        .args(["--record-pack-baseline", "session"])
        .output()
        .expect("record");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Ignoring existing seed packs") && err.contains("proxy_setup"),
        "stderr: {err}"
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

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Backup created:") && err.contains("entries"),
        "unexpected stderr: {err}"
    );
}

// ── Mixed-op summary (task 4.3) ─────────────────────────────────────────────

fn write_two_app_catalog_sub(path: &Path) {
    fs::write(
        path,
        r#"{
  "apps": [
    {
      "id": "feather",
      "display_name": "Feather Wallet",
      "download_url": "http://127.0.0.1:9/x.AppImage",
      "appimage_filename": "feather.AppImage"
    },
    {
      "id": "session",
      "display_name": "Session",
      "download_url": "http://127.0.0.1:9/y.AppImage",
      "appimage_filename": "session.AppImage"
    }
  ]
}"#,
    )
    .expect("two-app catalog");
}

/// `+feather +session`: feather has no cache (download refused) → FAILURE.
/// Summary must appear on stderr naming feather as FAILED and session as SKIPPED.
/// Session instance directory must NOT exist on disk.
#[test]
fn mixed_ops_fail_first_summary_shows_skipped() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_two_app_catalog_sub(&catalog);

    // Pre-seed session cache so it would succeed if run.
    let session_cache = home.join(".local/share/appimages/cache/session");
    fs::create_dir_all(&session_cache).expect("session cache");
    fs::write(session_cache.join("session.AppImage"), b"fake").expect("session cache file");

    let out = Command::new(app_exe())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .args(["+feather", "+session"])
        .output()
        .expect("spawn");

    assert!(
        !out.status.success(),
        "exit must be FAILURE; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Summary:"),
        "summary line must appear on stderr; got: {err}"
    );
    assert!(
        err.contains("+feather") && err.contains("FAILED"),
        "+feather must appear as FAILED in summary; got: {err}"
    );
    assert!(
        err.contains("+session") && err.contains("SKIPPED"),
        "+session must appear as SKIPPED in summary; got: {err}"
    );
    // Skip policy: session directory must not exist.
    assert!(
        !home.join(".local/share/appimages/apps/session").exists(),
        "session must not be installed (skip policy)"
    );
}

/// `+session +feather`: session succeeds (has cache), feather fails.
/// Summary must name session OK and feather FAILED.  Exit FAILURE.
#[test]
fn mixed_ops_ok_first_summary_shows_ok_then_failed() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_two_app_catalog_sub(&catalog);

    // Pre-seed session cache so install succeeds.
    let session_cache = home.join(".local/share/appimages/cache/session");
    fs::create_dir_all(&session_cache).expect("session cache");
    fs::write(session_cache.join("session.AppImage"), b"fake").expect("session cache file");

    let out = Command::new(app_exe())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .args(["+session", "+feather"])
        .output()
        .expect("spawn");

    assert!(
        !out.status.success(),
        "exit must be FAILURE; stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("Summary:"),
        "summary line must appear; got: {err}"
    );
    assert!(
        err.contains("+session") && err.contains("OK"),
        "+session must appear as OK in summary; got: {err}"
    );
    assert!(
        err.contains("+feather") && err.contains("FAILED"),
        "+feather must appear as FAILED in summary; got: {err}"
    );
}

#[test]
fn restore_refuses_stderr_matches_preflight() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);
    // Real managed state (an installed instance file), not just an empty dir:
    // BL-003 (2026-06-30) made empty managed dirs no longer count as state.
    fs::create_dir_all(home.join(".local/share/appimages/apps/session/1")).expect("managed");
    fs::write(
        home.join(".local/share/appimages/apps/session/1/session.AppImage"),
        b"binary",
    )
    .expect("instance file");

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

/// Regression (BL-004, 2026-06-30 functional test): `--clean-cache --seed-new`
/// must NOT slip into the catalog-free clean-cache fast path and silently
/// ignore `--seed-new`. It must error that `--seed-new` needs an
/// instance-adding op, and exit non-zero.
#[test]
fn clean_cache_with_seed_new_errors_instead_of_silently_ignoring() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let catalog = tmp.path().join("catalog.json");
    write_session_catalog(&catalog);

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .args(["--clean-cache", "--seed-new"])
        .output()
        .expect("clean-cache+seed-new");

    assert!(
        !out.status.success(),
        "--clean-cache --seed-new must not silently succeed"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--seed-new requires"),
        "expected a --seed-new-requires-instance-op error, got stderr: {err}"
    );
}
