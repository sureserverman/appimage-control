use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::Catalog;
use crate::error::{Error, Result};
use crate::paths::AppPaths;

pub const BACKUP_FORMAT_VERSION: u32 = 1;

fn instance_strategy_name(v: crate::catalog::InstanceStrategy) -> &'static str {
    match v {
        crate::catalog::InstanceStrategy::None => "none",
        crate::catalog::InstanceStrategy::ElectronClass => "electron_class",
        crate::catalog::InstanceStrategy::JoplinAltInstance => "joplin_alt_instance",
        crate::catalog::InstanceStrategy::CryptomatorExtract => "cryptomator_extract",
        crate::catalog::InstanceStrategy::QtX11 => "qt_x11",
        crate::catalog::InstanceStrategy::KeepassxcIsolated => "keepassxc_isolated",
        crate::catalog::InstanceStrategy::ElectronClassIsolated => "electron_class_isolated",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupManifest {
    pub format_version: u32,
    pub created_at_unix: u64,
    pub app_ids: Vec<String>,
    pub backup_entries: Vec<BackupEntry>,
    pub entry_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupCatalogSnapshot {
    /// Raw app objects from catalog JSON, preserved to restore deterministically even if
    /// live catalog parsing/schema evolves.
    pub apps: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupEntry {
    pub logical_path: String,
    pub source_path: String,
    pub size_bytes: u64,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupOptions {
    pub include_cache: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupReport {
    pub archive_path: PathBuf,
    pub entry_count: u64,
    pub total_bytes: u64,
}

fn path_sha256(path: &Path) -> Result<String> {
    let mut f = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let out = hasher.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out {
        use std::fmt::Write as _;
        let _ = write!(&mut s, "{:02x}", b);
    }
    Ok(s)
}

fn push_file_entry(out: &mut Vec<BackupEntry>, logical: String, source: &Path) -> Result<()> {
    let m = fs::metadata(source)?;
    let sha = if m.is_file() {
        Some(path_sha256(source)?)
    } else {
        None
    };
    out.push(BackupEntry {
        logical_path: logical,
        source_path: source.to_string_lossy().to_string(),
        size_bytes: m.len(),
        sha256: sha,
    });
    Ok(())
}

fn walk_dir_entries(root: &Path, root_logical: &str, out: &mut Vec<BackupEntry>) -> Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)?
            .filter_map(|e| e.ok().map(|x| x.path()))
            .collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(root).map_err(|_| {
                Error::InvalidPath(format!("backup strip_prefix failed: {}", p.display()))
            })?;
            let logical = if rel.as_os_str().is_empty() {
                root_logical.to_string()
            } else {
                format!("{}/{}", root_logical, rel.to_string_lossy())
            };
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                push_file_entry(out, logical, &p)?;
            }
        }
    }
    Ok(())
}

pub fn collect_backup_inventory(paths: &AppPaths) -> Result<Vec<BackupEntry>> {
    let mut out = Vec::new();

    let manager = paths.manager_data_dir();
    walk_dir_entries(&manager, "manager-data", &mut out)?;

    for p in paths.managed_desktop_entries()? {
        let name = p
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| Error::InvalidPath(format!("invalid desktop path: {}", p.display())))?;
        push_file_entry(&mut out, format!("applications/{name}"), &p)?;
    }
    for p in paths.managed_hicolor_icon_files()? {
        let hicolor_root = paths.home().join(".local/share/icons/hicolor");
        let rel = p.strip_prefix(&hicolor_root).map_err(|_| {
            Error::InvalidPath(format!("icon not under hicolor root: {}", p.display()))
        })?;
        push_file_entry(&mut out, format!("hicolor/{}", rel.to_string_lossy()), &p)?;
    }
    out.sort_by(|a, b| a.logical_path.cmp(&b.logical_path));
    Ok(out)
}

