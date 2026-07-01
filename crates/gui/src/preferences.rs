//! Preferences window (General / Network / Storage / Backup).
//!
//! Settings persist to `~/.config/appimage-control/config.json` via [`GuiConfig`];
//! every control writes through on change. Theme changes apply live; `Compact rows`
//! takes effect on the next catalog mount (reload/restart). The Storage/Backup pages
//! expose the maintenance actions that the redesigned primary menu doesn't list, so
//! no behavior is lost — they activate the existing `win.*` window actions.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;

use appimage_control_core::{portmux_port_probe_runtime, AppPaths, GuiConfig, ThemePref};

/// Apply a theme preference to the global libadwaita style manager.
pub fn apply_theme(theme: ThemePref) {
    let scheme = match theme {
        ThemePref::System => adw::ColorScheme::Default,
        ThemePref::Light => adw::ColorScheme::ForceLight,
        ThemePref::Dark => adw::ColorScheme::ForceDark,
    };
    adw::StyleManager::default().set_color_scheme(scheme);
}

fn detected_catalog_path() -> String {
    std::env::var("APPIMAGE_CONTROL_CATALOG")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "data/catalog.json (auto-detected) · built-in fallback".to_string())
}

/// Build and present the Preferences window, transient for `parent`.
pub fn present_preferences(parent: &gtk::ApplicationWindow, paths: Rc<AppPaths>) {
    let cfg = Rc::new(RefCell::new(GuiConfig::load(&paths)));

    // Persist the current config; bad writes are surfaced to stderr only (the
    // settings stay applied in-memory regardless).
    let save = {
        let cfg = Rc::clone(&cfg);
        let paths = Rc::clone(&paths);
        Rc::new(move || {
            if let Err(e) = cfg.borrow().save(&paths) {
                eprintln!("appimage-control: could not save preferences: {e}");
            }
        }) as Rc<dyn Fn()>
    };

    // Custom window with a left icon sidebar + content stack, matching the
    // handoff design (rather than the stock AdwPreferencesWindow top-switcher).
    let win = adw::Window::builder()
        .title("Preferences")
        .modal(true)
        .transient_for(parent)
        .default_width(780)
        .default_height(600)
        .build();

    let stack = gtk::Stack::new();
    stack.set_hexpand(true);
    stack.set_vexpand(true);
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);

    // (name, label, icon-name, page-widget)
    let pages: [(&str, &str, &str, gtk::Widget); 4] = [
        ("general", "General", "preferences-system-symbolic", general_page(parent, &cfg, &save).upcast()),
        ("storage", "Storage", "drive-harddisk-symbolic", storage_page(parent).upcast()),
        ("network", "Network", "network-transmit-receive-symbolic", network_page(&cfg, &save).upcast()),
        ("backup", "Backup", "document-save-symbolic", backup_page(parent).upcast()),
    ];
    for (name, _, _, w) in &pages {
        stack.add_named(w, Some(name));
    }

    // Sidebar: one selectable nav row per page (accent-highlighted when active).
    let sidebar = gtk::ListBox::new();
    sidebar.add_css_class("navigation-sidebar");
    sidebar.set_size_request(184, -1);
    for (_, label, icon, _) in &pages {
        let row = gtk::ListBoxRow::new();
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 11);
        b.set_margin_top(4);
        b.set_margin_bottom(4);
        b.append(&gtk::Image::from_icon_name(icon));
        b.append(&gtk::Label::new(Some(label)));
        row.set_child(Some(&b));
        sidebar.append(&row);
    }
    // Header title reflects the active page (design shows "General" etc. centered).
    let win_title = adw::WindowTitle::new("General", "Preferences");
    {
        let stack = stack.clone();
        let win_title = win_title.clone();
        sidebar.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some((name, label)) = PAGE_ORDER.get(row.index() as usize) {
                    stack.set_visible_child_name(name);
                    win_title.set_title(label);
                }
            }
        });
    }
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    let split = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let sidebar_scroll = gtk::ScrolledWindow::builder()
        .child(&sidebar)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .build();
    split.append(&sidebar_scroll);
    split.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    split.append(&stack);

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&win_title));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&split));
    win.set_content(Some(&toolbar));
    win.present();
}

/// Page order for mapping a selected sidebar row index to a stack page name.
const PAGE_ORDER: [(&str, &str); 4] = [
    ("general", "General"),
    ("storage", "Storage"),
    ("network", "Network"),
    ("backup", "Backup"),
];

