use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use appimage_control_core::SeedPreset;
use gtk::gdk::Texture;
use gtk::glib;
use gtk::{
    ApplicationWindow, Box as GtkBox, Button, CheckButton, Dialog, FileChooserAction,
    FileChooserNative, Image, Label, Orientation, ResponseType, ScrolledWindow, TextBuffer,
    TextTag, TextTagTable, TextView, ToggleButton, WrapMode,
};

/// Severity classes for `ask_confirmation`. Maps to libadwaita response appearance and
/// influences the heading/icon hint shown in the dialog body.
#[derive(Clone, Copy)]
pub enum ConfirmKind {
    /// Reversible-but-impactful: cleans installs, drops a cache.
    Warn,
    /// Permanently destructive: purges state, force-overwrites managed dirs.
    Danger,
    /// Constructive but worth confirming: legacy-dir migration.
    Info,
}

pub fn present_message_window(
    app: &impl IsA<gtk::Application>,
    title: &str,
    explanation: &str,
    detail: Option<&str>,
) {
    let w = ApplicationWindow::builder()
        .application(app)
        .title(title)
        .default_width(440)
        .default_height(220)
        .resizable(true)
        .build();

    let v = GtkBox::new(Orientation::Vertical, 6);
    v.set_hexpand(true);
    v.set_margin_top(24);
    v.set_margin_bottom(24);
    v.set_margin_start(24);
    v.set_margin_end(24);

    let heading = Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");

    let body = Label::builder()
        .label(explanation)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .xalign(0.0)
        .wrap(true)
        .justify(gtk::Justification::Left)
        .build();
    body.add_css_class("body");

    v.append(&heading);
    v.append(&body);
    if let Some(d) = detail {
        let extra = Label::builder()
            .label(d)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .xalign(0.0)
            .wrap(true)
            .justify(gtk::Justification::Left)
            .build();
        extra.add_css_class("dim-label");
        extra.add_css_class("caption");
        v.append(&extra);
    }

    w.set_child(Some(&v));
    w.present();
}

/// Modal confirmation dialog. Built on `AdwMessageDialog` (libadwaita ≥ 1.2; available
/// on the project's pinned 1.4 floor). `AdwAlertDialog` from 1.5 would be slightly nicer
/// but is intentionally avoided to keep Whonix/Bookworm compat.
pub fn ask_confirmation(
    parent: &ApplicationWindow,
    kind: ConfirmKind,
    title: &str,
    body: &str,
    detail_markup: Option<&str>,
    primary_label: &str,
) -> bool {
    let dialog = adw::MessageDialog::new(Some(parent), Some(title), Some(body));
    dialog.set_modal(true);
    dialog.set_default_width(440);

    if let Some(markup) = detail_markup {
        let detail = Label::builder()
            .label(markup)
            .use_markup(true)
            .halign(gtk::Align::Start)
            .xalign(0.0)
            .wrap(true)
            .justify(gtk::Justification::Left)
            .build();
        detail.add_css_class("dim-label");
        detail.add_css_class("caption");
        detail.set_margin_top(6);
        dialog.set_extra_child(Some(&detail));
    }

    dialog.add_response("cancel", "_Cancel");
    let primary_id = match kind {
        ConfirmKind::Danger => "destructive",
        ConfirmKind::Warn => "destructive",
        ConfirmKind::Info => "suggested",
    };
    dialog.add_response(primary_id, primary_label);
    let appearance = match kind {
        ConfirmKind::Danger | ConfirmKind::Warn => adw::ResponseAppearance::Destructive,
        ConfirmKind::Info => adw::ResponseAppearance::Suggested,
    };
    dialog.set_response_appearance(primary_id, appearance);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let decided: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));
    let decided_c = Rc::clone(&decided);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    let primary_id_owned = primary_id.to_string();
    dialog.connect_response(None, move |_d, r| {
        if r == primary_id_owned {
            *decided_c.borrow_mut() = true;
        }
        loop_c.quit();
    });
    dialog.present();
    loop_.run();
    let out = *decided.borrow();
    out
}

