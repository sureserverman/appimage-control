//! Non-GTK command-line mode for the `appimage-control` binary.

use std::io::{self, BufRead, IsTerminal, Write};

use gtk::glib;

use appimage_control_core::{
    clean_catalog_download_cache, clean_installed_catalog_apps_keep_cache, create_backup_archive,
    delete_seed_form, discover_migration_candidates, fetch_github_release_hints,
    installed_version_fleet_display, instance_count, list_seed_forms, load_seed_form,
    portmux_port_probe_runtime, parse_app_instance_spec, purge_appimage_control_state,
    reconcile_catalog_install_version_metadata, reconcile_instance_count, restore_backup_archive,
    run_portable_dir_migration, save_seed_form, seed_supported_installed_apps,
    update_all_installed_instances, update_seed_packs, AppPaths, BackupOptions, Catalog,
    MigrationConflict, MigrationResolution, SavedSeedForm, SeedPackLibrary, SeedPreset,
};

pub fn want_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "--help" || a == "-h")
}

pub fn print_help() {
    print!(
        "\
AppImage Control — command line

With no arguments, the graphical app starts.

  --help, -h       Print this message and exit (does not open the GUI).

Catalog JSON is resolved like the GUI: environment APPIMAGE_CONTROL_CATALOG (non-empty), then
data/catalog.json (current dir or any parent, then from the binary path), then /app/share/ or /usr/share/appimage-control/catalog.json, then the catalog compiled into the binary.

Operations (unrecognized arguments are skipped). If nothing runs (e.g. only unknown ids), the GUI starts.
When --purge, --clean, or --migrate is present, only one runs: --purge first if present, else --clean, else --migrate (other operation flags are ignored for that run).

  --list           Print catalog apps, installed instance counts, and per-instance version summary (aligned columns).
  --clean-cache    Delete ~/.local/share/appimages/cache/ (downloaded catalog AppImages) and recreate an empty cache dir.
  --clean          Remove all installed catalog app instances, managed .desktop files, and appimage-control hicolor PNGs; keep ~/.local/share/appimages/cache/ (downloaded AppImages). Asks Yes/No [Y/n] unless you pass -y or --yes.
  --purge          Remove all catalog installs, cache, standalone files under files/, manager icons, managed .desktop files, and appimage-control hicolor PNGs. Asks Yes/No [Y/n] unless you pass -y or --yes.
  --migrate        Move legacy portable data dirs in the current directory into catalog instance *.home trees (see naming below). Asks Yes/No [Y/n] unless you pass -y or --yes.
  --on-conflict=X  With --migrate only: deterministic collision handling (replace, renumber, or skip). Required when stdin is not a TTY or with -y/--yes if a collision can occur.
  --backup PATH    Create a manager-state backup archive at PATH (.tar.zst recommended).
  --restore-backup PATH
                   Restore manager state from PATH.
  --force          With --restore-backup only: overwrite existing manager-owned state.
  --seed-settings TARGETS
                   Seed supported app settings for installed instances. TARGETS is `all`,
                   one app id, or a comma-separated list like `feather,electrum-btc`.
                   The host is auto-detected to pick the right networking source of truth;
                   no manual switch needed.
  --seed-save NAME Save the current --seed-settings selection under NAME for later recall,
                   then run the seed. Form is stored under
                   ~/.local/share/appimage-control/seed-forms/<NAME>.json. Name must match
                   [A-Za-z0-9._-]+ (≤ 64 chars) and must not start with '.'.
  --seed-load NAME Load a previously-saved seed form named NAME and run it. Mutually
                   exclusive with --seed-settings.
  --seed-list-forms
                   Print one saved seed-form name per line (alphabetical) and exit.
  --seed-delete-form NAME
                   Delete a saved seed form by name and exit. Idempotent — missing forms
                   exit 0 with a note.
  --list-seed-packs
                   Print one tab-separated row per loaded seed pack:
                   `<app-id>\\t<source-path-or-embedded>`. Useful to confirm which
                   override the discovery chain (APPIMAGE_CONTROL_SEEDS_DIR, ancestor
                   walk, /app/share/ or /usr/share/, embedded) picked.
  --update-seed-packs
                   Fetch the latest signed seed-pack tarball from the companion
                   GitHub repo, verify the ed25519 signature against the embedded
                   public key, and install into ~/.local/share/appimage-control/seeds/.
                   Refuses if the tarball or signature does not verify, or if this
                   build was shipped with a placeholder public key (see
                   docs/seed-pack-companion-repo.md).
  -y, --yes        Use with --purge, --clean, or --migrate: skip the run confirmation prompt (for scripts).
  +APP             Add 1 instance of APP (capped by the catalog instance limit).
  +APP:N           Add N instances of APP (same cap).
  -APP             Remove all instances of APP.
  -APP:N           Remove N instances (count not below 0).
  -update          Update installed instances (same as the GUI): skips when already up to date; otherwise tries appimageupdatetool, then downloads the catalog build or—when the catalog URL is behind GitHub releases/latest—the matching latest-release .AppImage asset.
  --update         Same as -update

Examples:

  appimage-control --list
  appimage-control --clean-cache
  appimage-control --clean
  appimage-control --purge
  appimage-control --purge -y
  appimage-control --purge --yes
  appimage-control --migrate
  appimage-control --migrate -y --on-conflict=renumber
  appimage-control --backup /tmp/aic-backup.tar.zst
  appimage-control --restore-backup /tmp/aic-backup.tar.zst --force
  appimage-control --seed-settings all
  appimage-control +feather --seed-settings feather
  appimage-control --seed-settings feather,electrum-btc --seed-save daily
  appimage-control --seed-load daily
  appimage-control --seed-list-forms
  appimage-control --seed-delete-form tor-trio
  appimage-control +mattermost:3
  appimage-control -feather
  appimage-control -feather:2 -update
"
    );
}

