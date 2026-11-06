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

    let mut path = bin_dir.as_os_str().to_os_string();
    path.push(":");
    if let Ok(p) = std::env::var("PATH") {
        path.push(p);
    }

    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("APPIMAGE_CONTROL_CATALOG", &catalog)
        .env("PATH", &path)
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

/// The recorder only reads a pack for its defaults, so it carries on without
/// one, but it says that the pack was refused rather than acting as if none
/// exists. A refused pack is skipped by name and the others load
/// (WF-SEED-001 step 2), so the warning names the pack and the migration.
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
        r#"{ "config_path": "c.json", "preset": { "a": { "format": "x" } } }"#,
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
        err.contains("feather.json")
            && err.contains("skipping this pack")
            && err.contains("`static`"),
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

// --- Task 3.3/3.4: --allow-unverified -------------------------------------------------------

/// Serve `body` to the next `times` HTTP requests on a loopback port; returns the URL.
fn serve_bytes_times(body: &'static [u8], times: usize) -> String {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        for _ in 0..times {
            let Ok((mut s, _)) = listener.accept() else {
                return;
            };
            let mut buf = [0u8; 4096];
            let _ = s.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = s.write_all(head.as_bytes());
            let _ = s.write_all(body);
        }
    });
    format!("http://{addr}/vapp.AppImage")
}

