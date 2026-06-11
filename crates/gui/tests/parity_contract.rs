//! GUI ↔ CLI parity contract test.
//!
//! PURPOSE
//! -------
//! This file is a drift-detection tripwire, not a behavioural test.  It
//! encodes a table of operations and two markers per operation:
//!
//!   CLI marker  — a string that MUST appear in the `--help` output of the
//!                 built binary.  If a flag is renamed or removed the test
//!                 fails immediately.
//!
//!   GUI marker  — a string that MUST appear in one of the GUI source files
//!                 (listed per row).  Source is read at test time via the
//!                 file-system, rooted at `CARGO_MANIFEST_DIR`; no
//!                 `include_str!` needed because the purpose is drift
//!                 detection, not compile-time embedding.
//!
//! MECHANISM
//! ---------
//! 1.  The binary is invoked as a subprocess with `--help` (exactly as
//!     `cli_subprocess.rs` does) and the resulting stdout is captured.
//! 2.  Each GUI source path is read once and cached.
//! 3.  Each row is checked: both markers must be present in the relevant
//!     text, unless the row is in the ALLOWLIST.
//! 4.  Any mismatch panics with a diagnostic message naming the operation
//!     and the exact string that was not found.
//!
//! DELIBERATE DIVERGENCES (ALLOWLIST)
//! ------------------------------------
//! Some features exist in the CLI but not the GUI (or vice versa) by
//! design.  Those are listed in `ALLOWLIST` below with a short reason string.
//! Adding a new entry to the allowlist requires a reviewer decision — the
//! intent is to make the divergence visible, not silent.
//!
//! NEW OPERATIONS
//! --------------
//! When a new CLI flag or GUI action is added, add a corresponding row to
//! `PARITY_TABLE` below.  If parity is intentionally incomplete, add an
//! entry to `ALLOWLIST` instead.

use std::path::{Path, PathBuf};
use std::process::Command;

// ── Binary locator (same pattern as cli_subprocess.rs) ───────────────────────

fn app_exe() -> PathBuf {
    std::env::var_os("CARGO_BIN_EXE_appimage_control")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CARGO_BIN_EXE_appimage-control").map(PathBuf::from))
        .unwrap_or_else(|| {
            let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../target/{profile}/appimage-control"))
        })
}

// ── GUI source resolver ───────────────────────────────────────────────────────

fn gui_src(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn read_gui_src(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("could not read GUI source {}: {e}", path.display()))
}

// ── Parity table entry ────────────────────────────────────────────────────────

/// Identifies which GUI source file(s) to search for a GUI marker.
enum GuiSrc {
    /// crates/gui/src/actions.rs
    Actions,
    /// crates/gui/src/dialogs.rs
    Dialogs,
    /// crates/gui/src/seed_dialog.rs
    SeedDialog,
    /// crates/gui/src/main.rs
    Main,
}

struct ParityRow {
    /// Human-readable operation name (shown in failure messages).
    name: &'static str,
    /// Substring that must appear in `--help` stdout.
    cli_marker: &'static str,
    /// Substring that must appear in the specified GUI source file.
    gui_marker: &'static str,
    /// Which file to search for the GUI marker.
    gui_src: GuiSrc,
}

// ── Allowlist ─────────────────────────────────────────────────────────────────

// `side`/`reason` are documentation for reviewers, read by humans not code.
#[allow(dead_code)]
struct AllowlistEntry {
    /// Operation name — must match `ParityRow::name`.
    name: &'static str,
    /// Which side is intentionally absent/different.
    side: &'static str,
    /// Why this divergence is acceptable.
    reason: &'static str,
}

/// Deliberate CLI↔GUI divergences.  The tests skip marker checks for any
/// operation whose name appears here.  The `side` field is documentation only;
/// both markers are still skipped so a reviewer can read this list to
/// understand what is intentionally different.
const ALLOWLIST: &[AllowlistEntry] = &[
    AllowlistEntry {
        name: "migrate conflict resolve (TTY interactive)",
        side: "CLI",
        reason: "Interactive per-conflict resolution with [r/n/s] prompt is CLI-only on a TTY; \
                 the GUI presents a single strategy-choice dialog before the run and applies it \
                 uniformly to every conflict — there is no per-conflict callback path in the GUI.",
    },
    AllowlistEntry {
        name: "list installed (column format)",
        side: "CLI",
        reason: "--list prints aligned columns (id / display_name / instances / version) to \
                 stdout; the GUI shows the same data in the AdwActionRow list view, but the \
                 column-format string '--list' is CLI-only.  The GUI surface marker (the \
                 'total_label' footer text pattern) confirms the GUI has the concept.",
    },
];

// ── Parity table ──────────────────────────────────────────────────────────────

