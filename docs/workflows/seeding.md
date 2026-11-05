# Seeding — first-run settings provisioning

ID: WF-SEED-001

Entry points:
- GUI: **Seed Settings…** menu item (`win.seed_settings` → `crates/gui/src/seed_dialog.rs::choose_seed_settings`), and post-Apply auto-seed of newly added instances (`crates/gui/src/main.rs` → `seed_new_instances_after_reconcile`).
- CLI: `--seed-settings TARGETS` (`all` or comma-separated app ids), `--seed-save NAME`, `--seed-load NAME`, `--seed-list-forms`, `--seed-delete-form NAME`, and `--seed-new` (alongside `+APP[:N]`/`=APP:N` ops that add instances) — `crates/gui/src/cli.rs`.

Steps:
1. Pre-flight `portmux_port_probe_runtime` (`portmux-port configure-app --help`): if portmux is missing or older than `configure-app`, print the install hint once and skip/fail seeding — installed instances remain installed (CLI `--seed-new` counts the unseeded instances and exits failure).
2. Load the packs. Each pack is checked on its own: a pack that fails to parse, still uses `preset` or the `port` verb (refused with the migration message), or has an unsafe `config_path` (absolute, containing `..`, or empty) is refused by name with its reason and skipped, and every other pack still loads. Seeding an app whose pack was refused fails with that reason (counted in `failed`, so the CLI exits non-zero); other apps still seed. The GUI shows a "Some seed packs were skipped" notice naming them; `--list-seed-packs` lists them on stderr and exits non-zero. `SeedPreset::auto_default` still runs, because saved seed forms store its value, but it no longer affects output (backlog: remove it). portmux detects the host itself.
3. Per instance (`seed_until_config`): sweep stale PID files (prefix match, mtime older than 1 hour) from the instance dir, then launch the AppImage in a fresh session — `setsid --fork --wait sh -c '… exec xvfb-run -a …'` — writing the host-side session-leader PID (PGID) to a per-run PID file in the instance dir.
4. Poll the expected config path until it parses as a COMPLETE JSON *object* (`config_file_complete`), up to `SEED_TIMEOUT` (20 s); a partial/in-progress file keeps polling.
5. Teardown the whole process group (xvfb-run + Xvfb + app): TERM then KILL to `-pgid` — natively via the `kill(2)` syscall, inside Flatpak via `flatpak-spawn --host kill` — then remove the PID file.
6. Patch the config, in this order. If the pack has `proxy_setup`, run `portmux-port configure-app --app <recipe> --instance <N> --config <path> --desktop-app-id appimage-control-<app>-<N> --json-v2`, and show any warning or manual step it reports. Then `read_json_object` → `apply_pack` (`static` + `rotation`) → `write_json_object` atomic write: sibling temp `<path>.aic-tmp-<pid>`, `write_all`, `fsync`, `rename` over the original. Then, with `proxy_setup`, run `configure-app --desktop-app-id appimage-control-<app>-<N> --write-only`, so portmux's proxy settings win over a pack key of the same name, read from this instance's own slot.
7. Emit a per-run report: seeded / failed / unsupported / missing counts (CLI prints `Seed settings summary: …`; failure count > 0 → exit failure).

Postconditions:
- Each seeded instance's config file is a valid, complete JSON object containing the pack-applied settings and, with `proxy_setup`, portmux's proxy settings; no seed processes, PID files, or temp files remain.

Invariants:
- The seeded config is never truncated: the atomic temp+fsync+rename write removes the temp file on failure, and any failure in step 6, including one from portmux, restores the first run's config byte-identical. portmux may keep the port it allocated, so a retry finds it.
- PID files are per-run unique: `.appimage-control-seed-host-pid.<pid>.<counter>` (seeder process PID + monotonic atomic invocation counter); the stale sweep removes only files older than 1 hour — never a concurrent run's fresh file.
- Concurrent seeds of the same instance dir never kill each other's process groups: each run reads only its own PID file for the PGID.
- This app holds no port, proxy host or isolation class: portmux resolves and writes them (CLI and GUI behave identically).
- A seed pack's `config_path` stays inside the instance directory: it is checked at load and again after `{appimage}` is substituted.
- One refused pack never seeds its app and never stops the other packs from loading.

Last verified: 2026-11-05 — `2026-11-05-security-audit-remediation-plan.md` Task 2.3: step 2 changed (owner, 2026-11-05: "Skip only the bad pack") — a refused pack is skipped by name and the rest load; core tests on the host. Prior: 2026-11-02 — `2026-11-02-portmux-owns-proxy-setup-sub-02-appimage-control-delegates-plan.md`: steps 1, 2 and 6 now delegate to `portmux-port configure-app`; core tests on the host, CLI subprocess tests and `real_portmux_configure_app` on the Ubuntu VM. Prior: 2026-06-30, `2026-06-30-exhaustive-functional-test-plan.md`.