/// Modal activity-log dialog. Heading + 1-line subtitle on top, framed log body in the
/// middle (toolbar with status pill, line count, Wrap/Copy/Save controls; `TextView`
/// underneath with severity-colored line tags), single Close button at the bottom.
pub fn present_activity_log_dialog(parent: &ApplicationWindow, log_text: &str) {
    let summary = LogSummary::from(log_text);

    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Activity log")
        .default_width(680)
        .default_height(460)
        .resizable(true)
        .use_header_bar(0)
        .build();

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(0);
    area.set_hexpand(true);
    area.set_vexpand(true);

    // Header (icon + heading + subtitle)
    let header = GtkBox::new(Orientation::Horizontal, 12);
    header.set_margin_top(20);
    header.set_margin_bottom(0);
    header.set_margin_start(20);
    header.set_margin_end(20);

    let header_icon = Image::from_icon_name(summary.severity.header_icon_name());
    header_icon.set_pixel_size(28);
    header_icon.set_valign(gtk::Align::Start);

    let heading_box = GtkBox::new(Orientation::Vertical, 2);
    heading_box.set_hexpand(true);
    let heading = Label::builder()
        .label(summary.heading())
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .build();
    heading.add_css_class("title-2");
    let subtitle = Label::builder()
        .label(summary.subtitle())
        .halign(gtk::Align::Start)
        .xalign(0.0)
        .wrap(true)
        .build();
    subtitle.add_css_class("caption");
    subtitle.add_css_class("dim-label");
    heading_box.append(&heading);
    heading_box.append(&subtitle);

    header.append(&header_icon);
    header.append(&heading_box);
    area.append(&header);

    // Frame: toolbar + textview
    let frame_outer = GtkBox::new(Orientation::Vertical, 0);
    frame_outer.set_margin_top(12);
    frame_outer.set_margin_bottom(12);
    frame_outer.set_margin_start(20);
    frame_outer.set_margin_end(20);
    frame_outer.set_hexpand(true);
    frame_outer.set_vexpand(true);
    frame_outer.add_css_class("card");

    // Toolbar
    let toolbar = GtkBox::new(Orientation::Horizontal, 8);
    toolbar.set_margin_top(6);
    toolbar.set_margin_bottom(6);
    toolbar.set_margin_start(10);
    toolbar.set_margin_end(10);

    let pill = Label::new(Some(summary.severity.pill_label()));
    pill.add_css_class("row-pill");
    pill.add_css_class(summary.severity.pill_class());

    let counts = Label::new(Some(&format!(
        "{} line{} · {} error{}",
        summary.lines,
        if summary.lines == 1 { "" } else { "s" },
        summary.errors,
        if summary.errors == 1 { "" } else { "s" }
    )));
    counts.add_css_class("dim-label");
    counts.add_css_class("caption");

    let toolbar_left = GtkBox::new(Orientation::Horizontal, 8);
    toolbar_left.set_hexpand(true);
    toolbar_left.set_halign(gtk::Align::Start);
    toolbar_left.append(&pill);
    toolbar_left.append(&counts);

    let wrap_btn = ToggleButton::builder()
        .label("Wrap")
        .tooltip_text("Wrap long lines")
        .build();
    wrap_btn.add_css_class("flat");
    let copy_btn = Button::builder()
        .label("Copy")
        .tooltip_text("Copy log to clipboard")
        .build();
    copy_btn.add_css_class("flat");
    let save_btn = Button::builder()
        .label("Save…")
        .tooltip_text("Save log to a file")
        .build();
    save_btn.add_css_class("flat");

    let toolbar_right = GtkBox::new(Orientation::Horizontal, 6);
    toolbar_right.set_halign(gtk::Align::End);
    toolbar_right.append(&wrap_btn);
    toolbar_right.append(&copy_btn);
    toolbar_right.append(&save_btn);

    toolbar.append(&toolbar_left);
    toolbar.append(&toolbar_right);

    let toolbar_sep = gtk::Separator::new(Orientation::Horizontal);

    // TextView with weight-emphasis tags. We avoid foreground tinting because
    // libadwaita's `@success_color` / `@warning_color` / `@error_color` aren't
    // accessible from gtk-rs as RGBA at runtime, and hardcoded values would
    // misread under dark and high-contrast themes. The toolbar pill at the top
    // already conveys overall severity; per-line emphasis stays type-only.
    let tag_table = TextTagTable::new();
    let header_tag = TextTag::builder().name("header").weight(700).build();
    let warning_tag = TextTag::builder()
        .name("warning")
        .style(gtk::pango::Style::Italic)
        .build();
    let error_tag = TextTag::builder().name("error").weight(700).build();
    tag_table.add(&header_tag);
    tag_table.add(&warning_tag);
    tag_table.add(&error_tag);

    let buf = TextBuffer::new(Some(&tag_table));
    buf.set_text(log_text);
    apply_log_line_tags(&buf, log_text, &warning_tag, &error_tag, &header_tag);

    let tv = TextView::builder()
        .buffer(&buf)
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(WrapMode::None)
        .vexpand(true)
        .hexpand(true)
        .build();
    tv.add_css_class("monospace");
    tv.set_top_margin(12);
    tv.set_bottom_margin(12);
    tv.set_left_margin(12);
    tv.set_right_margin(12);

    {
        let tv_w = tv.clone();
        wrap_btn.connect_toggled(move |b| {
            tv_w.set_wrap_mode(if b.is_active() {
                WrapMode::WordChar
            } else {
                WrapMode::None
            });
        });
    }
    {
        let log_owned = log_text.to_string();
        let dialog_w = dialog.clone();
        copy_btn.connect_clicked(move |_| {
            let display = WidgetExt::display(&dialog_w);
            display.clipboard().set_text(&log_owned);
        });
    }
    {
        let log_owned = log_text.to_string();
        let dialog_w = dialog.clone();
        save_btn.connect_clicked(move |_| {
            let dialog_owner = dialog_w
                .clone()
                .upcast::<gtk::Window>()
                .downcast::<gtk::Window>()
                .ok();
            let chooser = FileChooserNative::builder()
                .title("Save activity log")
                .action(FileChooserAction::Save)
                .accept_label("Save")
                .cancel_label("Cancel")
                .modal(true)
                .build();
            if let Some(parent_win) = dialog_owner.as_ref() {
                chooser.set_transient_for(Some(parent_win));
            }
            let log_inner = log_owned.clone();
            chooser.connect_response(move |c, r| {
                if r == ResponseType::Accept {
                    if let Some(path) = c.file().and_then(|f| f.path()) {
                        let _ = std::fs::write(&path, &log_inner);
                    }
                }
                c.destroy();
            });
            chooser.show();
        });
    }

    let scroll = ScrolledWindow::builder()
        .child(&tv)
        .min_content_height(280)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();

    frame_outer.append(&toolbar);
    frame_outer.append(&toolbar_sep);
    frame_outer.append(&scroll);
    area.append(&frame_outer);

    let close_btn = dialog.add_button("_Close", ResponseType::Close);
    close_btn.add_css_class("suggested-action");
    close_btn.set_halign(gtk::Align::End);
    close_btn.set_margin_end(20);
    close_btn.set_margin_bottom(16);
    dialog.set_default_response(ResponseType::Close);
    dialog.connect_response(|d, _| {
        d.close();
    });
    dialog.present();
}

