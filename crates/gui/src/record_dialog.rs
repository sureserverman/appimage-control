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

/// Flat AdwHeaderBar used as the wizard's titlebar: a single "Record Seed Pack"
/// header with only a close button (design 1f), replacing the stock CSD title.
fn record_titlebar() -> adw::HeaderBar {
    let hb = adw::HeaderBar::new();
    hb.set_decoration_layout(Some(":close"));
    let title = Label::new(Some("Record Seed Pack"));
    title.add_css_class("title-4");
    hb.set_title_widget(Some(&title));
    hb
}

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

/// 3-step wizard indicator shared by all three modal steps (Baseline →
/// Configure → Review). `current` is 1-based: steps before it render
/// completed (✓, accent), the current step is accent-outlined, steps
/// after it are pending (`@borders`). Purely decorative — the modal-chain
/// control flow across the three functions is unchanged.
fn step_indicator(current: u32) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 8);
    row.set_halign(gtk::Align::Center);
    row.set_margin_bottom(8);

    let steps = ["Baseline", "Configure", "Review"];
    let total = steps.len() as u32;
    for (i, label) in steps.iter().enumerate() {
        let step_no = (i as u32) + 1;
        let state = if step_no < current {
            "done"
        } else if step_no == current {
            "current"
        } else {
            "pending"
        };

        let cell = GtkBox::new(Orientation::Horizontal, 6);
        let glyph = if step_no < current {
            "✓".to_string()
        } else {
            step_no.to_string()
        };
        let circle = Label::new(Some(&glyph));
        circle.add_css_class("wizard-step-circle");
        circle.add_css_class(state);
        cell.append(&circle);

        let text = Label::new(Some(*label));
        text.add_css_class("wizard-step-label");
        text.add_css_class(state);
        cell.append(&text);
        row.append(&cell);

        if step_no < total {
            let connector = GtkBox::new(Orientation::Horizontal, 0);
            connector.add_css_class("wizard-connector");
            connector.add_css_class(if step_no < current { "done" } else { "pending" });
            connector.set_size_request(28, 2);
            connector.set_valign(gtk::Align::Center);
            row.append(&connector);
        }
    }
    row
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
    dialog.set_titlebar(Some(&record_titlebar()));

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(10);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    area.append(&step_indicator(1));

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
        .title("Record Seed Pack")
        .default_width(520)
        .use_header_bar(0)
        .build();
    dialog.set_titlebar(Some(&record_titlebar()));

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(8);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    area.append(&step_indicator(2));

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
        .title("Record Seed Pack")
        .default_width(620)
        .default_height(460)
        .use_header_bar(0)
        .build();
    dialog.set_titlebar(Some(&record_titlebar()));

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(8);
    area.set_margin_top(20);
    area.set_margin_bottom(12);
    area.set_margin_start(20);
    area.set_margin_end(20);

    area.append(&step_indicator(3));

    let heading_row = GtkBox::new(Orientation::Horizontal, 8);
    let heading = Label::builder()
        .label("Choose which changes to record")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");
    heading_row.append(&heading);
    // "diffed" pill — this step's phase marker (maps to --record-pack-capture).
    let diffed_pill = Label::new(Some("diffed"));
    diffed_pill.add_css_class("row-pill");
    diffed_pill.add_css_class("pending");
    heading_row.append(&diffed_pill);
    area.append(&heading_row);

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

    // "Changed keys" section heading with a count badge.
    let diff_heading_row = GtkBox::new(Orientation::Horizontal, 8);
    diff_heading_row.set_margin_top(6);
    let diff_heading = Label::builder()
        .label("Changed keys")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    diff_heading.add_css_class("heading");
    diff_heading_row.append(&diff_heading);
    let changed_count = diff.added.len() + diff.changed.len();
    let count_badge = Label::new(Some(&changed_count.to_string()));
    count_badge.add_css_class("row-pill");
    count_badge.add_css_class("pending");
    diff_heading_row.append(&count_badge);
    area.append(&diff_heading_row);

    let list = GtkBox::new(Orientation::Vertical, 6);
    list.set_margin_top(6);

    // checks: (key, CheckButton)
    let checks: Rc<RefCell<Vec<(String, CheckButton)>>> = Rc::new(RefCell::new(Vec::new()));

    for key in &diff.added {
        let preview = current.get(key).map(render_value).unwrap_or_default();
        let row = diff_check_row(key, "+", "added", &format!("new: {preview}"), true, true);
        list.append(&row.0);
        checks.borrow_mut().push((key.clone(), row.1));
    }
    for key in &diff.changed {
        let old = baseline.get(key).map(render_value).unwrap_or_default();
        let new = current.get(key).map(render_value).unwrap_or_default();
        let row = diff_check_row(key, "~", "changed", &format!("{old}  →  {new}"), true, true);
        list.append(&row.0);
        checks.borrow_mut().push((key.clone(), row.1));
    }
    for key in &diff.removed {
        // Disabled, unchecked: static packs cannot remove keys.
        let row = diff_check_row(
            key,
            "−",
            "removed",
            "removed — a static pack cannot delete keys (not recorded)",
            false,
            false,
        );
        list.append(&row.0);
        // Not added to `checks`: never selectable.
    }

    let scroll = ScrolledWindow::builder()
        .child(&list)
        .vexpand(true)
        .hexpand(true)
        .min_content_height(240)
        .build();
    area.append(&scroll);

    // Note row: unsigned-pack caveat (mirrors the post-save message; shown
    // up front here per the wizard spec).
    let note_row = GtkBox::new(Orientation::Horizontal, 8);
    note_row.set_margin_top(6);
    let unsigned_pill = Label::new(Some("unsigned"));
    unsigned_pill.add_css_class("row-pill");
    unsigned_pill.add_css_class("update");
    note_row.append(&unsigned_pill);
    let note_text = Label::builder()
        .label("Saved as a static-only pack to your per-user seeds dir.")
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    note_text.add_css_class("dim-label");
    note_text.add_css_class("caption");
    note_row.append(&note_text);
    area.append(&note_row);

    dialog.add_button("_Back", ResponseType::Cancel);
    let save_btn = dialog.add_button("_Save pack", ResponseType::Ok);
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

