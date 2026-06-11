//! The seed-settings modal. Split into small builder helpers so each
//! section (header, saved-forms, apps) can be reasoned about
//! independently.
//!
//! Returned value is `Option<(SeedPreset, Vec<String>)>`: `None` when
//! the user cancels, otherwise the runtime-resolved preset (auto-
//! detected — never user-selectable) plus the catalog app ids whose
//! checkboxes were ticked at OK time. The dialog never asks which
//! host kind to target; that's decided by the system itself, by
//! looking for the canonical local marker file.
//!
//! Saved-forms section persists app selections under
//! `~/.local/share/appimage-control/seed-forms/<name>.json` via
//! [`appimage_control_core`]. Loading a form snaps the dialog's
//! checkboxes to the saved app-id selection; the form's stored
//! preset is informational only and is ignored on load — the
//! resolved preset always comes from the runtime environment.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use appimage_control_core::{
    default_seed_forms_dir, delete_seed_form, list_seed_forms, load_seed_form, save_seed_form,
    SavedSeedForm, SeedPreset,
};
use gtk::gdk::Texture;
use gtk::glib;
use gtk::{
    ApplicationWindow, Box as GtkBox, Button, CheckButton, Dialog, DropDown, Entry, Image, Label,
    Orientation, ResponseType, ScrolledWindow, StringList,
};

/// Public entry point — drop-in replacement for the previous
/// `dialogs::choose_seed_settings`. Returns
/// `(SeedPreset::auto_default(), selected_ids)` or `None` on cancel.
pub fn choose_seed_settings(
    parent: &ApplicationWindow,
    installed_apps: &[(String, String, u32)],
    icons: &[Option<Vec<u8>>],
) -> Option<(SeedPreset, Vec<String>)> {
    let forms_dir = default_seed_forms_dir();

    let dialog = build_dialog_shell(parent);
    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(0);
    area.set_hexpand(true);
    area.set_vexpand(true);

    area.append(&build_header());

    let body = GtkBox::new(Orientation::Vertical, 16);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_vexpand(true);

    let installed_app_ids: Vec<String> =
        installed_apps.iter().map(|(id, _, _)| id.clone()).collect();

    let apps_widgets = build_apps_group(installed_apps, icons);

    let saved_forms_widgets = build_saved_forms_group(
        parent.clone(),
        forms_dir.clone(),
        apps_widgets.checks.clone(),
        installed_app_ids.clone(),
    );

    if let Some(group) = saved_forms_widgets {
        body.append(&group);
    }
    body.append(&apps_widgets.group);

    let scroll = ScrolledWindow::builder()
        .child(&body)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    area.append(&scroll);

    build_action_buttons(&dialog, &apps_widgets.checks);

    type Decision = Rc<RefCell<Option<(SeedPreset, Vec<String>)>>>;
    let decided: Decision = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    let checks_for_resp = apps_widgets.checks.clone();
    dialog.connect_response(move |d, r| {
        if r == ResponseType::Ok {
            let selected: Vec<String> = checks_for_resp
                .iter()
                .filter_map(|(app_id, cb)| cb.is_active().then_some(app_id.clone()))
                .collect();
            if !selected.is_empty() {
                // Preset is resolved at the dialog edge, not in the
                // dialog — saved forms ignore their stored preset, and
                // there is no user-visible profile switch. See
                // `SeedPreset::auto_default`.
                *decided_c.borrow_mut() = Some((SeedPreset::auto_default(), selected));
            }
        }
        d.close();
        loop_c.quit();
    });
    // If the dialog is destroyed without emitting a response (e.g.
    // the parent window is closed or the WM sends a kill), quit the
    // nested loop so the caller is not permanently blocked.  Treated
    // as Cancel (decided stays None).
    let loop_destroy = loop_.clone();
    dialog.connect_destroy(move |_| {
        loop_destroy.quit();
    });
    dialog.present();
    loop_.run();
    let out = decided.borrow().clone();
    out
}

// ----- Dialog shell ---------------------------------------------------

fn build_dialog_shell(parent: &ApplicationWindow) -> Dialog {
    Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Seed Settings")
        .default_width(480)
        .default_height(520)
        .resizable(true)
        .use_header_bar(0)
        .build()
}

fn build_header() -> GtkBox {
    let header = GtkBox::new(Orientation::Vertical, 4);
    header.set_margin_top(20);
    header.set_margin_start(20);
    header.set_margin_end(20);
    header.set_margin_bottom(0);
    let title = Label::builder()
        .label("Seed Settings")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    title.add_css_class("title-2");
    let subtitle = Label::builder()
        .label("Run each AppImage once to materialize and patch its config file.")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("caption");
    subtitle.add_css_class("dim-label");
    header.append(&title);
    header.append(&subtitle);
    header
}

// ----- Apps group -----------------------------------------------------

struct AppsWidgets {
    group: adw::PreferencesGroup,
    checks: Vec<(String, CheckButton)>,
}

