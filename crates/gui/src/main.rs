//! GTK 4.10+ deprecates some dialog APIs; kept for gtk4-rs 0.9 on Ubuntu 24.04 / Whonix.
#![allow(deprecated)]

mod actions;
mod catalog_icons;
mod cli;
mod dialogs;
mod embedded_icons;
mod i18n;
mod preferences;
mod record_dialog;
mod seed_dialog;
mod status_text;
mod ui;
mod unverified;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::mpsc;

use crate::catalog_icons::batch_catalog_icons;
use crate::dialogs::{
    ask_confirmation, ask_migration_strategy, choose_open_path, choose_save_path,
    present_activity_log_dialog, present_message_window, ConfirmKind,
};
use crate::i18n::{fill, ntr, tr, trc};
use crate::record_dialog::{
    choose_pack_save_path, choose_record_target, confirm_manual_step, review_and_select, RecordApp,
};
use crate::seed_dialog::choose_seed_settings;
use adw::prelude::*;
use appimage_control_core::{
    build_static_pack, capture_first_run_config, catalog_row_latest_version_label,
    clean_catalog_download_cache, clean_installed_catalog_apps_keep_cache, create_backup_archive,
    diff_configs, discover_config_candidates, discover_migration_candidates,
    fetch_github_release_hints, installed_appimage_size, installed_version_fleet_display,
    instance_count, list_managed_instance_indices, purge_appimage_control_state,
    read_instance_config, reconcile_catalog_install_version_metadata,
    reconcile_instance_count_with_policy, repair_instance_launchers, restore_backup_archive,
    run_portable_dir_migration, save_recording, seed_instance_settings,
    update_all_installed_instances_with_policy, write_pack_to, AppPaths, BackupOptions, Catalog,
    CatalogApp, GithubReleaseHint, GuiConfig, PackRecording, SeedPackLibrary, SeedPreset,
    SupportedSeedApp, UpdateAllReport,
};
use cli::{has_cli_ops, print_help, run_cli_ops, want_help};
use gtk::glib;
use status_text::{
    apply_error_log_line, catalog_row_hint, format_apply_status, format_seed_status_summary,
    format_update_status_summary, nothing_pending_summary,
};

use gtk::{
    gdk::Texture, Box as GtkBox, Button, CssProvider, FlowBox, FlowBoxChild, Image, Label,
    Orientation, ProgressBar, ScrolledWindow, SearchEntry, SpinButton, ToggleButton,
};

const APP_ID: &str = "io.github.sureserverman.AppImageControl";

/// Pick the most likely settings file when config-path auto-detection finds
/// several JSON candidates: prefer one whose path mentions `settings`/`config`,
/// else the shortest path, else the first. `candidates` is assumed non-empty.
fn pick_best_config_candidate(candidates: &[String]) -> String {
    candidates
        .iter()
        .find(|p| {
            let l = p.to_lowercase();
            l.contains("settings") || l.contains("config")
        })
        .or_else(|| candidates.iter().min_by_key(|p| p.len()))
        .cloned()
        .unwrap_or_default()
}
const THEME_CSS: &str = include_str!("theme.css");

/// Per-app launcher card. The visible surface is a card of widgets (icon, name,
/// subtitle, status pill, a row of instance tiles + count); the deferred-apply
/// *target* is stored in a hidden [`SpinButton`] so the entire Apply/Update/refresh
/// machinery (WF-APPLY-001) keeps reading `spin.value()` exactly as before — tiles
/// merely drive that spin.
struct RowHandles {
    app_index: usize,
    icon: Image,
    subtitle_label: Label,
    status_pill: Label,
    /// Holds the numbered instance tiles + the dashed "add" tile; rebuilt on every change.
    tiles_box: GtkBox,
    count_label: Label,
    /// FlowBoxChild wrapping this card — visibility is toggled by filter + search.
    flow_child: FlowBoxChild,
    /// Hidden target store (see struct doc). Range enforces the per-app cap.
    spin: SpinButton,
    /// Resolved URL exists for this process CPU architecture.
    can_install: bool,
    max_allowed: u32,
    /// Whether an upstream update is available — drives the "Updates" header filter.
    update_flag: Rc<Cell<bool>>,
    /// Mirror of `instance_count` at last refresh — used for purely-client-side
    /// pending detection on every spin change.  Wrapped in `Rc<Cell<u32>>` so
    /// the `value-changed` closure always reads the live post-Apply value rather
    /// than the copy that was frozen at row-construction time.
    last_applied: Rc<Cell<u32>>,
}

/// A catalog category's section heading. The shipped categories are translated;
/// any other name comes from a custom catalog and is shown as written.
fn category_label(cat: &str) -> String {
    // TRANSLATORS: section headings that group the apps in the window. They are
    // shown in upper case.
    let known = match cat {
        "AI Tools" => trc("category", "AI Tools"),
        "Messengers" => trc("category", "Messengers"),
        "Notes" => trc("category", "Notes"),
        "Security" => trc("category", "Security"),
        "Wallets" => trc("category", "Wallets"),
        "Other" => trc("category", "Other"),
        other => return other.to_string(),
    };
    known.to_string()
}

/// Format a byte count the way the design card subtitle shows it ("38 MB", "1.2 GB").
fn format_size(bytes: u64) -> String {
    let mb = bytes as f64 / 1_000_000.0;
    if mb >= 1000.0 {
        fill(tr("{size} GB"), &[("size", &format!("{:.1}", mb / 1000.0))])
    } else if mb >= 10.0 {
        fill(tr("{size} MB"), &[("size", &(mb.round() as u64))])
    } else {
        fill(tr("{size} MB"), &[("size", &format!("{:.1}", mb))])
    }
}

/// Repaints one launcher card from its current target (`spin`) and applied count.
/// Sets the `v<version> · <size>` subtitle, the status pill (Update / Installed /
/// Available), the instance-count phrase, and rebuilds the keep/add/remove tiles +
/// the dashed add-tile. Takes individual widget handles (not `&RowHandles`) so the
/// `value-changed` closure can call it without re-borrowing the rows vector.
///
/// Tile state machine (mirrors the design mock): for index `i` in `1..=max(target,
/// applied)` — `add` when `i > applied`, `remove` when `i > target && i <= applied`,
/// else `keep`. The status pill needs a text label *and* a color (HIG: never color
/// only), so the label is always set.
#[allow(clippy::too_many_arguments)]
fn render_card(
    paths: &AppPaths,
    app: &CatalogApp,
    applied: u32,
    max_allowed: u32,
    remote_latest: Option<&str>,
    subtitle_label: &Label,
    status_pill: &Label,
    tiles_box: &GtkBox,
    count_label: &Label,
    update_flag: &Rc<Cell<bool>>,
    spin: &SpinButton,
) {
    let target = spin.value() as u32;
    let inst_display = installed_version_fleet_display(paths, app).ok();
    let latest_raw = catalog_row_latest_version_label(paths, app, remote_latest)
        .unwrap_or_else(|_| "—".to_string());

    // Subtitle: v<version>  ·  <size>  (size only when something is installed so we
    // never show a fabricated number)  ·  single instance (when capped at 1).
    let mut sub = format!("v{latest_raw}");
    if applied > 0 {
        if let Some(sz) = installed_appimage_size(paths, app) {
            sub.push_str(&format!("  ·  {}", format_size(sz)));
        }
    }
    if max_allowed == 1 {
        sub.push_str(&format!("  ·  {}", tr("single instance")));
    }
    subtitle_label.set_text(&sub);

    // Status pill.
    let update_available = applied > 0
        && remote_latest
            .map(|tag| version_is_newer(tag, inst_display.as_ref().map(|d| d.label.as_str())))
            .unwrap_or(false);
    update_flag.set(update_available);
    for c in ["update", "installed", "available"] {
        status_pill.remove_css_class(c);
    }
    if update_available {
        // TRANSLATORS: a status badge on an app card: an update is available.
        status_pill.set_text(trc("status badge", "Update"));
        status_pill.add_css_class("update");
        status_pill.set_tooltip_text(Some(tr("An upstream update is available")));
    } else if applied > 0 {
        status_pill.set_text(tr("Installed"));
        status_pill.add_css_class("installed");
        let tip = match inst_display.as_ref() {
            // TRANSLATORS: {version} is an app version, like "v2.8.1".
            Some(d) => fill(tr("{version} installed"), &[("version", &d.label)]),
            None => tr("Instances installed").to_string(),
        };
        status_pill.set_tooltip_text(Some(&tip));
    } else {
        status_pill.set_text(tr("Available"));
        status_pill.add_css_class("available");
        status_pill.set_tooltip_text(Some(tr("Nothing installed yet")));
    }

    // Count phrase (tracks the target, like the design mock).
    count_label.set_text(&match target {
        0 => tr("no instances").to_string(),
        n => fill(
            ntr("{count} instance", "{count} instances", n.into()),
            &[("count", &n)],
        ),
    });

    // Rebuild tiles.
    while let Some(child) = tiles_box.first_child() {
        tiles_box.remove(&child);
    }
    let hi = target.max(applied);
    for i in 1..=hi {
        let cls = if i > applied {
            "add"
        } else if i > target && i <= applied {
            "remove"
        } else {
            "keep"
        };
        let tile = Button::with_label(&i.to_string());
        tile.add_css_class("flat");
        tile.add_css_class("ac-tile");
        tile.add_css_class(cls);
        tile.set_valign(gtk::Align::Center);
        tile.set_tooltip_text(Some(tr("Remove an instance")));
        let spin_dec = spin.clone();
        tile.connect_clicked(move |_| {
            let v = spin_dec.value();
            spin_dec.set_value((v - 1.0).max(0.0));
        });
        tiles_box.append(&tile);
    }
    // Dashed add-tile.
    let add = Button::with_label("+");
    add.add_css_class("flat");
    add.add_css_class("ac-addtile");
    add.set_valign(gtk::Align::Center);
    let at_max = target >= max_allowed;
    add.set_sensitive(!at_max);
    let add_tip = if at_max {
        match app.instance_limit_tooltip_message() {
            // The catalog's own wording is data, shown as written.
            Some(custom) if app.max_instances_tooltip.is_some() => custom,
            Some(_) => fill(
                ntr(
                    "You can't use more than one instance of {app}.",
                    "You can't use more than {count} instances of {app}.",
                    max_allowed.into(),
                ),
                &[
                    ("count", &max_allowed),
                    ("app", &app.display_name_resolved()),
                ],
            ),
            None => tr("Maximum instances reached").to_string(),
        }
    } else {
        tr("Add an instance").to_string()
    };
    add.set_tooltip_text(Some(&add_tip));
    let spin_inc = spin.clone();
    add.connect_clicked(move |_| {
        let v = spin_inc.value() as u32;
        spin_inc.set_value((v + 1).min(max_allowed) as f64);
    });
    tiles_box.append(&add);
}

/// Best-effort "remote tag is newer than any installed version" compare. Strict
/// semver comparison would need pulling in a parser; we degrade to !equal so the
/// pill never lies aggressively. Equal strings → no update.
fn version_is_newer(remote_tag: &str, installed_label: Option<&str>) -> bool {
    let Some(installed) = installed_label else {
        return false;
    };
    let r = remote_tag.trim_start_matches('v');
    let i = installed.trim();
    !r.is_empty() && !i.is_empty() && r != i && !i.contains(r)
}

fn main() -> glib::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if want_help(&args) {
        print_help();
        return glib::ExitCode::SUCCESS;
    }
    if has_cli_ops(&args) {
        if let Some(code) = run_cli_ops(&args) {
            return code;
        }
    }
    // Translations are for the GUI only; the CLI above stays English.
    i18n::init();
    adw::init().expect("adw init");
    install_theme_css();
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
}