fn apply_log_line_tags(
    buf: &TextBuffer,
    text: &str,
    warning_tag: &TextTag,
    error_tag: &TextTag,
    header_tag: &TextTag,
) {
    let mut offset: i32 = 0;
    for line in text.split_inclusive('\n') {
        let len_chars = line.chars().count() as i32;
        let trimmed = line.trim_start();
        let lower = trimmed.to_lowercase();
        let tag = if lower.starts_with("error") || lower.contains("failed") {
            Some(error_tag)
        } else if trimmed.starts_with('!')
            || lower.starts_with("warning")
            || lower.starts_with("skipped")
        {
            Some(warning_tag)
        } else if trimmed.starts_with("── ") || trimmed.starts_with("==") {
            Some(header_tag)
        } else {
            None
        };
        if let Some(tag) = tag {
            let start = buf.iter_at_offset(offset);
            let end = buf.iter_at_offset(offset + len_chars);
            buf.apply_tag(tag, &start, &end);
        }
        offset += len_chars;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LogSeverity {
    Success,
    Warning,
    Error,
    Info,
}

impl LogSeverity {
    fn header_icon_name(self) -> &'static str {
        match self {
            LogSeverity::Success => "emblem-ok-symbolic",
            LogSeverity::Warning => "dialog-warning-symbolic",
            LogSeverity::Error => "dialog-error-symbolic",
            LogSeverity::Info => "dialog-information-symbolic",
        }
    }
    fn pill_label(self) -> &'static str {
        match self {
            LogSeverity::Success => "Success",
            LogSeverity::Warning => "Warnings",
            LogSeverity::Error => "Errors",
            LogSeverity::Info => "Info",
        }
    }
    fn pill_class(self) -> &'static str {
        match self {
            LogSeverity::Success => "success",
            LogSeverity::Warning => "update",
            LogSeverity::Error => "error",
            LogSeverity::Info => "pending",
        }
    }
}

