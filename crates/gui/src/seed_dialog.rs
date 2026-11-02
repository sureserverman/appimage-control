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
    ApplicationWindow, Box as GtkBox, Button, CheckButton, Dialog, Entry, FlowBox, Image, Label,
    Orientation, ResponseType, ScrolledWindow,
};

/// The seed-settings modal. Collects which installed apps to seed and returns
/// `(SeedPreset::auto_default(), selected_ids)`, or `None` on cancel. The caller
/// performs the seeding (and shows the activity log) after the dialog closes.
pub fn choose_seed_settings(
    parent: &ApplicationWindow,
    installed_apps: &[(String, String, u32)],
    icons: &[Option<Vec<u8>>],
) -> Option<(SeedPreset, Vec<String>)> {
    let forms_dir = default_seed_forms_dir();

    let dialog = build_dialog_shell(parent, "portmux sets up each app's proxy");
    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(0);
    area.set_hexpand(true);
    area.set_vexpand(true);

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

    // Design order: apps-to-seed first, then saved configurations.
    body.append(&seed_section_header("APPS TO SEED"));
    body.append(&apps_widgets.group);
    if let Some(group) = saved_forms_widgets {
        body.append(&seed_section_header("SAVED CONFIGURATIONS"));
        body.append(&group);
    }

    let scroll = ScrolledWindow::builder()
        .child(&body)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    area.append(&scroll);

    let footer = build_footer(&dialog, &apps_widgets.checks);
    area.append(&footer);

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
                *decided_c.borrow_mut() = Some((SeedPreset::auto_default(), selected));
            }
        }
        d.close();
        loop_c.quit();
    });
    // If the dialog is destroyed without a response (parent closed / WM kill),
    // quit the nested loop so the caller is not permanently blocked (Cancel).
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

fn build_dialog_shell(parent: &ApplicationWindow, subtitle: &str) -> Dialog {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Seed Settings")
        .default_width(480)
        .default_height(540)
        .resizable(true)
        .use_header_bar(0)
        .build();
    // Single header: a custom titlebar (left-aligned title + subtitle, close-only)
    // replaces both the stock CSD titlebar and the old in-body header.
    dialog.set_titlebar(Some(&dialog_titlebar("Seed Settings", Some(subtitle))));
    dialog
}

/// A flat AdwHeaderBar used as a dialog titlebar: bold title + optional dim
/// subtitle packed at the start, only a close button on the end.
fn dialog_titlebar(title: &str, subtitle: Option<&str>) -> adw::HeaderBar {
    let hb = adw::HeaderBar::new();
    hb.set_decoration_layout(Some(":close"));
    hb.set_title_widget(Some(&GtkBox::new(Orientation::Horizontal, 0)));
    let titles = GtkBox::new(Orientation::Vertical, 0);
    titles.set_valign(gtk::Align::Center);
    let t = Label::new(Some(title));
    t.add_css_class("title-4");
    t.set_halign(gtk::Align::Start);
    t.set_xalign(0.0);
    titles.append(&t);
    if let Some(sub) = subtitle {
        let s = Label::new(Some(sub));
        s.add_css_class("caption");
        s.add_css_class("dim-label");
        s.set_halign(gtk::Align::Start);
        s.set_xalign(0.0);
        titles.append(&s);
    }
    hb.pack_start(&titles);
    hb
}