/// True if any argument selects CLI mode (excluding -h/--help, handled separately).
pub fn has_cli_ops(args: &[String]) -> bool {
    args.iter().any(|a| {
        if a == "-h" || a == "--help" || a == "-y" || a == "--yes" {
            return false;
        }
        a == "--list"
            || a == "--clean-cache"
            || a == "--clean"
            || a == "--purge"
            || a == "--migrate"
            || a == "--backup"
            || a == "--restore-backup"
            || a == "--force"
            || a == "--seed-settings"
            || a == "--seed-save"
            || a == "--seed-load"
            || a == "--seed-list-forms"
            || a == "--seed-delete-form"
            || a == "--list-seed-packs"
            || a == "--update-seed-packs"
            || a == "-update"
            || a == "--update"
            || (a.starts_with('+') && a.len() > 1)
            || (a.starts_with('-') && !a.starts_with("--") && a.len() > 1)
    })
}

/// Only `--clean-cache` (and optional `-h`/`--help`): no catalog JSON required.
fn only_clean_cache_args(args: &[String]) -> bool {
    let mut any = false;
    for a in args {
        if a == "-h" || a == "--help" {
            continue;
        }
        if a == "--purge"
            || a == "--clean"
            || a == "--migrate"
            || a == "--yes"
            || a == "-y"
            || a.starts_with("--on-conflict=")
        {
            return false;
        }
        if a == "--clean-cache" {
            any = true;
        } else {
            return false;
        }
    }
    any
}

fn args_contain_purge(args: &[String]) -> bool {
    args.iter().any(|a| a == "--purge")
}

fn args_contain_clean(args: &[String]) -> bool {
    args.iter().any(|a| a == "--clean")
}

fn args_contain_migrate(args: &[String]) -> bool {
    args.iter().any(|a| a == "--migrate")
}

fn parse_migrate_on_conflict(
    args: &[String],
) -> std::result::Result<Option<MigrationResolution>, String> {
    for a in args {
        let Some(rest) = a.strip_prefix("--on-conflict=") else {
            continue;
        };
        let r = match rest.to_ascii_lowercase().as_str() {
            "replace" => MigrationResolution::Replace,
            "renumber" => MigrationResolution::Renumber,
            "skip" => MigrationResolution::Skip,
            other => {
                return Err(format!(
                    "invalid --on-conflict={other} (use replace, renumber, or skip)"
                ));
            }
        };
        return Ok(Some(r));
    }
    Ok(None)
}

fn arg_value<'a>(args: &'a [String], flag: &str) -> std::result::Result<Option<&'a str>, String> {
    let mut i = 0usize;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 >= args.len() {
                return Err(format!("{flag} requires a path argument"));
            }
            return Ok(Some(args[i + 1].as_str()));
        }
        i += 1;
    }
    Ok(None)
}

fn parse_seed_settings_targets(
    args: &[String],
) -> std::result::Result<Option<Option<Vec<String>>>, String> {
    let Some(raw) = arg_value(args, "--seed-settings")? else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("--seed-settings requires `all` or one or more app ids".into());
    }
    if trimmed.eq_ignore_ascii_case("all") {
        return Ok(Some(None));
    }
    let parts: Vec<String> = trimmed
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    if parts.is_empty() {
        return Err("--seed-settings requires `all` or one or more app ids".into());
    }
    Ok(Some(Some(parts)))
}

fn resolve_migrate_conflict_tty(c: MigrationConflict) -> MigrationResolution {
    loop {
        eprintln!(
            "Destination already exists: {}",
            c.conflicting_home.display()
        );
        eprintln!(
            "  Source: {}  (app {}, instance {})",
            c.source_path.display(),
            c.catalog_app_id,
            c.requested_instance
        );
        eprint!("[r]eplace / [n]ew instance (next # after your highest) / [s]kip: ");
        let _ = io::stderr().flush();
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).is_err() {
            return MigrationResolution::Skip;
        }
        match line.trim().to_ascii_lowercase().as_str() {
            "r" | "replace" => return MigrationResolution::Replace,
            "n" | "renumber" | "new" => return MigrationResolution::Renumber,
            "s" | "skip" => return MigrationResolution::Skip,
            "" => {}
            _ => eprintln!("Enter r, n, or s (n = new instance number after your existing fleet)."),
        }
    }
}

