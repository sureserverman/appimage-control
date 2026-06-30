//! "Record Seed Pack" wizard — author a seed pack by diffing an app's
//! config before/after manual configuration (see
//! `docs/workflows/recording.md`).
//!
//! This module holds the three modal steps; the orchestration (busy
//! state, the off-thread headless baseline capture, and the sequencing)
//! lives in `main.rs` because it needs the main window's progress/status
//! widgets. Each function here builds a modal dialog, runs a nested
//! `glib::MainLoop`, and returns a plain value — the same pattern as
//! `seed_dialog::choose_seed_settings` and the dialogs in `dialogs.rs`.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use gtk::glib;
use gtk::{
    ApplicationWindow, Box as GtkBox, CheckButton, Dialog, DropDown, Entry, FileChooserAction,
    FileChooserNative, Label, Orientation, ResponseType, ScrolledWindow, SpinButton, StringList,
};
use serde_json::{Map, Value};

use appimage_control_core::ConfigDiff;

/// One installable app the recorder can target (an app with ≥ 1 installed
/// instance). `known_config_path`/`first_run_args` are pre-filled from an
/// existing seed pack when one ships for this app; otherwise the config
/// path is auto-discovered at capture time, so the field stays blank.
pub struct RecordApp {
    pub app_id: String,
    pub display_name: String,
    pub instances: u32,
    pub known_config_path: Option<String>,
    pub first_run_args: Vec<String>,
}

/// User's phase-1 choices, returned by [`choose_record_target`].
#[derive(Clone)]
pub struct RecordTarget {
    pub app_id: String,
    pub instance_index: u32,
    /// config_path template (with any `{appimage}` placeholder), relative
    /// to the instance dir. **Empty means auto-detect** at capture time.
    pub config_path: String,
    pub first_run_args: Vec<String>,
}

/// Render a JSON value to a short, single-line preview for the diff list.
fn render_value(v: &Value) -> String {
    let s = serde_json::to_string(v).unwrap_or_else(|_| "<unprintable>".into());
    const MAX: usize = 80;
    if s.chars().count() > MAX {
        let truncated: String = s.chars().take(MAX).collect();
        format!("{truncated}…")
    } else {
        s
    }
}

/// Phase 1 dialog: pick the app, instance index, and config path to
/// record. Returns `None` on cancel.
pub fn choose_record_target(
    parent: &ApplicationWindow,
    apps: &[RecordApp],
) -> Option<RecordTarget> {
    debug_assert!(!apps.is_empty(), "caller must pass at least one app");

    // Owned snapshot so the change handler can outlive the borrow.
    let snapshot: Rc<Vec<RecordApp>> = Rc::new(
        apps.iter()
            .map(|a| RecordApp {
                app_id: a.app_id.clone(),
                display_name: a.display_name.clone(),
                instances: a.instances,
                known_config_path: a.known_config_path.clone(),
                first_run_args: a.first_run_args.clone(),
            })
            .collect(),
    );

    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Record Seed Pack")
        .default_width(560)
        .default_height(360)
        .use_header_bar(0)
        .build();

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(10);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    let heading = Label::builder()
        .label("Record a seed pack")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");
    area.append(&heading);

    let blurb = Label::builder()
        .label(
            "Pick an installed app and instance. A baseline of its first-run config is captured \
             headlessly. You then launch the app, change its settings, and quit — the recorder \
             diffs the two and writes a pack of the changed keys.",
        )
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    blurb.add_css_class("dim-label");
    blurb.add_css_class("caption");
    area.append(&blurb);

    // App dropdown.
    let names: Vec<&str> = snapshot.iter().map(|a| a.display_name.as_str()).collect();
    let model = StringList::new(&names);
    let dropdown = DropDown::builder().model(&model).build();
    area.append(&row_with_label("App", &dropdown));

    // Instance index.
    let first = &snapshot[0];
    let instance_spin = SpinButton::with_range(1.0, first.instances.max(1) as f64, 1.0);
    instance_spin.set_value(1.0);
    area.append(&row_with_label("Instance", &instance_spin));

    // Config path — OPTIONAL. Blank means "auto-detect at capture time":
    // the headless baseline launch also scans for the app's config file, so
    // the common case needs no input. Pre-filled only when an existing pack
    // already declares the path; otherwise left blank for auto-detection.
    let config_entry = Entry::builder()
        .placeholder_text("auto-detected — leave blank to detect at capture time")
        .hexpand(true)
        .build();
    if let Some(cfg) = &first.known_config_path {
        config_entry.set_text(cfg);
    }
    area.append(&row_with_label("Config path", &config_entry));

    let config_hint = Label::builder()
        .label(
            "Optional. Leave blank to auto-detect: the baseline launch scans the instance \
             dir for the app's JSON config. Set it only to override detection (e.g. an app \
             that writes several config files). A single `{appimage}` is replaced with the \
             AppImage filename.",
        )
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    config_hint.add_css_class("dim-label");
    config_hint.add_css_class("caption");
    area.append(&config_hint);

    let cancel_btn = dialog.add_button("_Cancel", ResponseType::Cancel);
    let _ = cancel_btn;
    let ok_btn = dialog.add_button("_Capture baseline", ResponseType::Ok);
    ok_btn.add_css_class("suggested-action");
    dialog.set_default_response(ResponseType::Ok);

    // When the app selection changes, retarget the instance range and
    // re-prefill the config path from that app's known pack (blank if none).
    {
        let snapshot = Rc::clone(&snapshot);
        let instance_spin = instance_spin.clone();
        let config_entry = config_entry.clone();
        dropdown.connect_selected_notify(move |dd| {
            let idx = dd.selected() as usize;
            if let Some(app) = snapshot.get(idx) {
                instance_spin.set_range(1.0, app.instances.max(1) as f64);
                instance_spin.set_value(1.0);
                config_entry.set_text(app.known_config_path.as_deref().unwrap_or(""));
            }
        });
    }

    let decided: Rc<RefCell<Option<RecordTarget>>> = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let snapshot_c = Rc::clone(&snapshot);
    let dropdown_c = dropdown.clone();
    let instance_c = instance_spin.clone();
    let entry_c = config_entry.clone();
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(move |d, r| {
        if r == ResponseType::Ok {
            let idx = dropdown_c.selected() as usize;
            if let Some(app) = snapshot_c.get(idx) {
                // Empty config_path is intentional: it signals the capture
                // step to auto-detect the path during its headless launch.
                let cfg = entry_c.text().trim().to_string();
                *decided_c.borrow_mut() = Some(RecordTarget {
                    app_id: app.app_id.clone(),
                    instance_index: instance_c.value() as u32,
                    config_path: cfg,
                    first_run_args: app.first_run_args.clone(),
                });
            }
        }
        d.close();
        loop_c.quit();
    });
    let loop_destroy = loop_.clone();
    dialog.connect_destroy(move |_| loop_destroy.quit());
    dialog.present();
    loop_.run();
    let out = decided.borrow_mut().take();
    out
}

