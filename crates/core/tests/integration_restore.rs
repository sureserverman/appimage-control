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
    assert!(
        result.is_err(),
        "expected Err from truncated archive, got Ok"
    );

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

// ---------------------------------------------------------------------------
// Hostile-archive path-traversal tests
//
// Each test builds a crafted archive in-memory, then asserts that restore
// either returns an Err *or* (if it somehow returns Ok) did not write any
// file outside the managed directory trees.  A sentinel file placed at
// `home/escape.txt` (sibling of the managed dirs) is checked to be untouched
// in every case.
// ---------------------------------------------------------------------------

/// Build a raw 512-byte POSIX ustar tar header block for a regular file with
/// the given `name` (up to 99 bytes, null-padded), `size` (content length),
/// and `content` written after. Returns the header block bytes.
///
/// We bypass the `tar` crate builder deliberately so that we can embed
/// hostile path strings (`../`, absolute `/`, interleaved `./..`) that the
/// builder would reject. This models what a real adversary would do: craft
/// archive bytes by hand.
fn raw_tar_header_block(name: &[u8], size: u64, typeflag: u8) -> [u8; 512] {
    let mut block = [0u8; 512];
    // name: bytes 0-99
    let name_len = name.len().min(99);
    block[..name_len].copy_from_slice(&name[..name_len]);
    // mode: bytes 100-107  "0000644\0"
    block[100..108].copy_from_slice(b"0000644\0");
    // uid: bytes 108-115
    block[108..116].copy_from_slice(b"0000000\0");
    // gid: bytes 116-123
    block[116..124].copy_from_slice(b"0000000\0");
    // size: bytes 124-135 (octal, 11 digits + NUL)
    let size_str = format!("{:011o}\0", size);
    block[124..136].copy_from_slice(size_str.as_bytes());
    // mtime: bytes 136-147
    block[136..148].copy_from_slice(b"00000000000\0");
    // checksum placeholder (bytes 148-155): fill with spaces first
    block[148..156].copy_from_slice(b"        ");
    // typeflag: byte 156
    block[156] = typeflag; // b'0' = regular file
                           // magic: bytes 257-262 "ustar"
    block[257..263].copy_from_slice(b"ustar ");
    // version: bytes 263-264 " \0" (GNU style)
    block[263..265].copy_from_slice(b" \0");
    // compute and write checksum
    let cksum: u32 = block.iter().map(|&b| b as u32).sum();
    let cksum_str = format!("{:06o}\0 ", cksum);
    block[148..156].copy_from_slice(cksum_str.as_bytes());
    block
}

/// Pad `data` bytes to a multiple of 512 bytes (tar block boundary).
fn tar_pad_to_block(data: &[u8]) -> Vec<u8> {
    let remainder = data.len() % 512;
    if remainder == 0 {
        data.to_vec()
    } else {
        let mut v = data.to_vec();
        v.resize(data.len() + (512 - remainder), 0u8);
        v
    }
}

/// Write a single tar entry (header + padded content) into `buf`.
fn append_raw_tar_entry(buf: &mut Vec<u8>, name: &[u8], content: &[u8]) {
    let header = raw_tar_header_block(name, content.len() as u64, b'0');
    buf.extend_from_slice(&header);
    buf.extend_from_slice(&tar_pad_to_block(content));
}

/// Build a minimal *valid* archive (correct manifest + catalog-snapshot +
/// one safe payload under `manager-data/sentinel-ok`) and additionally
/// appends a raw hostile tar entry whose header path bytes are exactly
/// `hostile_entry_path_bytes`.
///
/// The manifest lists only the safe entry, so manifest validation passes.
/// The hostile path is what `ent.path()` returns during extraction — this
/// tests whether the extraction loop's sanitization catches it.
fn build_archive_with_hostile_tar_entry(
    archive_path: &std::path::Path,
    hostile_entry_path_bytes: &[u8],
) {
    let snapshot = br#"{"apps":[]}"#;
    let manifest = br#"{"format_version":1,"created_at_unix":0,"app_ids":[],"backup_entries":[{"logical_path":"manager-data/sentinel-ok","source_path":"/tmp/x","size_bytes":12,"sha256":null}],"entry_count":1}"#;
    let safe_payload = b"safe-payload";
    let hostile_payload = b"pwned-content";

    let mut raw = Vec::<u8>::new();
    append_raw_tar_entry(&mut raw, b"catalog-snapshot.json", snapshot);
    append_raw_tar_entry(&mut raw, b"manifest.json", manifest);
    append_raw_tar_entry(&mut raw, b"manager-data/sentinel-ok", safe_payload);
    append_raw_tar_entry(&mut raw, hostile_entry_path_bytes, hostile_payload);
    // Two zero-filled 512-byte blocks mark end-of-archive.
    raw.extend_from_slice(&[0u8; 1024]);

    // Compress with zstd.
    let file = fs::File::create(archive_path).expect("create archive");
    let mut enc = zstd::stream::Encoder::new(file, 3).expect("enc");
    use std::io::Write as _;
    enc.write_all(&raw).expect("write raw tar");
    enc.finish().expect("finish enc");
}

