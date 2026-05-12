//! GTK 4.10+ deprecates some dialog APIs; kept for gtk4-rs 0.9 on Ubuntu 24.04 / Whonix.
#![allow(deprecated)]

mod actions;
mod catalog_icons;
mod cli;
mod dialogs;
mod embedded_icons;
mod seed_dialog;
mod status_text;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use crate::catalog_icons::batch_catalog_icons;
use crate::dialogs::{
    ask_confirmation, choose_open_path, choose_save_path, present_activity_log_dialog,
    present_message_window, ConfirmKind,
};
use crate::seed_dialog::choose_seed_settings;
use adw::prelude::*;
use appimage_control_core::{
    catalog_row_latest_version_label, clean_catalog_download_cache,
    clean_installed_catalog_apps_keep_cache, create_backup_archive, discover_migration_candidates,
    fetch_github_release_hints, installed_version_fleet_display, instance_count,
    list_managed_instance_indices, portmux_port_probe_runtime, purge_appimage_control_state,
    reconcile_catalog_install_version_metadata, reconcile_instance_count_with_report,
    repair_instance_launchers, restore_backup_archive, run_portable_dir_migration,
    seed_instance_settings, update_all_installed_instances, AppPaths, BackupOptions, Catalog,
    CatalogApp, GithubReleaseHint, MigrationResolution, SeedPreset, SupportedSeedApp,
    UpdateAllReport, MAX_INSTANCES_PER_APP,
};
use cli::{has_cli_ops, print_help, run_cli_ops, want_help};
use gtk::glib;
use status_text::{catalog_row_hint, format_seed_status_summary, format_update_status_summary};

use gtk::{
    gdk::Texture, Box as GtkBox, Button, CssProvider, GestureClick, Image, Label, ListBox,
    Orientation, ProgressBar, ScrolledWindow, SpinButton,
};

const APP_ID: &str = "io.github.sureserverman.AppImageControl";
const THEME_CSS: &str = include_str!("theme.css");

struct RowHandles {
    app_index: usize,
    icon: Image,
    row: adw::ActionRow,
    spin: SpinButton,
    pending_pill: Label,
    update_pill: Label,
    /// Resolved URL exists for this process CPU architecture.
    can_install: bool,
    /// Mirror of `instance_count` at last refresh — used for purely-client-side
    /// pending detection on every spin change.
    last_applied: u32,
}