fn general_page(
    parent: &gtk::ApplicationWindow,
    cfg: &Rc<RefCell<GuiConfig>>,
    save: &Rc<dyn Fn()>,
) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    // --- Appearance ---
    let appearance = adw::PreferencesGroup::builder().title("Appearance").build();

    // Theme: segmented System/Light/Dark (matches the design, not a dropdown).
    let theme_row = adw::ActionRow::builder()
        .title("Theme")
        .subtitle("Follow the system, or force a mode")
        .build();
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    seg.add_css_class("ac-segment");
    seg.set_valign(gtk::Align::Center);
    let t_sys = gtk::ToggleButton::with_label("System");
    let t_light = gtk::ToggleButton::with_label("Light");
    let t_dark = gtk::ToggleButton::with_label("Dark");
    for b in [&t_sys, &t_light, &t_dark] {
        b.add_css_class("flat");
    }
    t_light.set_group(Some(&t_sys));
    t_dark.set_group(Some(&t_sys));
    match cfg.borrow().theme {
        ThemePref::System => &t_sys,
        ThemePref::Light => &t_light,
        ThemePref::Dark => &t_dark,
    }
    .set_active(true);
    seg.append(&t_sys);
    seg.append(&t_light);
    seg.append(&t_dark);
    for (btn, pref) in [
        (&t_sys, ThemePref::System),
        (&t_light, ThemePref::Light),
        (&t_dark, ThemePref::Dark),
    ] {
        let cfg = Rc::clone(cfg);
        let save = Rc::clone(save);
        btn.connect_toggled(move |b| {
            if b.is_active() {
                cfg.borrow_mut().theme = pref;
                apply_theme(pref);
                (save)();
            }
        });
    }
    theme_row.add_suffix(&seg);
    appearance.add(&theme_row);

    let compact = adw::SwitchRow::builder()
        .title("Compact rows")
        .subtitle("Tighter launcher grid (applies on reload)")
        .active(cfg.borrow().compact_rows)
        .build();
    bind_switch(&compact, cfg, save, |c, v| c.compact_rows = v);
    appearance.add(&compact);
    page.add(&appearance);

    // --- Catalog ---
    let catalog = adw::PreferencesGroup::builder().title("Catalog").build();
    let catalog_row = adw::ActionRow::builder()
        .title("Catalog file")
        .subtitle(detected_catalog_path())
        .build();
    catalog_row.add_css_class("property");
    let open_btn = gtk::Button::with_label("Open…");
    open_btn.set_valign(gtk::Align::Center);
    {
        let parent = parent.clone();
        open_btn.connect_clicked(move |_| {
            let _ = WidgetExt::activate_action(&parent, "win.open_catalog", None);
        });
    }
    catalog_row.add_suffix(&open_btn);
    catalog.add(&catalog_row);

    let reload_focus = adw::SwitchRow::builder()
        .title("Reload on focus")
        .subtitle("Re-scan the catalog when the window regains focus")
        .active(cfg.borrow().reload_on_focus)
        .build();
    bind_switch(&reload_focus, cfg, save, |c, v| c.reload_on_focus = v);
    catalog.add(&reload_focus);
    page.add(&catalog);

    // --- On launch ---
    let launch = adw::PreferencesGroup::builder().title("On launch").build();
    let check_updates = adw::SwitchRow::builder()
        .title("Check for updates")
        .subtitle("Flag apps with a newer upstream release")
        .active(cfg.borrow().check_for_updates)
        .build();
    bind_switch(&check_updates, cfg, save, |c, v| c.check_for_updates = v);
    launch.add(&check_updates);

    let repair_auto = adw::SwitchRow::builder()
        .title("Repair launchers automatically")
        .subtitle("Recreate any missing .desktop files")
        .active(cfg.borrow().repair_launchers_auto)
        .build();
    bind_switch(&repair_auto, cfg, save, |c, v| c.repair_launchers_auto = v);
    launch.add(&repair_auto);
    page.add(&launch);

    page
}