/// Phase 2 dialog: instruct the user to configure the app, with a best-
/// effort "Launch app" button. Returns `true` when the user clicks
/// "Diff changes" (proceed), `false` on cancel.
pub fn confirm_manual_step(
    parent: &ApplicationWindow,
    app_id: &str,
    instance_index: u32,
    appimage_abs: &Path,
) -> bool {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Configure the app")
        .default_width(520)
        .use_header_bar(0)
        .build();

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(8);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    let heading = Label::builder()
        .label("Baseline captured")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");
    area.append(&heading);

    let body = Label::builder()
        .label(format!(
            "Now launch {app_id} (instance {instance_index}), change the settings you want \
             the pack to carry, then quit the app. When it has fully exited, click \
             “Diff changes”.\n\nLaunch it with the button below, or from your normal launcher.",
        ))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    area.append(&body);

    dialog.add_button("_Cancel", ResponseType::Cancel);
    // Custom non-closing response for "Launch app".
    let launch_response = ResponseType::Other(7);
    dialog.add_button("_Launch app", launch_response);
    let diff_btn = dialog.add_button("_Diff changes", ResponseType::Ok);
    diff_btn.add_css_class("suggested-action");
    dialog.set_default_response(ResponseType::Ok);

    let proceed: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let proceed_c = Rc::clone(&proceed);
    let appimage_owned = appimage_abs.to_path_buf();
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(move |d, r| {
        match r {
            ResponseType::Other(7) => {
                // Best-effort detached launch in the instance dir so the
                // app writes its portable config alongside the AppImage.
                if let Some(dir) = appimage_owned.parent() {
                    let _ = std::process::Command::new(&appimage_owned)
                        .current_dir(dir)
                        .spawn();
                }
                // Do NOT close — the user configures then clicks Diff.
            }
            ResponseType::Ok => {
                *proceed_c.borrow_mut() = true;
                d.close();
                loop_c.quit();
            }
            _ => {
                d.close();
                loop_c.quit();
            }
        }
    });
    let loop_destroy = loop_.clone();
    dialog.connect_destroy(move |_| loop_destroy.quit());
    dialog.present();
    loop_.run();
    let out = *proceed.borrow();
    out
}