/// A diff row: a `+`/`~`/`−` marker + monospace key + checkbox, with a dim
/// monospace caption underneath showing the value preview (or
/// before → after). Used by the "Changed keys" list in
/// [`review_and_select`]. `marker_class` selects the marker's CSS color
/// (`added` = accent, `changed` = amber, `removed` = dim); `sensitive`
/// disables the checkbox for removed keys (a `static` pack cannot delete
/// keys, so those are display-only).
fn diff_check_row(
    key: &str,
    marker: &str,
    marker_class: &str,
    caption: &str,
    active: bool,
    sensitive: bool,
) -> (GtkBox, CheckButton) {
    let row = GtkBox::new(Orientation::Vertical, 2);
    row.add_css_class("diff-row");

    let head = GtkBox::new(Orientation::Horizontal, 6);
    let cb = CheckButton::builder().active(active).build();
    cb.set_sensitive(sensitive);
    head.append(&cb);

    let marker_lbl = Label::new(Some(marker));
    marker_lbl.add_css_class("diff-marker");
    marker_lbl.add_css_class(marker_class);
    marker_lbl.add_css_class("monospace");
    head.append(&marker_lbl);

    let key_lbl = Label::builder()
        .label(key)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    key_lbl.add_css_class("monospace");
    key_lbl.add_css_class("diff-key");
    if !sensitive {
        key_lbl.add_css_class("dim-label");
    }
    head.append(&key_lbl);
    row.append(&head);

    let cap = Label::builder()
        .label(caption)
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .margin_start(40)
        .build();
    cap.add_css_class("dim-label");
    cap.add_css_class("caption");
    cap.add_css_class("monospace");
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
