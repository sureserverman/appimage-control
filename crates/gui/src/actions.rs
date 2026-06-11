use std::rc::Rc;

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
}

pub fn build_actions_menu_button() -> MenuButton {
    let menu = gio::Menu::new();
    menu.append(Some("Seed Settings…"), Some("win.seed_settings"));
    menu.append(Some("Refresh"), Some("win.refresh"));
    menu.append(Some("Update Seed Packs"), Some("win.update_seed_packs"));
    menu.append(Some("Show Seed Packs"), Some("win.list_seed_packs"));
    menu.append(Some("Clean Cache"), Some("win.clean_cache"));
    menu.append(Some("Clean Installs (Keep Cache)"), Some("win.clean"));
    menu.append(Some("Purge State"), Some("win.purge"));
    menu.append(Some("Migrate Legacy Dirs"), Some("win.migrate"));
    menu.append(Some("Backup"), Some("win.backup"));
    menu.append(Some("Restore from backup"), Some("win.restore_backup"));
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
        ] {
            assert!(
                window.lookup_action(name).is_some(),
                "expected action {name}"
            );
        }
    }
}