fn write_vapp_catalog(path: &Path, url: &str, sha256: Option<&str>) {
    let pin = sha256
        .map(|h| format!(r#","sha256":"{h}","sha256_aarch64":"{h}""#))
        .unwrap_or_default();
    fs::write(
        path,
        format!(
            r#"{{"apps":[{{"id":"vapp","display_name":"VApp","download_url":"{url}","download_url_aarch64":"{url}","appimage_filename":"vapp.AppImage"{pin}}}]}}"#
        ),
    )
    .expect("catalog");
}

fn run_vapp(tmp: &Path, extra: &[&str]) -> (bool, String) {
    let home = tmp.join("home");
    fs::create_dir_all(&home).expect("home");
    let mut args = vec!["+vapp"];
    args.extend_from_slice(extra);
    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("APPIMAGE_CONTROL_CATALOG", tmp.join("catalog.json"))
        .args(&args)
        .output()
        .expect("run");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn allow_unverified_needed_when_no_hash_is_published() {
    let tmp = tempfile::tempdir().expect("tmp");
    let url = serve_bytes_times(b"\x7fELF no hash anywhere", 1);
    write_vapp_catalog(&tmp.path().join("catalog.json"), &url, None);
    let (ok, err) = run_vapp(tmp.path(), &[]);
    assert!(!ok, "must fail without the flag: {err}");
    assert!(
        err.contains("no published sha256") && err.contains("--allow-unverified"),
        "stderr must name the flag: {err}"
    );
    let cached = tmp
        .path()
        .join("home/.local/share/appimages/cache/vapp/vapp.AppImage");
    assert!(
        !cached.exists(),
        "nothing may be installed without the flag"
    );
}

#[test]
fn allow_unverified_flag_installs_and_records_it() {
    let tmp = tempfile::tempdir().expect("tmp");
    let url = serve_bytes_times(b"\x7fELF no hash anywhere", 1);
    write_vapp_catalog(&tmp.path().join("catalog.json"), &url, None);
    let (ok, err) = run_vapp(tmp.path(), &["--allow-unverified"]);
    assert!(ok, "the flag must allow the install: {err}");
    let side = tmp
        .path()
        .join("home/.local/share/appimages/cache/vapp/.appimage-control-sha256");
    let side = fs::read_to_string(&side).expect("sidecar");
    assert!(side.trim().ends_with("unverified user-allowed"), "{side}");
}

#[test]
fn allow_unverified_never_overrides_a_mismatch() {
    let tmp = tempfile::tempdir().expect("tmp");
    let url = serve_bytes_times(b"\x7fELF not what was published", 2);
    let wrong = "ab".repeat(32);
    write_vapp_catalog(&tmp.path().join("catalog.json"), &url, Some(&wrong));
    for extra in [&[][..], &["--allow-unverified"][..]] {
        let (ok, err) = run_vapp(tmp.path(), extra);
        assert!(!ok, "a mismatch must fail ({extra:?}): {err}");
        assert!(
            err.contains("does not match the expected sha256") && err.contains(&wrong),
            "({extra:?}) {err}"
        );
    }
}

/// `-update` must fail (non-zero) when an instance's new download does not match its sha256.
#[test]
fn allow_unverified_update_mismatch_fails_the_run() {
    let tmp = tempfile::tempdir().expect("tmp");
    // Served twice: the install, then the update's re-download.
    let url = serve_bytes_times(b"\x7fELF installed bytes", 2);
    let cat = tmp.path().join("catalog.json");
    write_vapp_catalog(&cat, &url, None);
    let (ok, err) = run_vapp(tmp.path(), &["--allow-unverified"]);
    assert!(ok, "setup install: {err}");
    // The catalog now pins a hash the served bytes do not have: the cached blob fails its
    // check, the update downloads again, and the download mismatches.
    write_vapp_catalog(&cat, &url, Some(&"cd".repeat(32)));
    let home = tmp.path().join("home");
    let out = Command::new(app_exe().as_os_str())
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("APPIMAGE_CONTROL_CATALOG", &cat)
        .args(["-update"])
        .output()
        .expect("update");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "-update must fail on a mismatch: {err}"
    );
    assert!(err.contains("does not match the expected sha256"), "{err}");
}

// --- --check-updates (login update check) ---------------------------------------------------

/// A catalog with one app whose pinned, non-GitHub URL carries version 2.0.0, so the newest
/// release is known with no network lookup.
fn write_pinned_fixture_catalog(path: &Path) {
    fs::write(
        path,
        r#"{
  "apps": [
    {
      "id": "fixture",
      "display_name": "Fixture",
      "download_url": "https://example.invalid/releases/fixture-2.0.0.AppImage",
      "appimage_filename": "fixture.AppImage",
      "instance_strategy": "electron_class"
    }
  ]
}"#,
    )
    .expect("catalog");
}

/// Instance 1 of `fixture`, recorded at `version`, plus a cached download.
fn install_pinned_fixture(home: &Path, version: &str) {
    let inst = home.join(".local/share/appimages/apps/fixture/1");
    fs::create_dir_all(&inst).expect("instance dir");
    fs::write(inst.join("fixture.AppImage"), b"#!/bin/sh\n").expect("appimage");
    fs::write(
        inst.join(".appimage-control-version"),
        format!("{version}\n"),
    )
    .expect("version");
    let cache = home.join(".local/share/appimages/cache/fixture");
    fs::create_dir_all(&cache).expect("cache");
    fs::write(cache.join("fixture.AppImage"), b"#!/bin/sh\n").expect("cached");
}

/// Every path under `root` with its length and modification time, sorted.
fn tree_listing(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<String>) {
        let Ok(rd) = fs::read_dir(dir) else { return };
        for ent in rd.flatten() {
            let p = ent.path();
            let meta = fs::symlink_metadata(&p).expect("meta");
            out.push(format!(
                "{} {} {:?}",
                p.display(),
                meta.len(),
                meta.modified().ok()
            ));
            if meta.is_dir() {
                walk(&p, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn run_check_updates(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(app_exe().as_os_str())
        .env("HOME", home)
        .env_remove("APPIMAGE_CONTROL_CATALOG")
        .args(args)
        .output()
        .expect("spawn --check-updates")
}

#[test]
fn check_updates_lists_a_newer_pinned_release_and_writes_nothing() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let catalog = tmp.path().join("catalog.json");
    write_pinned_fixture_catalog(&catalog);
    install_pinned_fixture(&home, "1.0.0");
    let before = tree_listing(&home);

    let out = run_check_updates(
        &home,
        &["--check-updates", "--catalog", catalog.to_str().unwrap()],
    );

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "fixture: 1.0.0 -> 2.0.0\n"
    );
    assert_eq!(
        tree_listing(&home),
        before,
        "the check changed files under HOME"
    );
}

#[test]
fn check_updates_prints_nothing_when_current() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    let catalog = tmp.path().join("catalog.json");
    write_pinned_fixture_catalog(&catalog);
    install_pinned_fixture(&home, "2.0.0");
    let before = tree_listing(&home);

    let out = run_check_updates(
        &home,
        &["--check-updates", "--catalog", catalog.to_str().unwrap()],
    );

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(
        tree_listing(&home),
        before,
        "the check changed files under HOME"
    );
}

#[test]
fn check_updates_with_a_missing_catalog_exits_1() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let missing = tmp.path().join("no-such-catalog.json");

    let out = run_check_updates(
        &home,
        &[
            "--check-updates",
            "--notify",
            "--catalog",
            missing.to_str().unwrap(),
        ],
    );

    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("no-such-catalog.json") && !err.contains("unknown option"),
        "unexpected stderr: {err}"
    );
}

#[test]
fn check_updates_notify_needs_check_updates() {
    let tmp = tempfile::tempdir().expect("tmp");
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).expect("home");

    let out = run_check_updates(&home, &["--notify"]);

    assert_eq!(out.status.code(), Some(1));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--notify requires --check-updates"),
        "unexpected stderr: {err}"
    );
}
