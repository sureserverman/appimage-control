# Migrate — legacy portable dirs into managed instances

ID: WF-MIGRATE-001

Entry points:
- GUI: **Preferences → Storage → Migrate legacy directories** (`win.migrate`; relocated from the primary menu in the 2026-07-01 redesign) — strategy dialog `ask_migration_strategy` (`crates/gui/src/dialogs.rs`) offers Replace existing / Add as new instance (renumber, suggested default) / Skip conflicting / Cancel; one strategy applies uniformly to every conflict in the run.
- CLI: `--migrate [-y|--yes] [--on-conflict=replace|renumber|skip]` (`crates/gui/src/cli.rs`) — run confirmation prompt unless `-y`; without `--on-conflict` each conflict gets an interactive TTY prompt `[r]eplace / [n]ew instance / [s]kip`; non-TTY stdin or `-y` without `--on-conflict` aborts on the first conflict with a hint to pass `--on-conflict=…`.

Steps (`crates/core/src/migrate.rs::run_portable_dir_migration`):
1. `discover_migration_candidates`: scan the current directory for dirs matching catalog filesystem slugs — `<slug>` for instance 1, `<slug><digits>` for that instance number.
2. Per app, reconcile the instance count up to max(highest candidate index, highest existing install) via `reconcile_instance_count_for_portable_migration` (no empty portable-dir provisioning; sourced from WF-APPLY-001 machinery).
3. Per candidate: if the destination `<AppImage>.home` is non-empty, resolve the conflict — fixed `--on-conflict` value, GUI run-wide strategy, TTY per-conflict prompt, or abort (non-interactive without a fixed strategy).
4. Apply the resolution: **skip** leaves source and destination untouched; **replace** removes the conflicting destination `*.home` (and its non-empty sibling `*.config`) only; **renumber** allocates `next_instance_index_after_occupied`, reconciles up, and retargets the move.
5. Move the source dir onto `<AppImage>.home`: `fs::rename` first; on `EXDEV` (cross-device), staged copy to sibling `.{name}.aic-staging-{pid}`, rename the completed copy into place, and only then delete the source.
6. Post-move: Cryptomator instances run their extract step after legacy data lands on `*.home`; an empty `*.config` is created if missing.

Postconditions:
- Each migrated source dir's contents live at the managed instance's `*.home`; the instance has a launcher and binary; skipped sources remain in place untouched.

Invariants:
- The source portable dir survives ANY failure: cross-device moves go through staged copy + rename before source deletion — a failed copy or rename removes the staging residue and leaves the source untouched with no partial destination (`move_tree_via_staged_copy`).
- Replace deletes ONLY the conflicting destination (`*.home` and its sibling `*.config` for that instance) — never other instances' data.
- Renumber allocates the slot AFTER the highest occupied index (managed install or non-empty portable `*.home`) and never fills gaps — an empty `*.home` at index 2 with 1 and 3 occupied yields index 4, not 2 (`next_instance_index_after_occupied`).

Last verified: 2026-07-01 — entry point relocated to Preferences → Storage (design-handoff redesign); strategy/behavior unchanged. Prior: 2026-06-30 — re-verified end-to-end by the `2026-06-30-exhaustive-functional-test-plan.md` behavioral pass (no behavior change); 2026-06-11 against commit `0b568eaf` (`2026-06-10-workflow-flaws-parity-remediation-plan.md`).