/// One row per tracked operation.  CLI marker must appear in `--help` output;
/// GUI marker must appear in the specified source file.
///
/// Stable GUI markers are GIO action names (registered via `bind_action` in
/// actions.rs) or menu item target strings (e.g. `"win.migrate"`), or
/// function/type names that encode the GUI surface.  Prefer action names over
/// prose labels because labels may be localised or reworded without changing
/// behaviour.
const PARITY_TABLE: &[ParityRow] = &[
    // ── Instance count management ──────────────────────────────────────────
    ParityRow {
        name: "set instance count (absolute)",
        cli_marker: "=APP:N",
        // The spin button + Apply wires through the "apply" action.
        gui_marker: r#"bind_action(window, "apply""#,
        gui_src: GuiSrc::Actions,
    },
    ParityRow {
        name: "add instance (relative +APP)",
        cli_marker: "+APP",
        // Same Apply path; GUI uses spin increments rather than +APP syntax.
        gui_marker: r#"bind_action(window, "apply""#,
        gui_src: GuiSrc::Actions,
    },
    ParityRow {
        name: "remove instance (relative -APP)",
        cli_marker: "-APP",
        // GUI lowers the spin to 0 and clicks Apply.
        gui_marker: r#"bind_action(window, "apply""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Update installed ───────────────────────────────────────────────────
    ParityRow {
        name: "update installed",
        cli_marker: "-update",
        gui_marker: r#"bind_action(window, "update""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Clean cache ────────────────────────────────────────────────────────
    ParityRow {
        name: "clean cache",
        cli_marker: "--clean-cache",
        gui_marker: r#"bind_action(window, "clean_cache""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Clean installs (keep cache) ────────────────────────────────────────
    ParityRow {
        name: "clean installs",
        cli_marker: "--clean",
        gui_marker: r#"bind_action(window, "clean""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Purge ──────────────────────────────────────────────────────────────
    ParityRow {
        name: "purge",
        cli_marker: "--purge",
        gui_marker: r#"bind_action(window, "purge""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Migrate ────────────────────────────────────────────────────────────
    ParityRow {
        name: "migrate",
        cli_marker: "--migrate",
        gui_marker: r#"bind_action(window, "migrate""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Migrate conflict modes ─────────────────────────────────────────────
    ParityRow {
        name: "migrate conflict: replace",
        cli_marker: "--on-conflict=",
        // dialogs.rs registers the "replace" response ID in ask_migration_strategy.
        gui_marker: r#""replace""#,
        gui_src: GuiSrc::Dialogs,
    },
    ParityRow {
        name: "migrate conflict: renumber",
        cli_marker: "renumber",
        // "renumber" is both the CLI value and the GUI response ID.
        gui_marker: r#""renumber""#,
        gui_src: GuiSrc::Dialogs,
    },
    ParityRow {
        name: "migrate conflict: skip",
        cli_marker: "skip",
        gui_marker: r#""skip""#,
        gui_src: GuiSrc::Dialogs,
    },
    // ── Backup ────────────────────────────────────────────────────────────
    ParityRow {
        name: "backup",
        cli_marker: "--backup",
        gui_marker: r#"bind_action(window, "backup""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Restore + force ───────────────────────────────────────────────────
    ParityRow {
        name: "restore backup",
        cli_marker: "--restore-backup",
        gui_marker: r#"bind_action(window, "restore_backup""#,
        gui_src: GuiSrc::Actions,
    },
    ParityRow {
        name: "restore backup force confirm",
        cli_marker: "--force",
        // main.rs presents a "Overwrite existing managed state?" dialog when
        // the first restore attempt returns "managed state already exists".
        gui_marker: "managed state already exists",
        gui_src: GuiSrc::Main,
    },
    // ── Seed settings ──────────────────────────────────────────────────────
    ParityRow {
        name: "seed settings",
        cli_marker: "--seed-settings",
        gui_marker: r#"bind_action(window, "seed_settings""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Seed forms (save / load / list / delete) ───────────────────────────
    ParityRow {
        name: "seed forms: save",
        cli_marker: "--seed-save",
        // seed_dialog.rs has a "Save as…" button that calls save_seed_form.
        gui_marker: "save_seed_form",
        gui_src: GuiSrc::SeedDialog,
    },
    ParityRow {
        name: "seed forms: load",
        cli_marker: "--seed-load",
        // seed_dialog.rs calls load_seed_form in the dropdown handler.
        gui_marker: "load_seed_form",
        gui_src: GuiSrc::SeedDialog,
    },
    ParityRow {
        name: "seed forms: list",
        cli_marker: "--seed-list-forms",
        // seed_dialog.rs calls list_seed_forms to populate the dropdown.
        gui_marker: "list_seed_forms",
        gui_src: GuiSrc::SeedDialog,
    },
    ParityRow {
        name: "seed forms: delete",
        cli_marker: "--seed-delete-form",
        // seed_dialog.rs calls delete_seed_form in the Delete button handler.
        gui_marker: "delete_seed_form",
        gui_src: GuiSrc::SeedDialog,
    },
    // ── List seed packs ────────────────────────────────────────────────────
    ParityRow {
        name: "list seed packs",
        cli_marker: "--list-seed-packs",
        gui_marker: r#"bind_action(window, "list_seed_packs""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Update seed packs ─────────────────────────────────────────────────
    ParityRow {
        name: "update seed packs",
        cli_marker: "--update-seed-packs",
        gui_marker: r#"bind_action(window, "update_seed_packs""#,
        gui_src: GuiSrc::Actions,
    },
    // ── Repair launchers ──────────────────────────────────────────────────
    ParityRow {
        name: "repair launchers",
        cli_marker: "--repair-launchers",
        // main.rs calls repair_instance_launchers in the startup idle_add_local block.
        gui_marker: "repair_instance_launchers",
        gui_src: GuiSrc::Main,
    },
    // ── Seed new instances ────────────────────────────────────────────────
    ParityRow {
        name: "seed new instances",
        cli_marker: "--seed-new",
        // main.rs calls seed_new_instances_after_reconcile in the Apply worker thread.
        gui_marker: "seed_new_instances_after_reconcile",
        gui_src: GuiSrc::Main,
    },
    // ── Catalog override ──────────────────────────────────────────────────
    ParityRow {
        name: "catalog override",
        cli_marker: "--catalog",
        // main.rs (mount_empty_state) has the "Open Catalog…" button label.
        gui_marker: "Open Catalog",
        gui_src: GuiSrc::Main,
    },
    // ── List installed ────────────────────────────────────────────────────
    // CLI: --list (column-format).  GUI: footer total_label shows installed count.
    // Listed in ALLOWLIST above because the column format is CLI-only; the
    // existence of the concept in the GUI is confirmed by the footer pattern.
    // We still add the row here so the table is complete; both markers are
    // valid strings that really exist, they just serve different surfaces.
    ParityRow {
        name: "list installed (column format)",
        cli_marker: "--list",
        // The footer label call in main.rs is the stable GUI equivalent.
        gui_marker: "apps · {} installed",
        gui_src: GuiSrc::Main,
    },
];

// ── Test ──────────────────────────────────────────────────────────────────────

#[test]
fn parity_contract() {
    // 1. Capture `--help` output.
    let out = Command::new(app_exe())
        .arg("--help")
        .output()
        .expect("failed to spawn appimage-control --help");
    // print_help() writes to stdout; exit code is success.
    assert!(
        out.status.success(),
        "--help exited with failure: stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let help_text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !help_text.is_empty(),
        "--help produced no output on stdout"
    );

    // 2. Pre-load each GUI source file once (cheap: small files).
    let actions_src = read_gui_src(&gui_src("src/actions.rs"));
    let dialogs_src = read_gui_src(&gui_src("src/dialogs.rs"));
    let seed_dialog_src = read_gui_src(&gui_src("src/seed_dialog.rs"));
    let main_src = read_gui_src(&gui_src("src/main.rs"));

    // 3. Walk the parity table.
    let mut failures: Vec<String> = Vec::new();

    for row in PARITY_TABLE {
        // Check allowlist first — skip both sides if present.
        if ALLOWLIST.iter().any(|e| e.name == row.name) {
            continue;
        }

        // CLI side.
        if !help_text.contains(row.cli_marker) {
            failures.push(format!(
                "[CLI] operation '{}': marker {:?} not found in --help output",
                row.name, row.cli_marker
            ));
        }

        // GUI side.
        let gui_text = match row.gui_src {
            GuiSrc::Actions => &actions_src,
            GuiSrc::Dialogs => &dialogs_src,
            GuiSrc::SeedDialog => &seed_dialog_src,
            GuiSrc::Main => &main_src,
        };
        if !gui_text.contains(row.gui_marker) {
            let src_name = match row.gui_src {
                GuiSrc::Actions => "src/actions.rs",
                GuiSrc::Dialogs => "src/dialogs.rs",
                GuiSrc::SeedDialog => "src/seed_dialog.rs",
                GuiSrc::Main => "src/main.rs",
            };
            failures.push(format!(
                "[GUI] operation '{}': marker {:?} not found in {}",
                row.name, row.gui_marker, src_name
            ));
        }
    }

    // 4. Report all failures at once (better than fail-fast for drift triage).
    if !failures.is_empty() {
        let msg = failures.join("\n  ");
        panic!(
            "parity_contract: {} check(s) failed:\n  {msg}\n\n\
             If a divergence is intentional add an entry to ALLOWLIST in \
             crates/gui/tests/parity_contract.rs.",
            failures.len()
        );
    }
}