/// Tiny uppercase section header with a trailing hairline (design turn 1),
/// matching the launcher grid and Preferences.
fn seed_section_header(text: &str) -> GtkBox {
    let b = GtkBox::new(Orientation::Horizontal, 9);
    let lbl = Label::new(Some(text));
    lbl.add_css_class("ac-section-head");
    lbl.set_halign(gtk::Align::Start);
    let line = gtk::Separator::new(Orientation::Horizontal);
    line.set_hexpand(true);
    line.set_valign(gtk::Align::Center);
    b.append(&lbl);
    b.append(&line);
    b
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
        icon.add_css_class("ac-appicon");
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
        let cb = CheckButton::new();
        cb.set_active(true);
        cb.set_valign(gtk::Align::Center);
        // Design turn 1: leading check, then icon, then name.
        // add_prefix prepends, so add the icon first to end up with [check][icon].
        row.add_prefix(&icon);
        row.add_prefix(&cb);

        // Status pill, same "supported" vocabulary as the main grid's
        // `.ac-status` card pills. `installed_apps` is pre-filtered by
        // the caller (main.rs) to apps that are both seed-supported
        // and have ≥1 installed instance, so every row reaching here
        // today takes the "Supported" branch; the `count == 0` arm is
        // kept for callers that don't pre-filter, matching the dimmed
        // "no instances" treatment from the design spec.
        let pill = Label::new(None);
        pill.add_css_class("ac-status");
        pill.set_valign(gtk::Align::Center);
        if *count > 0 {
            pill.set_label("Supported");
            pill.add_css_class("installed");
        } else {
            pill.set_label("No instances");
            pill.add_css_class("available");
            row.add_css_class("dim-label");
            cb.set_active(false);
            cb.set_sensitive(false);
        }
        row.add_suffix(&pill);
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

// ----- Saved-forms group (chip row) -----------------------------------

/// Shared state for the saved-configuration chips. Cloning is cheap —
/// every field is either a `Rc` or a GTK widget handle (itself
/// reference-counted).
#[derive(Clone)]
struct ChipCtx {
    home: PathBuf,
    checks: Vec<(String, CheckButton)>,
    installed: Vec<String>,
    parent: ApplicationWindow,
    names_state: Rc<RefCell<Vec<String>>>,
    flow: FlowBox,
    save_chip: Button,
    last_selected: Rc<RefCell<Option<GtkBox>>>,
}

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

    let flow = FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_min_children_per_line(1);
    flow.set_max_children_per_line(8);
    flow.set_row_spacing(6);
    flow.set_column_spacing(6);
    flow.set_homogeneous(false);
    flow.set_halign(gtk::Align::Fill);

    let save_chip = Button::builder().label("+ Save current…").build();
    save_chip.add_css_class("flat");
    save_chip.add_css_class("ac-chip-add");

    let names = list_seed_forms(&home).unwrap_or_default();
    let names_state: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(names.clone()));

    let ctx = ChipCtx {
        home: home.clone(),
        checks,
        installed: installed_app_ids,
        parent: parent.clone(),
        names_state: Rc::clone(&names_state),
        flow: flow.clone(),
        save_chip: save_chip.clone(),
        last_selected: Rc::new(RefCell::new(None)),
    };

    // "+ Save current…" click — same write path as the previous
    // "Save as…" button: prompt for a name, persist the ticked app
    // ids under that name, then re-render the chip row.
    {
        let ctx = ctx.clone();
        save_chip.connect_clicked(move |_| {
            let Some(name) = prompt_form_name(&ctx.parent) else {
                return;
            };
            let selected: Vec<String> = ctx
                .checks
                .iter()
                .filter_map(|(id, cb)| cb.is_active().then_some(id.clone()))
                .collect();
            // The preset is recorded for diagnostics (auto-resolved
            // from the host at save time) but is ignored on load.
            match SavedSeedForm::new(&name, SeedPreset::auto_default(), selected) {
                Ok(form) => match save_seed_form(&ctx.home, &form) {
                    Ok(()) => {
                        let names = list_seed_forms(&ctx.home).unwrap_or_default();
                        *ctx.names_state.borrow_mut() = names.clone();
                        rebuild_chips(&ctx, &names);
                    }
                    Err(e) => warn(&ctx.parent, "Could not save form", &format!("{e}")),
                },
                Err(e) => warn(&ctx.parent, "Invalid name", &format!("{e}")),
            }
        });
    }

    rebuild_chips(&ctx, &names);

    group.add(&flow);
    Some(group)
}

/// Clear and rebuild the chip row from `names`, then re-append the
/// persistent "+ Save current…" chip at the end.
fn rebuild_chips(ctx: &ChipCtx, names: &[String]) {
    while let Some(child) = ctx.flow.first_child() {
        ctx.flow.remove(&child);
    }
    *ctx.last_selected.borrow_mut() = None;
    for name in names {
        let chip = build_chip(ctx, name.clone());
        ctx.flow.append(&chip);
    }
    ctx.flow.append(&ctx.save_chip);
}