fn parse_y_n_default_y(line: &str) -> Option<bool> {
    let t = line.trim();
    if t.is_empty() {
        return Some(true);
    }
    if t.eq_ignore_ascii_case("y") || t.eq_ignore_ascii_case("yes") {
        return Some(true);
    }
    if t.eq_ignore_ascii_case("n") || t.eq_ignore_ascii_case("no") {
        return Some(false);
    }
    eprintln!("Please answer y or n (default: y). Cancelled.");
    Some(false)
}

fn confirm_unless_yes_flag(args: &[String], intro: &str, prompt: &str) -> Option<bool> {
    if args.iter().any(|a| a == "--yes" || a == "-y") {
        return Some(true);
    }
    eprintln!("{intro}");
    eprint!("{prompt}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    if io::stdin().lock().read_line(&mut line).is_err() {
        eprintln!("\nCould not read confirmation.");
        return None;
    }
    parse_y_n_default_y(&line)
}

fn cell_display_width(s: &str) -> usize {
    s.chars().count()
}

fn print_list(paths: &AppPaths, catalog: &Catalog) {
    let _ = reconcile_catalog_install_version_metadata(paths, catalog, &mut |_| {});
    let headers = ["id", "display_name", "instances", "version"];
    let mut rows: Vec<[String; 4]> = Vec::new();
    for app in &catalog.apps {
        let n = instance_count(paths, app).unwrap_or(0);
        let ver = installed_version_fleet_display(paths, app)
            .ok()
            .map(|d| d.label);
        rows.push([
            app.id.clone(),
            app.display_name_resolved(),
            n.to_string(),
            ver.unwrap_or_else(|| "installed unknown".into()),
        ]);
    }

    let mut w = [
        cell_display_width(headers[0]),
        cell_display_width(headers[1]),
        cell_display_width(headers[2]),
        cell_display_width(headers[3]),
    ];
    for r in &rows {
        for i in 0..4 {
            w[i] = w[i].max(cell_display_width(&r[i]));
        }
    }

    println!(
        "{:<wi$}  {:<wn$}  {:>wc$}  {:<wv$}",
        headers[0],
        headers[1],
        headers[2],
        headers[3],
        wi = w[0],
        wn = w[1],
        wc = w[2],
        wv = w[3],
    );
    for r in rows {
        println!(
            "{:<wi$}  {:<wn$}  {:>wc$}  {:<wv$}",
            r[0],
            r[1],
            r[2],
            r[3],
            wi = w[0],
            wn = w[1],
            wc = w[2],
            wv = w[3],
        );
    }
}

fn home_dir_or_exit() -> std::result::Result<std::path::PathBuf, glib::ExitCode> {
    match dirs::home_dir() {
        Some(h) => Ok(h),
        None => {
            eprintln!("Cannot resolve home directory ($HOME unset).");
            Err(glib::ExitCode::FAILURE)
        }
    }
}