pub fn create_backup_archive(
    paths: &AppPaths,
    catalog: &Catalog,
    archive_path: &Path,
    _options: &BackupOptions,
) -> Result<BackupReport> {
    let parent = archive_path.parent().ok_or_else(|| {
        Error::InvalidPath(format!(
            "backup path has no parent: {}",
            archive_path.display()
        ))
    })?;
    fs::create_dir_all(parent)?;

    let entries = collect_backup_inventory(paths)?;
    let created_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::Backup(format!("clock error: {e}")))?
        .as_secs();
    let app_ids = catalog
        .apps
        .iter()
        .map(|a| a.id.clone())
        .collect::<Vec<_>>();
    let snapshot = BackupCatalogSnapshot {
        apps: catalog
            .apps
            .iter()
            .map(|a| {
                serde_json::json!({
                    "id": a.id,
                    "display_name": a.display_name,
                    "homepage": a.homepage,
                    "download_url": a.download_url,
                    "download_url_aarch64": a.download_url_aarch64,
                    "appimage_filename": a.appimage_filename,
                    "instance_strategy": instance_strategy_name(a.instance_strategy),
                    "instance_exec_args_template": a.instance_exec_args_template,
                    "instance_name_template": a.instance_name_template,
                    "desktop_name_template": a.desktop_name_template,
                    "reset_config_dirs": a.reset_config_dirs,
                    "reset_cache_dirs": a.reset_cache_dirs,
                    "reset_data_dirs": a.reset_data_dirs,
                    "icon_url": a.icon_url,
                    "wayland_app_id": a.wayland_app_id,
                    "desktop_pass_file_urls": a.desktop_pass_file_urls,
                    "instance_subdir": a.instance_subdir,
                    "isolate_dbus_session": a.isolate_dbus_session,
                    "max_instances": a.max_instances,
                    "max_instances_tooltip": a.max_instances_tooltip
                })
            })
            .collect::<Vec<_>>(),
    };

    // Use a PID-qualified staging filename so two concurrent backups targeting
    // the same archive path cannot overwrite each other's in-flight staging
    // file.  Pattern matches the `{path}.aic-tmp-{pid}` convention used in
    // seed.rs.  We do NOT pre-remove an existing staging file: with unique-pid
    // naming there is no legitimate prior occupant, and any stale file from a
    // crashed previous run will be overwritten by fs::File::create below.
    let tmp = {
        let file_name = archive_path
            .file_name()
            .ok_or_else(|| {
                Error::InvalidPath(format!(
                    "backup path has no file name: {}",
                    archive_path.display()
                ))
            })?
            .to_string_lossy();
        archive_path.with_file_name(format!("{}.aic-tmp-{}", file_name, std::process::id()))
    };

    let result = (|| -> Result<BackupReport> {
        // The archive holds wallet and password-manager homes: owner-only from the first byte
        // (umask cannot widen it; an existing stale staging file is truncated and re-moded).
        let file = {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let f = fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.set_permissions(fs::Permissions::from_mode(0o600))?;
            f
        };
        let enc = zstd::stream::Encoder::new(file, 3)
            .map_err(|e| Error::Backup(format!("zstd encoder init failed: {e}")))?;
        let mut tar = tar::Builder::new(enc);

        let snapshot_bytes = serde_json::to_vec_pretty(&snapshot)?;
        let mut sh = tar::Header::new_gnu();
        sh.set_size(snapshot_bytes.len() as u64);
        sh.set_mode(0o644);
        sh.set_cksum();
        tar.append_data(&mut sh, "catalog-snapshot.json", snapshot_bytes.as_slice())?;

        let mut archived_entries = Vec::new();
        let mut total_bytes = snapshot_bytes.len() as u64;
        for ent in &entries {
            let src = Path::new(&ent.source_path);
            if !src.is_file() {
                continue;
            }
            // Hash the bytes as they go into the archive, so the manifest describes exactly
            // what was archived even if a running app changes the file meanwhile (a hash taken
            // during inventory would then fail an untampered restore).
            let f = fs::File::open(src)?;
            let meta = f.metadata()?;
            let mut header = tar::Header::new_gnu();
            header.set_metadata(&meta);
            header.set_size(meta.len());
            let mut reader = HashingReader {
                inner: f.take(meta.len()),
                hasher: Sha256::new(),
                read: 0,
            };
            tar.append_data(&mut header, &ent.logical_path, &mut reader)?;
            if reader.read != meta.len() {
                return Err(Error::Backup(format!(
                    "{} changed size while it was being backed up; close the app and retry",
                    ent.source_path
                )));
            }
            let mut archived = ent.clone();
            archived.size_bytes = reader.read;
            archived.sha256 = Some(
                reader
                    .hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
            );
            total_bytes += archived.size_bytes;
            archived_entries.push(archived);
        }

        let manifest = BackupManifest {
            format_version: BACKUP_FORMAT_VERSION,
            created_at_unix,
            app_ids: app_ids.clone(),
            entry_count: archived_entries.len() as u64,
            backup_entries: archived_entries.clone(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        let mut mh = tar::Header::new_gnu();
        mh.set_size(manifest_bytes.len() as u64);
        mh.set_mode(0o644);
        mh.set_cksum();
        tar.append_data(&mut mh, "manifest.json", manifest_bytes.as_slice())?;
        total_bytes += manifest_bytes.len() as u64;

        let enc = tar.into_inner()?;
        enc.finish()
            .map_err(|e| Error::Backup(format!("zstd finalize failed: {e}")))?;

        fs::rename(&tmp, archive_path)?;
        Ok(BackupReport {
            archive_path: archive_path.to_path_buf(),
            entry_count: archived_entries.len() as u64,
            total_bytes,
        })
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// True if `dir` contains at least one regular file at any depth. Empty
/// directories and trees made up only of empty subdirectories return false.
/// Used so a `--purge`d manager dir (which is left holding an empty `cache/`)
/// does not count as managed state for the restore preflight (BL-003).
///
/// Recurses unbounded, which is safe here: the only caller is the manager
/// data dir, whose layout is shallow (`manager/apps/<id>/<n>/<file>`,
/// depth ~4). Symlinks are not followed — `DirEntry::file_type` uses
/// `lstat`, so a symlinked dir reports `is_dir() == false` and is counted as
/// a (non-directory) entry rather than recursed into, so cycles are impossible.
fn dir_contains_any_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for result in entries {
        // Any I/O ambiguity is treated as "state may exist" so the restore
        // preflight stays conservative (a false negative here would let a
        // restore clobber real managed state). This matches the Err arm on
        // `file_type()` below.
        let Ok(entry) = result else {
            return true;
        };
        match entry.file_type() {
            Ok(ft) if ft.is_dir() => {
                if dir_contains_any_file(&entry.path()) {
                    return true;
                }
            }
            Ok(_) => return true,
            Err(_) => return true,
        }
    }
    false
}

pub fn restore_preflight(paths: &AppPaths, force: bool) -> Result<()> {
    if force {
        return Ok(());
    }
    let manager = paths.manager_data_dir();
    let has_manager_state = manager.is_dir() && dir_contains_any_file(&manager);
    let has_desktops = !paths.managed_desktop_entries()?.is_empty();
    let has_icons = !paths.managed_hicolor_icon_files()?.is_empty();
    if has_manager_state || has_desktops || has_icons {
        return Err(Error::RestoreRefused(
            "managed state already exists; re-run restore with --force to overwrite".to_string(),
        ));
    }
    Ok(())
}

/// A reader that hashes everything read through it.
struct HashingReader<R> {
    inner: R,
    hasher: Sha256,
    read: u64,
}

impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.read += n as u64;
        Ok(n)
    }
}

fn safe_relative_archive_path(p: &Path) -> Result<PathBuf> {
    if p.is_absolute() {
        return Err(Error::Backup(format!(
            "archive entry must be relative: {}",
            p.display()
        )));
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(seg) => out.push(seg),
            // CurDir (`.`) components are silently skipped; they cannot mask
            // subsequent `..` because `Path::components()` always exposes every
            // `ParentDir` regardless of interleaved CurDir segments.
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(Error::Backup(format!(
                    "archive entry contains unsafe component: {}",
                    p.display()
                )));
            }
        }
    }
    // Reject an all-CurDir input like `./././` that collapses to an empty path;
    // writing to `stage_dir.join("")` would silently target the staging root.
    if out.as_os_str().is_empty() {
        return Err(Error::Backup(format!(
            "archive entry resolves to empty path: {}",
            p.display()
        )));
    }
    Ok(out)
}

