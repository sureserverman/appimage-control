# Apply / Reconcile — instance count changes

ID: WF-APPLY-001

Entry points:
- GUI: **Apply** button in the header bar (`crates/gui/src/main.rs`, `apply.connect_clicked`) — reconciles every row's spin value to its target instance count.
- CLI: `+APP` / `+APP:N` (add), `-APP` / `-APP:N` (remove all / remove N), `=APP:N` (absolute set; `=APP` without `:N` is an error) — `crates/gui/src/cli.rs::run_cli_ops`. Ops run in argv order via the mixed-op machinery.

Steps:
1. (GUI only) Compute removal summary on the main thread. If any row's target is below its current count, show the "Remove instance data?" warning dialog ("Remove and Apply"); cancelling aborts the entire Apply. Apply button is desensitized while the modal is up so a second click cannot spawn a concurrent worker.
2. (CLI only) Strict two-pass validation of all op tokens against the catalog before ANY operation runs — nothing is partially applied when a later arg is invalid.
3. Per app, `reconcile_instance_count_with_report` (`crates/core/src/instances.rs`): validate target against `MAX_INSTANCES_PER_APP` and the catalog per-app cap.
4. Removals first, highest index down: `remove_instance` deletes the instance dir, then the launcher.
5. Additions: `ensure_cached_appimage` — reuse a fresh cached blob, otherwise download via `.partdownload` sibling + atomic rename; record cache version metadata.
6. `install_instance_from_cache` per new index: copy/hardlink the binary, provision portable `*.config`/`*.home` dirs, record installed version, write the `.desktop` launcher + icon.
7. Build `ReconcileReport { kept, removed, added }`; refresh desktop database/icon cache. GUI then auto-seeds newly added instances (see WF-SEED-001) and recomputes pending state; CLI prints a `Summary:` line when ≥ 2 ops were dispatched.

Postconditions:
- Each app has exactly indices `1..=target` installed, each with a working launcher; the download cache holds the current blob; the report reflects only work that actually completed.

Invariants:
- The cached AppImage is never deleted before its replacement downloads successfully: a stale cache file is NOT pre-removed; `download_to_file` writes to a `.partdownload` sibling and renames over the destination only on success (`instances.rs::ensure_cached_appimage`).
- A failed launcher write rolls back the just-created instance dir (`fs::remove_dir_all`) so no ghost instance remains — binary present but unlaunchable (`instances.rs::install_instance_from_cache_inner`).
- Reconcile reports list only completed work: `removed`/`added` are appended and `on_progress` fires only AFTER the removal/install succeeded; the first failure stops the run.
- Removal deletes the instance dir BEFORE its launcher; if the dir removal fails the launcher stays so the instance remains visible and reconcile can retry (`instances.rs::remove_instance`).
- Lowering counts in the GUI requires explicit confirmation listing the exact instances to be deleted; cancel leaves everything untouched.
- CLI mixed ops: after the first failed op, remaining ops are skipped (listed as `SKIPPED`), and a `Summary: <op> OK; <op> FAILED (…); <op> SKIPPED` line is printed to stderr when ≥ 2 ops were dispatched.

Last verified: 2026-06-30 — re-verified end-to-end by the `2026-06-30-exhaustive-functional-test-plan.md` behavioral pass (no behavior change). Prior: 2026-06-11 against commit `0b568eaf` (`2026-06-10-workflow-flaws-parity-remediation-plan.md`).