/// Returns `None` if nothing ran (e.g. unknown ids) so the caller can start the GUI instead.
pub fn run_cli_ops(args: &[String]) -> Option<glib::ExitCode> {
    if args.iter().any(|a| a == "--seed-list-forms") {
        let home = match home_dir_or_exit() {
            Ok(h) => h,
            Err(code) => return Some(code),
        };
        match list_seed_forms(&home) {
            Ok(names) => {
                for n in names {
                    println!("{n}");
                }
                return Some(glib::ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    if let Some(name) = match arg_value(args, "--seed-delete-form") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    } {
        let home = match home_dir_or_exit() {
            Ok(h) => h,
            Err(code) => return Some(code),
        };
        match delete_seed_form(&home, name) {
            Ok(true) => {
                eprintln!("Deleted saved seed form '{name}'.");
                return Some(glib::ExitCode::SUCCESS);
            }
            Ok(false) => {
                eprintln!("No saved seed form named '{name}'.");
                return Some(glib::ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    if args.iter().any(|a| a == "--list-seed-packs") {
        match SeedPackLibrary::discover_load(None) {
            Ok(lib) => {
                for (id, src) in lib.list_rows() {
                    println!("{id}\t{src}");
                }
                return Some(glib::ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    if args.iter().any(|a| a == "--update-seed-packs") {
        match update_seed_packs(None, &mut |s| eprintln!("{s}")) {
            Ok(report) => {
                eprintln!(
                    "Installed {} seed packs to {} from {}",
                    report.installed_count,
                    report.install_dir.display(),
                    report.source_url
                );
                return Some(glib::ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    let paths = match AppPaths::from_env() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };

    let backup_path = match arg_value(args, "--backup") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };
    let restore_path = match arg_value(args, "--restore-backup") {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };
    let force_restore = args.iter().any(|a| a == "--force");
    let mut seed_targets = match parse_seed_settings_targets(args) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };
    let seed_save_name = match arg_value(args, "--seed-save") {
        Ok(v) => v.map(|s| s.to_string()),
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };
    let seed_load_name = match arg_value(args, "--seed-load") {
        Ok(v) => v.map(|s| s.to_string()),
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };

    if backup_path.is_some() && restore_path.is_some() {
        eprintln!("Use either --backup or --restore-backup, not both in one run.");
        return Some(glib::ExitCode::FAILURE);
    }
    if force_restore && restore_path.is_none() {
        eprintln!("--force is only valid with --restore-backup PATH.");
        return Some(glib::ExitCode::FAILURE);
    }
    if (backup_path.is_some() || restore_path.is_some())
        && (args_contain_purge(args) || args_contain_clean(args) || args_contain_migrate(args))
    {
        eprintln!("Backup/restore flags cannot be combined with --purge/--clean/--migrate.");
        return Some(glib::ExitCode::FAILURE);
    }

    if let Some(name) = seed_load_name.as_deref() {
        if seed_targets.is_some() {
            eprintln!("--seed-load is mutually exclusive with --seed-settings.");
            return Some(glib::ExitCode::FAILURE);
        }
        if seed_save_name.is_some() {
            eprintln!("--seed-load cannot be combined with --seed-save.");
            return Some(glib::ExitCode::FAILURE);
        }
        let home = match home_dir_or_exit() {
            Ok(h) => h,
            Err(code) => return Some(code),
        };
        match load_seed_form(&home, name) {
            // The saved preset is intentionally ignored: networking
            // source-of-truth is resolved fresh from the runtime
            // environment, not whatever happened to be active at save
            // time. We only restore the app-id selection.
            Ok(form) => {
                seed_targets = Some(Some(form.app_ids));
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    if seed_targets.is_some()
        && (args_contain_purge(args) || args_contain_clean(args) || args_contain_migrate(args))
    {
        eprintln!("--seed-settings cannot be combined with --purge/--clean/--migrate.");
        return Some(glib::ExitCode::FAILURE);
    }
    if seed_save_name.is_some() && seed_targets.is_none() {
        eprintln!("--seed-save requires --seed-settings.");
        return Some(glib::ExitCode::FAILURE);
    }

    if let (Some(name), Some(Some(ids))) = (seed_save_name.as_deref(), seed_targets.clone()) {
        let home = match home_dir_or_exit() {
            Ok(h) => h,
            Err(code) => return Some(code),
        };
        // Persist with the runtime-resolved preset so the file is
        // self-describing for diagnostics; the value will be ignored
        // on subsequent --seed-load runs (host is re-detected each
        // time).
        match SavedSeedForm::new(name, SeedPreset::auto_default(), ids) {
            Ok(form) => {
                if let Err(e) = save_seed_form(&home, &form) {
                    eprintln!("{e}");
                    return Some(glib::ExitCode::FAILURE);
                }
                eprintln!("Saved seed form '{}'.", form.name);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    } else if seed_save_name.is_some() && matches!(seed_targets, Some(None)) {
        // `--seed-settings all` doesn't yield a concrete app-id list,
        // so refuse to save a form that means "everything installed at
        // some past moment" — that's not a stable selection.
        eprintln!(
            "--seed-save needs an explicit app-id list (cannot save `--seed-settings all` as a form)."
        );
        return Some(glib::ExitCode::FAILURE);
    }

    if let Some(path) = backup_path {
        let catalog = match Catalog::discover_load(None) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        };
        match create_backup_archive(
            &paths,
            &catalog,
            std::path::Path::new(path),
            &BackupOptions::default(),
        ) {
            Ok(rep) => {
                eprintln!(
                    "Backup created: {} ({} entries, {} bytes)",
                    rep.archive_path.display(),
                    rep.entry_count,
                    rep.total_bytes
                );
                return Some(glib::ExitCode::SUCCESS);
            }
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        }
    }

    if let Some(path) = restore_path {
        if let Err(e) = restore_backup_archive(&paths, std::path::Path::new(path), force_restore) {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
        return Some(glib::ExitCode::SUCCESS);
    }

    if args_contain_purge(args) {
        match confirm_unless_yes_flag(
            args,
            "This removes all AppImage Control catalog installs, download cache, standalone files under files/, manager icons, managed .desktop entries, and appimage-control icon PNGs under ~/.local/share/icons/hicolor.",
            "Proceed with purge? [Y/n] ",
        ) {
            None => return Some(glib::ExitCode::FAILURE),
            Some(false) => {
                eprintln!("Purge cancelled.");
                return Some(glib::ExitCode::SUCCESS);
            }
            Some(true) => {}
        }
        let catalog = Catalog::discover_load(None).ok();
        if let Err(e) = purge_appimage_control_state(&paths, catalog.as_ref(), &mut |m| {
            eprintln!("{m}");
        }) {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
        return Some(glib::ExitCode::SUCCESS);
    }

    if args_contain_clean(args) {
        match confirm_unless_yes_flag(
            args,
            "This removes every installed catalog app instance (under ~/.local/share/appimages/apps/), managed .desktop files, and appimage-control icon PNGs under ~/.local/share/icons/hicolor. Downloaded catalog AppImages in ~/.local/share/appimages/cache/ are not deleted.",
            "Proceed with clean? [Y/n] ",
        ) {
            None => return Some(glib::ExitCode::FAILURE),
            Some(false) => {
                eprintln!("Clean cancelled.");
                return Some(glib::ExitCode::SUCCESS);
            }
            Some(true) => {}
        }
        let catalog = match Catalog::discover_load(None) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        };
        if let Err(e) =
            clean_installed_catalog_apps_keep_cache(&paths, &catalog, &mut |m| eprintln!("{m}"))
        {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
        return Some(glib::ExitCode::SUCCESS);
    }

    if args_contain_migrate(args) {
        match confirm_unless_yes_flag(
            args,
            "This moves legacy portable data directories from the current folder into ~/.local/share/appimages/apps/<app>/<instance>/ (as *.home). Directory names must match each catalog app filesystem slug (see catalog id / instance_subdir): <slug> for instance 1, or <slug> followed by digits for that instance number (e.g. session, session2).",
            "Proceed with migrate? [Y/n] ",
        ) {
            None => return Some(glib::ExitCode::FAILURE),
            Some(false) => {
                eprintln!("Migrate cancelled.");
                return Some(glib::ExitCode::SUCCESS);
            }
            Some(true) => {}
        }
        let on_conflict = match parse_migrate_on_conflict(args) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        };
        let catalog = match Catalog::discover_load(None) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        };
        let cwd = match std::env::current_dir() {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
        };
        let candidates = discover_migration_candidates(&cwd, &catalog);
        let skip_yes = args.iter().any(|a| a == "-y" || a == "--yes");
        let abort_unresolved = on_conflict.is_none() && (!io::stdin().is_terminal() || skip_yes);
        if let Err(e) = run_portable_dir_migration(
            &paths,
            &catalog,
            &candidates,
            on_conflict,
            abort_unresolved,
            resolve_migrate_conflict_tty,
            &mut |m| eprintln!("{m}"),
        ) {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
        return Some(glib::ExitCode::SUCCESS);
    }

    if only_clean_cache_args(args) {
        if let Err(e) = clean_catalog_download_cache(&paths) {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
        return Some(glib::ExitCode::SUCCESS);
    }

    let catalog = match Catalog::discover_load(None) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return Some(glib::ExitCode::FAILURE);
        }
    };
    let by_id = catalog.by_id();
    let mut ran = false;

    for arg in args {
        if arg == "-h" || arg == "--help" {
            continue;
        }
        if arg == "--list" {
            print_list(&paths, &catalog);
            ran = true;
            continue;
        }
        if arg == "--clean-cache" {
            if let Err(e) = clean_catalog_download_cache(&paths) {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
            ran = true;
            continue;
        }
        if arg == "-update" || arg == "--update" {
            let remotes = fetch_github_release_hints(&catalog);
            if let Err(e) = update_all_installed_instances(
                &paths,
                &catalog,
                Some(&remotes),
                &mut |m| {
                    eprintln!("{m}");
                },
                None,
            ) {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
            ran = true;
            continue;
        }
        if arg.starts_with('+') && arg.len() > 1 {
            let body = &arg[1..];
            let Ok((id, n)) = parse_app_instance_spec(body) else {
                continue;
            };
            let Some(app) = by_id.get(id.as_str()) else {
                continue;
            };
            let cur = instance_count(&paths, app).unwrap_or(0);
            let target = cur.saturating_add(n).min(app.max_instances_allowed());
            if let Err(e) = reconcile_instance_count(&paths, app, target, &mut |m| eprintln!("{m}"))
            {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
            ran = true;
            continue;
        }
        if arg.starts_with('-') && !arg.starts_with("--") && arg.len() > 1 {
            let body = &arg[1..];
            if !body.contains(':') {
                let Some(app) = by_id.get(body) else {
                    continue;
                };
                if let Err(e) = reconcile_instance_count(&paths, app, 0, &mut |m| eprintln!("{m}"))
                {
                    eprintln!("{e}");
                    return Some(glib::ExitCode::FAILURE);
                }
                ran = true;
                continue;
            }
            let Ok((id, n)) = parse_app_instance_spec(body) else {
                continue;
            };
            let Some(app) = by_id.get(id.as_str()) else {
                continue;
            };
            let cur = instance_count(&paths, app).unwrap_or(0);
            let target = cur.saturating_sub(n);
            if let Err(e) = reconcile_instance_count(&paths, app, target, &mut |m| eprintln!("{m}"))
            {
                eprintln!("{e}");
                return Some(glib::ExitCode::FAILURE);
            }
            ran = true;
        }
    }

    if !ran && seed_targets.is_none() {
        return None;
    }

    if let Some(targets) = seed_targets {
        // Pre-flight: bail before the per-instance loop if the host
        // is missing the required system helper. The seed pipeline
        // resolves every per-app port through it; without it every
        // instance would fail with the same install hint. One message
        // at the top is the right UX.
        if let Err(msg) = portmux_port_probe_runtime() {
            eprintln!("{msg}");
            return Some(glib::ExitCode::FAILURE);
        }

        let preset = SeedPreset::auto_default();
        let selected_ref_storage = targets;
        let selected_ref = selected_ref_storage.as_deref();
        let report =
            match seed_supported_installed_apps(&paths, &catalog, preset, selected_ref, &mut |m| {
                eprintln!("{m}")
            }) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("{e}");
                    return Some(glib::ExitCode::FAILURE);
                }
            };
        eprintln!(
            "Seed settings summary: {} seeded, {} failed, {} unsupported, {} missing.",
            report.seeded, report.failed, report.skipped_unsupported, report.skipped_missing
        );
        if report.failed > 0 {
            return Some(glib::ExitCode::FAILURE);
        }
        ran = true;
    }

    if !ran {
        return None;
    }

    Some(glib::ExitCode::SUCCESS)
}

#[cfg(test)]
mod purge_prompt_tests {
    use super::{parse_seed_settings_targets, parse_y_n_default_y, run_cli_ops};
    use gtk::glib;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn with_test_env<T>(
        home: &std::path::Path,
        catalog: &std::path::Path,
        f: impl FnOnce() -> T,
    ) -> T {
        let _g = env_lock().lock().expect("env lock");
        let old_home = std::env::var_os("HOME");
        let old_catalog = std::env::var_os("APPIMAGE_CONTROL_CATALOG");
        std::env::set_var("HOME", home);
        std::env::set_var("APPIMAGE_CONTROL_CATALOG", catalog);
        let out = f();
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_catalog {
            Some(v) => std::env::set_var("APPIMAGE_CONTROL_CATALOG", v),
            None => std::env::remove_var("APPIMAGE_CONTROL_CATALOG"),
        }
        out
    }

    fn write_min_catalog(path: &std::path::Path) {
        fs::write(
            path,
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
    }

    fn write_seed_catalog(path: &std::path::Path) {
        fs::write(
            path,
            r#"{
  "apps": [
    {
      "id": "feather",
      "display_name": "Feather Wallet",
      "download_url": "https://example.invalid/feather.AppImage",
      "appimage_filename": "feather.AppImage"
    },
    {
      "id": "electrum-btc",
      "display_name": "Electrum BTC",
      "download_url": "https://example.invalid/eb.AppImage",
      "appimage_filename": "eb.AppImage"
    }
  ]
}"#,
        )
        .expect("write seed catalog");
    }

    fn write_exec(path: &std::path::Path, body: &str) {
        fs::write(path, body).expect("script");
        let mut perms = fs::metadata(path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod");
    }

    /// Hermetic environment for end-to-end seed tests:
    /// - pinned HOME + catalog;
    /// - a fake `xvfb-run` so AppImage launches are no-ops;
    /// - a fake port-allocation helper wired through the documented
    ///   override env var so the seed pre-flight + per-instance
    ///   lookups don't depend on an installed system helper;
    /// - optional seed-pack directory override so callers can inject
    ///   synthetic packs without shipping anything in this repo.
    ///
    /// The stub helper returns a deterministic JSON record sized to
    /// match the real helper's documented contract. Test bodies do
    /// not need to know which binary is being stubbed — that's the
    /// point of the abstraction.
    fn with_fake_seed_env<T>(
        home: &std::path::Path,
        catalog: &std::path::Path,
        seeds_dir: Option<&std::path::Path>,
        f: impl FnOnce() -> T,
    ) -> T {
        use appimage_control_core::{ENV_PORT_HELPER_BIN, ENV_SEEDS_DIR};

        let _g = env_lock().lock().expect("env lock");
        let old_home = std::env::var_os("HOME");
        let old_catalog = std::env::var_os("APPIMAGE_CONTROL_CATALOG");
        let old_path = std::env::var_os("PATH");
        let old_helper = std::env::var_os(ENV_PORT_HELPER_BIN);
        let old_seeds = std::env::var_os(ENV_SEEDS_DIR);
        let bin = home.join("bin");
        fs::create_dir_all(&bin).expect("bin");
        write_exec(
            &bin.join("xvfb-run"),
            "#!/bin/sh\nshift\nshift\nexec \"$@\"\n",
        );
        // Fake port-allocation helper. Accepts `--help` (probe path)
        // and `ensure …` (per-instance lookup); both succeed. The
        // JSON shape matches the real helper's documented contract.
        let helper = bin.join("port-helper");
        write_exec(
            &helper,
            "#!/bin/sh\n\
             if [ \"$1\" = \"--help\" ]; then exit 0; fi\n\
             if [ \"$1\" = \"ensure\" ]; then\n  \
               printf '%s\\n' '{\"host\":\"10.152.152.10\",\"port\":9168,\
                                \"isolation\":\"dest-addr\",\
                                \"source_file\":\"/tmp/stub\",\
                                \"tor_reload_done\":false,\
                                \"already_assigned\":false}'\n  \
               exit 0\n\
             fi\n\
             exit 0\n",
        );
        let mut path = bin.as_os_str().to_os_string();
        path.push(":");
        if let Some(old) = &old_path {
            path.push(old);
        }
        std::env::set_var("HOME", home);
        std::env::set_var("APPIMAGE_CONTROL_CATALOG", catalog);
        std::env::set_var("PATH", path);
        std::env::set_var(ENV_PORT_HELPER_BIN, &helper);
        if let Some(sd) = seeds_dir {
            std::env::set_var(ENV_SEEDS_DIR, sd);
        } else {
            std::env::remove_var(ENV_SEEDS_DIR);
        }
        let out = f();
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        match old_catalog {
            Some(v) => std::env::set_var("APPIMAGE_CONTROL_CATALOG", v),
            None => std::env::remove_var("APPIMAGE_CONTROL_CATALOG"),
        }
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match old_helper {
            Some(v) => std::env::set_var(ENV_PORT_HELPER_BIN, v),
            None => std::env::remove_var(ENV_PORT_HELPER_BIN),
        }
        match old_seeds {
            Some(v) => std::env::set_var(ENV_SEEDS_DIR, v),
            None => std::env::remove_var(ENV_SEEDS_DIR),
        }
        out
    }

    #[test]
    fn y_n_default_y() {
        assert_eq!(parse_y_n_default_y(""), Some(true));
        assert_eq!(parse_y_n_default_y("\n"), Some(true));
        assert_eq!(parse_y_n_default_y("  \t  "), Some(true));
        assert_eq!(parse_y_n_default_y("y"), Some(true));
        assert_eq!(parse_y_n_default_y("Y"), Some(true));
        assert_eq!(parse_y_n_default_y("yes"), Some(true));
        assert_eq!(parse_y_n_default_y("YES"), Some(true));
        assert_eq!(parse_y_n_default_y("n"), Some(false));
        assert_eq!(parse_y_n_default_y("no"), Some(false));
        assert_eq!(parse_y_n_default_y("NO"), Some(false));
        assert_eq!(parse_y_n_default_y("maybe"), Some(false));
    }

    #[test]
    fn backup_flag_missing_archive_path_fails() {
        let code = run_cli_ops(&["--backup".to_string()]);
        assert_eq!(code, Some(glib::ExitCode::FAILURE));
    }

    #[test]
    fn restore_refuses_without_force_when_state_exists() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog = tmp.path().join("catalog.json");
        write_min_catalog(&catalog);
        let dummy_archive = tmp.path().join("dummy.tar.zst");
        fs::write(&dummy_archive, b"not-used").expect("archive");

        let managed = home.join(".local/share/appimages/apps");
        fs::create_dir_all(&managed).expect("managed");

        let args = vec![
            "--restore-backup".to_string(),
            dummy_archive.to_string_lossy().to_string(),
        ];
        let code = with_test_env(&home, &catalog, || run_cli_ops(&args));
        assert_eq!(code, Some(glib::ExitCode::FAILURE));
    }

    #[test]
    fn backup_path_created_successfully() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog = tmp.path().join("catalog.json");
        write_min_catalog(&catalog);
        let out = tmp.path().join("aic-backup.tar.zst");
        let args = vec!["--backup".to_string(), out.to_string_lossy().to_string()];
        let code = with_test_env(&home, &catalog, || run_cli_ops(&args));
        assert_eq!(code, Some(glib::ExitCode::SUCCESS));
        assert!(out.is_file(), "backup archive should be created");
    }

    #[test]
    fn seed_flag_parsing_accepts_expected_forms() {
        assert!(matches!(
            parse_seed_settings_targets(&["--seed-settings".into(), "all".into()]).unwrap(),
            Some(None)
        ));
        assert_eq!(
            parse_seed_settings_targets(&["--seed-settings".into(), "feather,electrum-btc".into()])
                .unwrap(),
            Some(Some(vec!["feather".into(), "electrum-btc".into()]))
        );
    }

    #[test]
    fn seed_load_and_seed_settings_are_mutually_exclusive() {
        let code = run_cli_ops(&[
            "--seed-settings".to_string(),
            "feather".to_string(),
            "--seed-load".to_string(),
            "anything".to_string(),
        ]);
        assert_eq!(code, Some(glib::ExitCode::FAILURE));
    }

    #[test]
    fn backup_and_restore_flags_conflict() {
        let code = run_cli_ops(&[
            "--backup".to_string(),
            "/tmp/aic-both.tar.zst".to_string(),
            "--restore-backup".to_string(),
            "/tmp/aic-restore.tar.zst".to_string(),
        ]);
        assert_eq!(code, Some(glib::ExitCode::FAILURE));
    }

    #[test]
    fn force_without_restore_fails() {
        let code = run_cli_ops(&["--force".to_string()]);
        assert_eq!(code, Some(glib::ExitCode::FAILURE));
    }

    #[test]
    fn list_prints_with_isolated_home() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog = tmp.path().join("catalog.json");
        write_min_catalog(&catalog);
        let code = with_test_env(&home, &catalog, || run_cli_ops(&["--list".to_string()]));
        assert_eq!(code, Some(glib::ExitCode::SUCCESS));
    }

    #[test]
    fn clean_cache_succeeds_without_other_ops() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog = tmp.path().join("catalog.json");
        write_min_catalog(&catalog);
        let code = with_test_env(&home, &catalog, || {
            run_cli_ops(&["--clean-cache".to_string()])
        });
        assert_eq!(code, Some(glib::ExitCode::SUCCESS));
    }

    #[test]
    fn plus_lm_studio_respects_catalog_max_instances() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/catalog.json");
        assert!(catalog.is_file(), "need shipped catalog");
        let cache = home.join(".local/share/appimages/cache/lm-studio");
        fs::create_dir_all(&cache).expect("cache dir");
        fs::write(cache.join("LMStudio.AppImage"), b"fake").expect("cached appimage");
        let code = with_test_env(&home, &catalog, || {
            run_cli_ops(&["+lm-studio:5".to_string()])
        });
        assert_eq!(code, Some(glib::ExitCode::SUCCESS));
        let inst = home.join(".local/share/appimages/apps/lm-studio");
        let n = fs::read_dir(&inst).expect("apps/lm-studio").count();
        assert_eq!(n, 1, "expected one instance dir at cap=1");
    }

    #[test]
    fn seed_cli_reseeds_supported_instances() {
        let tmp = tempfile::tempdir().expect("tmp");
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).expect("home");
        let catalog = tmp.path().join("catalog.json");
        write_seed_catalog(&catalog);

        let dir = home.join(".local/share/appimages/apps/feather/1");
        fs::create_dir_all(&dir).expect("dir");
        let appimage = dir.join("feather.AppImage");
        let cfg = dir.join("feather.AppImage.config/feather/settings.json");
        write_exec(
            &appimage,
            &format!(
                "#!/bin/sh\nset -eu\nmkdir -p \"{}\"\nprintf '{{}}' > \"{}\"\ntrap 'exit 0' TERM INT\nwhile :; do sleep 1; done\n",
                cfg.parent().expect("parent").display(),
                cfg.display()
            ),
        );

        // Curated packs are no longer shipped in this repo (Stage 8).
        // Inject a synthetic feather-shaped pack that exercises the
        // `preset.from` pipeline so the assertions on socks5Host /
        // useLocalTor have something to render.
        let seeds_dir = tmp.path().join("seeds");
        fs::create_dir_all(&seeds_dir).expect("seeds dir");
        fs::write(
            seeds_dir.join("feather.json"),
            r#"{
                "config_path": "feather.AppImage.config/feather/settings.json",
                "first_run_args": [],
                "preset": {
                    "socks5Host": { "from": "socks5_host" },
                    "useLocalTor": { "from": "feather_use_local_tor" }
                }
            }"#,
        )
        .expect("seed pack");

        let args = vec!["--seed-settings".to_string(), "feather".to_string()];
        let code = with_fake_seed_env(&home, &catalog, Some(&seeds_dir), || {
            // Pin the networking branch deterministically — without
            // this the assertion would flip when the test host happens
            // to have the workstation marker file present. Done inside
            // the closure so it's serialised by the env-lock guard the
            // helper already holds.
            let prev_preset = std::env::var("APPIMAGE_CONTROL_SEED_PRESET").ok();
            std::env::set_var("APPIMAGE_CONTROL_SEED_PRESET", "whonix");
            let r = run_cli_ops(&args);
            match prev_preset {
                Some(v) => std::env::set_var("APPIMAGE_CONTROL_SEED_PRESET", v),
                None => std::env::remove_var("APPIMAGE_CONTROL_SEED_PRESET"),
            }
            r
        });
        assert_eq!(code, Some(glib::ExitCode::SUCCESS));
        let body = fs::read_to_string(&cfg).expect("cfg");
        assert!(body.contains("\"socks5Host\": \"10.152.152.10\""));
        assert!(body.contains("\"useLocalTor\": false"));
    }
}