/// Phase 3 dialog: show the diff and let the user untick keys. Returns the
/// selected (checked) capturable keys, or `None` on cancel. Added/changed
/// keys are checkable (ticked by default); removed keys are shown disabled
/// (a `static` pack cannot delete keys).
pub fn review_and_select(
    parent: &ApplicationWindow,
    baseline: &Map<String, Value>,
    current: &Map<String, Value>,
    diff: &ConfigDiff,
) -> Option<Vec<String>> {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Review captured settings")
        .default_width(620)
        .default_height(460)
        .use_header_bar(0)
        .build();

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(8);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    let heading = Label::builder()
        .label("Choose which changes to record")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");
    area.append(&heading);

    let summary = Label::builder()
        .label(format!(
            "{} added · {} changed · {} removed · {} unchanged",
            diff.added.len(),
            diff.changed.len(),
            diff.removed.len(),
            diff.unchanged
        ))
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    summary.add_css_class("dim-label");
    summary.add_css_class("caption");
    area.append(&summary);

    let list = GtkBox::new(Orientation::Vertical, 6);
    list.set_margin_top(6);

    // checks: (key, CheckButton)
    let checks: Rc<RefCell<Vec<(String, CheckButton)>>> = Rc::new(RefCell::new(Vec::new()));

    for key in &diff.added {
        let preview = current.get(key).map(render_value).unwrap_or_default();
        let cb = key_check_row(key, &format!("new: {preview}"), true);
        list.append(&cb.0);
        checks.borrow_mut().push((key.clone(), cb.1));
    }
    for key in &diff.changed {
        let old = baseline.get(key).map(render_value).unwrap_or_default();
        let new = current.get(key).map(render_value).unwrap_or_default();
        let cb = key_check_row(key, &format!("{old}  →  {new}"), true);
        list.append(&cb.0);
        checks.borrow_mut().push((key.clone(), cb.1));
    }
    for key in &diff.removed {
        // Disabled, unchecked: static packs cannot remove keys.
        let cb = key_check_row(key, "removed — a static pack cannot delete keys (not recorded)", false);
        cb.1.set_sensitive(false);
        list.append(&cb.0);
        // Not added to `checks`: never selectable.
    }

    let scroll = ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(240)
        .build();
    area.append(&scroll);

    dialog.add_button("_Cancel", ResponseType::Cancel);
    let save_btn = dialog.add_button("_Save pack…", ResponseType::Ok);
    save_btn.add_css_class("suggested-action");
    dialog.set_default_response(ResponseType::Ok);

    let result: Rc<RefCell<Option<Vec<String>>>> = Rc::new(RefCell::new(None));
    let result_c = Rc::clone(&result);
    let checks_c = Rc::clone(&checks);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(move |d, r| {
        if r == ResponseType::Ok {
            let selected: Vec<String> = checks_c
                .borrow()
                .iter()
                .filter(|(_, cb)| cb.is_active())
                .map(|(k, _)| k.clone())
                .collect();
            *result_c.borrow_mut() = Some(selected);
        }
        d.close();
        loop_c.quit();
    });
    let loop_destroy = loop_.clone();
    dialog.connect_destroy(move |_| loop_destroy.quit());
    dialog.present();
    loop_.run();
    let out = result.borrow_mut().take();
    out
}

/// A labelled control row: a left-aligned caption label + the control.
fn row_with_label(label: &str, control: &impl IsA<gtk::Widget>) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    let lbl = Label::builder()
        .label(label)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .width_chars(12)
        .build();
    row.append(&lbl);
    control.set_hexpand(true);
    row.append(control);
    row
}

/// A check row: a CheckButton with the key as label and a dim caption
/// underneath showing the value preview.
fn key_check_row(key: &str, caption: &str, active: bool) -> (GtkBox, CheckButton) {
    let row = GtkBox::new(Orientation::Vertical, 0);
    let cb = CheckButton::builder().label(key).active(active).build();
    let cap = Label::builder()
        .label(caption)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .margin_start(28)
        .build();
    cap.add_css_class("dim-label");
    cap.add_css_class("caption");
    cap.add_css_class("monospace");
    row.append(&cb);
    row.append(&cap);
    (row, cb)
}

/// Save-file chooser pre-seeded with `<app-id>.json` in the per-user seeds
/// dir (the recommended drop location). Returns the chosen path or `None`.
pub fn choose_pack_save_path(parent: &ApplicationWindow, app_id: &str) -> Option<PathBuf> {
    let chooser = FileChooserNative::builder()
        .title("Save seed pack")
        .transient_for(parent)
        .action(FileChooserAction::Save)
        .accept_label("Save")
        .cancel_label("Cancel")
        .modal(true)
        .build();
    chooser.set_current_name(&format!("{app_id}.json"));
    if let Some(dir) = appimage_control_core::user_seeds_dir() {
        let _ = std::fs::create_dir_all(&dir);
        let _ = chooser.set_current_folder(Some(&gtk::gio::File::for_path(&dir)));
    }
    let result: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let result_c = Rc::clone(&result);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    chooser.connect_response(move |c, r| {
        if r == ResponseType::Accept {
            *result_c.borrow_mut() = c.file().and_then(|f| f.path());
        }
        c.destroy();
        loop_c.quit();
    });
    chooser.show();
    loop_.run();
    let out = result.borrow_mut().take();
    out
}