/// Asserts that `restore_backup_archive` did not create the sentinel escape
/// file at `home/escape.txt`, regardless of whether the call returned Ok or
/// Err.  Returns `(errored, error_message)`.
fn run_hostile_restore(hostile_path_bytes: &[u8]) -> (bool, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let archive = tmp.path().join("hostile.tar.zst");

    // Place home at `tmp/home` so manager_data_dir becomes
    // `tmp/home/.local/share/appimages`.  Any escape via `../../..` from
    // within the staging dir (which sits next to manager_data_dir) would
    // land somewhere under `tmp/home` or above it.
    let home_dir = tmp.path().join("home");
    fs::create_dir_all(&home_dir).expect("create home");

    // Sentinel file *above* home that should never be written by restore.
    let sentinel_outside = tmp.path().join("sentinel-outside.txt");
    fs::write(&sentinel_outside, b"untouched").expect("write sentinel");

    build_archive_with_hostile_tar_entry(&archive, hostile_path_bytes);

    let paths = AppPaths::new(home_dir.clone()).expect("paths");
    let result = restore_backup_archive(&paths, &archive, true);
    let errored = result.is_err();
    let err_msg = result.err().map(|e| e.to_string()).unwrap_or_default();

    // Assert the sentinel outside the home was not touched.
    assert_eq!(
        fs::read(&sentinel_outside).expect("read sentinel"),
        b"untouched",
        "hostile path {:?} wrote to sentinel outside home dir",
        String::from_utf8_lossy(hostile_path_bytes),
    );

    // Assert no file named `escape.txt` or `escape` appeared at home root or
    // above it — these are the expected landing zones for a successful escape.
    for escaped_name in &["escape.txt", "escape"] {
        let at_home = home_dir.join(escaped_name);
        let at_tmp = tmp.path().join(escaped_name);
        assert!(
            !at_home.exists(),
            "escape file appeared at home root for hostile path {:?}",
            String::from_utf8_lossy(hostile_path_bytes),
        );
        assert!(
            !at_tmp.exists(),
            "escape file appeared above home for hostile path {:?}",
            String::from_utf8_lossy(hostile_path_bytes),
        );
    }

    (errored, err_msg)
}

/// Classic `../escape.txt` in the tar entry path.
#[test]
fn hostile_archive_classic_parent_dir_escape_rejected() {
    let (errored, msg) = run_hostile_restore(b"../escape.txt");
    assert!(
        errored,
        "expected restore to return Err for ../escape.txt entry; got Ok",
    );
    assert!(
        msg.contains("unsafe component") || msg.contains("absolute") || msg.contains("outside"),
        "unexpected error message for ../escape.txt: {msg}",
    );
}

/// `manager-data/./../../escape.txt` — parent-dir escape with interleaved
/// CurDir components.  This is the specific case flagged by the audit: if the
/// sanitizer silently strips `.` components and then stops checking, the
/// remaining `..` components could slip through.
#[test]
fn hostile_archive_curdot_interleaved_parent_dir_rejected() {
    let (errored, msg) = run_hostile_restore(b"manager-data/./../../escape.txt");
    assert!(
        errored,
        "expected restore to return Err for manager-data/./../../escape.txt; got Ok",
    );
    assert!(
        msg.contains("unsafe component") || msg.contains("absolute") || msg.contains("outside"),
        "unexpected error message for curdot interleaved escape: {msg}",
    );
}

/// Absolute path `/tmp/escape.txt` in the tar entry header.
#[test]
fn hostile_archive_absolute_path_rejected() {
    let (errored, msg) = run_hostile_restore(b"/tmp/escape.txt");
    assert!(
        errored,
        "expected restore to return Err for /tmp/escape.txt entry; got Ok",
    );
    assert!(
        msg.contains("unsafe component")
            || msg.contains("absolute")
            || msg.contains("outside")
            || msg.contains("must be relative"),
        "unexpected error message for absolute path: {msg}",
    );
}

/// `manager-data/nested/../../../escape.txt` — deep nesting that tries to
/// climb three levels above `manager-data`.
#[test]
fn hostile_archive_deep_nested_parent_dir_rejected() {
    let (errored, msg) = run_hostile_restore(b"manager-data/nested/../../../escape.txt");
    assert!(
        errored,
        "expected restore to return Err for manager-data/nested/../../../escape.txt; got Ok",
    );
    assert!(
        msg.contains("unsafe component") || msg.contains("absolute") || msg.contains("outside"),
        "unexpected error message for deep nested escape: {msg}",
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
    assert!(dst_paths
        .applications_dir()
        .join("appimage-control-session-1.desktop")
        .is_file());
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
