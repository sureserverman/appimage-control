use std::rc::Rc;

use crate::i18n::tr;
use gtk::gio;
use gtk::prelude::*;
use gtk::{ApplicationWindow, MenuButton};

pub struct ActionHooks {
    pub apply: Rc<dyn Fn()>,
    pub update: Rc<dyn Fn()>,
    pub seed_settings: Rc<dyn Fn()>,
    pub refresh: Rc<dyn Fn()>,
    pub clean_cache: Rc<dyn Fn()>,
    pub clean: Rc<dyn Fn()>,
    pub purge: Rc<dyn Fn()>,
    pub migrate: Rc<dyn Fn()>,
    pub backup: Rc<dyn Fn()>,
    pub restore_backup: Rc<dyn Fn()>,
    /// Fetch + install the latest signed seed-pack tarball. Mirrors the
    /// CLI's `--update-seed-packs`.
    pub update_seed_packs: Rc<dyn Fn()>,
    /// Show one row per loaded seed pack with its source path. Mirrors
    /// the CLI's `--list-seed-packs`.
    pub list_seed_packs: Rc<dyn Fn()>,
    /// Launch the "Record Seed Pack" wizard (capture baseline → manual
    /// config → diff → save). Mirrors the CLI's `--record-pack-baseline`
    /// / `--record-pack-capture` flow.
    pub record_seed_pack: Rc<dyn Fn()>,
    /// Re-run the best-effort per-instance launcher/icon repair (formerly only
    /// silent-on-mount); now also an explicit primary-menu item.
    pub repair: Rc<dyn Fn()>,
    /// Open the Preferences window (General / Network / Storage / Backup).
    pub preferences: Rc<dyn Fn()>,
    /// Show the keyboard-shortcuts reference.
    pub shortcuts: Rc<dyn Fn()>,
    /// Show the About window.
    pub about: Rc<dyn Fn()>,
}

fn bind_action(window: &ApplicationWindow, name: &str, f: Rc<dyn Fn()>) {
    let a = gio::SimpleAction::new(name, None);
    a.connect_activate(move |_, _| (f)());
    window.add_action(&a);
}

pub fn install_window_actions(window: &ApplicationWindow, hooks: ActionHooks) {
    bind_action(window, "apply", hooks.apply);
    bind_action(window, "update", hooks.update);
    bind_action(window, "seed_settings", hooks.seed_settings);
    bind_action(window, "refresh", hooks.refresh);
    bind_action(window, "clean_cache", hooks.clean_cache);
    bind_action(window, "clean", hooks.clean);
    bind_action(window, "purge", hooks.purge);
    bind_action(window, "migrate", hooks.migrate);
    bind_action(window, "backup", hooks.backup);
    bind_action(window, "restore_backup", hooks.restore_backup);
    bind_action(window, "update_seed_packs", hooks.update_seed_packs);
    bind_action(window, "list_seed_packs", hooks.list_seed_packs);
    bind_action(window, "record_seed_pack", hooks.record_seed_pack);
    bind_action(window, "repair", hooks.repair);
    bind_action(window, "preferences", hooks.preferences);
    bind_action(window, "shortcuts", hooks.shortcuts);
    bind_action(window, "about", hooks.about);
}

/// Primary menu (HIG-grouped). The maintenance actions that aren't surfaced here
/// (refresh / migrate / update_seed_packs / list_seed_packs) stay bound and reachable
/// from the Preferences → Storage page and keyboard accelerators, so no behavior is lost.
pub fn build_actions_menu_button() -> MenuButton {
    let menu = gio::Menu::new();

    let g1 = gio::Menu::new();
    g1.append(Some(tr("Update All Installed")), Some("win.update"));
    g1.append(Some(tr("Seed Settings…")), Some("win.seed_settings"));
    g1.append(Some(tr("Record Seed Pack…")), Some("win.record_seed_pack"));
    menu.append_section(None, &g1);

    let g2 = gio::Menu::new();
    g2.append(Some(tr("Repair Launchers")), Some("win.repair"));
    g2.append(Some(tr("Back Up…")), Some("win.backup"));
    g2.append(Some(tr("Restore…")), Some("win.restore_backup"));
    menu.append_section(None, &g2);

    let g3 = gio::Menu::new();
    g3.append(Some(tr("Clean Download Cache")), Some("win.clean_cache"));
    g3.append(Some(tr("Remove All Instances…")), Some("win.clean"));
    g3.append(Some(tr("Purge Everything…")), Some("win.purge"));
    menu.append_section(None, &g3);

    let g4 = gio::Menu::new();
    g4.append(Some(tr("Preferences")), Some("win.preferences"));
    g4.append(Some(tr("Keyboard Shortcuts")), Some("win.shortcuts"));
    g4.append(Some(tr("About AppImage Control")), Some("win.about"));
    menu.append_section(None, &g4);

    let b = MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .build();
    b.set_menu_model(Some(&menu));
    b
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gtk::gio;
    use gtk::prelude::*;

    use super::{install_window_actions, ActionHooks};

    #[test]
    fn install_window_actions_registers_documented_ops() {
        if gtk::init().is_err() {
            eprintln!("skipping: gtk::init failed (no display?)");
            return;
        }
        let app = gtk::Application::new(
            Some("io.github.appimagecontrol.ActionWireTest"),
            Default::default(),
        );
        app.register(gio::Cancellable::NONE)
            .expect("GApplication::register");
        let window = gtk::ApplicationWindow::builder().application(&app).build();
        let noop: Rc<dyn Fn()> = Rc::new(|| ());
        install_window_actions(
            &window,
            ActionHooks {
                apply: Rc::clone(&noop),
                update: Rc::clone(&noop),
                seed_settings: Rc::clone(&noop),
                refresh: Rc::clone(&noop),
                clean_cache: Rc::clone(&noop),
                clean: Rc::clone(&noop),
                purge: Rc::clone(&noop),
                migrate: Rc::clone(&noop),
                backup: Rc::clone(&noop),
                restore_backup: Rc::clone(&noop),
                update_seed_packs: Rc::clone(&noop),
                list_seed_packs: Rc::clone(&noop),
                record_seed_pack: Rc::clone(&noop),
                repair: Rc::clone(&noop),
                preferences: Rc::clone(&noop),
                shortcuts: Rc::clone(&noop),
                about: Rc::clone(&noop),
            },
        );
        for name in [
            "apply",
            "update",
            "seed_settings",
            "refresh",
            "clean_cache",
            "clean",
            "purge",
            "migrate",
            "backup",
            "restore_backup",
            "update_seed_packs",
            "list_seed_packs",
            "record_seed_pack",
            "repair",
            "preferences",
            "shortcuts",
            "about",
        ] {
            assert!(
                window.lookup_action(name).is_some(),
                "expected action {name}"
            );
        }
    }
}