/// Loads the small `theme.css` chunk into the default display so the pill rules
/// in the boxed-list rows can use libadwaita named colors.
fn install_theme_css() {
    let provider = CssProvider::new();
    provider.load_from_data(THEME_CSS);
    if let Some(display) = gtk::gdk::Display::default() {
        // USER priority (800) so our `@define-color accent_*` override beats the
        // system-accent recoloring libadwaita applies above APPLICATION priority.
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_USER,
        );
    }
}

/// Primary-menu "Keyboard Shortcuts" item — a compact reference dialog.
fn present_shortcuts(parent: &gtk::ApplicationWindow) {
    let body = [
        ("Ctrl+U", tr("Update all installed")),
        ("Ctrl+,", tr("Preferences")),
        ("Ctrl+?", tr("Keyboard shortcuts")),
        ("Ctrl+R", tr("Reload catalog")),
        ("Ctrl+W", tr("Close window")),
    ]
    .map(|(keys, what)| format!("{keys}\t{what}"))
    .join("\n");
    let dialog = adw::MessageDialog::new(Some(parent), Some(tr("Keyboard Shortcuts")), Some(&body));
    dialog.add_response("close", tr("Close"));
    dialog.set_default_response(Some("close"));
    dialog.present();
}

/// Primary-menu "About AppImage Control" item.
fn present_about(parent: &gtk::ApplicationWindow) {
    let about = adw::AboutWindow::builder()
        .transient_for(parent)
        .modal(true)
        .application_name("AppImage Control")
        .application_icon(APP_ID)
        .version(env!("CARGO_PKG_VERSION"))
        .developer_name("Sure Serverman")
        .website("https://github.com/sureserverman/appimage-control")
        .license_type(gtk::License::MitX11)
        .comments(tr(
            "Manage a catalog of apps and the number of isolated AppImage instances to keep for each.",
        ))
        // TRANSLATORS: put your name here, one per line, to be credited in the About window.
        .translator_credits(tr("translator-credits"))
        .build();
    about.present();
}

fn build_ui(app: &adw::Application) {
    let paths = match AppPaths::from_env() {
        Ok(p) => Rc::new(p),
        Err(e) => {
            eprintln!("{}", e);
            present_message_window(
                app,
                "AppImage Control",
                tr("The app could not determine your home directory, so managed installs cannot be used."),
                Some(&e.to_string()),
            );
            return;
        }
    };

    // Apply the saved appearance preference before the first window paints.
    preferences::apply_theme(GuiConfig::load(&paths).theme);

    // Primary-menu accelerators (shown right-aligned in the popover).
    app.set_accels_for_action("win.update", &["<Ctrl>u"]);
    app.set_accels_for_action("win.preferences", &["<Ctrl>comma"]);
    app.set_accels_for_action("win.shortcuts", &["<Ctrl>question"]);
    app.set_accels_for_action("win.refresh", &["<Ctrl>r"]);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("AppImage Control")
        .default_width(840)
        .default_height(620)
        .build();
    // Compact typography to match the design's density.
    window.add_css_class("ac-compact-root");

    // Catalog icons cache — populated in background once, reused by the catalog
    // view AND by the seed-settings dialog.
    let icons_cache: Rc<RefCell<Vec<Option<Vec<u8>>>>> = Rc::new(RefCell::new(Vec::new()));

    // Closures that re-mount the window content. Held in a RefCell so they can
    // call each other (Reload re-runs discover_load and dispatches).
    type Mounter = Rc<dyn Fn()>;
    let try_mount: Rc<RefCell<Option<Mounter>>> = Rc::new(RefCell::new(None));

    let try_mount_setter = Rc::clone(&try_mount);
    let try_mount_inner: Mounter = {
        let app = app.clone();
        let window = window.clone();
        let paths = Rc::clone(&paths);
        let icons_cache = Rc::clone(&icons_cache);
        Rc::new(move || match Catalog::discover_load(None) {
            Ok(c) => mount_catalog_view(
                &app,
                &window,
                Rc::clone(&paths),
                Rc::new(c),
                Rc::clone(&icons_cache),
                || {},
            ),
            Err(e) => {
                let try_mount_for_reload = try_mount_setter
                    .borrow()
                    .as_ref()
                    .cloned()
                    .expect("try_mount installed before first call");
                mount_empty_state(
                    &app,
                    &window,
                    Rc::clone(&paths),
                    Rc::clone(&icons_cache),
                    e.to_string(),
                    try_mount_for_reload,
                );
            }
        })
    };
    *try_mount.borrow_mut() = Some(Rc::clone(&try_mount_inner));

    (try_mount_inner)();
    window.present();
}

/// Replaces the window content with the empty/no-catalog state — `AdwStatusPage`
/// inside the same toolbar view. Header bar carries only the primary menu.
fn mount_empty_state(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    paths: Rc<AppPaths>,
    icons_cache: Rc<RefCell<Vec<Option<Vec<u8>>>>>,
    error_msg: String,
    try_mount: Rc<dyn Fn()>,
) {
    let header = adw::HeaderBar::builder().show_title(true).build();
    header.set_title_widget(Some(&adw::WindowTitle::new("AppImage Control", "")));
    let menu_btn = actions::build_actions_menu_button();
    menu_btn.set_tooltip_text(Some(tr("Main Menu")));
    header.pack_end(&menu_btn);

    // No-catalog disables most window actions; install a minimal hook set so the
    // primary menu doesn't crash when items get clicked.
    let app_for_actions = app.clone();
    let window_for_actions: gtk::ApplicationWindow = window.clone().upcast();
    let try_mount_for_refresh = Rc::clone(&try_mount);
    let unavailable: Rc<dyn Fn()> = {
        let win = window_for_actions.clone();
        Rc::new(move || {
            present_activity_log_dialog(
                &win,
                tr("No catalog loaded — load one first.\nUse the Reload button or Open Catalog… to recover.\n"),
            );
        })
    };
    let hooks = actions::ActionHooks {
        apply: Rc::clone(&unavailable),
        update: Rc::clone(&unavailable),
        seed_settings: Rc::clone(&unavailable),
        refresh: try_mount_for_refresh,
        clean_cache: Rc::clone(&unavailable),
        clean: Rc::clone(&unavailable),
        purge: Rc::clone(&unavailable),
        migrate: Rc::clone(&unavailable),
        backup: Rc::clone(&unavailable),
        restore_backup: Rc::clone(&unavailable),
        update_seed_packs: Rc::clone(&unavailable),
        list_seed_packs: Rc::clone(&unavailable),
        record_seed_pack: Rc::clone(&unavailable),
        repair: Rc::clone(&unavailable),
        preferences: {
            let win = window_for_actions.clone();
            let paths = Rc::clone(&paths);
            Rc::new(move || preferences::present_preferences(&win, Rc::clone(&paths)))
        },
        shortcuts: {
            let win = window_for_actions.clone();
            Rc::new(move || present_shortcuts(&win))
        },
        about: {
            let win = window_for_actions.clone();
            Rc::new(move || present_about(&win))
        },
    };
    actions::install_window_actions(&window_for_actions, hooks);
    let _ = (app_for_actions, error_msg);

    let status = adw::StatusPage::new();
    status.set_icon_name(Some("package-x-generic-symbolic"));
    status.set_title(tr("No catalog loaded"));
    status.set_description(Some(tr(
        "AppImage Control couldn’t find an application catalog. Open one, or set APPIMAGE_CONTROL_CATALOG to a catalog file.",
    )));

    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_halign(gtk::Align::Center);

    let detail_card = GtkBox::new(Orientation::Vertical, 4);
    detail_card.set_margin_top(0);
    detail_card.add_css_class("card");
    detail_card.set_margin_top(0);

    let detail_label = Label::builder()
        .label(tr(
            "Search order:\n  1. APPIMAGE_CONTROL_CATALOG environment variable\n  2. data/catalog.json above a dev build (cargo target/ dir only)\n  3. /app/share/, /usr/share/appimage-control/catalog.json\n  4. catalog compiled into this build",
        ))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(false)
        .build();
    detail_label.add_css_class("monospace");
    detail_label.add_css_class("caption");
    detail_label.set_margin_top(10);
    detail_label.set_margin_bottom(10);
    detail_label.set_margin_start(14);
    detail_label.set_margin_end(14);
    detail_card.append(&detail_label);

    let actions_box = GtkBox::new(Orientation::Horizontal, 8);
    actions_box.set_halign(gtk::Align::Center);

    let open_btn = Button::with_label(tr("Open Catalog…"));
    open_btn.add_css_class("suggested-action");
    let reload_btn = Button::with_label(tr("Reload"));

    {
        let try_mount = Rc::clone(&try_mount);
        reload_btn.connect_clicked(move |_| (try_mount)());
    }
    {
        let app = app.clone();
        let window = window.clone();
        let paths = Rc::clone(&paths);
        let icons_cache = Rc::clone(&icons_cache);
        let try_mount_outer = Rc::clone(&try_mount);
        open_btn.connect_clicked(move |_| {
            let win_for_chooser: gtk::ApplicationWindow = window.clone().upcast();
            let Some(path) = choose_open_path(&win_for_chooser, tr("Choose catalog.json")) else {
                return;
            };
            match Catalog::discover_load(Some(&path)) {
                Ok(c) => {
                    mount_catalog_view(
                        &app,
                        &window,
                        Rc::clone(&paths),
                        Rc::new(c),
                        Rc::clone(&icons_cache),
                        || {},
                    );
                }
                Err(_) => (try_mount_outer)(),
            }
        });
    }

    actions_box.append(&reload_btn);
    actions_box.append(&open_btn);

    body.append(&detail_card);
    body.append(&actions_box);
    status.set_child(Some(&body));

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&status));
    window.set_content(Some(&toolbar));
}