fn is_payload_file_entry(kind: tar::EntryType) -> bool {
    kind.is_file() || kind.as_byte() == b'S'
}

fn clear_directory_contents(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for ent in fs::read_dir(dir)? {
        let ent = ent?;
        let p = ent.path();
        if p.is_dir() {
            fs::remove_dir_all(&p).map_err(|e| {
                Error::Backup(format!("failed to remove directory {}: {e}", p.display()))
            })?;
        } else {
            fs::remove_file(&p).map_err(|e| {
                Error::Backup(format!("failed to remove file {}: {e}", p.display()))
            })?;
        }
    }
    Ok(())
}

fn validate_backup_archive(archive_path: &Path) -> Result<(BackupManifest, BackupCatalogSnapshot)> {
    let f = fs::File::open(archive_path)?;
    let dec = zstd::stream::Decoder::new(f)
        .map_err(|e| Error::Backup(format!("zstd decode init failed: {e}")))?;
    let mut ar = tar::Archive::new(dec);
    let mut manifest: Option<BackupManifest> = None;
    let mut snapshot: Option<BackupCatalogSnapshot> = None;
    let mut seen_entries = Vec::new();

    for ent in ar.entries()? {
        let mut ent = ent?;
        if !is_payload_file_entry(ent.header().entry_type()) {
            continue;
        }
        let p = safe_relative_archive_path(&ent.path()?)?;
        let ps = p.to_string_lossy().to_string();
        seen_entries.push(ps.clone());
        if ps == "manifest.json" {
            let mut buf = Vec::new();
            ent.read_to_end(&mut buf)?;
            manifest = Some(serde_json::from_slice(&buf)?);
        } else if ps == "catalog-snapshot.json" {
            let mut buf = Vec::new();
            ent.read_to_end(&mut buf)?;
            snapshot = Some(serde_json::from_slice(&buf)?);
        }
    }
    let manifest =
        manifest.ok_or_else(|| Error::Backup("missing manifest.json in backup archive".into()))?;
    let snapshot = snapshot
        .ok_or_else(|| Error::Backup("missing catalog-snapshot.json in backup archive".into()))?;
    if manifest.format_version != BACKUP_FORMAT_VERSION {
        return Err(Error::Backup(format!(
            "unsupported backup format version {}; expected {}",
            manifest.format_version, BACKUP_FORMAT_VERSION
        )));
    }
    if manifest.entry_count != manifest.backup_entries.len() as u64 {
        return Err(Error::Backup(
            "manifest entry_count does not match backup_entries length".into(),
        ));
    }
    for e in &manifest.backup_entries {
        let p = PathBuf::from(&e.logical_path);
        let p = safe_relative_archive_path(&p)?;
        let ps = p.to_string_lossy().to_string();
        if (ps.starts_with("manager-data/")
            || ps.starts_with("applications/")
            || ps.starts_with("hicolor/"))
            && !seen_entries.iter().any(|s| s == &ps)
        {
            return Err(Error::Backup(format!(
                "manifest entry missing from archive payload: {ps}"
            )));
        }
    }
    Ok((manifest, snapshot))
}