fn build_apps_group(
    installed_apps: &[(String, String, u32)],
    icons: &[Option<Vec<u8>>],
) -> AppsWidgets {
    let group = adw::PreferencesGroup::new();
    group.set_title("Installed supported apps");

    let mut checks: Vec<(String, CheckButton)> = Vec::new();
    for (i, (app_id, display_name, count)) in installed_apps.iter().enumerate() {
        let row = adw::ActionRow::new();
        row.set_title(display_name);
        row.set_subtitle(&format!(
            "{count} installed instance{}",
            if *count == 1 { "" } else { "s" }
        ));

        let icon = Image::new();
        icon.set_pixel_size(24);
        icon.set_margin_start(2);
        icon.set_margin_end(2);
        if let Some(Some(bytes)) = icons.get(i) {
            let gbytes = glib::Bytes::from(bytes.as_slice());
            if let Ok(texture) = Texture::from_bytes(&gbytes) {
                icon.set_paintable(Some(&texture));
            } else {
                icon.set_icon_name(Some("application-x-executable-symbolic"));
            }
        } else {
            icon.set_icon_name(Some("application-x-executable-symbolic"));
        }
        row.add_prefix(&icon);

        let cb = CheckButton::new();
        cb.set_active(true);
        cb.set_valign(gtk::Align::Center);
        row.add_suffix(&cb);
        row.set_activatable_widget(Some(&cb));

        group.add(&row);
        checks.push((app_id.clone(), cb));
    }

    AppsWidgets { group, checks }
}

fn apply_app_selection(checks: &[(String, CheckButton)], wanted: &[String]) {
    for (app_id, cb) in checks {
        cb.set_active(wanted.iter().any(|w| w == app_id));
    }
}

// ----- Saved-forms group ---------------------------------------------

fn build_saved_forms_group(
    parent: ApplicationWindow,
    forms_dir: Option<PathBuf>,
    checks: Vec<(String, CheckButton)>,
    installed_app_ids: Vec<String>,
) -> Option<adw::PreferencesGroup> {
    // Without a resolvable home dir we cannot persist anything; hide
    // the section entirely instead of showing dead controls.
    let _ = forms_dir.as_ref()?;
    let home = dirs::home_dir()?;

    let group = adw::PreferencesGroup::new();
    group.set_title("Saved configurations");
    group.set_description(Some("Recall a previously-saved app selection."));

    let names = list_seed_forms(&home).unwrap_or_default();
    let store = build_named_store(&names);

    let combo_row = adw::ActionRow::new();
    combo_row.set_title("Recall");
    combo_row.set_subtitle("Pick a saved form to load its app selection.");

    let dropdown = DropDown::builder()
        .model(&store)
        .valign(gtk::Align::Center)
        .build();
    combo_row.add_suffix(&dropdown);
    group.add(&combo_row);

    let names_state: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(names));

    let buttons_row = adw::ActionRow::new();
    buttons_row.set_title("");
    let save_btn = Button::builder().label("Save as…").build();
    save_btn.add_css_class("flat");
    let delete_btn = Button::builder().label("Delete").build();
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");
    delete_btn.set_sensitive(false);
    buttons_row.add_suffix(&save_btn);
    buttons_row.add_suffix(&delete_btn);
    group.add(&buttons_row);

    // Dropdown selection → load form (or clear when "—" picked)
    {
        let names_state = Rc::clone(&names_state);
        let home = home.clone();
        let checks = checks.clone();
        let installed = installed_app_ids.clone();
        let delete_btn = delete_btn.clone();
        let parent = parent.clone();
        dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected();
            if idx == 0 {
                delete_btn.set_sensitive(false);
                return;
            }
            let names = names_state.borrow();
            let Some(name) = names.get((idx as usize).saturating_sub(1)).cloned() else {
                delete_btn.set_sensitive(false);
                return;
            };
            drop(names);
            delete_btn.set_sensitive(true);
            match load_seed_form(&home, &name) {
                Ok(form) => {
                    // The form's stored `preset` field is intentionally
                    // ignored: networking source-of-truth is detected
                    // from the runtime host, not from whatever was
                    // active at save time.
                    let wanted: Vec<String> = form
                        .app_ids
                        .into_iter()
                        .filter(|id| installed.iter().any(|i| i == id))
                        .collect();
                    apply_app_selection(&checks, &wanted);
                }
                Err(e) => warn(&parent, "Could not load saved form", &format!("{e}")),
            }
        });
    }

    // Save as… click
    {
        let names_state = Rc::clone(&names_state);
        let home = home.clone();
        let checks = checks.clone();
        let parent = parent.clone();
        let dropdown = dropdown.clone();
        save_btn.connect_clicked(move |_| {
            let Some(name) = prompt_form_name(&parent) else {
                return;
            };
            let selected: Vec<String> = checks
                .iter()
                .filter_map(|(id, cb)| cb.is_active().then_some(id.clone()))
                .collect();
            // The preset is recorded for diagnostics (auto-resolved
            // from the host at save time) but is ignored on load.
            match SavedSeedForm::new(&name, SeedPreset::auto_default(), selected) {
                Ok(form) => match save_seed_form(&home, &form) {
                    Ok(()) => {
                        let names = list_seed_forms(&home).unwrap_or_default();
                        let store = build_named_store(&names);
                        dropdown.set_model(Some(&store));
                        if let Some(pos) = names.iter().position(|n| n == &form.name) {
                            dropdown.set_selected((pos + 1) as u32);
                        }
                        *names_state.borrow_mut() = names;
                    }
                    Err(e) => warn(&parent, "Could not save form", &format!("{e}")),
                },
                Err(e) => warn(&parent, "Invalid name", &format!("{e}")),
            }
        });
    }

    // Delete click
    {
        let names_state = Rc::clone(&names_state);
        let home = home.clone();
        let parent = parent.clone();
        let dropdown = dropdown.clone();
        let delete_btn_inner = delete_btn.clone();
        delete_btn.connect_clicked(move |_| {
            let idx = dropdown.selected();
            if idx == 0 {
                return;
            }
            let name = {
                let names = names_state.borrow();
                names.get((idx as usize).saturating_sub(1)).cloned()
            };
            let Some(name) = name else {
                return;
            };
            match delete_seed_form(&home, &name) {
                Ok(_) => {
                    let names = list_seed_forms(&home).unwrap_or_default();
                    let store = build_named_store(&names);
                    dropdown.set_model(Some(&store));
                    dropdown.set_selected(0);
                    *names_state.borrow_mut() = names;
                    delete_btn_inner.set_sensitive(false);
                }
                Err(e) => warn(&parent, "Could not delete form", &format!("{e}")),
            }
        });
    }

    Some(group)
}

