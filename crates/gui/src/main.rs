//! GTK 4.10+ deprecates some dialog APIs; kept for gtk4-rs 0.9 on Ubuntu 24.04 / Whonix.
#![allow(deprecated)]

mod actions;
mod catalog_icons;
mod cli;
mod dialogs;
mod embedded_icons;
mod status_text;
mod ui;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use crate::catalog_icons::batch_catalog_icons;
use crate::dialogs::{
    ask_confirmation, choose_open_path, choose_save_path, choose_seed_settings,
    present_activity_log_dialog, present_message_window,
};
use appimage_control_core::{
    catalog_row_latest_version_label, clean_catalog_download_cache,
    clean_installed_catalog_apps_keep_cache, create_backup_archive, discover_migration_candidates,
    fetch_github_release_hints, installed_version_fleet_display, instance_count,
    list_managed_instance_indices, purge_appimage_control_state, reconcile_catalog_install_version_metadata,
    reconcile_instance_count_with_report, repair_instance_launchers,
    restore_backup_archive, run_portable_dir_migration, seed_instance_settings, SeedPreset,
    SupportedSeedApp, update_all_installed_instances, AppPaths, BackupOptions, Catalog, CatalogApp,
    GithubReleaseHint, MigrationResolution, UpdateAllReport, MAX_INSTANCES_PER_APP,
};
use adw::prelude::*;
use cli::{has_cli_ops, print_help, run_cli_ops, want_help};
use gtk::glib;
use status_text::{catalog_row_hint, format_seed_status_summary, format_update_status_summary};

use gtk::{
    gdk::Texture, Box as GtkBox, Button, GestureClick, Image, Label, ListBox, ListBoxRow,
    Orientation, ProgressBar, ScrolledWindow, SpinButton,
};

const APP_ID: &str = "io.github.appimagecontrol.AppImageControl";

struct RowHandles {
    app_index: usize,
    icon: Image,
    meta: Label,
    spin: SpinButton,
    /// Resolved URL exists for this process CPU architecture.
    can_install: bool,
}

