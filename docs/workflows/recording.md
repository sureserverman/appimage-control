# Recording — authoring a seed pack by diffing config

ID: WF-RECORD-001

The recorder authors a `static`-only seed pack semi-automatically: it
captures an installed instance's untouched first-run config (baseline),
lets the operator configure the app by hand, then diffs the two and
writes a pack of the changed top-level keys. It is the authoring
counterpart to seeding (WF-SEED-001) — seeding *applies* a pack; the
recorder *produces* one. The output is **unsigned**; the signed
companion-repo delivery pipeline (`--update-seed-packs`) is unchanged and
operator-controlled.

Entry points:
- GUI: **Record Seed Pack…** menu item (`win.record_seed_pack` →
  `crates/gui/src/main.rs` orchestrator → the three modal steps in
  `crates/gui/src/record_dialog.rs`: `choose_record_target`,
  `confirm_manual_step`, `review_and_select`).
- CLI: `--record-pack-baseline APP[:N]` (with optional
  `--pack-config-path PATH` and `--pack-first-run-args A,B`), then
  `--record-pack-capture APP[:N]` (with optional `--out PATH`); plus
  `--list-pack-recordings` and `--delete-pack-recording APP[:N]`
  (`crates/gui/src/cli.rs`).

Steps:
1. **Resolve the config path.** Where the app writes its settings (relative
   to the instance dir, with an optional `{appimage}` placeholder) comes
   from, in order: an explicit `--pack-config-path` / the GUI field; the
   `config_path` of an existing seed pack for the app id; or, CLI-only,
   auto-discovery — `discover_config_candidates` launches the instance
   headlessly and scans for JSON-object files (one candidate → used; zero
   or many → the operator must name it explicitly).
2. **Capture baseline** (`capture_first_run_config` in `crates/core/src/seed.rs`):
   launch the instance once under headless `xvfb-run` via the shared
   `run_headless_session` engine (same setsid / process-group teardown as
   seeding), poll until the config is a complete JSON *object*, and read it
   raw — **no seed pack is applied**. The baseline + metadata (`app_id`,
   `instance_index`, `config_path`, `first_run_args`, detected preset,
   timestamp) is persisted as a `PackRecording` under
   `~/.local/share/appimage-control/pack-recordings/<app-id>.<N>.json`
   (`crates/core/src/seed_record.rs`).
3. **Manual configuration.** The operator launches the app normally
   (the GUI offers a best-effort *Launch app* button), changes settings,
   and quits it. The recorder does not drive this step.
4. **Diff** (`diff_configs` in `crates/core/src/seed_pack_build.rs`): read
   the instance's current config (`read_instance_config`, no launch) and
   compare to the stored baseline at **top-level-key granularity** —
   `apply_pack` replaces whole top-level keys, so a changed nested value
   surfaces as its entire top-level key being captured in full. Keys are
   classified added / changed / removed / unchanged.
5. **Review + synthesise** (`build_static_pack`): added + changed keys
   become the pack's `static` block (taken in full from the current
   config). The GUI shows a checklist (old → new previews) so the operator
   can untick noise; the CLI captures all added/changed keys. Removed keys
   are reported but **not** encoded (a `static` pack cannot delete keys).
6. **Validate + write** (`render_pack_json` → `write_pack_to`): the pack is
   serialised and round-tripped through the real loader + validator before
   any bytes hit disk, then written (pretty JSON + trailing newline) to the
   chosen path — default `~/.local/share/appimage-control/seeds/<app-id>.json`.

Postconditions:
- A valid, loadable, `static`-only seed pack exists at the chosen path,
  carrying exactly the operator-selected added/changed top-level keys; no
  seed processes, PID files, or temp files remain from the baseline launch.
- A baseline recording persists under `pack-recordings/` until explicitly
  deleted (`--delete-pack-recording`), so capture can be retried.

Invariants:
- The baseline is the app's *untouched* first-run config — `capture_first_run_config`
  never applies a seed pack (unlike `seed_instance_settings`).
- The synthesised pack is `static`-only: no `rotation`
  is inferred (that requires human judgement and is hand-added).
- A written pack always re-parses and validates — `write_pack_to` validates
  before writing, so a malformed pack never reaches disk.
- Diff granularity matches `apply_pack` semantics (whole top-level keys),
  so replaying the pack onto the baseline reproduces the operator's choices.
- The recorder never reads config beyond the resolved `config_path`, and
  output is never signed.

Last verified: 2026-06-30 — core capture/diff/synthesis covered by
`crates/core/src/{seed,seed_record,seed_pack_build}.rs` unit tests; CLI
baseline→capture round-trip by `crates/gui/src/cli.rs`
(`record_baseline_then_capture_writes_static_pack_of_changed_keys`); GUI
wiring by `crates/gui/tests/parity_contract.rs`. Interactive GTK walkthrough
not yet run on a live Wayland session.