/// One named-selection chip: a recall button (loads the form's app
/// selection, same as the previous dropdown's "select" handler) plus
/// a small delete affordance (same write path as the previous
/// standalone "Delete" button, scoped to this chip's name).
fn build_chip(ctx: &ChipCtx, name: String) -> GtkBox {
    let chip = GtkBox::new(Orientation::Horizontal, 0);
    chip.add_css_class("ac-chip");

    let recall_btn = Button::builder().label(&name).build();
    recall_btn.add_css_class("flat");
    recall_btn.add_css_class("ac-chip-recall");
    chip.append(&recall_btn);

    let delete_btn = Button::from_icon_name("window-close-symbolic");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("circular");
    delete_btn.add_css_class("ac-chip-delete");
    delete_btn.set_tooltip_text(Some("Delete this saved configuration"));
    chip.append(&delete_btn);

    {
        let ctx = ctx.clone();
        let name = name.clone();
        let chip_for_select = chip.clone();
        recall_btn.connect_clicked(move |_| match load_seed_form(&ctx.home, &name) {
            Ok(form) => {
                // The form's stored `preset` field is intentionally
                // ignored: it no longer affects output.
                let wanted: Vec<String> = form
                    .app_ids
                    .into_iter()
                    .filter(|id| ctx.installed.iter().any(|i| i == id))
                    .collect();
                apply_app_selection(&ctx.checks, &wanted);
                if let Some(prev) = ctx.last_selected.borrow_mut().take() {
                    prev.remove_css_class("selected");
                }
                chip_for_select.add_css_class("selected");
                *ctx.last_selected.borrow_mut() = Some(chip_for_select.clone());
            }
            Err(e) => warn(&ctx.parent, "Could not load saved form", &format!("{e}")),
        });
    }

    {
        let ctx = ctx.clone();
        let name = name.clone();
        delete_btn.connect_clicked(move |_| match delete_seed_form(&ctx.home, &name) {
            Ok(_) => {
                let names = list_seed_forms(&ctx.home).unwrap_or_default();
                *ctx.names_state.borrow_mut() = names.clone();
                rebuild_chips(&ctx, &names);
            }
            Err(e) => warn(&ctx.parent, "Could not delete form", &format!("{e}")),
        });
    }

    chip
}

// ----- Footer -----------------------------------------------------------

/// Faint "<n> apps selected" + Cancel + the `.suggested-action` Seed
/// button, built as a plain box (rather than `Dialog::add_button`) so
/// the selected-count label can sit to its left. Buttons drive the
/// same `dialog.response(...)` path `connect_response` already
/// handles, so the modal-loop/result semantics are unchanged.
fn build_footer(dialog: &Dialog, checks: &[(String, CheckButton)]) -> GtkBox {
    let footer = GtkBox::new(Orientation::Horizontal, 12);
    footer.set_margin_top(12);
    footer.set_margin_bottom(16);
    footer.set_margin_start(20);
    footer.set_margin_end(20);

    let count_label = Label::new(None);
    count_label.add_css_class("caption");
    count_label.add_css_class("ac-summary-faint");
    count_label.set_halign(gtk::Align::Start);
    count_label.set_hexpand(true);
    footer.append(&count_label);

    let cancel_btn = Button::builder().label("_Cancel").build();
    cancel_btn.set_use_underline(true);
    footer.append(&cancel_btn);

    let seed_btn = Button::builder().label("Seed 0 apps").build();
    seed_btn.add_css_class("suggested-action");
    seed_btn.set_sensitive(false);
    footer.append(&seed_btn);

    let dialog_cancel = dialog.clone();
    cancel_btn.connect_clicked(move |_| dialog_cancel.response(ResponseType::Cancel));
    let dialog_ok = dialog.clone();
    seed_btn.connect_clicked(move |_| dialog_ok.response(ResponseType::Ok));
    dialog.set_default_widget(Some(&seed_btn));

    let checks_for_label = checks.to_vec();
    let seed_btn_clone = seed_btn.clone();
    let count_label_clone = count_label.clone();
    let recompute: Rc<dyn Fn()> = Rc::new(move || {
        let n = checks_for_label
            .iter()
            .filter(|(_, cb)| cb.is_active())
            .count();
        seed_btn_clone.set_label(&format!("Seed {n} app{}", if n == 1 { "" } else { "s" }));
        seed_btn_clone.set_sensitive(n > 0);
        count_label_clone.set_label(&format!(
            "{n} app{} selected",
            if n == 1 { "" } else { "s" }
        ));
    });
    for (_, cb) in checks {
        let r = Rc::clone(&recompute);
        cb.connect_toggled(move |_| (r)());
    }
    (recompute)();

    footer
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