/// One-line status + versions. `remote_latest` is the GitHub `releases/latest` tag when known.
fn apply_row_meta(
    paths: &AppPaths,
    app: &CatalogApp,
    instance_count_n: u32,
    remote_latest: Option<&str>,
    meta: &Label,
    hint: Option<&str>,
) {
    let inst_display = installed_version_fleet_display(paths, app).ok();

    let latest_raw = catalog_row_latest_version_label(paths, app, remote_latest)
        .unwrap_or_else(|_| "—".to_string());
    let latest_esc = glib::markup_escape_text(&latest_raw);
    let st = ui::status_text(instance_count_n);
    let st_esc = glib::markup_escape_text(&st);

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
        let installed_text = inst_display
            .as_ref()
            .map(|d| d.label.as_str())
            .unwrap_or("installed unknown");
        let installed_esc = glib::markup_escape_text(installed_text);
        meta.set_markup(&format!(
            "{} · {} {} · Latest: {}",
            st_esc,
            glib::markup_escape_text(&app.display_name_resolved()),
            installed_esc,
            latest_esc
        ));
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
    } else {
        meta.set_markup(&format!(
            r#"<span alpha="55%">{} · Latest: {}</span>"#,
            st_esc, latest_esc
        ));
    }
    if let Some(h) = hint {
        tooltip.push_str("\n\n");
        tooltip.push_str(h);
    }
    meta.set_tooltip_text(Some(tooltip.trim()));
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
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_ui);
    app.run()
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

    let catalog = match Catalog::discover_load(None) {
        Ok(c) => Rc::new(c),
        Err(e) => {
            present_message_window(
                app,
                "No catalog",
                "The application catalog could not be loaded.",
                Some(&format!(
                    "{e}\n\nSearch order: APPIMAGE_CONTROL_CATALOG (non-empty), then non-empty data/catalog.json on cwd or parents and on the binary path, then /app/share/ and /usr/share/appimage-control/catalog.json, then the catalog compiled into this program.\n\nTip: a stray empty data/catalog.json (apps array empty) on an ancestor path is skipped. Unset a bad APPIMAGE_CONTROL_CATALOG or reinstall this app."
                )),
            );
            return;
        }
    };
    // GitHub `releases/latest` results (same order as `catalog.apps`), filled after background fetch.
    let remote_latest_cache: Rc<RefCell<Vec<Option<GithubReleaseHint>>>> =
        Rc::new(RefCell::new(Vec::new()));
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("AppImage Control")
        .default_width(840)
        .default_height(620)
        .build();

    // Best-effort: repair missing per-instance launchers/icons so dock icons can resolve.
    // Keep this quiet unless we actually repaired something (we show it in Activity log then).
    {
        let paths_t = (*paths).clone();
        let catalog_t = (*catalog).clone();
        let window_idle: gtk::ApplicationWindow = window.clone().upcast();
        let (tx, rx) = mpsc::channel::<String>();
        glib::idle_add_local(move || match rx.try_recv() {
            Ok(log) => {
                if !log.trim().is_empty() {
                    // Only show if something actually happened.
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
            // Only report when we likely repaired something.
            if log.contains("launcher repaired") || log.contains("reconciled install metadata") {
                let _ = tx.send(log);
            } else {
                let _ = tx.send(String::new());
            }
        });
    }

    let header = adw::HeaderBar::builder().show_title(true).build();
    let update_installed = Button::builder()
        .icon_name("software-update-available-symbolic")
        .tooltip_text("Update installed instances")
        .valign(gtk::Align::Center)
        .tooltip_text(
            "Update instances: skips when the install is already at the newest known version (catalog and GitHub releases/latest). Otherwise runs appimageupdatetool, then downloads the catalog AppImage or—if GitHub’s latest tag is newer than the catalog URL—a matching .AppImage from that release.",
        )
        .build();
    let apply = Button::builder()
        .icon_name("document-save-symbolic")
        .tooltip_text("Apply instance count changes")
        .valign(gtk::Align::Center)
        .build();
    apply.add_css_class("suggested-action");
    let menu_btn = actions::build_actions_menu_button();
    menu_btn.set_tooltip_text(Some("Main Menu"));
    let header_left = GtkBox::new(Orientation::Horizontal, 6);
    header_left.set_margin_start(10);
    header_left.set_margin_top(10);
    header_left.append(&apply);
    header_left.append(&update_installed);
    header.pack_start(&header_left);
    header.pack_end(&menu_btn);
    let apply_weak = apply.downgrade();
    let update_weak = update_installed.downgrade();

    let outer = GtkBox::new(Orientation::Vertical, 12);
    outer.set_hexpand(true);
    outer.set_vexpand(true);

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_hexpand(true);
    content.set_vexpand(true);
    content.set_margin_top(24);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);

    let intro = Label::new(Some(
        "Set the number of isolated instances to keep for each app, then apply changes.",
    ));
    intro.set_wrap(true);
    intro.set_hexpand(true);
    intro.set_halign(gtk::Align::Fill);
    intro.set_xalign(0.0);
    intro.add_css_class("caption");

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

    let rows: Rc<RefCell<Vec<RowHandles>>> = Rc::new(RefCell::new(Vec::new()));

    let _ =
        reconcile_catalog_install_version_metadata(paths.as_ref(), catalog.as_ref(), &mut |_| {});

    for (app_index, app) in catalog.apps.iter().enumerate() {
        let row = ListBoxRow::new();
        row.set_selectable(false);

        let row_box = GtkBox::new(Orientation::Horizontal, 8);
        row_box.set_margin_top(0);
        row_box.set_margin_bottom(0);
        row_box.set_margin_start(10);
        row_box.set_margin_end(2);
        row_box.set_hexpand(true);

        let has_any_url = app.has_any_download_url();
        let can_install = app.resolved_download_url().is_ok();

        let installed = list_managed_instance_indices(paths.as_ref(), app)
            .map(|v| v.len() as u32)
            .unwrap_or(0);

        let icon = Image::from_icon_name("image-x-generic-symbolic");
        icon.set_pixel_size(32);
        icon.set_valign(gtk::Align::Center);
        icon.set_halign(gtk::Align::Center);
        icon.add_css_class("catalog-app-icon");

        let title = Label::builder()
            .label(app.display_name_resolved())
            .halign(gtk::Align::Start)
            .hexpand(false)
            .xalign(0.0)
            .build();
        title.add_css_class("catalog-title");
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_max_width_chars(24);

        let meta = Label::builder()
            .label("")
            .use_markup(true)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .build();
        meta.add_css_class("catalog-meta");
        meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let info_box = GtkBox::new(Orientation::Vertical, 0);
        info_box.add_css_class("catalog-info");
        info_box.set_hexpand(true);
        info_box.append(&title);
        info_box.append(&meta);

        let hint = catalog_row_hint(app);

        let max_allowed = app.max_instances_allowed();
        let spin = SpinButton::with_range(0.0, max_allowed as f64, 1.0);
        spin.set_value(installed.min(max_allowed) as f64);
        let base_tip = "Number of instances to keep for this app.";
        let hover_tip = Rc::new(match app.instance_limit_tooltip_message() {
            Some(extra) => format!("{base_tip}\n{extra}"),
            None => base_tip.to_string(),
        });
        spin.set_tooltip_text(Some(hover_tip.as_str()));
        spin.set_sensitive(has_any_url && can_install);
        spin.set_width_chars(3);
        spin.set_halign(gtk::Align::End);
        spin.set_valign(gtk::Align::Center);

        if max_allowed < MAX_INSTANCES_PER_APP && app.instance_limit_tooltip_message().is_some() {
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
                // Value at press avoids a false positive when going from max-1 → max.
                if *v_rel.borrow() >= max_allowed {
                    spin_rel.set_tooltip_text(Some(hover_clone.as_str()));
                    spin_rel.trigger_tooltip_query();
                }
            });
            spin.add_controller(gesture);
        }

        let meta_for_spin = meta.clone();
        let paths_spin = Rc::clone(&paths);
        let catalog_spin = Rc::clone(&catalog);
        let remote_cache_spin = Rc::clone(&remote_latest_cache);
        let app_index_spin = app_index;
        spin.connect_value_changed(move |spin| {
            let app = &catalog_spin.apps[app_index_spin];
            let n = spin.value() as u32;
            let cache = remote_cache_spin.borrow();
            let remote = cache
                .get(app_index_spin)
                .and_then(|o| o.as_ref().map(|h| h.tag.as_str()));
            apply_row_meta(
                paths_spin.as_ref(),
                app,
                n,
                remote,
                &meta_for_spin,
                catalog_row_hint(app).as_deref(),
            );
        });

        apply_row_meta(paths.as_ref(), app, installed, None, &meta, hint.as_deref());

        row_box.append(&icon);
        row_box.append(&info_box);
        row_box.append(&spin);

        row.set_child(Some(&row_box));
        if let Some(ref h) = hint {
            row.set_tooltip_text(Some(h.as_str()));
        }
        list.append(&row);

        rows.borrow_mut().push(RowHandles {
            app_index,
            icon,
            meta,
            spin,
            can_install,
        });
    }

    let (tx_icons, rx_icons) = mpsc::channel::<Vec<Option<Vec<u8>>>>();
    let catalog_for_icons = (*catalog).clone();
    std::thread::spawn(move || {
        let v = batch_catalog_icons(&catalog_for_icons);
        let _ = tx_icons.send(v);
    });

    let rows_icons = Rc::clone(&rows);
    glib::idle_add_local(move || match rx_icons.try_recv() {
        Ok(icon_list) => {
            for (i, bytes_opt) in icon_list.into_iter().enumerate() {
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
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });

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
                apply_row_meta(
                    paths_remote.as_ref(),
                    app,
                    n,
                    remote.as_ref().map(|hint| hint.tag.as_str()),
                    &h.meta,
                    catalog_row_hint(app).as_deref(),
                );
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });

    let progress = ProgressBar::new();
    progress.set_show_text(true);
    progress.set_visible(false);
    progress.set_hexpand(true);

    let statusbar = Label::new(Some("Ready"));
    statusbar.add_css_class("dim-label");
    statusbar.set_halign(gtk::Align::Start);
    statusbar.set_xalign(0.0);
    statusbar.set_margin_start(12);
    statusbar.set_margin_end(12);
    statusbar.set_margin_bottom(12);

    progress.set_margin_top(10);
    progress.set_margin_bottom(10);

    let list_and_progress = GtkBox::new(Orientation::Vertical, 0);
    list_and_progress.set_hexpand(true);
    list_and_progress.set_vexpand(true);
    list_and_progress.append(&list_scroll);
    list_and_progress.append(&progress);

    content.append(&intro);
    content.append(&list_and_progress);

    outer.append(&content);
    outer.append(&statusbar);

    let window_for_apply: gtk::ApplicationWindow = window.clone().upcast();
    let window_for_update: gtk::ApplicationWindow = window.clone().upcast();
    apply.connect_clicked({
        let paths = Rc::clone(&paths);
        let catalog = Rc::clone(&catalog);
        let rows = Rc::clone(&rows);
        let remote_latest_cache = Rc::clone(&remote_latest_cache);
        let progress = progress.clone();
        let statusbar = statusbar.clone();
        let apply_weak = apply_weak.clone();
        let update_weak = update_weak.clone();
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
            statusbar.set_visible(false);

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            let (tx, rx) = mpsc::channel::<String>();

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_apply.clone();
            let status_idle = statusbar.clone();
            glib::idle_add_local(move || {
                while let Ok((f, label)) = rx_prog.try_recv() {
                    progress_done.set_fraction(f);
                    progress_done.set_text(Some(&label));
                }
                match rx.try_recv() {
                Ok(log_final) => {
                    progress_done.set_visible(false);
                    progress_done.set_fraction(0.0);
                    status_idle.set_visible(true);
                    status_idle.set_text("Ready");
                    present_activity_log_dialog(&window_idle, &log_final);
                    let rem = remote_for_idle.borrow();
                    for h in rows_idle.borrow().iter() {
                        let app = &catalog_idle.apps[h.app_index];
                        let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                        let cap = app.max_instances_allowed();
                        h.spin.set_value(n.min(cap) as f64);
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
                            remote,
                            &h.meta,
                            catalog_row_hint(app).as_deref(),
                        );
                    }
                    if let Some(a) = apply_done.upgrade() {
                        a.set_sensitive(true);
                    }
                    if let Some(u) = update_done.upgrade() {
                        u.set_sensitive(true);
                    }
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    progress_done.set_visible(false);
                    status_idle.set_visible(true);
                    status_idle.set_text("Apply interrupted.");
                    if let Some(a) = apply_done.upgrade() {
                        a.set_sensitive(true);
                    }
                    if let Some(u) = update_done.upgrade() {
                        u.set_sensitive(true);
                    }
                    let rem = remote_for_idle.borrow();
                    for h in rows_idle.borrow().iter() {
                        let app = &catalog_idle.apps[h.app_index];
                        let has_any = app.has_any_download_url();
                        let can = h.can_install;
                        h.spin.set_sensitive(has_any && can);
                        let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                        let remote = rem
                            .get(h.app_index)
                            .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                        apply_row_meta(
                            paths_idle.as_ref(),
                            app,
                            n,
                            remote,
                            &h.meta,
                            catalog_row_hint(app).as_deref(),
                        );
                    }
                    glib::ControlFlow::Break
                }
            }
            });

            std::thread::spawn(move || {
                let mut log = String::new();
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
                            let r = reconcile_instance_count_with_report(&paths_t, app, target, &mut |m| {
                                log.push_str(m);
                                log.push('\n');
                            });
                            match r {
                                Ok(rep) => {
                                    if !rep.added.is_empty() {
                                        let preset = SeedPreset::auto_default();
                                        log.push_str(&format!(
                                            "{}: auto-seeding {} new instance(s) with {} preset\n",
                                            app.id,
                                            rep.added.len(),
                                            preset.as_str()
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
                                Err(e) => {
                                    log.push_str(&format!("{}: {}\n", app.id, e));
                                }
                            }
                        }
                        let done = step + 1;
                        let frac = done as f64 / n as f64;
                        let _ = tx_prog.send((
                            frac,
                            format!("Working… ({}/{}): {}", done, n, app.id),
                        ));
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
        let apply_weak = apply_weak.clone();
        let update_weak = update_weak.clone();
        move |_| {
            let paths_t = (*paths).clone();
            let catalog_t = (*catalog).clone();
            let rows_idle = Rc::clone(&rows);
            let catalog_idle = Rc::clone(&catalog);
            let paths_idle = Rc::clone(&paths);
            let progress_done = progress.clone();
            let apply_done = apply_weak.clone();
            let update_done = update_weak.clone();

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
            statusbar.set_visible(false);

            let (tx_prog, rx_prog) = mpsc::channel::<(f64, String)>();
            let (tx, rx) = mpsc::channel::<(String, UpdateAllReport)>();

            let remote_for_idle = Rc::clone(&remote_latest_cache);
            let window_idle = window_for_update.clone();
            let status_idle = statusbar.clone();
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
                    status_idle.set_visible(true);
                    status_idle.set_text(summary.as_str());
                    present_activity_log_dialog(&window_idle, &log_final);
                    let rem = remote_for_idle.borrow();
                    for h in rows_idle.borrow().iter() {
                        let app = &catalog_idle.apps[h.app_index];
                        let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                        let cap = app.max_instances_allowed();
                        h.spin.set_value(n.min(cap) as f64);
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
                            remote,
                            &h.meta,
                            catalog_row_hint(app).as_deref(),
                        );
                    }
                    if let Some(a) = apply_done.upgrade() {
                        a.set_sensitive(true);
                    }
                    if let Some(u) = update_done.upgrade() {
                        u.set_sensitive(true);
                    }
                    glib::ControlFlow::Break
                }
                Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    progress_done.set_visible(false);
                    status_idle.set_visible(true);
                    status_idle.set_text("Update interrupted.");
                    if let Some(a) = apply_done.upgrade() {
                        a.set_sensitive(true);
                    }
                    if let Some(u) = update_done.upgrade() {
                        u.set_sensitive(true);
                    }
                    let rem = remote_for_idle.borrow();
                    for h in rows_idle.borrow().iter() {
                        let app = &catalog_idle.apps[h.app_index];
                        let has_any = app.has_any_download_url();
                        let can = h.can_install;
                        h.spin.set_sensitive(has_any && can);
                        let n = instance_count(paths_idle.as_ref(), app).unwrap_or(0);
                        let remote = rem
                            .get(h.app_index)
                            .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                        apply_row_meta(
                            paths_idle.as_ref(),
                            app,
                            n,
                            remote,
                            &h.meta,
                            catalog_row_hint(app).as_deref(),
                        );
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

    {
        let window_for_actions: gtk::ApplicationWindow = window.clone().upcast();
        let apply_btn = apply.clone();
        let update_btn = update_installed.clone();
        let progress_a = progress.clone();
        let statusbar_a = statusbar.clone();
        let rows_for_actions = Rc::clone(&rows);
        let paths_seed = Rc::clone(&paths);
        let catalog_seed = Rc::clone(&catalog);
        let paths_a = Rc::clone(&paths);
        let catalog_a = Rc::clone(&catalog);
        let rows_a = Rc::clone(&rows);
        let remote_a = Rc::clone(&remote_latest_cache);
        let status_a = statusbar.clone();

        let refresh_fn: Rc<dyn Fn()> = Rc::new(move || {
            let rem = remote_a.borrow();
            for h in rows_a.borrow().iter() {
                let app = &catalog_a.apps[h.app_index];
                let n = instance_count(paths_a.as_ref(), app).unwrap_or(0);
                h.spin.set_value(n.min(app.max_instances_allowed()) as f64);
                let remote = rem
                    .get(h.app_index)
                    .and_then(|o| o.as_ref().map(|hint| hint.tag.as_str()));
                apply_row_meta(
                    paths_a.as_ref(),
                    app,
                    n,
                    remote,
                    &h.meta,
                    catalog_row_hint(app).as_deref(),
                );
            }
            status_a.set_text("Ready");
        });

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
                let paths = Rc::clone(&paths_seed);
                let catalog = Rc::clone(&catalog_seed);
                let progress = progress_a.clone();
                let statusbar = statusbar_a.clone();
                let rows = Rc::clone(&rows_for_actions);
                let apply_btn = apply_btn.clone();
                let update_btn = update_btn.clone();
                let refresh = Rc::clone(&refresh_fn);
                Rc::new(move || {
                    let installed_supported: Vec<(String, String, u32)> = catalog
                        .apps
                        .iter()
                        .filter(|app| SupportedSeedApp::from_catalog_app(app).is_some())
                        .filter_map(|app| {
                            let n = instance_count(paths.as_ref(), app).ok()?;
                            (n > 0).then(|| (app.id.clone(), app.display_name_resolved(), n))
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
                    let Some((preset, selected_ids)) = choose_seed_settings(&window, &installed_supported) else {
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
                    statusbar.set_visible(false);

                    let total: u32 = installed_supported
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
                                status_idle.set_visible(true);
                                status_idle.set_text(format_seed_status_summary(seeded, failed).as_str());
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
                                status_idle.set_visible(true);
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
                        for app in &catalog_t.apps {
                            if !selected_ids.iter().any(|id| id == &app.id) {
                                continue;
                            }
                            if SupportedSeedApp::from_catalog_app(app).is_none() {
                                continue;
                            }
                            let indices = list_managed_instance_indices(&paths_t, app).unwrap_or_default();
                            for i in indices {
                                match seed_instance_settings(&paths_t, app, i, preset, &mut |m| {
                                    log.push_str(m);
                                    log.push('\n');
                                }) {
                                    Ok(appimage_control_core::SeedOutcome::Seeded) => seeded += 1,
                                    Ok(_) => {}
                                    Err(e) => {
                                        failed += 1;
                                        log.push_str(&format!("{} instance {}: seeding failed: {e}\n", app.id, i));
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
                let paths = Rc::clone(&paths);
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
                let paths = Rc::clone(&paths);
                let catalog = Rc::clone(&catalog);
                let window = window_for_actions.clone();
                let refresh = Rc::clone(&refresh_fn);
                Rc::new(move || {
                    if !ask_confirmation(
                        &window,
                        "Clean installs",
                        "Remove installed catalog instances and managed launchers/icons, but keep cache?",
                    ) {
                        return;
                    }
                    let mut log = String::new();
                    let r = clean_installed_catalog_apps_keep_cache(paths.as_ref(), catalog.as_ref(), &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    });
                    if let Err(e) = r {
                        log.push_str(&format!("Clean failed: {e}\n"));
                    }
                    present_activity_log_dialog(&window, &log);
                    refresh();
                })
            },
            purge: {
                let paths = Rc::clone(&paths);
                let catalog = Rc::clone(&catalog);
                let window = window_for_actions.clone();
                let refresh = Rc::clone(&refresh_fn);
                Rc::new(move || {
                    if !ask_confirmation(
                        &window,
                        "Purge state",
                        "Remove all manager-owned installs, cache, managed files, launchers and icons?",
                    ) {
                        return;
                    }
                    let mut log = String::new();
                    let r = purge_appimage_control_state(paths.as_ref(), Some(catalog.as_ref()), &mut |m| {
                        log.push_str(m);
                        log.push('\n');
                    });
                    if let Err(e) = r {
                        log.push_str(&format!("Purge failed: {e}\n"));
                    }
                    present_activity_log_dialog(&window, &log);
                    refresh();
                })
            },
            migrate: {
                let paths = Rc::clone(&paths);
                let catalog = Rc::clone(&catalog);
                let window = window_for_actions.clone();
                let refresh = Rc::clone(&refresh_fn);
                Rc::new(move || {
                    if !ask_confirmation(
                        &window,
                        "Migrate legacy dirs",
                        "Scan current directory and migrate matching legacy folders into managed instance homes?",
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
                let paths = Rc::clone(&paths);
                let catalog = Rc::clone(&catalog);
                let window = window_for_actions.clone();
                Rc::new(move || {
                    let Some(path) = choose_save_path(&window, "Choose backup archive destination") else {
                        return;
                    };
                    let mut log = String::new();
                    match create_backup_archive(paths.as_ref(), catalog.as_ref(), &path, &BackupOptions::default()) {
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
                let paths = Rc::clone(&paths);
                let window = window_for_actions.clone();
                let refresh = Rc::clone(&refresh_fn);
                Rc::new(move || {
                    let Some(path) = choose_open_path(&window, "Choose backup archive to restore") else {
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
                                    "Overwrite existing managed state?",
                                    "Managed state already exists. Force restore and overwrite manager-owned state?",
                                )
                            {
                                match restore_backup_archive(paths.as_ref(), &path, true) {
                                    Ok(()) => log.push_str("Forced restore finished.\n"),
                                    Err(e2) => log.push_str(&format!("Forced restore failed: {e2}\n")),
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
        };
        actions::install_window_actions(&window_for_actions, hooks);
    }

    let clamp = adw::Clamp::new();
    clamp.set_maximum_size(960);
    clamp.set_tightening_threshold(640);
    clamp.set_child(Some(&outer));
    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&header);
    toolbar_view.set_content(Some(&clamp));
    window.set_content(Some(&toolbar_view));
    window.present();
}