fn clear_managed_state_for_restore(paths: &AppPaths) -> Result<()> {
    let manager = paths.manager_data_dir();
    fs::create_dir_all(&manager).map_err(|e| {
        Error::Backup(format!(
            "failed to recreate managed data dir {}: {e}",
            manager.display()
        ))
    })?;
    clear_directory_contents(&manager)?;
    for d in paths.managed_desktop_entries()? {
        let _ = fs::remove_file(d);
    }
    for i in paths.managed_hicolor_icon_files()? {
        let _ = fs::remove_file(i);
    }
    Ok(())
}

fn restore_destination_path(paths: &AppPaths, logical_path: &Path) -> Result<PathBuf> {
    let rel = safe_relative_archive_path(logical_path)?;
    let comps = rel.components().collect::<Vec<_>>();
    let Some(first) = comps.first() else {
        return Err(Error::Backup("empty archive path".into()));
    };
    let mut tail = PathBuf::new();
    for c in comps.iter().skip(1) {
        if let Component::Normal(seg) = c {
            tail.push(seg);
        }
    }
    match first {
        Component::Normal(seg) if *seg == std::ffi::OsStr::new("manager-data") => {
            Ok(paths.manager_data_dir().join(tail))
        }
        Component::Normal(seg) if *seg == std::ffi::OsStr::new("applications") => {
            Ok(paths.applications_dir().join(tail))
        }
        Component::Normal(seg) if *seg == std::ffi::OsStr::new("hicolor") => {
            Ok(paths.home().join(".local/share/icons/hicolor").join(tail))
        }
        _ => Err(Error::Backup(format!(
            "archive entry outside managed restore roots: {}",
            rel.display()
        ))),
    }
}