/// Mounts the full catalog view (header bar with Apply/Update/menu, AdwClamp body
/// with intro caption, AdwBanner for pending changes, boxed-list of AdwActionRows,
/// progress bar, footer status row).
fn mount_catalog_view(
    app: &adw::Application,
    window: &adw::ApplicationWindow,
    paths: Rc<AppPaths>,
    catalog: Rc<Catalog>,
    icons_cache: Rc<RefCell<Vec<Option<Vec<u8>>>>>,
    _on_remount_done: impl FnOnce(),
) {
    let _ = app; // kept in signature for future Open/Reload re-mount paths
                 // Compact (default) vs. regular launcher-grid metrics, per the saved preference.
    let compact = GuiConfig::load(&paths).compact_rows;
    let icon_px = if compact { 32 } else { 42 };
    let remote_latest_cache: Rc<RefCell<Vec<Option<GithubReleaseHint>>>> =
        Rc::new(RefCell::new(Vec::new()));

    // Best-effort: repair missing per-instance launchers/icons (silent if nothing changed).
    {
        let paths_t = (*paths).clone();
        let catalog_t = (*catalog).clone();
        let window_idle: gtk::ApplicationWindow = window.clone().upcast();
        let (tx, rx) = mpsc::channel::<String>();
        glib::idle_add_local(move || match rx.try_recv() {
            Ok(log) => {
                if !log.trim().is_empty() {
                    present_activity_log_dialog(&window_idle, &log);
                }
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
        });
        std::thread::spawn(move || {
            let mut log = String::new();
            let _ = repair_instance_launchers(&paths_t, &catalog_t, &mut |m| {
                log.push_str(m);
                log.push('\n');
            });
            if log.contains("launcher repaired") || log.contains("reconciled install metadata") {
                let _ = tx.send(log);
            } else {
                let _ = tx.send(String::new());
            }
        });
    }

    // ---- Header: [title · filter] at start · search · menu at end ----
    // Per the design, the title sits at the start with the filter immediately after
    // it (not a centered title). Blank the centered title slot to avoid duplication.
    let header = adw::HeaderBar::builder().build();
    header.set_title_widget(Some(&GtkBox::new(Orientation::Horizontal, 0)));

    let header_title = Label::new(Some("AppImage Control"));
    header_title.add_css_class("heading");

    // 3-segment filter (All / Installed / Updates) as a slim dark-inset pill group
    // (design "ac-segment"); the active segment fills with the accent.
    let filter_box = GtkBox::new(Orientation::Horizontal, 0);
    filter_box.add_css_class("ac-segment");
    filter_box.add_css_class("ac-filter");
    filter_box.set_valign(gtk::Align::Center);
    // TRANSLATORS: filter buttons above the app grid: show all apps, installed apps,
    // or apps with an update.
    let filter_all = ToggleButton::with_label(trc("filter", "All"));
    let filter_installed = ToggleButton::with_label(trc("filter", "Installed"));
    let filter_updates = ToggleButton::with_label(trc("filter", "Updates"));
    for b in [&filter_all, &filter_installed, &filter_updates] {
        b.add_css_class("flat");
    }
    filter_all.set_active(true);
    filter_installed.set_group(Some(&filter_all));
    filter_updates.set_group(Some(&filter_all));
    filter_box.append(&filter_all);
    filter_box.append(&filter_installed);
    filter_box.append(&filter_updates);

    let header_start = GtkBox::new(Orientation::Horizontal, 12);
    header_start.append(&header_title);
    header_start.append(&filter_box);
    header.pack_start(&header_start);

    let search = SearchEntry::new();
    search.set_placeholder_text(Some(tr("Search")));
    search.add_css_class("ac-search-inset");
    search.set_max_width_chars(14);
    search.set_valign(gtk::Align::Center);

    let menu_btn = actions::build_actions_menu_button();
    menu_btn.set_tooltip_text(Some(tr("Main Menu")));
    header.pack_end(&menu_btn);
    header.pack_end(&search);

    // Apply + Update all now live in the bottom action bar (built below). The button
    // objects are unchanged so the existing Apply/Update wiring keeps working.
    let apply = Button::builder()
        .label(tr("Apply"))
        .tooltip_text(tr("Apply pending instance count changes"))
        .valign(gtk::Align::Center)
        .build();
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);

    let update_installed = Button::builder()
        .label(tr("Update all"))
        .valign(gtk::Align::Center)
        .tooltip_text(tr(
            "Update installed instances: skips when already at the newest known version (catalog and GitHub releases/latest). Otherwise runs appimageupdatetool, then downloads the catalog AppImage or—if GitHub’s latest tag is newer than the catalog URL—a matching .AppImage from that release.",
        ))
        .build();
    update_installed.add_css_class("flat");

    let apply_weak = apply.downgrade();
    let update_weak = update_installed.downgrade();

    // ---- Body: per-category card grid in a scroller ----
    let grid_box = GtkBox::new(Orientation::Vertical, 18);
    grid_box.set_hexpand(true);
    grid_box.set_margin_top(14);
    grid_box.set_margin_bottom(14);
    grid_box.set_margin_start(14);
    grid_box.set_margin_end(14);

    let body_scroll = ScrolledWindow::builder()
        .child(&grid_box)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(220)
        .build();

    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    progress.set_hexpand(true);
    progress.set_margin_top(6);
    progress.set_margin_bottom(6);
    progress.set_margin_start(12);
    progress.set_margin_end(12);

    // ---- Bottom action bar: 2-line pending summary · Update all · Apply ----
    let summary_line1 = Label::new(Some(tr("All changes applied")));
    summary_line1.add_css_class("caption");
    summary_line1.set_halign(gtk::Align::Start);
    summary_line1.set_xalign(0.0);

    // `statusbar` doubles as the summary's second line AND the background-work
    // status line ("Working…", "Applied.") that the Apply/Update/Seed handlers set.
    let statusbar = Label::new(Some(tr("Nothing pending — you're up to date")));
    statusbar.add_css_class("ac-summary-faint");
    statusbar.set_halign(gtk::Align::Start);
    statusbar.set_xalign(0.0);

    let summary_box = GtkBox::new(Orientation::Vertical, 1);
    summary_box.set_hexpand(true);
    summary_box.set_valign(gtk::Align::Center);
    summary_box.append(&summary_line1);
    summary_box.append(&statusbar);

    let actionbar = GtkBox::new(Orientation::Horizontal, 8);
    actionbar.add_css_class("ac-actionbar");
    actionbar.set_margin_start(14);
    actionbar.set_margin_end(14);
    actionbar.set_margin_top(8);
    actionbar.set_margin_bottom(8);
    actionbar.append(&summary_box);
    actionbar.append(&update_installed);
    actionbar.append(&apply);

    // Kept only so the existing `update_total_label()` calls still have a target;
    // the redesign surfaces counts via the pending summary, not a footer total.
    let total_label = Label::new(None);

    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.set_hexpand(true);
    outer.set_vexpand(true);
    outer.append(&body_scroll);
    outer.append(&progress);
    outer.append(&actionbar);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    // Build cards, grouped into per-category sections (first-seen order).
    let _ =
        reconcile_catalog_install_version_metadata(paths.as_ref(), catalog.as_ref(), &mut |_| {});

    let rows: Rc<RefCell<Vec<RowHandles>>> = Rc::new(RefCell::new(Vec::new()));
    // (category name, section box [header + flowbox], flowbox holding the cards)
    let mut sections: Vec<(String, GtkBox, FlowBox)> = Vec::new();

    for (app_index, catalog_app) in catalog.apps.iter().enumerate() {
        let has_url = catalog_app.has_any_download_url();
        let can_install = catalog_app.resolved_download_url().is_ok();
        let installed = list_managed_instance_indices(paths.as_ref(), catalog_app)
            .map(|v| v.len() as u32)
            .unwrap_or(0);
        let max_allowed = catalog_app.max_instances_allowed();

        // Find (or create) the category section + flowbox for this app.
        let cat = catalog_app.category_resolved();
        let flow = match sections.iter().find(|(n, _, _)| *n == cat) {
            Some((_, _, f)) => f.clone(),
            None => {
                let head_lbl = Label::new(Some(&category_label(&cat).to_uppercase()));
                head_lbl.add_css_class("ac-section-head");
                head_lbl.set_halign(gtk::Align::Start);
                head_lbl.set_xalign(0.0);
                let hairline = gtk::Separator::new(Orientation::Horizontal);
                hairline.set_hexpand(true);
                hairline.set_valign(gtk::Align::Center);
                let head_row = GtkBox::new(Orientation::Horizontal, 10);
                head_row.append(&head_lbl);
                head_row.append(&hairline);

                let flow = FlowBox::new();
                flow.add_css_class("ac-grid");
                if !compact {
                    flow.add_css_class("ac-regular");
                }
                flow.set_selection_mode(gtk::SelectionMode::None);
                flow.set_min_children_per_line(1);
                flow.set_max_children_per_line(2);
                flow.set_column_spacing(9);
                flow.set_row_spacing(9);
                flow.set_homogeneous(true);

                let section = GtkBox::new(Orientation::Vertical, 9);
                section.append(&head_row);
                section.append(&flow);
                grid_box.append(&section);
                sections.push((cat.clone(), section, flow.clone()));
                flow
            }
        };

        // --- Card widgets ---
        let icon = Image::from_icon_name("image-x-generic-symbolic");
        icon.set_pixel_size(icon_px);
        icon.set_valign(gtk::Align::Center);
        icon.add_css_class("ac-appicon");

        let name_label = Label::new(Some(&catalog_app.display_name_resolved()));
        name_label.add_css_class("heading");
        name_label.set_halign(gtk::Align::Start);
        name_label.set_xalign(0.0);
        name_label.set_hexpand(true);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let subtitle_label = Label::new(None);
        subtitle_label.add_css_class("caption");
        subtitle_label.add_css_class("dim-label");
        subtitle_label.set_halign(gtk::Align::Start);
        subtitle_label.set_xalign(0.0);
        subtitle_label.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let status_pill = Label::new(Some(tr("Available")));
        status_pill.add_css_class("ac-status");
        status_pill.set_valign(gtk::Align::Center);

        let titles = GtkBox::new(Orientation::Vertical, 1);
        titles.set_hexpand(true);
        titles.set_valign(gtk::Align::Center);
        titles.append(&name_label);
        titles.append(&subtitle_label);

        let card_head = GtkBox::new(Orientation::Horizontal, 9);
        card_head.append(&icon);
        card_head.append(&titles);
        card_head.append(&status_pill);

        // Tiles row + faint right-aligned count.
        let tiles_box = GtkBox::new(Orientation::Horizontal, 5);
        tiles_box.set_valign(gtk::Align::Center);
        let count_label = Label::new(None);
        count_label.add_css_class("ac-count");
        count_label.set_halign(gtk::Align::End);
        count_label.set_hexpand(true);
        count_label.set_valign(gtk::Align::Center);

        let bottom = GtkBox::new(Orientation::Horizontal, 8);
        bottom.append(&tiles_box);
        bottom.append(&count_label);

        let card = GtkBox::new(Orientation::Vertical, 10);
        card.add_css_class("ac-card");
        card.set_size_request(270, -1);
        card.set_tooltip_text(catalog_row_hint(catalog_app).as_deref());
        card.append(&card_head);
        card.append(&bottom);

        // Hidden target store. Range enforces the per-app instance cap.
        let spin = SpinButton::with_range(0.0, max_allowed as f64, 1.0);
        spin.set_value(installed.min(max_allowed) as f64);

        let flow_child = FlowBoxChild::new();
        flow_child.set_child(Some(&card));
        flow_child.set_can_focus(false);
        flow.append(&flow_child);

        // Apps with no usable download URL for this arch can't gain instances.
        tiles_box.set_sensitive(has_url && can_install);

        rows.borrow_mut().push(RowHandles {
            app_index,
            icon,
            subtitle_label,
            status_pill,
            tiles_box,
            count_label,
            flow_child,
            spin,
            can_install,
            max_allowed,
            update_flag: Rc::new(Cell::new(false)),
            last_applied: Rc::new(Cell::new(installed)),
        });
    }

    // Initial card render — needs catalog/paths/remote-cache borrow.
    refresh_all_rows(&paths, &catalog, &remote_latest_cache, &rows);
    update_total_label(&total_label, &paths, &catalog);

    // Pending-summary recompute: drives the bottom bar's two lines + the Apply
    // button label/sensitivity. (Mirrors the design mock's summary math.)
    // Apps that failed in the last Apply run (a refused download included), so the bottom bar
    // does not claim "All changes applied" after the failed targets were dropped.
    let last_apply_failed: Rc<Cell<u32>> = Rc::new(Cell::new(0));
    let recompute_pending = {
        let last_apply_failed = Rc::clone(&last_apply_failed);
        let rows = Rc::clone(&rows);
        let apply_weak = apply_weak.clone();
        let summary_line1 = summary_line1.clone();
        let statusbar = statusbar.clone();
        Rc::new(move || {
            let (mut changed, mut add, mut rem) = (0u32, 0u32, 0u32);
            for h in rows.borrow().iter() {
                let delta = h.spin.value() as i64 - h.last_applied.get() as i64;
                if delta != 0 {
                    changed += 1;
                }
                if delta > 0 {
                    add += delta as u32;
                } else if delta < 0 {
                    rem += (-delta) as u32;
                }
            }
            let has_pending = changed > 0;
            if let Some(a) = apply_weak.upgrade() {
                a.set_label(&if has_pending {
                    // TRANSLATORS: the Apply button, with the number of apps it will change.
                    fill(tr("Apply ({count})"), &[("count", &changed)])
                } else {
                    tr("Apply").to_string()
                });
                a.set_sensitive(has_pending);
            }
            if has_pending {
                summary_line1.set_text(&fill(
                    ntr(
                        "{count} app changing",
                        "{count} apps changing",
                        changed.into(),
                    ),
                    &[("count", &changed)],
                ));
                let mut parts: Vec<String> = Vec::new();
                if add > 0 {
                    // TRANSLATORS: pending change: this many instances will be added.
                    parts.push(fill(
                        ntr("+{count} instance", "+{count} instances", add.into()),
                        &[("count", &add)],
                    ));
                }
                if rem > 0 {
                    // TRANSLATORS: pending change: this many instances will be removed.
                    // The sign is a minus (U+2212).
                    parts.push(fill(
                        ntr("−{count} instance", "−{count} instances", rem.into()),
                        &[("count", &rem)],
                    ));
                }
                statusbar.set_text(&parts.join("  \u{b7}  "));
            } else {
                let (line1, status) = nothing_pending_summary(last_apply_failed.get());
                summary_line1.set_text(&line1);
                statusbar.set_text(&status);
            }
        }) as Rc<dyn Fn()>
    };

    // Filter + search: toggle each card's visibility and hide empty sections.
    let section_widgets: Vec<(GtkBox, FlowBox)> = sections
        .iter()
        .map(|(_, s, f)| (s.clone(), f.clone()))
        .collect();
    let apply_filter: Rc<dyn Fn()> = {
        let rows = Rc::clone(&rows);
        let catalog = Rc::clone(&catalog);
        let filter_installed = filter_installed.clone();
        let filter_updates = filter_updates.clone();
        let search = search.clone();
        let section_widgets = section_widgets.clone();
        Rc::new(move || {
            let q = search.text().to_lowercase();
            let mode: u8 = if filter_installed.is_active() {
                1
            } else if filter_updates.is_active() {
                2
            } else {
                0
            };
            for h in rows.borrow().iter() {
                let app = &catalog.apps[h.app_index];
                let name = app.display_name_resolved().to_lowercase();
                let matches_q = q.is_empty() || name.contains(q.trim());
                let matches_mode = match mode {
                    1 => h.last_applied.get() > 0,
                    2 => h.update_flag.get(),
                    _ => true,
                };
                h.flow_child.set_visible(matches_q && matches_mode);
            }
            for (section, flow) in section_widgets.iter() {
                let mut any = false;
                let mut child = flow.first_child();
                while let Some(c) = child {
                    if c.property::<bool>("visible") {
                        any = true;
                        break;
                    }
                    child = c.next_sibling();
                }
                section.set_visible(any);
            }
        })
    };

    // Wire each card's hidden spin → re-render that card + recompute summary.
    for handle in rows.borrow().iter() {
        let paths_spin = Rc::clone(&paths);
        let catalog_spin = Rc::clone(&catalog);
        let remote_cache_spin = Rc::clone(&remote_latest_cache);
        let recompute = Rc::clone(&recompute_pending);
        let app_index_spin = handle.app_index;
        let subtitle_w = handle.subtitle_label.clone();
        let status_w = handle.status_pill.clone();
        let tiles_w = handle.tiles_box.clone();
        let count_w = handle.count_label.clone();
        let update_flag_w = Rc::clone(&handle.update_flag);
        let last_applied = Rc::clone(&handle.last_applied);
        let max_allowed = handle.max_allowed;
        handle.spin.connect_value_changed(move |spin| {
            let app = &catalog_spin.apps[app_index_spin];
            let cache = remote_cache_spin.borrow();
            let remote = cache
                .get(app_index_spin)
                .and_then(|o| o.as_ref().map(|h| h.tag.as_str()));
            render_card(
                paths_spin.as_ref(),
                app,
                last_applied.get(),
                max_allowed,
                remote,
                &subtitle_w,
                &status_w,
                &tiles_w,
                &count_w,
                &update_flag_w,
                spin,
            );
            drop(cache);
            (recompute)();
        });
    }

    // Connect the filter toggles + search to the filter pass.
    for toggle in [&filter_all, &filter_installed, &filter_updates] {
        let f = Rc::clone(&apply_filter);
        toggle.connect_toggled(move |b| {
            if b.is_active() {
                (f)();
            }
        });
    }
    {
        let f = Rc::clone(&apply_filter);
        search.connect_search_changed(move |_| (f)());
    }

    (recompute_pending)();
    (apply_filter)();

    // Async catalog icons fetch + paint.
    let (tx_icons, rx_icons) = mpsc::channel::<Vec<Option<Vec<u8>>>>();
    let catalog_for_icons = (*catalog).clone();
    std::thread::spawn(move || {
        let v = batch_catalog_icons(&catalog_for_icons);
        let _ = tx_icons.send(v);
    });

    let rows_icons = Rc::clone(&rows);
    let icons_cache_idle = Rc::clone(&icons_cache);
    glib::idle_add_local(move || match rx_icons.try_recv() {
        Ok(icon_list) => {
            for (i, bytes_opt) in icon_list.iter().enumerate() {
                let rows_b = rows_icons.borrow();
                let Some(h) = rows_b.get(i) else {
                    continue;
                };
                if let Some(bytes) = bytes_opt {
                    let gbytes = glib::Bytes::from(bytes.as_slice());
                    if let Ok(texture) = Texture::from_bytes(&gbytes) {
                        h.icon.set_paintable(Some(&texture));
                    }
                }
            }
            *icons_cache_idle.borrow_mut() = icon_list;
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });

    // Async GitHub release-tag fetch + meta refresh.
    let (tx_remote, rx_remote) = mpsc::channel::<Vec<Option<GithubReleaseHint>>>();
    let catalog_for_fetch = (*catalog).clone();
    std::thread::spawn(move || {
        let v = fetch_github_release_hints(&catalog_for_fetch);
        let _ = tx_remote.send(v);
    });

    let rows_remote = Rc::clone(&rows);
    let paths_remote = Rc::clone(&paths);
    let catalog_remote = Rc::clone(&catalog);
    let remote_latest_cache_idle = Rc::clone(&remote_latest_cache);
    let apply_filter_remote = Rc::clone(&apply_filter);
    glib::idle_add_local(move || match rx_remote.try_recv() {
        Ok(remote_list) => {
            *remote_latest_cache_idle.borrow_mut() = remote_list.clone();
            for (i, remote) in remote_list.into_iter().enumerate() {
                let rows_b = rows_remote.borrow();
                let Some(h) = rows_b.get(i) else {
                    continue;
                };
                let app = &catalog_remote.apps[h.app_index];
                render_card(
                    paths_remote.as_ref(),
                    app,
                    h.last_applied.get(),
                    h.max_allowed,
                    remote.as_ref().map(|hint| hint.tag.as_str()),
                    &h.subtitle_label,
                    &h.status_pill,
                    &h.tiles_box,
                    &h.count_label,
                    &h.update_flag,
                    &h.spin,
                );
            }
            // Update flags may have changed → re-apply the "Updates" filter.
            (apply_filter_remote)();
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });

    // Apply button: reconcile every row's spin to its target instance count.
    let window_for_apply: gtk::ApplicationWindow = window.clone().upcast();
    let window_for_update: gtk::ApplicationWindow = window.clone().upcast();
    // App ids the user allowed to install unverified for the NEXT Apply / Update run only
    // (set by the "Install without verification?" dialog, drained when the run starts).
    let apply_unverified_once: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));
    let update_unverified_once: Rc<RefCell<HashSet<String>>> =
        Rc::new(RefCell::new(HashSet::new()));
    apply.connect_clicked({
        let paths = Rc::clone(&paths);
        let catalog = Rc::clone(&catalog);
        let rows = Rc::clone(&rows);
        let remote_latest_cache = Rc::clone(&remote_latest_cache);
        let progress = progress.clone();
        let statusbar = statusbar.clone();
        let total_label = total_label.clone();
        let apply_weak = apply_weak.clone();
        let update_weak = update_weak.clone();
        let recompute_pending = Rc::clone(&recompute_pending);
        let apply_filter = Rc::clone(&apply_filter);
        let apply_unverified_once = Rc::clone(&apply_unverified_once);
        let last_apply_failed = Rc::clone(&last_apply_failed);
        move |_| {
            last_apply_failed.set(0);
            // DEC-001: only apps the user just allowed in the dialog skip verification.
            let allow_unverified_ids: HashSet<String> =
                apply_unverified_once.borrow_mut().drain().collect();
            let mut specs = Vec::new();
            for h in rows.borrow().iter() {
                let target = h.spin.value() as u32;
                specs.push((h.app_index, target, h.can_install));
            }

            // Confirm before removing any instance data.  Build the removal
            // summary now, on the main thread, before any thread is spawned.
            {
                let rows_b = rows.borrow();
                let mut removal_lines: Vec<String> = Vec::new();
                for h in rows_b.iter() {
                    let target = h.spin.value() as u32;
                    let current = h.last_applied.get();
                    if target < current {
                        // Mirror reconcile_instance_count_inner: to_remove = indices
                        // in current set that are NOT in 1..=target, sorted descending.
                        let mut to_remove: Vec<u32> = (target + 1..=current).collect();
                        to_remove.sort_by(|a, b| b.cmp(a));
                        let count = to_remove.len();
                        let highest = to_remove[0];
                        let lowest = *to_remove.last().expect("non-empty removal list");
                        let app = &catalog.apps[h.app_index];
                        let name = app.display_name_resolved();
                        let range_label = if count == 1 {
                            fill(tr("instance {number}"), &[("number", &highest)])
                        } else {
                            // TRANSLATORS: a range of instance numbers, like "instances 2–4".
                            fill(
                                tr("instances {first}–{last}"),
                                &[("first", &lowest), ("last", &highest)],
                            )
                        };
                        let count_label = fill(
                            ntr("{count} instance", "{count} instances", count as u64),
                            &[("count", &count)],
                        );
                        removal_lines.push(format!(
                            "<b>{}</b>: {range_label} ({count_label})",
                            glib::markup_escape_text(&name),
                        ));
                    }
                }
                if !removal_lines.is_empty() {
                    // Disable Apply for the duration of the nested modal loop:
                    // the dialog's modality is only advisory to the WM, so
                    // without this a second click could spawn a concurrent
                    // apply worker while the dialog is up.
                    if let Some(a) = apply_weak.upgrade() {
                        a.set_sensitive(false);
                    }
                    let detail = removal_lines.join("\n");
                    let confirmed = ask_confirmation(
                        &window_for_apply,
                        ConfirmKind::Warn,
                        tr("Remove instance data?"),
                        tr("Lowering the instance count will permanently delete the home directory data for the removed instances. This cannot be undone."),
                        Some(&format!("<small>{detail}</small>")),
                        tr("Remove and Apply"),
                    );
                    if !confirmed {
                        if let Some(a) = apply_weak.upgrade() {
                            a.set_sensitive(true);
                        }
                        return;
                    }
                }
            }

            let paths_t = (*paths).clone();
            let catalog_t = (*catalog).clone();
            let rows_idle = Rc::clone(&rows);
            let catalog_idle = Rc::clone(&catalog);
            let paths_idle = Rc::clone(&paths);
            let progress_done = progress.clone();
            let apply_done = apply_weak.clone();
            let update_done = update_weak.clone();
            let recompute_done = Rc::clone(&recompute_pending);

            if let Some(a) = apply_weak.upgrade() {
                a.set_sensitive(false);
            }
            if let Some(u) = update_weak.upgrade() {
                u.set_sensitive(false);
            }
            for h in rows.borrow().iter() {
                h.tiles_box.set_sensitive(false);
            }

            progress.set_visible(true);
            progress.set_fraction(0.0);
            progress.set_text(Some(tr("Working…")));
            statusbar.set_text(tr("Working…"));

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            // (log, seed_failed, apply_failed, unverified): seed_failed = seeding
            // errors from seed_new_instances_after_reconcile; unverified =
            // apps refused for lack of a published sha256 (offered in a dialog).
            let (tx, rx) =
                mpsc::channel::<(String, u32, u32, Vec<unverified::UnverifiedApp>)>();
            let last_apply_failed_idle = Rc::clone(&last_apply_failed);

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_apply.clone();
            let status_idle = statusbar.clone();
            let total_idle = total_label.clone();
            let paths_total = Rc::clone(&paths);
            let catalog_total = Rc::clone(&catalog);
            let unverified_once_idle = Rc::clone(&apply_unverified_once);
            let apply_filter_done = Rc::clone(&apply_filter);
            glib::idle_add_local(move || {
                while let Ok((f, label)) = rx_prog.try_recv() {
                    progress_done.set_fraction(f);
                    progress_done.set_text(Some(&label));
                }
                match rx.try_recv() {
                    Ok((log_final, seed_failed, apply_failed, unverified_apps)) => {
                        last_apply_failed_idle.set(apply_failed);
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        let not_seeded = seed_failed;
                        let display_log = if not_seeded > 0 {
                            format!(
                                "{log_final}\nWARNING: {not_seeded} instance(s) installed but NOT seeded — see log above"
                            )
                        } else {
                            log_final
                        };
                        present_activity_log_dialog(&window_idle, &display_log);
                        status_idle.set_text(&format_apply_status(apply_failed, not_seeded));
                        let rem = remote_for_idle.borrow();
                        for h in rows_idle.borrow().iter() {
                            let app = &catalog_idle.apps[h.app_index];
                            let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                            let cap = app.max_instances_allowed();
                            h.last_applied.set(n);
                            h.spin.set_value(n.min(cap) as f64);
                            let can = h.can_install;
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.tiles_box.set_sensitive(has_any && can);
                            let remote = rem
                                .get(h.app_index)
                                .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                            render_card(
                                paths_idle.as_ref(),
                                app,
                                h.last_applied.get(),
                                cap,
                                remote,
                                &h.subtitle_label,
                                &h.status_pill,
                                &h.tiles_box,
                                &h.count_label,
                                &h.update_flag,
                                &h.spin,
                            );
                        }
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        update_total_label(&total_idle, &paths_total, &catalog_total);
                        (recompute_done)();
                        (apply_filter_done)();
                        if !unverified_apps.is_empty() {
                            offer_unverified_install(
                                &window_idle,
                                unverified_apps,
                                Rc::clone(&rows_idle),
                                Rc::clone(&unverified_once_idle),
                                apply_done.clone(),
                            );
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Apply interrupted."));
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        for h in rows_idle.borrow().iter() {
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.tiles_box.set_sensitive(has_any && h.can_install);
                        }
                        glib::ControlFlow::Break
                    }
                }
            });

            std::thread::spawn(move || {
                let mut log = String::new();
                // Counts propagated to the idle handler so the status
                // bar and activity-log dialog can surface seeding failures
                // visibly rather than leaving the user on a plain "Applied."
                let mut seed_failed: u32 = 0;
                let mut unverified_list: Vec<unverified::UnverifiedApp> = Vec::new();
                let mut apply_failed: u32 = 0;
                let n = specs.len();
                if n == 0 {
                    let _ = tx_prog.send((1.0, tr("Working…").to_string()));
                } else {
                    let _ = tx_prog.send((
                        0.0,
                        fill(tr("Working… (0 of {total})"), &[("total", &n)]),
                    ));
                    for (step, (app_index, target, can_install)) in specs.into_iter().enumerate() {
                        let app = &catalog_t.apps[app_index];
                        if target > 0 && !can_install {
                            log.push_str(&format!(
                                "{}: skipped (no AppImage URL for {})\n",
                                app.id,
                                std::env::consts::ARCH
                            ));
                        } else {
                            let policy = appimage_control_core::VerifyPolicy {
                                allow_unverified: allow_unverified_ids.contains(&app.id),
                            };
                            let r = reconcile_instance_count_with_policy(
                                &paths_t,
                                app,
                                target,
                                policy,
                                &mut |m| {
                                    log.push_str(m);
                                    log.push('\n');
                                },
                            );
                            match r {
                                Ok(rep) => {
                                    if !rep.added.is_empty() {
                                        let preset = SeedPreset::auto_default();
                                        log.push_str(&format!(
                                            "{}: auto-seeding {} new instance(s)\n",
                                            app.id,
                                            rep.added.len(),
                                        ));
                                        match appimage_control_core::seed_new_instances_after_reconcile(
                                            &paths_t,
                                            app,
                                            &rep.added,
                                            preset,
                                            &mut |m| {
                                                log.push_str(m);
                                                log.push('\n');
                                            },
                                        ) {
                                            Ok(seed_rep) => {
                                                seed_failed += seed_rep.failed;
                                            }
                                            Err(e) => {
                                                log.push_str(&format!(
                                                    "{}: seeding error: {e}\n",
                                                    app.id
                                                ));
                                                seed_failed +=
                                                    rep.added.len() as u32;
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    apply_failed += 1;
                                    log.push_str(&apply_error_log_line(&app.id, &e.to_string()));
                                    log.push('\n');
                                    let name = app.display_name_resolved();
                                    unverified_list.extend(unverified::unverified_from_outcomes(&[
                                        unverified::AppOutcome {
                                            app_index,
                                            app_id: &app.id,
                                            name: &name,
                                            target,
                                            error: Some(&e),
                                        },
                                    ]));
                                }
                            }
                        }
                        let done = step + 1;
                        let frac = done as f64 / n as f64;
                        let _ = tx_prog
                            .send((
                            frac,
                            // TRANSLATORS: progress: {done} of {total} apps, then the app's id.
                            fill(
                                tr("Working… ({done}/{total}): {app}"),
                                &[("done", &done), ("total", &n), ("app", &app.id)],
                            ),
                        ));
                    }
                }
                if log.is_empty() {
                    log.push_str("No changes applied.\n");
                }
                let _ = tx.send((log, seed_failed, apply_failed, unverified_list));
            });
        }
    });

    update_installed.connect_clicked({
        let paths = Rc::clone(&paths);
        let catalog = Rc::clone(&catalog);
        let rows = Rc::clone(&rows);
        let remote_latest_cache = Rc::clone(&remote_latest_cache);
        let progress = progress.clone();
        let statusbar = statusbar.clone();
        let total_label = total_label.clone();
        let apply_weak = apply_weak.clone();
        let update_weak = update_weak.clone();
        let recompute_pending = Rc::clone(&recompute_pending);
        let apply_filter = Rc::clone(&apply_filter);
        let update_unverified_once = Rc::clone(&update_unverified_once);
        move |_| {
            // DEC-001: a re-run after "Install unverified" updates only those apps, with the
            // opt-in; a normal run updates everything, strictly.
            let allow_ids: HashSet<String> = update_unverified_once.borrow_mut().drain().collect();
            let policy = appimage_control_core::VerifyPolicy {
                allow_unverified: !allow_ids.is_empty(),
            };
            let paths_t = (*paths).clone();
            let catalog_t = if allow_ids.is_empty() {
                (*catalog).clone()
            } else {
                Catalog {
                    apps: catalog
                        .apps
                        .iter()
                        .filter(|a| allow_ids.contains(&a.id))
                        .cloned()
                        .collect(),
                }
            };
            let rows_idle = Rc::clone(&rows);
            let catalog_idle = Rc::clone(&catalog);
            let paths_idle = Rc::clone(&paths);
            let progress_done = progress.clone();
            let apply_done = apply_weak.clone();
            let update_done = update_weak.clone();
            let recompute_done = Rc::clone(&recompute_pending);

            if let Some(a) = apply_weak.upgrade() {
                a.set_sensitive(false);
            }
            if let Some(u) = update_weak.upgrade() {
                u.set_sensitive(false);
            }
            for h in rows.borrow().iter() {
                h.tiles_box.set_sensitive(false);
            }

            progress.set_visible(true);
            progress.set_fraction(0.0);
            progress.set_text(Some(tr("Updating…")));
            statusbar.set_text(tr("Updating…"));

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            let (tx, rx) = mpsc::channel::<(String, UpdateAllReport)>();

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_update.clone();
            let status_idle = statusbar.clone();
            let total_idle = total_label.clone();
            let paths_total = Rc::clone(&paths);
            let catalog_total = Rc::clone(&catalog);
            let apply_filter_done = Rc::clone(&apply_filter);
            let unverified_once_idle = Rc::clone(&update_unverified_once);
            glib::idle_add_local(move || {
                while let Ok((f, label)) = rx_prog.try_recv() {
                    progress_done.set_fraction(f);
                    progress_done.set_text(Some(&label));
                }
                match rx.try_recv() {
                    Ok((log_final, rep)) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        let summary = format_update_status_summary(&rep);
                        status_idle.set_text(summary.as_str());
                        present_activity_log_dialog(&window_idle, &log_final);
                        let rem = remote_for_idle.borrow();
                        for h in rows_idle.borrow().iter() {
                            let app = &catalog_idle.apps[h.app_index];
                            let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                            let cap = app.max_instances_allowed();
                            h.last_applied.set(n);
                            h.spin.set_value(n.min(cap) as f64);
                            let can = h.can_install;
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.tiles_box.set_sensitive(has_any && can);
                            let remote = rem
                                .get(h.app_index)
                                .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                            render_card(
                                paths_idle.as_ref(),
                                app,
                                h.last_applied.get(),
                                cap,
                                remote,
                                &h.subtitle_label,
                                &h.status_pill,
                                &h.tiles_box,
                                &h.count_label,
                                &h.update_flag,
                                &h.spin,
                            );
                        }
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        update_total_label(&total_idle, &paths_total, &catalog_total);
                        (recompute_done)();
                        (apply_filter_done)();
                        if !rep.unverified_apps.is_empty() {
                            let apps: Vec<unverified::UnverifiedApp> = rep
                                .unverified_apps
                                .iter()
                                .filter_map(|(id, reason)| {
                                    let (i, a) = catalog_idle
                                        .apps
                                        .iter()
                                        .enumerate()
                                        .find(|(_, a)| &a.id == id)?;
                                    Some(unverified::UnverifiedApp {
                                        app_index: i,
                                        app_id: a.id.clone(),
                                        name: a.display_name_resolved(),
                                        target: instance_count(paths_idle.as_ref(), a).unwrap_or(0),
                                        reason: reason.clone(),
                                    })
                                })
                                .collect();
                            offer_unverified_install(
                                &window_idle,
                                apps,
                                Rc::clone(&rows_idle),
                                Rc::clone(&unverified_once_idle),
                                update_done.clone(),
                            );
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Update interrupted."));
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        for h in rows_idle.borrow().iter() {
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.tiles_box.set_sensitive(has_any && h.can_install);
                        }
                        glib::ControlFlow::Break
                    }
                }
            });

            std::thread::spawn(move || {
                let mut log = String::new();
                let remotes = fetch_github_release_hints(&catalog_t);
                let mut send_frac = move |f: f64, done: u32, total: u32| {
                    let label = if total == 0 {
                        tr("Updating…").to_string()
                    } else {
                        fill(
                            tr("Updating… ({done}/{total})"),
                            &[("done", &done), ("total", &total)],
                        )
                    };
                    let _ = tx_prog.send((f, label));
                };
                let report = match update_all_installed_instances_with_policy(
                    &paths_t,
                    &catalog_t,
                    Some(&remotes),
                    policy,
                    &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    },
                    Some(&mut send_frac),
                ) {
                    Ok(r) => r,
                    Err(e) => {
                        log.push_str(&format!("Update failed: {e}\n"));
                        UpdateAllReport {
                            errors: 1,
                            ..Default::default()
                        }
                    }
                };
                if log.trim().is_empty() {
                    log.push_str("No installed instances to update.\n");
                }
                let _ = tx.send((log, report));
            });
        }
    });

    install_window_actions_for_catalog(
        window,
        &paths,
        &catalog,
        &rows,
        &remote_latest_cache,
        &icons_cache,
        &apply,
        &update_installed,
        &progress,
        &statusbar,
        &total_label,
        Rc::clone(&recompute_pending),
        Rc::clone(&apply_filter),
    );
}

/// After an Apply or Update run, ask whether to install the apps that were refused for lack of a
/// published sha256 (DEC-001). Cancel (the default) changes nothing. "Install unverified" puts
/// the Apply targets back, marks exactly those apps as allowed for the next run only, and
/// re-runs it by clicking `rerun`. Shown from a fresh idle so it never nests inside the
/// worker's completion handler.
fn offer_unverified_install(
    window: &gtk::ApplicationWindow,
    apps: Vec<unverified::UnverifiedApp>,
    rows: Rc<RefCell<Vec<RowHandles>>>,
    allow_once: Rc<RefCell<HashSet<String>>>,
    rerun: glib::WeakRef<Button>,
) {
    let window = window.clone();
    glib::idle_add_local_once(move || {
        let detail = glib::markup_escape_text(&unverified::dialog_detail(&apps));
        let confirmed = ask_confirmation(
            &window,
            ConfirmKind::Warn,
            tr("Install without verification?"),
            tr("No published sha256 is available to check these downloads, so they were not installed. Install them anyway only if you trust where they come from."),
            Some(&format!("<small>{detail}</small>")),
            tr("Install unverified"),
        );
        let Some(req) = unverified::rerun_for_response(&apps, confirmed) else {
            return;
        };
        for (app_index, target) in &req.apps {
            if let Some(h) = rows.borrow().iter().find(|h| h.app_index == *app_index) {
                h.spin.set_value(*target as f64);
            }
        }
        let Some(b) = rerun.upgrade() else {
            // Nothing to re-run: do not leave an opt-in behind for a later, unrelated click.
            return;
        };
        allow_once
            .borrow_mut()
            .extend(apps.iter().map(|a| a.app_id.clone()));
        b.emit_clicked();
    });
}

fn refresh_all_rows(
    paths: &Rc<AppPaths>,
    catalog: &Rc<Catalog>,
    remote_latest_cache: &Rc<RefCell<Vec<Option<GithubReleaseHint>>>>,
    rows: &Rc<RefCell<Vec<RowHandles>>>,
) {
    let rem = remote_latest_cache.borrow();
    for h in rows.borrow().iter() {
        let app = &catalog.apps[h.app_index];
        let remote = rem
            .get(h.app_index)
            .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
        render_card(
            paths.as_ref(),
            app,
            h.last_applied.get(),
            h.max_allowed,
            remote,
            &h.subtitle_label,
            &h.status_pill,
            &h.tiles_box,
            &h.count_label,
            &h.update_flag,
            &h.spin,
        );
    }
}

fn update_total_label(label: &Label, paths: &Rc<AppPaths>, catalog: &Rc<Catalog>) {
    let total = catalog.apps.len();
    let installed = catalog
        .apps
        .iter()
        .filter(|app| {
            instance_count(paths.as_ref(), app)
                .map(|n| n > 0)
                .unwrap_or(false)
        })
        .count();
    // TRANSLATORS: window footer, like "12 apps · 3 installed".
    let apps = fill(
        ntr("{count} app", "{count} apps", total as u64),
        &[("count", &total)],
    );
    let inst = fill(tr("{count} installed"), &[("count", &installed)]);
    label.set_text(&format!("{apps} · {inst}"));
}

#[allow(clippy::too_many_arguments)]
fn install_window_actions_for_catalog(
    window: &adw::ApplicationWindow,
    paths: &Rc<AppPaths>,
    catalog: &Rc<Catalog>,
    rows: &Rc<RefCell<Vec<RowHandles>>>,
    remote_latest_cache: &Rc<RefCell<Vec<Option<GithubReleaseHint>>>>,
    icons_cache: &Rc<RefCell<Vec<Option<Vec<u8>>>>>,
    apply_btn: &Button,
    update_btn: &Button,
    progress: &ProgressBar,
    statusbar: &Label,
    total_label: &Label,
    recompute_pending: Rc<dyn Fn()>,
    apply_filter: Rc<dyn Fn()>,
) {
    let window_for_actions: gtk::ApplicationWindow = window.clone().upcast();

    // `win.open_catalog` — pick a catalog.json and re-mount the view with it.
    // Backs the Preferences → General "Open…" button (design 3a).
    {
        let a = gtk::gio::SimpleAction::new("open_catalog", None);
        let win_c = window.clone();
        let paths = Rc::clone(paths);
        let icons_cache = Rc::clone(icons_cache);
        a.connect_activate(move |_, _| {
            let Some(app) = win_c.application().and_downcast::<adw::Application>() else {
                return;
            };
            let chooser_parent: gtk::ApplicationWindow = win_c.clone().upcast();
            if let Some(path) = choose_open_path(&chooser_parent, tr("Choose catalog.json")) {
                if let Ok(c) = Catalog::discover_load(Some(&path)) {
                    mount_catalog_view(
                        &app,
                        &win_c,
                        Rc::clone(&paths),
                        Rc::new(c),
                        Rc::clone(&icons_cache),
                        || {},
                    );
                }
            }
        });
        window.add_action(&a);
    }

    let refresh_fn: Rc<dyn Fn()> = {
        let paths = Rc::clone(paths);
        let catalog = Rc::clone(catalog);
        let rows = Rc::clone(rows);
        let remote = Rc::clone(remote_latest_cache);
        let total_label = total_label.clone();
        let statusbar = statusbar.clone();
        let recompute_pending = Rc::clone(&recompute_pending);
        let apply_filter = Rc::clone(&apply_filter);
        Rc::new(move || {
            for h in rows.borrow().iter() {
                let app = &catalog.apps[h.app_index];
                let n = instance_count(paths.as_ref(), app).unwrap_or(0);
                h.last_applied.set(n);
                h.spin.set_value(n.min(app.max_instances_allowed()) as f64);
            }
            refresh_all_rows(&paths, &catalog, &remote, &rows);
            update_total_label(&total_label, &paths, &catalog);
            statusbar.set_text(tr("Ready"));
            (recompute_pending)();
            (apply_filter)();
        })
    };

    let hooks = actions::ActionHooks {
        apply: {
            let b = apply_btn.clone();
            Rc::new(move || b.emit_clicked())
        },
        update: {
            let b = update_btn.clone();
            Rc::new(move || b.emit_clicked())
        },
        seed_settings: {
            let window = window_for_actions.clone();
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let icons_cache = Rc::clone(icons_cache);
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            let rows = Rc::clone(rows);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                // The check below falls back to the embedded packs, so a library that
                // cannot load would read as "nothing to seed". Say what is wrong instead.
                match SeedPackLibrary::discover_load(None) {
                    Err(e) => {
                        present_message_window(
                            window.application().as_ref().expect("app"),
                            tr("Seed packs not loaded"),
                            &e.to_string(),
                            None,
                        );
                        return;
                    }
                    // A refused pack is skipped and the others still seed (WF-SEED-001
                    // step 2); name it so its app is not silently missing from the list.
                    Ok(lib) => {
                        let refused: Vec<String> = lib
                            .refused()
                            .map(|(id, why)| format!("{id}: {why}"))
                            .collect();
                        if !refused.is_empty() {
                            present_message_window(
                                window.application().as_ref().expect("app"),
                                tr("Some seed packs were skipped"),
                                &refused.join("\n\n"),
                                None,
                            );
                        }
                    }
                }
                let installed_supported: Vec<(String, String, u32, usize)> = catalog
                    .apps
                    .iter()
                    .enumerate()
                    .filter(|(_, app)| SupportedSeedApp::from_catalog_app(app).is_some())
                    .filter_map(|(i, app)| {
                        let n = instance_count(paths.as_ref(), app).ok()?;
                        (n > 0).then(|| (app.id.clone(), app.display_name_resolved(), n, i))
                    })
                    .collect();
                if installed_supported.is_empty() {
                    present_message_window(
                        window.application().as_ref().expect("app"),
                        tr("No supported installs"),
                        tr("There are no installed Feather or Electrum instances to seed."),
                        None,
                    );
                    return;
                }
                let public_apps: Vec<(String, String, u32)> = installed_supported
                    .iter()
                    .map(|(id, name, n, _)| (id.clone(), name.clone(), *n))
                    .collect();
                let icons_borrow = icons_cache.borrow();
                let icons_for_dialog: Vec<Option<Vec<u8>>> = installed_supported
                    .iter()
                    .map(|(_, _, _, i)| icons_borrow.get(*i).cloned().unwrap_or(None))
                    .collect();
                drop(icons_borrow);

                let Some((preset, selected_ids)) =
                    choose_seed_settings(&window, &public_apps, &icons_for_dialog)
                else {
                    return;
                };

                if !selected_ids.is_empty() {
                    apply_btn.set_sensitive(false);
                    update_btn.set_sensitive(false);
                    for h in rows.borrow().iter() {
                        h.tiles_box.set_sensitive(false);
                    }
                }
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Seeding settings…")));
                statusbar.set_text(tr("Seeding settings…"));

                let total: u32 = public_apps
                    .iter()
                    .filter(|(app_id, _, _)| selected_ids.iter().any(|id| id == app_id))
                    .map(|(_, _, n)| *n)
                    .sum();
                let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
                let (tx, rx) = mpsc::channel::<(String, u32, u32)>();
                let progress_done = progress.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                let apply_idle = apply_btn.clone();
                let update_idle = update_btn.clone();
                let rows_idle = Rc::clone(&rows);
                glib::idle_add_local(move || {
                    while let Ok((f, label)) = rx_prog.try_recv() {
                        progress_done.set_fraction(f);
                        progress_done.set_text(Some(&label));
                    }
                    match rx.try_recv() {
                        Ok((log, seeded, failed)) => {
                            progress_done.set_visible(false);
                            progress_done.set_fraction(0.0);
                            status_idle
                                .set_text(format_seed_status_summary(seeded, failed).as_str());
                            present_activity_log_dialog(&window_idle, &log);
                            refresh_idle();
                            apply_idle.set_sensitive(true);
                            update_idle.set_sensitive(true);
                            for h in rows_idle.borrow().iter() {
                                h.tiles_box.set_sensitive(true);
                            }
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            progress_done.set_visible(false);
                            progress_done.set_fraction(0.0);
                            progress_done.set_text(None);
                            status_idle.set_text(tr("Seed settings interrupted."));
                            apply_idle.set_sensitive(true);
                            update_idle.set_sensitive(true);
                            for h in rows_idle.borrow().iter() {
                                h.tiles_box.set_sensitive(true);
                            }
                            glib::ControlFlow::Break
                        }
                    }
                });

                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();
                std::thread::spawn(move || {
                    let mut log = String::new();
                    let mut seeded = 0u32;
                    let mut failed = 0u32;
                    let mut done = 0u32;
                    if total == 0 {
                        let _ = tx_prog.send((1.0, tr("Seeding settings…").to_string()));
                    }
                    for app in &catalog_t.apps {
                        if !selected_ids.iter().any(|id| id == &app.id) {
                            continue;
                        }
                        if SupportedSeedApp::from_catalog_app(app).is_none() {
                            continue;
                        }
                        let indices =
                            list_managed_instance_indices(&paths_t, app).unwrap_or_default();
                        for i in indices {
                            match seed_instance_settings(&paths_t, app, i, preset, &mut |m| {
                                log.push_str(m);
                                log.push('\n');
                            }) {
                                Ok(appimage_control_core::SeedOutcome::Seeded) => seeded += 1,
                                Ok(_) => {}
                                Err(e) => {
                                    failed += 1;
                                    log.push_str(&format!(
                                        "{} instance {}: seeding failed: {e}\n",
                                        app.id, i
                                    ));
                                }
                            }
                            done += 1;
                            if total > 0 {
                                let _ = tx_prog.send((
                                    done as f64 / total as f64,
                                    fill(
                                        tr("Seeding settings… ({done}/{total})"),
                                        &[("done", &done), ("total", &total)],
                                    ),
                                ));
                            }
                        }
                    }
                    if log.trim().is_empty() {
                        log.push_str("No supported installed instances to seed.\n");
                    }
                    let _ = tx.send((log, seeded, failed));
                });
            })
        },
        record_seed_pack: {
            let window = window_for_actions.clone();
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            let rows = Rc::clone(rows);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                // Installable apps (≥ 1 instance), with config_path /
                // first_run_args pre-filled from any existing seed pack.
                // Stops here: the notice is not modal, so going on would open the
                // recorder's modal dialog over it.
                let lib = match SeedPackLibrary::discover_load(None) {
                    Ok(lib) => lib,
                    Err(e) => {
                        present_message_window(
                            window.application().as_ref().expect("app"),
                            tr("Seed packs not loaded"),
                            &e.to_string(),
                            None,
                        );
                        return;
                    }
                };
                let apps: Vec<RecordApp> = catalog
                    .apps
                    .iter()
                    .filter_map(|app| {
                        let n = instance_count(paths.as_ref(), app).ok()?;
                        if n == 0 {
                            return None;
                        }
                        let (known_config_path, first_run_args) = lib
                            .get(&app.id)
                            .map(|p| (Some(p.config_path.clone()), p.first_run_args.clone()))
                            .unwrap_or((None, Vec::new()));
                        Some(RecordApp {
                            app_id: app.id.clone(),
                            display_name: app.display_name_resolved(),
                            instances: n,
                            known_config_path,
                            first_run_args,
                        })
                    })
                    .collect();
                if apps.is_empty() {
                    present_message_window(
                        window.application().as_ref().expect("app"),
                        tr("No installed apps"),
                        tr("Install at least one app instance before recording a seed pack."),
                        None,
                    );
                    return;
                }
                let Some(target) = choose_record_target(&window, &apps) else {
                    return;
                };

                // Busy state while the headless baseline capture runs off
                // the UI thread (it launches the AppImage under xvfb).
                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                for h in rows.borrow().iter() {
                    h.tiles_box.set_sensitive(false);
                }
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Capturing baseline…")));
                statusbar.set_text(tr("Capturing baseline…"));

                // Worker returns (baseline config object, resolved config_path).
                // When the target's config_path is blank, the path is
                // auto-detected during the same headless launch — no manual
                // entry, no extra click.
                type Captured = serde_json::Map<String, serde_json::Value>;
                let (tx, rx) = mpsc::channel::<Result<(Captured, String), String>>();
                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();
                let target_t = target.clone();
                std::thread::spawn(move || {
                    let res = (|| -> Result<(Captured, String), String> {
                        let app = catalog_t
                            .apps
                            .iter()
                            .find(|a| a.id == target_t.app_id)
                            .ok_or_else(|| {
                                fill(
                                    tr("App “{app}” is not in the catalog."),
                                    &[("app", &target_t.app_id)],
                                )
                            })?;
                        if target_t.config_path.trim().is_empty() {
                            // Auto-detect: one headless launch produces the
                            // config; scan for it, then read it back (no
                            // second launch).
                            let cands = discover_config_candidates(
                                &paths_t,
                                app,
                                target_t.instance_index,
                                &target_t.first_run_args,
                                &mut |_| {},
                            )
                            .map_err(|e| e.to_string())?;
                            let path = match cands.len() {
                                0 => {
                                    return Err(
                                        "Could not auto-detect a config file for this app. \
                                     Re-open the wizard and set the Config path field."
                                            .into(),
                                    )
                                }
                                1 => cands[0].clone(),
                                _ => pick_best_config_candidate(&cands),
                            };
                            let map =
                                read_instance_config(&paths_t, app, target_t.instance_index, &path)
                                    .map_err(|e| e.to_string())?;
                            Ok((map, path))
                        } else {
                            let cap = capture_first_run_config(
                                &paths_t,
                                app,
                                target_t.instance_index,
                                &target_t.config_path,
                                &target_t.first_run_args,
                                &mut |_| {},
                            )
                            .map_err(|e| e.to_string())?;
                            Ok((cap.config, target_t.config_path.clone()))
                        }
                    })();
                    let _ = tx.send(res);
                });

                let window_idle = window.clone();
                let paths_idle = Rc::clone(&paths);
                let catalog_idle = Rc::clone(&catalog);
                let progress_idle = progress.clone();
                let status_idle = statusbar.clone();
                let apply_idle = apply_btn.clone();
                let update_idle = update_btn.clone();
                let rows_idle = Rc::clone(&rows);
                let refresh_idle = Rc::clone(&refresh);
                let target_idle = target.clone();
                glib::idle_add_local(move || {
                    let msg = match rx.try_recv() {
                        Ok(m) => m,
                        Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            progress_idle.set_visible(false);
                            progress_idle.set_fraction(0.0);
                            status_idle.set_text(tr("Baseline capture interrupted."));
                            apply_idle.set_sensitive(true);
                            update_idle.set_sensitive(true);
                            for h in rows_idle.borrow().iter() {
                                h.tiles_box.set_sensitive(true);
                            }
                            return glib::ControlFlow::Break;
                        }
                    };

                    // Restore the UI to interactive before the (modal) follow-up steps.
                    progress_idle.set_visible(false);
                    progress_idle.set_fraction(0.0);
                    progress_idle.set_text(None);
                    apply_idle.set_sensitive(true);
                    update_idle.set_sensitive(true);
                    for h in rows_idle.borrow().iter() {
                        h.tiles_box.set_sensitive(true);
                    }

                    let app_handle = window_idle.application();
                    let gapp = app_handle.as_ref().expect("app");
                    match msg {
                        Err(e) => {
                            status_idle.set_text(tr("Baseline capture failed."));
                            present_message_window(gapp, tr("Baseline capture failed"), &e, None);
                        }
                        Ok((baseline, resolved_path)) => {
                            let target = &target_idle;
                            let Some(app) =
                                catalog_idle.apps.iter().find(|a| a.id == target.app_id)
                            else {
                                return glib::ControlFlow::Break;
                            };
                            // `resolved_path` is the config path actually used —
                            // either the user's override or the auto-detected one.
                            // Persist the baseline so it survives even if the
                            // user comes back later (mirrors the CLI recording).
                            if let Some(home) = dirs::home_dir() {
                                if let Ok(rec) = PackRecording::new(
                                    target.app_id.clone(),
                                    target.instance_index,
                                    resolved_path.clone(),
                                    target.first_run_args.clone(),
                                    SeedPreset::auto_default(),
                                    baseline.clone(),
                                ) {
                                    let _ = save_recording(&home, &rec);
                                }
                            }
                            let appimage_abs = paths_idle
                                .instance_dir_for(app, target.instance_index)
                                .join(app.resolved_appimage_filename());
                            status_idle
                                .set_text(tr("Baseline captured — configure the app, then diff."));
                            if confirm_manual_step(
                                &window_idle,
                                &target.app_id,
                                target.instance_index,
                                &appimage_abs,
                            ) {
                                match read_instance_config(
                                    &paths_idle,
                                    app,
                                    target.instance_index,
                                    &resolved_path,
                                ) {
                                    Ok(current) => {
                                        let diff = diff_configs(&baseline, &current);
                                        if diff.is_empty_capture() {
                                            status_idle.set_text(tr("No settings changed."));
                                            present_message_window(
                                                gapp,
                                                tr("No changes to record"),
                                                tr("The config is unchanged from the baseline — nothing to capture."),
                                                None,
                                            );
                                        } else if let Some(keys) = review_and_select(
                                            &window_idle,
                                            &baseline,
                                            &current,
                                            &diff,
                                        ) {
                                            if keys.is_empty() {
                                                status_idle.set_text(tr("No keys selected."));
                                            } else {
                                                let pack = build_static_pack(
                                                    &resolved_path,
                                                    &target.first_run_args,
                                                    &current,
                                                    &keys,
                                                );
                                                if let Some(out) = choose_pack_save_path(
                                                    &window_idle,
                                                    &target.app_id,
                                                ) {
                                                    match write_pack_to(&out, &target.app_id, &pack)
                                                    {
                                                        Ok(()) => {
                                                            status_idle
                                                                .set_text(tr("Seed pack written."));
                                                            present_message_window(
                                                                gapp,
                                                                tr("Seed pack written"),
                                                                &fill(
                                                                    ntr(
                                                                        "Wrote {count} key to {path}",
                                                                        "Wrote {count} keys to {path}",
                                                                        keys.len() as u64,
                                                                    ),
                                                                    &[
                                                                        ("count", &keys.len()),
                                                                        ("path", &out.display()),
                                                                    ],
                                                                ),
                                                                Some(tr("This pack is NOT signed — review it before distributing.")),
                                                            );
                                                        }
                                                        Err(e) => present_message_window(
                                                            gapp,
                                                            tr("Could not write pack"),
                                                            &e.to_string(),
                                                            None,
                                                        ),
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    Err(e) => present_message_window(
                                        gapp,
                                        tr("Could not read current config"),
                                        &e.to_string(),
                                        None,
                                    ),
                                }
                            }
                        }
                    }
                    refresh_idle();
                    glib::ControlFlow::Break
                });
            })
        },
        refresh: Rc::clone(&refresh_fn),
        clean_cache: {
            let paths = Rc::clone(paths);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                let mut log = String::new();
                match clean_catalog_download_cache(paths.as_ref()) {
                    Ok(()) => log.push_str("Cache cleaned.\n"),
                    Err(e) => log.push_str(&format!("Clean cache failed: {e}\n")),
                }
                present_activity_log_dialog(&window, &log);
                refresh();
            })
        },
        clean: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                if !ask_confirmation(
                    &window,
                    ConfirmKind::Warn,
                    tr("Clean installs?"),
                    tr("Remove installed catalog instances and their managed launchers and icons. The download cache is kept, so reinstalling is fast."),
                    Some(
                        "<small>• <tt>~/.local/share/appimages/apps/*</tt>\n\
                         • <tt>~/.local/share/applications/appimage-control-*.desktop</tt></small>",
                    ),
                    tr("Clean"),
                ) {
                    return;
                }

                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Cleaning…")));
                statusbar.set_text(tr("Cleaning…"));

                let (tx, rx) = mpsc::channel::<String>();
                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok(log) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log);
                        refresh_idle();
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        status_idle.set_text(tr("Ready"));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Clean interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    let r =
                        clean_installed_catalog_apps_keep_cache(&paths_t, &catalog_t, &mut |m| {
                            log.push_str(m);
                            log.push('\n');
                        });
                    if let Err(e) = r {
                        log.push_str(&format!("Clean failed: {e}\n"));
                    }
                    let _ = tx.send(log);
                });
            })
        },
        purge: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                if !ask_confirmation(
                    &window,
                    ConfirmKind::Danger,
                    tr("Purge all manager state?"),
                    tr("This removes every install, the download cache, managed launchers, and icon tiles owned by AppImage Control. App data inside instance home dirs is also removed."),
                    Some(
                        "<small>• <tt>~/.local/share/appimages/</tt> (apps, cache, files, icons)\n\
                         • <tt>~/.local/share/applications/appimage-control-*.desktop</tt>\n\
                         • <tt>~/.local/share/icons/hicolor/*/apps/appimage-control-*.png</tt></small>",
                    ),
                    tr("Purge state"),
                ) {
                    return;
                }

                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Purging…")));
                statusbar.set_text(tr("Purging…"));

                let (tx, rx) = mpsc::channel::<String>();
                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok(log) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log);
                        refresh_idle();
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        status_idle.set_text(tr("Ready"));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Purge interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    let r = purge_appimage_control_state(&paths_t, Some(&catalog_t), &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    });
                    if let Err(e) = r {
                        log.push_str(&format!("Purge failed: {e}\n"));
                    }
                    let _ = tx.send(log);
                });
            })
        },
        migrate: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                // Present the strategy dialog (which doubles as the "are you sure?" gate —
                // Cancel aborts). The chosen resolution is fixed for the whole run so the
                // worker thread never needs to call back into the UI.
                let Some(resolution) = ask_migration_strategy(&window) else {
                    return;
                };

                let cwd = match std::env::current_dir() {
                    Ok(p) => p,
                    Err(e) => {
                        let log = format!("Could not read current dir: {e}\n");
                        present_activity_log_dialog(&window, &log);
                        return;
                    }
                };
                let candidates = discover_migration_candidates(&cwd, catalog.as_ref());

                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Migrating…")));
                statusbar.set_text(tr("Migrating…"));

                let (tx, rx) = mpsc::channel::<String>();
                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok(log) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log);
                        refresh_idle();
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        status_idle.set_text(tr("Ready"));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Migration interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    let r = run_portable_dir_migration(
                        &paths_t,
                        &catalog_t,
                        &candidates,
                        Some(resolution),
                        false,
                        // Dead-fallback: fixed_on_conflict covers every conflict;
                        // this closure is reached only if fixed_on_conflict is somehow
                        // exhausted, which cannot happen with the current implementation.
                        |_| resolution,
                        &mut |m| {
                            log.push_str(m);
                            log.push('\n');
                        },
                    );
                    if let Err(e) = r {
                        log.push_str(&format!("Migration failed: {e}\n"));
                    }
                    let _ = tx.send(log);
                });
            })
        },
        backup: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                let Some(dest_path) =
                    choose_save_path(&window, tr("Choose backup archive destination"))
                else {
                    return;
                };

                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Backing up…")));
                statusbar.set_text(tr("Backing up…"));

                let (tx, rx) = mpsc::channel::<String>();
                let paths_t = (*paths).clone();
                let catalog_t = (*catalog).clone();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok(log) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log);
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        status_idle.set_text(tr("Ready"));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Backup interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    match create_backup_archive(
                        &paths_t,
                        &catalog_t,
                        &dest_path,
                        &BackupOptions::default(),
                    ) {
                        Ok(rep) => log.push_str(&format!(
                            "Backup created: {} ({} entries, {} bytes)\n",
                            rep.archive_path.display(),
                            rep.entry_count,
                            rep.total_bytes
                        )),
                        Err(e) => log.push_str(&format!("Backup failed: {e}\n")),
                    }
                    let _ = tx.send(log);
                });
            })
        },
        restore_backup: {
            let paths = Rc::clone(paths);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                let Some(src_path) =
                    choose_open_path(&window, tr("Choose backup archive to restore"))
                else {
                    return;
                };

                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Restoring…")));
                statusbar.set_text(tr("Restoring…"));

                // Channel carries: (log_so_far, needs_force_confirm).
                // `needs_force_confirm = true` means the first attempt hit
                // "managed state already exists"; the idle handler then asks
                // the user and, if confirmed, spawns the force-restore thread.
                let (tx, rx) = mpsc::channel::<(String, bool)>();
                let paths_t = (*paths).clone();
                // src_path_t goes into the idle closure (for a potential
                // force-retry); src_path_spawn is the independent copy for
                // the first-pass thread.
                let src_path_t = src_path.clone();
                let src_path_spawn = src_path.clone();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                let paths_idle = (*paths).clone();
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok((log, needs_force)) => {
                        if needs_force {
                            // Ask the user on the main thread; if they confirm,
                            // run a second thread for the force restore.
                            //
                            // SAFETY of the nested MainLoop inside this idle
                            // handler: ask_confirmation spins a nested
                            // glib::MainLoop, during which other idles still
                            // fire. Re-entrancy into a concurrent Apply/Update
                            // is impossible only because apply_btn/update_btn
                            // were disabled before the restore worker spawned
                            // and are re-enabled strictly after this branch
                            // completes. Keep that ordering if this flow is
                            // restructured.
                            if ask_confirmation(
                                &window_idle,
                                ConfirmKind::Danger,
                                tr("Overwrite existing managed state?"),
                                tr("Managed state already exists. Force restore and overwrite manager-owned state?"),
                                None,
                                tr("Overwrite"),
                            ) {
                                progress_done.set_text(Some(tr("Restoring (forced)…")));
                                status_idle.set_text(tr("Restoring (forced)…"));
                                let (tx2, rx2) = mpsc::channel::<String>();
                                let paths_f = paths_idle.clone();
                                let src_f = src_path_t.clone();
                                let progress_f = progress_done.clone();
                                let apply_f = apply_done.clone();
                                let update_f = update_done.clone();
                                let status_f = status_idle.clone();
                                let window_f = window_idle.clone();
                                let refresh_f = Rc::clone(&refresh_idle);
                                glib::idle_add_local(move || match rx2.try_recv() {
                                    Ok(log2) => {
                                        progress_f.set_visible(false);
                                        progress_f.set_fraction(0.0);
                                        present_activity_log_dialog(&window_f, &log2);
                                        refresh_f();
                                        apply_f.set_sensitive(true);
                                        update_f.set_sensitive(true);
                                        status_f.set_text(tr("Ready"));
                                        glib::ControlFlow::Break
                                    }
                                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        progress_f.set_visible(false);
                                        progress_f.set_fraction(0.0);
                                        progress_f.set_text(None);
                                        status_f.set_text(tr("Restore interrupted."));
                                        apply_f.set_sensitive(true);
                                        update_f.set_sensitive(true);
                                        glib::ControlFlow::Break
                                    }
                                });
                                std::thread::spawn(move || {
                                    let mut log2 = String::new();
                                    match restore_backup_archive(&paths_f, &src_f, true) {
                                        Ok(()) => log2.push_str("Forced restore finished.\n"),
                                        Err(e2) => log2
                                            .push_str(&format!("Forced restore failed: {e2}\n")),
                                    }
                                    let _ = tx2.send(log2);
                                });
                            } else {
                                // User declined the overwrite — show what we have and re-enable.
                                progress_done.set_visible(false);
                                progress_done.set_fraction(0.0);
                                present_activity_log_dialog(&window_idle, &log);
                                apply_done.set_sensitive(true);
                                update_done.set_sensitive(true);
                                status_idle.set_text(tr("Ready"));
                            }
                        } else {
                            progress_done.set_visible(false);
                            progress_done.set_fraction(0.0);
                            present_activity_log_dialog(&window_idle, &log);
                            refresh_idle();
                            apply_done.set_sensitive(true);
                            update_done.set_sensitive(true);
                            status_idle.set_text(tr("Ready"));
                        }
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Restore interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    match restore_backup_archive(&paths_t, &src_path_spawn, false) {
                        Ok(()) => {
                            log.push_str("Restore finished.\n");
                            let _ = tx.send((log, false));
                        }
                        Err(e) => {
                            let msg = e.to_string();
                            if msg.contains("managed state already exists") {
                                // Signal the idle handler to ask the user; include
                                // the original error text for context.
                                log.push_str(&format!("Restore failed: {e}\n"));
                                let _ = tx.send((log, true));
                            } else {
                                log.push_str(&format!("Restore failed: {e}\n"));
                                let _ = tx.send((log, false));
                            }
                        }
                    }
                });
            })
        },
        update_seed_packs: {
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            let apply_btn = apply_btn.clone();
            let update_btn = update_btn.clone();
            let progress = progress.clone();
            let statusbar = statusbar.clone();
            Rc::new(move || {
                apply_btn.set_sensitive(false);
                update_btn.set_sensitive(false);
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some(tr("Updating seed packs…")));
                statusbar.set_text(tr("Updating seed packs…"));

                let (tx, rx) = mpsc::channel::<String>();

                let progress_done = progress.clone();
                let apply_done = apply_btn.clone();
                let update_done = update_btn.clone();
                let status_idle = statusbar.clone();
                let window_idle = window.clone();
                let refresh_idle = Rc::clone(&refresh);
                glib::idle_add_local(move || match rx.try_recv() {
                    Ok(log) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log);
                        refresh_idle();
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        status_idle.set_text(tr("Ready"));
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        progress_done.set_text(None);
                        status_idle.set_text(tr("Update seed packs interrupted."));
                        apply_done.set_sensitive(true);
                        update_done.set_sensitive(true);
                        glib::ControlFlow::Break
                    }
                });

                std::thread::spawn(move || {
                    let mut log = String::new();
                    match appimage_control_core::update_seed_packs(None, &mut |s| {
                        log.push_str(s);
                        log.push('\n');
                    }) {
                        Ok(report) => log.push_str(&format!(
                            "Installed {} seed packs to {} from {}\n",
                            report.installed_count,
                            report.install_dir.display(),
                            report.source_url
                        )),
                        Err(e) => log.push_str(&format!("Update seed packs failed: {e}\n")),
                    }
                    let _ = tx.send(log);
                });
            })
        },
        list_seed_packs: {
            let window = window_for_actions.clone();
            Rc::new(move || {
                let mut log = String::new();
                match appimage_control_core::SeedPackLibrary::discover_load(None) {
                    Ok(lib) => {
                        let rows = lib.list_rows();
                        if rows.is_empty() {
                            log.push_str("No seed packs loaded.\n");
                        } else {
                            log.push_str("Loaded seed packs:\n");
                            for (id, src) in rows {
                                log.push_str(&format!("  {id}\t{src}\n"));
                            }
                        }
                        let refused: Vec<_> = lib.refused().collect();
                        if !refused.is_empty() {
                            log.push_str("Refused seed packs (skipped):\n");
                            for (id, why) in refused {
                                log.push_str(&format!("  {id}\t{why}\n"));
                            }
                        }
                    }
                    Err(e) => log.push_str(&format!("Could not enumerate seed packs: {e}\n")),
                }
                present_activity_log_dialog(&window, &log);
            })
        },
        repair: {
            let window = window_for_actions.clone();
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            Rc::new(move || {
                let mut log = String::new();
                let _ = repair_instance_launchers(paths.as_ref(), catalog.as_ref(), &mut |m| {
                    log.push_str(m);
                    log.push('\n');
                });
                if log.trim().is_empty() {
                    log.push_str("All launchers and icons are already in good shape.\n");
                }
                present_activity_log_dialog(&window, &log);
            })
        },
        preferences: {
            let window = window_for_actions.clone();
            let paths = Rc::clone(paths);
            Rc::new(move || preferences::present_preferences(&window, Rc::clone(&paths)))
        },
        shortcuts: {
            let window = window_for_actions.clone();
            Rc::new(move || present_shortcuts(&window))
        },
        about: {
            let window = window_for_actions.clone();
            Rc::new(move || present_about(&window))
        },
    };
    actions::install_window_actions(&window_for_actions, hooks);
}
