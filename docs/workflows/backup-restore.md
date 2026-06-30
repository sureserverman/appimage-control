# Backup / Restore — manager state archive

ID: WF-BACKUP-001

Entry points:
- GUI: Main Menu → **Backup** (`win.backup`) and **Restore from backup** (`win.restore_backup`) — `crates/gui/src/actions.rs`; restore prompts for `--force`-equivalent confirmation when the first attempt is refused over existing state (`crates/gui/src/main.rs`).
- CLI: `--backup PATH` (`.tar.zst` recommended), `--restore-backup PATH [--force]` — `crates/gui/src/cli.rs`. Mutually exclusive with each other and with `--purge`/`--clean`/`--migrate`.

Steps (backup, `crates/core/src/backup.rs::create_backup_archive`):
1. `collect_backup_inventory`: enumerate managed files (manager data dir, managed `.desktop` entries, managed hicolor icons) into logical paths under `manager-data/`, `applications/`, `hicolor/`.
2. Stream a zstd-compressed tar to the staging file `{name}.aic-tmp-{pid}` next to the target path: `catalog-snapshot.json`, every payload file, then `manifest.json` (format version, entry count, entry list).
3. On success, `fs::rename` the staging file over the archive path; on any error, remove the staging file and return the error.

Steps (restore, `crates/core/src/backup.rs::restore_backup_archive`):
1. `restore_preflight`: unless `--force`, refuse when any managed state exists (non-empty manager data dir, managed desktop entries, or managed icons).
2. `validate_backup_archive`: full pass over the archive — manifest + snapshot present, supported `format_version`, `entry_count` matches, every manifest entry present in the payload, every entry path sanitized.
3. Phase 1 — extract the ENTIRE payload into the staging dir `.appimages-restore-stage.<pid>` (sibling of the manager data dir, outside it). Any I/O or path error removes the staging dir and returns with managed state untouched.
4. Phase 2 — swap: clear managed state (manager data dir contents, managed desktops, managed icons), then copy every staged file to its destination under the three managed roots.
5. Remove the staging dir (best-effort) regardless of swap outcome.

Postconditions:
- Backup: archive at PATH contains snapshot + manifest + all managed files; no staging file remains.
- Restore: managed state matches the archive contents exactly; no staging dir remains.

Invariants:
- The backup staging file is `{name}.aic-tmp-{pid}` (PID-qualified, so concurrent backups to the same path cannot clobber each other); an existing archive is never deleted before its replacement is complete — it is only atomically renamed over on success.
- Restore extracts the ENTIRE archive into `.appimages-restore-stage.<pid>` and validates it BEFORE clearing any managed state: a failed restore (corrupt archive, bad path, I/O error in Phase 1) leaves the prior state byte-identical.
- Hostile tar entries are rejected by `safe_relative_archive_path`: `..` components, absolute/rooted/prefixed paths, and `.`-interleaved sequences cannot escape (`Path::components()` exposes every `ParentDir`); an all-`.` entry collapsing to an empty path is also rejected, plus a defense-in-depth `starts_with(stage_dir)` check on the join.
- Restore refuses without `--force` when managed state exists, where "state" means at least one actual **file** under the managed roots. **Empty managed directories do not count** (`restore_preflight` → `dir_contains_any_file`): a `--purge`d manager dir holds only an empty `cache/`, so a follow-up `--restore-backup` proceeds **without** `--force` (BL-003 fix, 2026-06-30). `--force` still bypasses the check unconditionally.

Last verified: 2026-06-30 — re-verified by the `2026-06-30-exhaustive-functional-test-plan.md` behavioral pass; BL-003 fix landed (empty-dir preflight) with regression tests `restore_preflight_allows_purged_empty_cache_without_force` / `restore_preflight_still_refuses_real_state_without_force`. Prior: 2026-06-11 against commit `0b568eaf`.