struct LogSummary {
    severity: LogSeverity,
    lines: u32,
    errors: u32,
    warnings: u32,
    successes: u32,
}

impl LogSummary {
    fn from(text: &str) -> Self {
        let mut lines = 0u32;
        let mut errors = 0u32;
        let mut warnings = 0u32;
        let mut successes = 0u32;
        for raw in text.lines() {
            lines += 1;
            let line = raw.trim_start();
            if line.is_empty() {
                continue;
            }
            let lower = line.to_lowercase();
            if lower.starts_with("error") || lower.contains("failed") {
                errors += 1;
            } else if line.starts_with('!')
                || lower.starts_with("warning")
                || lower.starts_with("skipped")
            {
                warnings += 1;
            } else if line.starts_with('✓') || lower.starts_with("updated") || lower.starts_with("seeded") {
                successes += 1;
            }
        }
        let severity = if errors > 0 {
            LogSeverity::Error
        } else if warnings > 0 {
            LogSeverity::Warning
        } else if successes > 0 {
            LogSeverity::Success
        } else {
            LogSeverity::Info
        };
        Self {
            severity,
            lines,
            errors,
            warnings,
            successes,
        }
    }

    fn heading(&self) -> &'static str {
        match self.severity {
            LogSeverity::Error => "Activity log",
            LogSeverity::Warning => "Activity log",
            LogSeverity::Success => "Activity log",
            LogSeverity::Info => "Activity log",
        }
    }

    fn subtitle(&self) -> String {
        match self.severity {
            LogSeverity::Error => format!(
                "{} error{} · {} warning{} · {} success{}",
                self.errors,
                if self.errors == 1 { "" } else { "s" },
                self.warnings,
                if self.warnings == 1 { "" } else { "s" },
                self.successes,
                if self.successes == 1 { "" } else { "es" },
            ),
            LogSeverity::Warning => format!(
                "{} warning{} · {} success{}",
                self.warnings,
                if self.warnings == 1 { "" } else { "s" },
                self.successes,
                if self.successes == 1 { "" } else { "es" },
            ),
            LogSeverity::Success => format!(
                "{} successful step{}",
                self.successes,
                if self.successes == 1 { "" } else { "s" },
            ),
            LogSeverity::Info => format!(
                "{} log line{}",
                self.lines,
                if self.lines == 1 { "" } else { "s" },
            ),
        }
    }
}

fn choose_file(parent: &ApplicationWindow, title: &str, action: FileChooserAction) -> Option<PathBuf> {
    let chooser = FileChooserNative::builder()
        .title(title)
        .transient_for(parent)
        .action(action)
        .accept_label("Select")
        .cancel_label("Cancel")
        .modal(true)
        .build();
    let result: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(None));
    let result_c = Rc::clone(&result);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    chooser.connect_response(move |c, r| {
        if r == ResponseType::Accept {
            let path = c.file().and_then(|f| f.path());
            *result_c.borrow_mut() = path;
        }
        c.destroy();
        loop_c.quit();
    });
    chooser.show();
    loop_.run();
    let out = result.borrow().clone();
    out
}

pub fn choose_save_path(parent: &ApplicationWindow, title: &str) -> Option<PathBuf> {
    choose_file(parent, title, FileChooserAction::Save)
}

pub fn choose_open_path(parent: &ApplicationWindow, title: &str) -> Option<PathBuf> {
    choose_file(parent, title, FileChooserAction::Open)
}

