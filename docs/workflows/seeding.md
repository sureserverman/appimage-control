# Seeding — first-run settings provisioning

ID: WF-SEED-001

Entry points:
- GUI: **Seed Settings…** menu item (`win.seed_settings` → `crates/gui/src/seed_dialog.rs::choose_seed_settings`), and post-Apply auto-seed of newly added instances (`crates/gui/src/main.rs` → `seed_new_instances_after_reconcile`).
- CLI: `--seed-settings TARGETS` (`all` or comma-separated app ids), `--seed-save NAME`, `--seed-load NAME`, `--seed-list-forms`, `--seed-delete-form NAME`, and `--seed-new` (alongside `+APP[:N]`/`=APP:N` ops that add instances) — `crates/gui/src/cli.rs`.

Steps:
1. Pre-flight `portmux_port_probe_runtime`: if the portmux port helper is missing, print the install hint once and skip/fail seeding — installed instances remain installed (CLI `--seed-new` counts the unseeded instances and exits failure).
2. Resolve the host preset automatically (`SeedPreset::auto_default` in `crates/core/src/seed.rs`): `APPIMAGE_CONTROL_SEED_PRESET` env override, else Whonix marker-file probe, else Ubuntu. There is no user-facing preset flag.
3. Per instance (`seed_until_config`): sweep stale PID files (prefix match, mtime older than 1 hour) from the instance dir, then launch the AppImage in a fresh session — `setsid --fork --wait sh -c '… exec xvfb-run -a …'` — writing the host-side session-leader PID (PGID) to a per-run PID file in the instance dir.
4. Poll the expected config path until it parses as a COMPLETE JSON *object* (`config_file_complete`), up to `SEED_TIMEOUT` (20 s); a partial/in-progress file keeps polling.
5. Teardown the whole process group (xvfb-run + Xvfb + app): TERM then KILL to `-pgid` — natively via the `kill(2)` syscall, inside Flatpak via `flatpak-spawn --host kill` — then remove the PID file.
6. Patch the config: `read_json_object` → `apply_pack` (seed-pack recipe + preset fields + per-instance portmux port via `portmux-port ensure`) → `write_json_object` atomic write: sibling temp `<path>.aic-tmp-<pid>`, `write_all`, `fsync`, `rename` over the original.
7. Emit a per-run report: seeded / failed / unsupported / missing counts (CLI prints `Seed settings summary: …`; failure count > 0 → exit failure).

Postconditions:
- Each seeded instance's config file is a valid, complete JSON object containing the pack-applied settings; no seed processes, PID files, or temp files remain.

Invariants:
- The seeded config is never truncated: the atomic temp+fsync+rename write means any failure leaves the original file byte-identical and removes the temp file.
- PID files are per-run unique: `.appimage-control-seed-host-pid.<pid>.<counter>` (seeder process PID + monotonic atomic invocation counter); the stale sweep removes only files older than 1 hour — never a concurrent run's fresh file.
- Concurrent seeds of the same instance dir never kill each other's process groups: each run reads only its own PID file for the PGID.
- The host preset is auto-detected; no user flag selects it (CLI and GUI behave identically).

Last verified: 2026-06-11 against commit `0b568eaf` — by the Stage 1–6 test suites and live gate checks of `2026-06-10-workflow-flaws-parity-remediation-plan.md`.