fn build_named_store(names: &[String]) -> StringList {
    let store = StringList::new(&[]);
    store.append("—");
    for n in names {
        store.append(n);
    }
    store
}

// ----- Action buttons -------------------------------------------------

fn build_action_buttons(dialog: &Dialog, checks: &[(String, CheckButton)]) {
    let cancel_widget = dialog.add_button("_Cancel", ResponseType::Cancel);
    let seed_widget = dialog.add_button("Seed 0 apps", ResponseType::Ok);
    cancel_widget.set_margin_bottom(16);
    cancel_widget.set_margin_end(0);
    dialog.set_default_response(ResponseType::Ok);

    // `Dialog::add_button` returns a `gtk::Widget`; downcast to `Button` to
    // apply styling and label updates.  If the downcast fails (which would be
    // a gtk-rs or GTK ABI anomaly), skip the wiring rather than panicking —
    // the dialog still functions; the button label just stays "Seed 0 apps".
    if let Ok(seed_btn) = seed_widget.downcast::<Button>() {
        seed_btn.add_css_class("suggested-action");
        seed_btn.set_sensitive(false);
        seed_btn.set_margin_bottom(16);
        seed_btn.set_margin_end(20);

        let seed_btn_clone = seed_btn.clone();
        let checks_for_label = checks.to_vec();
        let recompute_label: Rc<dyn Fn()> = Rc::new(move || {
            let n = checks_for_label
                .iter()
                .filter(|(_, cb)| cb.is_active())
                .count();
            seed_btn_clone
                .set_label(&format!("Seed {n} app{}", if n == 1 { "" } else { "s" }));
            seed_btn_clone.set_sensitive(n > 0);
        });
        for (_, cb) in checks {
            let r = Rc::clone(&recompute_label);
            cb.connect_toggled(move |_| (r)());
        }
        (recompute_label)();
    }
}

// ----- Helper sub-dialogs --------------------------------------------

fn prompt_form_name(parent: &ApplicationWindow) -> Option<String> {
    let dialog = adw::MessageDialog::new(
        Some(parent),
        Some("Save configuration"),
        Some("Name this configuration for later recall."),
    );
    dialog.set_modal(true);
    dialog.set_default_width(380);
    let entry = Entry::builder()
        .placeholder_text("e.g. weekday-feather")
        .activates_default(true)
        .build();
    dialog.set_extra_child(Some(&entry));
    dialog.add_response("cancel", "_Cancel");
    dialog.add_response("save", "_Save");
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");

    let decided: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let entry_c = entry.clone();
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(None, move |_d, r| {
        if r == "save" {
            let text = entry_c.text().to_string().trim().to_string();
            if !text.is_empty() {
                *decided_c.borrow_mut() = Some(text);
            }
        }
        loop_c.quit();
    });
    // If the dialog is destroyed without a response, quit the nested
    // loop so the save flow does not hang.  Treated as Cancel.
    let loop_destroy = loop_.clone();
    dialog.connect_destroy(move |_| {
        loop_destroy.quit();
    });
    dialog.present();
    loop_.run();
    let out = decided.borrow().clone();
    out
}

fn warn(parent: &ApplicationWindow, title: &str, body: &str) {
    let d = adw::MessageDialog::new(Some(parent), Some(title), Some(body));
    d.set_modal(true);
    d.add_response("ok", "_OK");
    d.set_default_response(Some("ok"));
    d.set_close_response("ok");
    d.connect_response(None, |dialog, _| dialog.close());
    d.present();
}