fn network_page(cfg: &Rc<RefCell<GuiConfig>>, save: &Rc<dyn Fn()>) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Network")
        .icon_name("network-transmit-receive-symbolic")
        .build();

    // --- Downloads ---
    let downloads = adw::PreferencesGroup::builder().title("Downloads").build();

    let route_opts = ["System proxy (Tor)", "Direct (no proxy)"];
    let route_model = gtk::StringList::new(&route_opts);
    let route_row = adw::ComboRow::builder()
        .title("Route through")
        .model(&route_model)
        .build();
    let cur = cfg.borrow().route_through.clone();
    route_row.set_selected(route_opts.iter().position(|o| *o == cur).unwrap_or(0) as u32);
    {
        let cfg = Rc::clone(cfg);
        let save = Rc::clone(save);
        route_row.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            cfg.borrow_mut().route_through = route_opts
                .get(idx)
                .copied()
                .unwrap_or(route_opts[0])
                .to_string();
            (save)();
        });
    }
    downloads.add(&route_row);

    let verify = adw::SwitchRow::builder()
        .title("Verify checksums & signatures")
        .active(cfg.borrow().verify_checksums)
        .build();
    bind_switch(&verify, cfg, save, |c, v| c.verify_checksums = v);
    downloads.add(&verify);

    let conc_adj = gtk::Adjustment::new(
        cfg.borrow().clamp_concurrent() as f64,
        1.0,
        8.0,
        1.0,
        1.0,
        0.0,
    );
    let conc = adw::SpinRow::builder()
        .title("Concurrent downloads")
        .subtitle("Lower is gentler over Tor")
        .adjustment(&conc_adj)
        .build();
    {
        let cfg = Rc::clone(cfg);
        let save = Rc::clone(save);
        conc.connect_value_notify(move |row| {
            cfg.borrow_mut().concurrent_downloads = row.value() as u32;
            (save)();
        });
    }
    downloads.add(&conc);
    page.add(&downloads);

    // --- Seeding ---
    let seeding = adw::PreferencesGroup::builder().title("Seeding").build();

    let auto_seed = adw::SwitchRow::builder()
        .title("Auto-seed new instances")
        .subtitle("Seed settings right after creating an instance")
        .active(cfg.borrow().auto_seed)
        .build();
    bind_switch(&auto_seed, cfg, save, |c, v| c.auto_seed = v);
    seeding.add(&auto_seed);

    // Per-app ports via portmux-port — reflect a real runtime probe.
    let (port_sub, connected) = match portmux_port_probe_runtime() {
        Ok(o) => (format!("{} · detected on host", o.invocation), true),
        Err(_) => ("portmux-port · not detected".to_string(), false),
    };
    let ports_row = adw::ActionRow::builder()
        .title("Per-app ports via")
        .subtitle(port_sub)
        .build();
    let pill = gtk::Label::new(Some(if connected { "connected" } else { "missing" }));
    pill.add_css_class("ac-status");
    pill.add_css_class(if connected { "installed" } else { "available" });
    pill.set_valign(gtk::Align::Center);
    ports_row.add_suffix(&pill);
    seeding.add(&ports_row);

    let runner_opts = ["Auto", "flatpak-spawn --host xvfb-run", "xvfb-run"];
    let runner_model = gtk::StringList::new(&runner_opts);
    let runner_row = adw::ComboRow::builder()
        .title("Headless runner")
        .model(&runner_model)
        .build();
    let curr = cfg.borrow().headless_runner.clone();
    runner_row.set_selected(runner_opts.iter().position(|o| *o == curr).unwrap_or(0) as u32);
    {
        let cfg = Rc::clone(cfg);
        let save = Rc::clone(save);
        runner_row.connect_selected_notify(move |row| {
            let idx = row.selected() as usize;
            cfg.borrow_mut().headless_runner = runner_opts
                .get(idx)
                .copied()
                .unwrap_or(runner_opts[0])
                .to_string();
            (save)();
        });
    }
    seeding.add(&runner_row);
    page.add(&seeding);

    page
}

/// Maintenance actions that aren't in the primary menu, kept reachable here.
fn storage_page(parent: &gtk::ApplicationWindow) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Storage")
        .icon_name("drive-harddisk-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder()
        .title("Maintenance")
        .description("Catalog, cache, and legacy-directory upkeep")
        .build();

    group.add(&action_row(parent, "Reload catalog now", "Re-scan and rebuild the list", "win.refresh", "Reload", false));
    group.add(&action_row(parent, "Clean download cache", "Delete cached AppImage downloads", "win.clean_cache", "Clean", false));
    group.add(&action_row(parent, "Update seed packs", "Fetch the latest signed seed-pack tarball", "win.update_seed_packs", "Update", false));
    group.add(&action_row(parent, "Show seed packs", "List loaded seed packs and their sources", "win.list_seed_packs", "Show", false));
    group.add(&action_row(parent, "Migrate legacy directories", "Import old portable dirs into managed instances", "win.migrate", "Migrate", false));
    page.add(&group);
    page
}

fn backup_page(parent: &gtk::ApplicationWindow) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Backup")
        .icon_name("document-save-symbolic")
        .build();
    let group = adw::PreferencesGroup::builder().title("Manager state").build();
    group.add(&action_row(parent, "Back up…", "Archive manager state to a file", "win.backup", "Back Up", false));
    group.add(&action_row(parent, "Restore…", "Restore manager state from an archive", "win.restore_backup", "Restore", false));
    page.add(&group);
    page
}

/// An action row whose suffix button activates a `win.*` action on the main window.
fn action_row(
    parent: &gtk::ApplicationWindow,
    title: &str,
    subtitle: &str,
    action: &'static str,
    btn_label: &str,
    destructive: bool,
) -> adw::ActionRow {
    let row = adw::ActionRow::builder().title(title).subtitle(subtitle).build();
    let btn = gtk::Button::with_label(btn_label);
    btn.set_valign(gtk::Align::Center);
    if destructive {
        btn.add_css_class("destructive-action");
    }
    let parent = parent.clone();
    btn.connect_clicked(move |_| {
        let _ = WidgetExt::activate_action(&parent, action, None);
    });
    row.add_suffix(&btn);
    row.set_activatable_widget(Some(&btn));
    row
}

/// Wire an `AdwSwitchRow` to a bool field of the config, saving on every toggle.
fn bind_switch(
    row: &adw::SwitchRow,
    cfg: &Rc<RefCell<GuiConfig>>,
    save: &Rc<dyn Fn()>,
    setter: fn(&mut GuiConfig, bool),
) {
    let cfg = Rc::clone(cfg);
    let save = Rc::clone(save);
    row.connect_active_notify(move |r| {
        setter(&mut cfg.borrow_mut(), r.is_active());
        (save)();
    });
}