/// Renders the row's subtitle/meta string and updates the per-row "Update available" pill.
/// Called on row creation, after Apply/Update/Refresh, on remote-latest fetch completion,
/// and on every spin value change (so the count phrase tracks the spinner).
#[allow(clippy::too_many_arguments)]
fn apply_row_meta(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_count_n: u32,
    spin_value: u32,
    last_applied: u32,
    remote_latest: Option<&str>,
    row: &adw::ActionRow,
    update_pill: &Label,
    pending_pill: &Label,
    hint: Option<&str>,
) {
    let inst_display = installed_version_fleet_display(paths, app).ok();
    let latest_raw = catalog_row_latest_version_label(paths, app, remote_latest)
        .unwrap_or_else(|_| "—".to_string());
    let latest_esc = glib::markup_escape_text(&latest_raw);

    let count_for_phrase = spin_value;

    let subtitle_markup = if instance_count_n > 0 {
        let installed_text = inst_display
            .as_ref()
            .map(|d| d.label.as_str())
            .unwrap_or("installed unknown");
        let installed_esc = glib::markup_escape_text(installed_text);
        format!(
            "<b>{} instance{}</b> · Latest {} · {}",
            count_for_phrase,
            if count_for_phrase == 1 { "" } else { "s" },
            latest_esc,
            installed_esc
        )
    } else {
        format!(
            r#"<span alpha="55%">Not installed · Latest {}</span>"#,
            latest_esc
        )
    };
    row.set_subtitle(&subtitle_markup);

    // Tooltip stays rich (it explains where "Latest" comes from + per-instance breakdown).
    let mut tooltip = String::new();
    tooltip.push_str(
        "Latest: highest semver among the catalog URL pin, GitHub releases/latest (when loaded), and each instance’s .appimage-control-version.\n",
    );
    if remote_latest.is_some() {
        tooltip.push_str("GitHub tag was included in that comparison.\n");
    } else {
        tooltip.push_str("GitHub tag was not loaded yet or unavailable for this app.\n");
    }
    if instance_count_n > 0 {
        tooltip.push_str("Installed: derived from instance version metadata; hash appears when version metadata is missing.\n");
        if instance_count_n > 1 {
            if let Some(d) = inst_display.as_ref() {
                if !d.details.is_empty() {
                    tooltip.push_str("Per instance: ");
                    tooltip.push_str(&d.details.join(", "));
                    tooltip.push('.');
                }
            }
        }
    }
    if let Some(h) = hint {
        tooltip.push_str("\n\n");
        tooltip.push_str(h);
    }
    row.set_tooltip_text(Some(tooltip.trim()));

    // Pending pill: spin diverges from the last-applied count.
    let is_pending = spin_value != last_applied;
    pending_pill.set_visible(is_pending);

    // Update-available pill: a remote tag exists and beats every installed instance.
    let update_available = !is_pending
        && instance_count_n > 0
        && remote_latest
            .map(|tag| version_is_newer(tag, inst_display.as_ref().map(|d| d.label.as_str())))
            .unwrap_or(false);
    update_pill.set_visible(update_available);
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
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn build_ui(app: &adw::Application) {
    let paths = match AppPaths::from_env() {
        Ok(p) => Rc::new(p),
        Err(e) => {
            eprintln!("{}", e);
            present_message_window(
                app,
                "AppImage Control",
                "The app could not determine your home directory, so managed installs cannot be used.",
                Some(&e.to_string()),
            );
            return;
        }
    };

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("AppImage Control")
        .default_width(840)
        .default_height(620)
        .build();

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
    menu_btn.set_tooltip_text(Some("Main Menu"));
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
                "No catalog loaded — load one first.\nUse the empty-state Reload button or Open Catalog… to recover.\n",
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
    };
    actions::install_window_actions(&window_for_actions, hooks);
    let _ = (app_for_actions, error_msg);

    let status = adw::StatusPage::new();
    status.set_icon_name(Some("package-x-generic-symbolic"));
    status.set_title("No catalog loaded");
    status.set_description(Some(
        "AppImage Control couldn’t find an application catalog. Set APPIMAGE_CONTROL_CATALOG, or run from a directory that contains data/catalog.json.",
    ));

    let body = GtkBox::new(Orientation::Vertical, 18);
    body.set_halign(gtk::Align::Center);

    let detail_card = GtkBox::new(Orientation::Vertical, 4);
    detail_card.set_margin_top(0);
    detail_card.add_css_class("card");
    detail_card.set_margin_top(0);

    let detail_label = Label::builder()
        .label(
            "Search order:\n  1. APPIMAGE_CONTROL_CATALOG environment variable\n  2. ./data/catalog.json (cwd and parents)\n  3. /app/share/, /usr/share/appimage-control/catalog.json\n  4. catalog compiled into this build",
        )
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

    let open_btn = Button::with_label("Open Catalog…");
    let reload_btn = Button::with_label("Reload");
    reload_btn.add_css_class("suggested-action");

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
            let Some(path) = choose_open_path(&win_for_chooser, "Choose catalog.json") else {
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

    actions_box.append(&open_btn);
    actions_box.append(&reload_btn);

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

    let header = adw::HeaderBar::builder().show_title(true).build();
    header.set_title_widget(Some(&adw::WindowTitle::new("AppImage Control", "")));

    let apply = Button::builder()
        .label("Apply")
        .tooltip_text("Apply pending instance count changes")
        .valign(gtk::Align::Center)
        .build();
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);

    let update_installed = Button::builder()
        .icon_name("software-update-available-symbolic")
        .valign(gtk::Align::Center)
        .tooltip_text(
            "Update installed instances: skips when already at the newest known version (catalog and GitHub releases/latest). Otherwise runs appimageupdatetool, then downloads the catalog AppImage or—if GitHub’s latest tag is newer than the catalog URL—a matching .AppImage from that release.",
        )
        .build();
    let menu_btn = actions::build_actions_menu_button();
    menu_btn.set_tooltip_text(Some("Main Menu"));

    header.pack_start(&apply);
    header.pack_start(&update_installed);
    header.pack_end(&menu_btn);

    let apply_weak = apply.downgrade();
    let update_weak = update_installed.downgrade();

    // Body: AdwClamp wrapping intro + banner + list + progress.
    let body = GtkBox::new(Orientation::Vertical, 0);
    body.set_hexpand(true);
    body.set_vexpand(true);
    body.set_margin_top(24);
    body.set_margin_bottom(0);
    body.set_margin_start(12);
    body.set_margin_end(12);

    let intro = Label::new(Some(
        "Set the number of isolated instances to keep for each app, then apply changes.",
    ));
    intro.set_wrap(true);
    intro.set_hexpand(true);
    intro.set_halign(gtk::Align::Fill);
    intro.set_xalign(0.0);
    intro.add_css_class("caption");
    intro.add_css_class("dim-label");
    intro.set_margin_bottom(20);

    let banner = adw::Banner::new("");
    banner.set_revealed(false);

    let list = ListBox::new();
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list.set_vexpand(true);
    list.set_hexpand(true);

    let list_scroll = ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(220)
        .build();

    body.append(&intro);
    body.append(&banner);
    body.append(&list_scroll);

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(720);
    clamp.set_tightening_threshold(560);
    clamp.set_child(Some(&body));

    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    progress.set_hexpand(true);
    progress.set_margin_top(6);
    progress.set_margin_bottom(6);
    progress.set_margin_start(12);
    progress.set_margin_end(12);

    // Footer: 32px row with status + counts. Top divider for visual anchor.
    let footer_sep = gtk::Separator::new(Orientation::Horizontal);
    let footer = GtkBox::new(Orientation::Horizontal, 0);
    footer.set_margin_start(12);
    footer.set_margin_end(12);
    footer.set_margin_top(6);
    footer.set_margin_bottom(6);
    footer.set_height_request(20);

    let statusbar = Label::new(Some("Ready"));
    statusbar.add_css_class("dim-label");
    statusbar.add_css_class("caption");
    statusbar.set_halign(gtk::Align::Start);
    statusbar.set_xalign(0.0);
    statusbar.set_hexpand(true);

    let total_label = Label::new(None);
    total_label.add_css_class("dim-label");
    total_label.add_css_class("caption");
    total_label.set_halign(gtk::Align::End);
    total_label.set_xalign(1.0);
    let total_attrs = gtk::pango::AttrList::new();
    total_attrs.insert(gtk::pango::AttrFontFeatures::new("tnum=1"));
    total_label.set_attributes(Some(&total_attrs));

    footer.append(&statusbar);
    footer.append(&total_label);

    // Outer column: clamp content (top-stacked), progress, footer divider, footer.
    let outer = GtkBox::new(Orientation::Vertical, 0);
    outer.set_hexpand(true);
    outer.set_vexpand(true);
    outer.append(&clamp);
    outer.append(&progress);
    outer.append(&footer_sep);
    outer.append(&footer);

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&outer));
    window.set_content(Some(&toolbar_view));

    // Build rows.
    let _ =
        reconcile_catalog_install_version_metadata(paths.as_ref(), catalog.as_ref(), &mut |_| {});

    let rows: Rc<RefCell<Vec<RowHandles>>> = Rc::new(RefCell::new(Vec::new()));

    for (app_index, catalog_app) in catalog.apps.iter().enumerate() {
        let has_any_url = catalog_app.has_any_download_url();
        let can_install = catalog_app.resolved_download_url().is_ok();
        let installed = list_managed_instance_indices(paths.as_ref(), catalog_app)
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        let row = adw::ActionRow::new();
        row.set_title(&glib::markup_escape_text(
            &catalog_app.display_name_resolved(),
        ));
        row.set_use_markup(true);
        row.set_title_lines(1);
        row.set_subtitle_lines(1);
        row.set_activatable(false);

        let icon = Image::from_icon_name("image-x-generic-symbolic");
        icon.set_pixel_size(32);
        icon.set_valign(gtk::Align::Center);
        icon.set_halign(gtk::Align::Center);
        icon.add_css_class("catalog-app-icon");
        row.add_prefix(&icon);

        let pending_pill = Label::new(Some("pending"));
        pending_pill.add_css_class("row-pill");
        pending_pill.add_css_class("pending");
        pending_pill.set_valign(gtk::Align::Center);
        pending_pill.set_visible(false);

        let update_pill = Label::new(Some("Update available"));
        update_pill.add_css_class("row-pill");
        update_pill.add_css_class("update");
        update_pill.set_valign(gtk::Align::Center);
        update_pill.set_visible(false);

        let max_allowed = catalog_app.max_instances_allowed();
        let spin = SpinButton::with_range(0.0, max_allowed as f64, 1.0);
        spin.set_value(installed.min(max_allowed) as f64);
        let base_tip = "Number of instances to keep for this app.";
        let hover_tip = Rc::new(match catalog_app.instance_limit_tooltip_message() {
            Some(extra) => format!("{base_tip}\n{extra}"),
            None => base_tip.to_string(),
        });
        spin.set_tooltip_text(Some(hover_tip.as_str()));
        spin.set_sensitive(has_any_url && can_install);
        spin.set_width_chars(3);
        spin.set_valign(gtk::Align::Center);

        if max_allowed < MAX_INSTANCES_PER_APP
            && catalog_app.instance_limit_tooltip_message().is_some()
        {
            let value_at_press = Rc::new(RefCell::new(0u32));
            let gesture = GestureClick::new();
            let spin_press = spin.clone();
            let v_press = Rc::clone(&value_at_press);
            gesture.connect_pressed(move |_, _, _, _| {
                *v_press.borrow_mut() = spin_press.value() as u32;
            });
            let spin_rel = spin.clone();
            let v_rel = Rc::clone(&value_at_press);
            let hover_clone = Rc::clone(&hover_tip);
            gesture.connect_released(move |_, _n, x, _y| {
                let width = spin_rel.width();
                if width <= 1 {
                    return;
                }
                let rel = x / f64::from(width);
                let rtl = spin_rel.direction() == gtk::TextDirection::Rtl;
                let increment_zone = if rtl { rel < 0.45 } else { rel > 0.55 };
                if !increment_zone {
                    return;
                }
                if *v_rel.borrow() >= max_allowed {
                    spin_rel.set_tooltip_text(Some(hover_clone.as_str()));
                    spin_rel.trigger_tooltip_query();
                }
            });
            spin.add_controller(gesture);
        }

        let suffix = GtkBox::new(Orientation::Horizontal, 8);
        suffix.set_valign(gtk::Align::Center);
        suffix.append(&pending_pill);
        suffix.append(&update_pill);
        suffix.append(&spin);
        row.add_suffix(&suffix);

        list.append(&row);

        rows.borrow_mut().push(RowHandles {
            app_index,
            icon,
            row: row.clone(),
            spin: spin.clone(),
            pending_pill: pending_pill.clone(),
            update_pill: update_pill.clone(),
            can_install,
            last_applied: installed,
        });
    }

    // Initial meta render — needs catalog/paths/remote-cache borrow.
    refresh_all_rows(&paths, &catalog, &remote_latest_cache, &rows);
    update_total_label(&total_label, &paths, &catalog);

    // Pending-state recompute helper. Called from spin change, after Apply, after Refresh.
    let recompute_pending = {
        let rows = Rc::clone(&rows);
        let apply_weak = apply_weak.clone();
        let banner = banner.clone();
        let statusbar = statusbar.clone();
        Rc::new(move || {
            let pending_count = rows
                .borrow()
                .iter()
                .filter(|h| (h.spin.value() as u32) != h.last_applied)
                .count();
            if let Some(a) = apply_weak.upgrade() {
                let label = if pending_count == 0 {
                    "Apply".to_string()
                } else {
                    format!("Apply ({pending_count})")
                };
                a.set_label(&label);
                a.set_sensitive(pending_count > 0);
            }
            if pending_count == 0 {
                banner.set_revealed(false);
                statusbar.set_text("Ready");
            } else {
                banner.set_title(&format!(
                    "{pending_count} pending change{} — click Apply to reconcile.",
                    if pending_count == 1 { "" } else { "s" }
                ));
                banner.set_revealed(true);
                statusbar.set_text(&format!(
                    "{pending_count} pending change{}",
                    if pending_count == 1 { "" } else { "s" }
                ));
            }
        }) as Rc<dyn Fn()>
    };

    // Wire spin → meta + pending recompute (after rows exist so we can clone refs in).
    for handle in rows.borrow().iter() {
        let paths_spin = Rc::clone(&paths);
        let catalog_spin = Rc::clone(&catalog);
        let remote_cache_spin = Rc::clone(&remote_latest_cache);
        let recompute = Rc::clone(&recompute_pending);
        let app_index_spin = handle.app_index;
        let row_w = handle.row.clone();
        let pending_pill_w = handle.pending_pill.clone();
        let update_pill_w = handle.update_pill.clone();
        let last_applied = handle.last_applied;
        handle.spin.connect_value_changed(move |spin| {
            let app = &catalog_spin.apps[app_index_spin];
            let n = spin.value() as u32;
            let installed_n = instance_count(paths_spin.as_ref(), app).unwrap_or(0);
            let cache = remote_cache_spin.borrow();
            let remote = cache
                .get(app_index_spin)
                .and_then(|o| o.as_ref().map(|h| h.tag.as_str()));
            apply_row_meta(
                paths_spin.as_ref(),
                app,
                installed_n,
                n,
                last_applied,
                remote,
                &row_w,
                &update_pill_w,
                &pending_pill_w,
                catalog_row_hint(app).as_deref(),
            );
            (recompute)();
        });
    }
    (recompute_pending)();

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
    glib::idle_add_local(move || match rx_remote.try_recv() {
        Ok(remote_list) => {
            *remote_latest_cache_idle.borrow_mut() = remote_list.clone();
            for (i, remote) in remote_list.into_iter().enumerate() {
                let rows_b = rows_remote.borrow();
                let Some(h) = rows_b.get(i) else {
                    continue;
                };
                let app = &catalog_remote.apps[h.app_index];
                let n = h.spin.value() as u32;
                let installed_n = instance_count(paths_remote.as_ref(), app).unwrap_or(0);
                apply_row_meta(
                    paths_remote.as_ref(),
                    app,
                    installed_n,
                    n,
                    h.last_applied,
                    remote.as_ref().map(|hint| hint.tag.as_str()),
                    &h.row,
                    &h.update_pill,
                    &h.pending_pill,
                    catalog_row_hint(app).as_deref(),
                );
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });

    // Apply button: reconcile every row's spin to its target instance count.
    let window_for_apply: gtk::ApplicationWindow = window.clone().upcast();
    let window_for_update: gtk::ApplicationWindow = window.clone().upcast();
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
        move |_| {
            let mut specs = Vec::new();
            for h in rows.borrow().iter() {
                let target = h.spin.value() as u32;
                specs.push((h.app_index, target, h.can_install));
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
                h.spin.set_sensitive(false);
            }

            progress.set_visible(true);
            progress.set_fraction(0.0);
            progress.set_text(Some("Working…"));
            statusbar.set_text("Working…");

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            let (tx, rx) = mpsc::channel::<String>();

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_apply.clone();
            let status_idle = statusbar.clone();
            let total_idle = total_label.clone();
            let paths_total = Rc::clone(&paths);
            let catalog_total = Rc::clone(&catalog);
            glib::idle_add_local(move || {
                while let Ok((f, label)) = rx_prog.try_recv() {
                    progress_done.set_fraction(f);
                    progress_done.set_text(Some(&label));
                }
                match rx.try_recv() {
                    Ok(log_final) => {
                        progress_done.set_visible(false);
                        progress_done.set_fraction(0.0);
                        present_activity_log_dialog(&window_idle, &log_final);
                        let rem = remote_for_idle.borrow();
                        for h in rows_idle.borrow_mut().iter_mut() {
                            let app = &catalog_idle.apps[h.app_index];
                            let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                            let cap = app.max_instances_allowed();
                            h.spin.set_value(n.min(cap) as f64);
                            h.last_applied = n;
                            let can = h.can_install;
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.spin.set_sensitive(has_any && can);
                            let remote = rem
                                .get(h.app_index)
                                .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                            apply_row_meta(
                                paths_idle.as_ref(),
                                app,
                                n,
                                h.spin.value() as u32,
                                h.last_applied,
                                remote,
                                &h.row,
                                &h.update_pill,
                                &h.pending_pill,
                                catalog_row_hint(app).as_deref(),
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
                        let _ = status_idle;
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        status_idle.set_text("Apply interrupted.");
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        for h in rows_idle.borrow().iter() {
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.spin.set_sensitive(has_any && h.can_install);
                        }
                        glib::ControlFlow::Break
                    }
                }
            });

            std::thread::spawn(move || {
                let mut log = String::new();
                // Pre-flight cache: `portmux_port_probe_runtime` itself
                // caches across the whole process, but we also want to
                // log the install hint *once* even when several apps
                // each add new instances in the same Apply run.
                let mut probe_failure_logged: Option<String> = None;
                let n = specs.len();
                if n == 0 {
                    let _ = tx_prog.send((1.0, "Working…".to_string()));
                } else {
                    let _ = tx_prog.send((0.0, format!("Working… (0 of {})", n)));
                    for (step, (app_index, target, can_install)) in specs.into_iter().enumerate() {
                        let app = &catalog_t.apps[app_index];
                        if target > 0 && !can_install {
                            log.push_str(&format!(
                                "{}: skipped (no AppImage URL for {})\n",
                                app.id,
                                std::env::consts::ARCH
                            ));
                        } else {
                            let r = reconcile_instance_count_with_report(
                                &paths_t,
                                app,
                                target,
                                &mut |m| {
                                    log.push_str(m);
                                    log.push('\n');
                                },
                            );
                            match r {
                                Ok(rep) => {
                                    if !rep.added.is_empty() {
                                        // Probe lazily — only when we
                                        // actually have new instances to
                                        // seed. Avoids dragging the
                                        // missing-portmux message in
                                        // front of users who never
                                        // touched a seedable app.
                                        //
                                        // First failure in this Apply
                                        // run writes the full install
                                        // hint; subsequent apps in the
                                        // same run get a short line
                                        // pointing back at it.
                                        let probe_ok = match &probe_failure_logged {
                                            Some(_) => {
                                                log.push_str(&format!(
                                                    "{}: auto-seeding skipped — required system helper still unavailable\n",
                                                    app.id
                                                ));
                                                false
                                            }
                                            None => match portmux_port_probe_runtime() {
                                                Ok(_) => true,
                                                Err(msg) => {
                                                    log.push_str(&format!(
                                                        "{}: auto-seeding skipped — required system helper unavailable.\n{msg}\n",
                                                        app.id
                                                    ));
                                                    probe_failure_logged = Some(msg);
                                                    false
                                                }
                                            },
                                        };
                                        if probe_ok {
                                            let preset = SeedPreset::auto_default();
                                            log.push_str(&format!(
                                                "{}: auto-seeding {} new instance(s)\n",
                                                app.id,
                                                rep.added.len(),
                                            ));
                                            let _ = appimage_control_core::seed_new_instances_after_reconcile(
                                                &paths_t,
                                                app,
                                                &rep.added,
                                                preset,
                                                &mut |m| {
                                                    log.push_str(m);
                                                    log.push('\n');
                                                },
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    log.push_str(&format!("{}: {}\n", app.id, e));
                                }
                            }
                        }
                        let done = step + 1;
                        let frac = done as f64 / n as f64;
                        let _ = tx_prog
                            .send((frac, format!("Working… ({}/{}): {}", done, n, app.id)));
                    }
                }
                if log.is_empty() {
                    log.push_str("No changes applied.\n");
                }
                let _ = tx.send(log);
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
        move |_| {
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
                h.spin.set_sensitive(false);
            }

            progress.set_visible(true);
            progress.set_fraction(0.0);
            progress.set_text(Some("Updating…"));
            statusbar.set_text("Updating…");

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            let (tx, rx) = mpsc::channel::<(String, UpdateAllReport)>();

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_update.clone();
            let status_idle = statusbar.clone();
            let total_idle = total_label.clone();
            let paths_total = Rc::clone(&paths);
            let catalog_total = Rc::clone(&catalog);
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
                        for h in rows_idle.borrow_mut().iter_mut() {
                            let app = &catalog_idle.apps[h.app_index];
                            let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                            let cap = app.max_instances_allowed();
                            h.spin.set_value(n.min(cap) as f64);
                            h.last_applied = n;
                            let can = h.can_install;
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.spin.set_sensitive(has_any && can);
                            let remote = rem
                                .get(h.app_index)
                                .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                            apply_row_meta(
                                paths_idle.as_ref(),
                                app,
                                n,
                                h.spin.value() as u32,
                                h.last_applied,
                                remote,
                                &h.row,
                                &h.update_pill,
                                &h.pending_pill,
                                catalog_row_hint(app).as_deref(),
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
                        glib::ControlFlow::Break
                    }
                    Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        progress_done.set_visible(false);
                        status_idle.set_text("Update interrupted.");
                        if let Some(a) = apply_done.upgrade() {
                            a.set_sensitive(true);
                        }
                        if let Some(u) = update_done.upgrade() {
                            u.set_sensitive(true);
                        }
                        for h in rows_idle.borrow().iter() {
                            let has_any = catalog_idle.apps[h.app_index].has_any_download_url();
                            h.spin.set_sensitive(has_any && h.can_install);
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
                        "Updating…".to_string()
                    } else {
                        format!("Updating… ({done}/{total})")
                    };
                    let _ = tx_prog.send((f, label));
                };
                let report = match update_all_installed_instances(
                    &paths_t,
                    &catalog_t,
                    Some(&remotes),
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
    );
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
        let installed_n = instance_count(paths.as_ref(), app).unwrap_or(0);
        let remote = rem
            .get(h.app_index)
            .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
        apply_row_meta(
            paths.as_ref(),
            app,
            installed_n,
            h.spin.value() as u32,
            h.last_applied,
            remote,
            &h.row,
            &h.update_pill,
            &h.pending_pill,
            catalog_row_hint(app).as_deref(),
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
    label.set_text(&format!("{total} apps · {installed} installed"));
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
) {
    let window_for_actions: gtk::ApplicationWindow = window.clone().upcast();

    let refresh_fn: Rc<dyn Fn()> = {
        let paths = Rc::clone(paths);
        let catalog = Rc::clone(catalog);
        let rows = Rc::clone(rows);
        let remote = Rc::clone(remote_latest_cache);
        let total_label = total_label.clone();
        let statusbar = statusbar.clone();
        let recompute_pending = Rc::clone(&recompute_pending);
        Rc::new(move || {
            for h in rows.borrow_mut().iter_mut() {
                let app = &catalog.apps[h.app_index];
                let n = instance_count(paths.as_ref(), app).unwrap_or(0);
                h.spin.set_value(n.min(app.max_instances_allowed()) as f64);
                h.last_applied = n;
            }
            refresh_all_rows(&paths, &catalog, &remote, &rows);
            update_total_label(&total_label, &paths, &catalog);
            statusbar.set_text("Ready");
            (recompute_pending)();
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
                        "No supported installs",
                        "There are no installed Feather or Electrum instances to seed.",
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
                        h.spin.set_sensitive(false);
                    }
                }
                progress.set_visible(true);
                progress.set_fraction(0.0);
                progress.set_text(Some("Seeding settings…"));
                statusbar.set_text("Seeding settings…");

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
                                h.spin.set_sensitive(true);
                            }
                            glib::ControlFlow::Break
                        }
                        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            progress_done.set_visible(false);
                            status_idle.set_text("Seed settings interrupted.");
                            apply_idle.set_sensitive(true);
                            update_idle.set_sensitive(true);
                            for h in rows_idle.borrow().iter() {
                                h.spin.set_sensitive(true);
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
                        let _ = tx_prog.send((1.0, "Seeding settings…".to_string()));
                    }
                    // Pre-flight: surface a missing portmux-port up
                    // front in the activity-log dialog instead of
                    // letting every per-instance call fail with the
                    // same install hint. Sets `failed = max(total, 1)`
                    // so the status bar phrasing
                    // (`format_seed_status_summary`) renders the
                    // failure prominently rather than the misleading
                    // "Seeded 0 instance(s)" zero-state.
                    if let Err(msg) = portmux_port_probe_runtime() {
                        log.push_str(
                            "Seeding aborted: a required system helper is not available on the host.\n",
                        );
                        log.push_str(&msg);
                        log.push('\n');
                        failed = total.max(1);
                        let _ = tx.send((log, seeded, failed));
                        return;
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
                                    format!("Seeding settings… ({done}/{total})"),
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
            Rc::new(move || {
                if !ask_confirmation(
                    &window,
                    ConfirmKind::Warn,
                    "Clean installs?",
                    "Remove installed catalog instances and their managed launchers and icons. The download cache is kept, so reinstalling is fast.",
                    Some(
                        "<small>• <tt>~/.local/share/appimages/apps/*</tt>\n\
                         • <tt>~/.local/share/applications/appimage-control-*.desktop</tt></small>",
                    ),
                    "Clean",
                ) {
                    return;
                }
                let mut log = String::new();
                let r = clean_installed_catalog_apps_keep_cache(
                    paths.as_ref(),
                    catalog.as_ref(),
                    &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    },
                );
                if let Err(e) = r {
                    log.push_str(&format!("Clean failed: {e}\n"));
                }
                present_activity_log_dialog(&window, &log);
                refresh();
            })
        },
        purge: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                if !ask_confirmation(
                    &window,
                    ConfirmKind::Danger,
                    "Purge all manager state?",
                    "This removes every install, the download cache, managed launchers, and icon tiles owned by AppImage Control. App data inside instance home dirs is also removed.",
                    Some(
                        "<small>• <tt>~/.local/share/appimages/</tt> (apps, cache, files, icons)\n\
                         • <tt>~/.local/share/applications/appimage-control-*.desktop</tt>\n\
                         • <tt>~/.local/share/icons/hicolor/*/apps/appimage-control-*.png</tt></small>",
                    ),
                    "Purge state",
                ) {
                    return;
                }
                let mut log = String::new();
                let r = purge_appimage_control_state(
                    paths.as_ref(),
                    Some(catalog.as_ref()),
                    &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    },
                );
                if let Err(e) = r {
                    log.push_str(&format!("Purge failed: {e}\n"));
                }
                present_activity_log_dialog(&window, &log);
                refresh();
            })
        },
        migrate: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                if !ask_confirmation(
                    &window,
                    ConfirmKind::Info,
                    "Migrate legacy directories?",
                    "Scan the current directory for matching legacy folders (e.g. feather1, feather2) and move them into managed instance homes. Conflicting destinations are renumbered automatically.",
                    None,
                    "Migrate",
                ) {
                    return;
                }
                let mut log = String::new();
                let cwd = match std::env::current_dir() {
                    Ok(p) => p,
                    Err(e) => {
                        log.push_str(&format!("Could not read current dir: {e}\n"));
                        present_activity_log_dialog(&window, &log);
                        return;
                    }
                };
                let candidates = discover_migration_candidates(&cwd, catalog.as_ref());
                let r = run_portable_dir_migration(
                    paths.as_ref(),
                    catalog.as_ref(),
                    &candidates,
                    Some(MigrationResolution::Renumber),
                    false,
                    |_| MigrationResolution::Renumber,
                    &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    },
                );
                if let Err(e) = r {
                    log.push_str(&format!("Migration failed: {e}\n"));
                }
                present_activity_log_dialog(&window, &log);
                refresh();
            })
        },
        backup: {
            let paths = Rc::clone(paths);
            let catalog = Rc::clone(catalog);
            let window = window_for_actions.clone();
            Rc::new(move || {
                let Some(path) = choose_save_path(&window, "Choose backup archive destination")
                else {
                    return;
                };
                let mut log = String::new();
                match create_backup_archive(
                    paths.as_ref(),
                    catalog.as_ref(),
                    &path,
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
                present_activity_log_dialog(&window, &log);
            })
        },
        restore_backup: {
            let paths = Rc::clone(paths);
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
                let Some(path) = choose_open_path(&window, "Choose backup archive to restore")
                else {
                    return;
                };
                let mut log = String::new();
                match restore_backup_archive(paths.as_ref(), &path, false) {
                    Ok(()) => log.push_str("Restore finished.\n"),
                    Err(e) => {
                        let msg = e.to_string();
                        if msg.contains("managed state already exists")
                            && ask_confirmation(
                                &window,
                                ConfirmKind::Danger,
                                "Overwrite existing managed state?",
                                "Managed state already exists. Force restore and overwrite manager-owned state?",
                                None,
                                "Overwrite",
                            )
                        {
                            match restore_backup_archive(paths.as_ref(), &path, true) {
                                Ok(()) => log.push_str("Forced restore finished.\n"),
                                Err(e2) => {
                                    log.push_str(&format!("Forced restore failed: {e2}\n"))
                                }
                            }
                        } else {
                            log.push_str(&format!("Restore failed: {e}\n"));
                        }
                    }
                }
                present_activity_log_dialog(&window, &log);
                refresh();
            })
        },
        update_seed_packs: {
            let window = window_for_actions.clone();
            let refresh = Rc::clone(&refresh_fn);
            Rc::new(move || {
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
                present_activity_log_dialog(&window, &log);
                refresh();
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
                    }
                    Err(e) => log.push_str(&format!("Could not enumerate seed packs: {e}\n")),
                }
                present_activity_log_dialog(&window, &log);
            })
        },
    };
    actions::install_window_actions(&window_for_actions, hooks);
}