pub fn restore_backup_archive(paths: &AppPaths, archive_path: &Path, force: bool) -> Result<()> {
    restore_preflight(paths, force)?;
    let (manifest, _snapshot) = validate_backup_archive(archive_path)?;
    // Every payload entry must be listed in the manifest with a sha256 (keys normalised the
    // same way the archive paths are), so nothing is restored unchecked.
    let mut recorded: std::collections::HashMap<String, Option<String>> =
        std::collections::HashMap::new();
    for e in &manifest.backup_entries {
        let key = safe_relative_archive_path(Path::new(&e.logical_path))?
            .to_string_lossy()
            .into_owned();
        recorded.insert(key, e.sha256.clone());
    }
    // Archived mode per entry, applied (owner bits only) when the file is restored.
    let mut modes: std::collections::HashMap<PathBuf, u32> = std::collections::HashMap::new();

    // Phase 1 — extract the ENTIRE archive into a staging directory that lives
    // OUTSIDE manager_data_dir so it is not touched by clear_managed_state_for_restore.
    // If extraction fails for any reason we clean up the staging dir and return Err
    // with the original managed state fully intact.
    let stage_dir = {
        let parent = paths
            .manager_data_dir()
            .parent()
            .ok_or_else(|| Error::Backup("manager_data_dir has no parent directory".to_string()))?
            .to_path_buf();
        parent.join(format!(".appimages-restore-stage.{}", std::process::id()))
    };

    if stage_dir.exists() {
        let _ = fs::remove_dir_all(&stage_dir);
    }
    // Owner-only: the staged files are the user's decrypted wallet and password data.
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&stage_dir)
    }
    .map_err(|e| {
        Error::Backup(format!(
            "failed to create restore stage dir {}: {e}",
            stage_dir.display()
        ))
    })?;

    // Extract all payload entries into the staging area.  Any I/O error here
    // leaves managed state untouched.
    let extract_result = (|| -> Result<()> {
        let f = fs::File::open(archive_path)?;
        let dec = zstd::stream::Decoder::new(f)
            .map_err(|e| Error::Backup(format!("zstd decode init failed: {e}")))?;
        let mut ar = tar::Archive::new(dec);
        for ent in ar.entries()? {
            let mut ent = ent?;
            if !is_payload_file_entry(ent.header().entry_type()) {
                continue;
            }
            let logical = safe_relative_archive_path(&ent.path()?)?;
            let logical_s = logical.to_string_lossy().to_string();
            if logical_s == "manifest.json" || logical_s == "catalog-snapshot.json" {
                continue;
            }

            // Validate the logical path maps to a known managed root before extracting.
            // This is a lightweight check; restore_destination_path does the authoritative
            // mapping in the swap phase below.
            let stage_path = stage_dir.join(&logical);
            // Defense-in-depth: confirm the join did not escape the staging dir.
            // `safe_relative_archive_path` already rejects all escape-capable components;
            // this check closes any future sanitizer gap without relying on canonicalize
            // (which would require the path to already exist on disk).
            if !stage_path.starts_with(&stage_dir) {
                return Err(Error::Backup(format!(
                    "archive entry would escape staging directory: {}",
                    logical.display()
                )));
            }
            if let Some(parent) = stage_path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    Error::Backup(format!(
                        "failed to create stage parent {}: {e}",
                        parent.display()
                    ))
                })?;
            }
            let mode = ent.header().mode().unwrap_or(0o600);
            let mut out_file = {
                use std::os::unix::fs::OpenOptionsExt;
                fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&stage_path)
            }
            .map_err(|e| {
                Error::Backup(format!(
                    "failed to create staged file {}: {e}",
                    stage_path.display()
                ))
            })?;
            std::io::copy(&mut ent, &mut out_file)?;
            drop(out_file);
            // Each payload entry must match the sha256 its manifest entry recorded at backup
            // time, checked here, before any managed state is cleared. An entry the manifest
            // does not list, or lists without a hash, is refused too.
            let want = match recorded.get(&logical_s) {
                Some(Some(h)) => h,
                Some(None) => {
                    return Err(Error::Backup(format!(
                        "archive entry {logical_s} has no recorded sha256; nothing was restored"
                    )))
                }
                None => {
                    return Err(Error::Backup(format!(
                    "archive entry {logical_s} is not listed in the manifest; nothing was restored"
                )))
                }
            };
            let got = path_sha256(&stage_path)?;
            if &got != want {
                return Err(Error::Backup(format!(
                    "archive entry {logical_s} does not match its recorded sha256 \
                     (expected {want}, got {got}); nothing was restored"
                )));
            }
            modes.insert(logical.clone(), mode);
        }
        Ok(())
    })();

    if let Err(e) = extract_result {
        // Extraction failed — original managed state is fully intact.
        let _ = fs::remove_dir_all(&stage_dir);
        return Err(e);
    }

    // Phase 2 — swap: only now clear managed state and install from the staging area.
    // Any error here has already committed us to the swap (we cleared state), but that
    // mirrors the pre-existing behaviour; the critical invariant is that corruption in
    // the archive itself is caught in Phase 1.
    let swap_result = (|| -> Result<()> {
        clear_managed_state_for_restore(paths)?;

        // Walk all staged files and copy each to its final destination.
        let mut stack = vec![stage_dir.clone()];
        while let Some(dir) = stack.pop() {
            let mut children: Vec<PathBuf> = fs::read_dir(&dir)?
                .filter_map(|e| e.ok().map(|x| x.path()))
                .collect();
            children.sort();
            for child in children {
                if child.is_dir() {
                    stack.push(child);
                } else if child.is_file() {
                    let logical = child.strip_prefix(&stage_dir).map_err(|_| {
                        Error::Backup(format!(
                            "staged file not under stage root: {}",
                            child.display()
                        ))
                    })?;
                    let dst = restore_destination_path(paths, logical)?;
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent).map_err(|e| {
                            Error::Backup(format!(
                                "failed to create restore parent {}: {e}",
                                parent.display()
                            ))
                        })?;
                    }
                    fs::copy(&child, &dst).map_err(|e| {
                        Error::Backup(format!(
                            "failed to copy staged file {} to {}: {e}",
                            child.display(),
                            dst.display()
                        ))
                    })?;
                    // The archived owner bits (an AppImage stays executable), never group or
                    // other: restored files hold wallet and password-manager data.
                    let mode = modes.get(logical).copied().unwrap_or(0o600) & 0o700 | 0o600;
                    {
                        use std::os::unix::fs::PermissionsExt;
                        fs::set_permissions(&dst, fs::Permissions::from_mode(mode))?;
                    }
                }
            }
        }
        Ok(())
    })();

    // Best-effort staging cleanup regardless of swap outcome.
    let _ = fs::remove_dir_all(&stage_dir);
    swap_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogApp;
    use crate::catalog::InstanceStrategy;

    fn fake_app() -> CatalogApp {
        CatalogApp {
            id: "session".to_string(),
            display_name: Some("Session".to_string()),
            homepage: None,
            download_url: Some("https://example.invalid/session.AppImage".to_string()),
            download_url_aarch64: None,
            appimage_filename: Some("session.AppImage".to_string()),
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
            sha256: None,
            sha256_aarch64: None,
        }
    }

    #[test]
    fn creates_tar_zst_with_manifest_and_snapshot() {
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
        let cat = Catalog {
            apps: vec![fake_app()],
        };
        let out = tmp.path().join("backup.tar.zst");
        let rep = create_backup_archive(&paths, &cat, &out, &BackupOptions::default())
            .expect("create backup");
        assert!(out.is_file());
        assert!(rep.entry_count >= 1);

        let f = fs::File::open(&out).expect("open backup");
        let dec = zstd::stream::Decoder::new(f).expect("decoder");
        let mut ar = tar::Archive::new(dec);
        let mut names = Vec::new();
        for e in ar.entries().expect("entries") {
            let e = e.expect("entry");
            names.push(e.path().expect("entry path").to_string_lossy().to_string());
        }
        names.sort();
        assert!(names.iter().any(|n| n == "manifest.json"));
        assert!(names.iter().any(|n| n == "catalog-snapshot.json"));
    }

    #[test]
    fn payload_entry_type_accepts_gnu_sparse() {
        assert!(is_payload_file_entry(tar::EntryType::Regular));
        assert!(is_payload_file_entry(tar::EntryType::new(b'S')));
        assert!(!is_payload_file_entry(tar::EntryType::Directory));
    }

    #[test]
    fn dir_contains_any_file_ignores_empty_subdir_trees() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().join("root");
        fs::create_dir_all(root.join("cache")).expect("mkdir cache");
        fs::create_dir_all(root.join("apps/empty")).expect("mkdir nested");
        // Only empty directories so far → no real state.
        assert!(!dir_contains_any_file(&root));
        // A real file anywhere makes it true.
        fs::write(root.join("apps/empty/blob"), b"x").expect("write");
        assert!(dir_contains_any_file(&root));
    }

    /// Regression (BL-003, 2026-06-30 functional test): `--purge` leaves an
    /// empty `cache/` dir inside the manager data dir. The restore preflight
    /// must treat that empty tree as "no managed state" so a follow-up
    /// `--restore-backup` does not spuriously require `--force`.
    #[test]
    fn restore_preflight_allows_purged_empty_cache_without_force() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::new(tmp.path().to_path_buf()).expect("paths");
        // Simulate post-purge: manager dir exists but holds only an empty cache/.
        fs::create_dir_all(paths.manager_data_dir().join("cache")).expect("mkdir cache");
        restore_preflight(&paths, false)
            .expect("empty managed dirs must not count as state (BL-003)");
    }

    #[test]
    fn restore_preflight_still_refuses_real_state_without_force() {
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
        assert!(restore_preflight(&paths, false).is_err());
        // …and --force always bypasses.
        restore_preflight(&paths, true).expect("force bypasses");
    }
}
