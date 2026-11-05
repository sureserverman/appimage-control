# Apply / Reconcile — instance count changes

ID: WF-APPLY-001

Entry points:
- GUI: **Apply** button in the **bottom action bar** (`crates/gui/src/main.rs`, `apply.connect_clicked`) — reconciles every card's *target* to its instance count. The target is set per app via the launcher-grid **instance tiles** (click a tile = decrement) and the dashed **add-tile** (= increment, capped at `max_instances`); these drive a hidden per-card `SpinButton` that remains the target store, so the reconcile semantics are unchanged from the former spin control.
- CLI: `+APP` / `+APP:N` (add), `-APP` / `-APP:N` (remove all / remove N), `=APP:N` (absolute set; `=APP` without `:N` is an error) — `crates/gui/src/cli.rs::run_cli_ops`. Ops run in argv order via the mixed-op machinery.

Steps:
1. (GUI only) Compute removal summary on the main thread. If any card's target is below its current count, show the "Remove instance data?" warning dialog ("Remove and Apply"); cancelling aborts the entire Apply. Apply button is desensitized while the modal is up so a second click cannot spawn a concurrent worker.
2. (CLI only) Strict two-pass validation of all op tokens against the catalog before ANY operation runs — nothing is partially applied when a later arg is invalid.
3. Per app, `reconcile_instance_count_with_report` (`crates/core/src/instances.rs`): validate target against `MAX_INSTANCES_PER_APP` and the catalog per-app cap.
4. Removals first, highest index down: `remove_instance` deletes the instance dir, then the launcher.
5. Additions: `ensure_cached_appimage_with_policy` — reuse a fresh cached blob only while its sha256 matches the catalog pin (when the catalog has one) or the cache sidecar `.appimage-control-sha256` (when present); a blob cached before verification existed (no pin, no sidecar) is reused as before. Otherwise resolve the expected sha256 (`integrity::expected_digest`: the catalog `sha256`/`sha256_aarch64`, else GitHub's published asset `digest`, else unavailable) and download (`integrity::download_verified`: `.partdownload` → hidden `.<name>.unverified` sibling → hash → rename onto the cache path only on a match). **Verify sha256 before rename/chmod; refuse a mismatch always and an unavailable hash unless allowed** (`VerifyPolicy { allow_unverified }`). Then chmod 0755, write the sidecar (`<hex> verified catalog|github-digest` or `<hex> unverified user-allowed`) and record cache version metadata. The update path's newer GitHub release is checked against GitHub's digest only, never the catalog pin.
6. `install_instance_from_cache` per new index: copy/hardlink the binary, provision portable `*.config`/`*.home` dirs, record installed version, write the `.desktop` launcher + icon.
7. Build `ReconcileReport { kept, removed, added }`; refresh desktop database/icon cache. GUI then auto-seeds newly added instances (see WF-SEED-001) and recomputes pending state; CLI prints a `Summary:` line when ≥ 2 ops were dispatched.
8. Unverified downloads (DEC-001). CLI: an add op whose download has no published sha256 fails naming `--allow-unverified`; re-running with that flag installs it. GUI: after Apply (or **Update all**) finishes, the apps refused for lack of a sha256 (`Error::UnverifiedDownload`; never a mismatch) are listed in an **"Install without verification?"** dialog (`crates/gui/src/unverified.rs`, `main.rs::offer_unverified_install`). **Cancel** (the default and close response) leaves everything untouched; **Install unverified** (destructive appearance) restores those apps' targets and re-runs Apply (or Update) for exactly those apps with `allow_unverified`; the opt-in is one-shot and drained when the run starts.

Postconditions:
- Each app has exactly indices `1..=target` installed, each with a working launcher; the download cache holds the current blob; the report reflects only work that actually completed.

Invariants:
- The cached AppImage is never deleted before its replacement downloads and verifies: a stale cache file is NOT pre-removed; the download lands in a hidden sibling and renames over the destination only once accepted (`instances.rs::ensure_cached_appimage_with_policy`, `integrity.rs::download_verified`).
- A downloaded AppImage whose expected sha256 is known and differs is never renamed into place or made executable; the error names the app, the expected and the actual hash (`Error::DigestMismatch`). No opt-in overrides a mismatch.
- With no known sha256 (catalog has none, GitHub returns `null`, the URL is not a GitHub asset, or the lookup failed), nothing is downloaded unless the user opted in (`Error::UnverifiedDownload`; CLI `--allow-unverified`, GUI confirm dialog). A failed lookup counts as "no hash", never as "verified".
- A cache sidecar that exists but cannot be parsed is untrusted (the blob is downloaded again); sidecars are written atomically. Known limit: a blob cached before verification existed (no catalog pin, no sidecar) is reused unverified (DEC-001).
- The update path's `appimageupdatetool` step (zsync from the AppImage's own update info, no checkable sha256) runs only when unverified installs are allowed; otherwise the verified download is used.
- A failed launcher write rolls back the just-created instance dir (`fs::remove_dir_all`) so no ghost instance remains — binary present but unlaunchable (`instances.rs::install_instance_from_cache_inner`).
- Reconcile reports list only completed work: `removed`/`added` are appended and `on_progress` fires only AFTER the removal/install succeeded; the first failure stops the run.
- Removal deletes the instance dir BEFORE its launcher; if the dir removal fails the launcher stays so the instance remains visible and reconcile can retry (`instances.rs::remove_instance`).
- Lowering counts in the GUI requires explicit confirmation listing the exact instances to be deleted; cancel leaves everything untouched.
- CLI mixed ops: after the first failed op, remaining ops are skipped (listed as `SKIPPED`), and a `Summary: <op> OK; <op> FAILED (…); <op> SKIPPED` line is printed to stderr when ≥ 2 ops were dispatched.

Last verified: 2026-11-05 — `2026-11-05-security-audit-remediation-plan.md` Task 3.3: step 5 now verifies every download's sha256 before rename/chmod (DEC-001); core fixture tests on the host. Prior: 2026-07-01 — GUI affordance changed from a per-row spin button to the launcher-grid instance tiles + add-tile (design-handoff redesign); reconcile semantics unchanged (the tiles drive the same hidden target store the spin used). Prior: 2026-06-30 — re-verified end-to-end by the `2026-06-30-exhaustive-functional-test-plan.md` behavioral pass (no behavior change); 2026-06-11 against commit `0b568eaf`.