/// Modal seed-settings dialog. Two `AdwPreferencesGroup` sections (preset radio rows,
/// per-app checkbox rows). Returns `(preset, selected_app_ids)` or `None` if cancelled.
pub fn choose_seed_settings(
    parent: &ApplicationWindow,
    installed_apps: &[(String, String, u32)],
    icons: &[Option<Vec<u8>>],
) -> Option<(SeedPreset, Vec<String>)> {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Seed Settings")
        .default_width(460)
        .default_height(520)
        .resizable(true)
        .use_header_bar(0)
        .build();

    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(0);
    area.set_hexpand(true);
    area.set_vexpand(true);

    // Heading block
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
    area.append(&header);

    // Body — clamped column with two preferences groups
    let body = GtkBox::new(Orientation::Vertical, 16);
    body.set_margin_top(16);
    body.set_margin_bottom(16);
    body.set_margin_start(20);
    body.set_margin_end(20);
    body.set_vexpand(true);

    // Preset group
    let preset_group = adw::PreferencesGroup::new();
    preset_group.set_title("Preset");

    let ubuntu_check = CheckButton::new();
    ubuntu_check.set_active(true);
    let ubuntu_row = adw::ActionRow::new();
    ubuntu_row.set_title("Ubuntu");
    ubuntu_row.set_subtitle("Local nodes, no Tor by default.");
    ubuntu_row.add_prefix(&ubuntu_check);
    ubuntu_row.set_activatable_widget(Some(&ubuntu_check));
    preset_group.add(&ubuntu_row);

    let whonix_check = CheckButton::new();
    whonix_check.set_group(Some(&ubuntu_check));
    let whonix_row = adw::ActionRow::new();
    whonix_row.set_title("Whonix");
    whonix_row.set_subtitle("Tor-only; bind to Whonix gateway ports.");
    whonix_row.add_prefix(&whonix_check);
    whonix_row.set_activatable_widget(Some(&whonix_check));
    preset_group.add(&whonix_row);

    body.append(&preset_group);

    // Apps group
    let apps_group = adw::PreferencesGroup::new();
    apps_group.set_title("Installed supported apps");

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

        apps_group.add(&row);
        checks.push((app_id.clone(), cb));
    }

    body.append(&apps_group);

    let scroll = ScrolledWindow::builder()
        .child(&body)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    area.append(&scroll);

    // Action area
    let cancel_widget = dialog.add_button("_Cancel", ResponseType::Cancel);
    let seed_widget = dialog.add_button("Seed 0 apps", ResponseType::Ok);
    let seed_btn: Button = seed_widget
        .downcast::<Button>()
        .expect("Dialog::add_button returns a Button under the hood");
    seed_btn.add_css_class("suggested-action");
    seed_btn.set_sensitive(false);
    cancel_widget.set_margin_bottom(16);
    cancel_widget.set_margin_end(0);
    seed_btn.set_margin_bottom(16);
    seed_btn.set_margin_end(20);
    dialog.set_default_response(ResponseType::Ok);

    let seed_btn_clone = seed_btn.clone();
    let checks_for_label = checks.clone();
    let recompute_label: Rc<dyn Fn()> = Rc::new(move || {
        let n = checks_for_label
            .iter()
            .filter(|(_, cb)| cb.is_active())
            .count();
        seed_btn_clone.set_label(&format!(
            "Seed {n} app{}",
            if n == 1 { "" } else { "s" }
        ));
        seed_btn_clone.set_sensitive(n > 0);
    });
    for (_, cb) in &checks {
        let r = Rc::clone(&recompute_label);
        cb.connect_toggled(move |_| (r)());
    }
    (recompute_label)();

    type Decision = Rc<RefCell<Option<(SeedPreset, Vec<String>)>>>;
    let decided: Decision = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    let checks_for_resp = checks;
    dialog.connect_response(move |d, r| {
        if r == ResponseType::Ok {
            let preset = if whonix_check.is_active() {
                SeedPreset::Whonix
            } else {
                SeedPreset::Ubuntu
            };
            let selected: Vec<String> = checks_for_resp
                .iter()
                .filter_map(|(app_id, cb)| cb.is_active().then_some(app_id.clone()))
                .collect();
            if !selected.is_empty() {
                *decided_c.borrow_mut() = Some((preset, selected));
            }
        }
        d.close();
        loop_c.quit();
    });
    dialog.present();
    loop_.run();
    let out = decided.borrow().clone();
    out
}
