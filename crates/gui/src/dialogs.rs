use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use appimage_control_core::SeedPreset;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    ApplicationWindow, Box as GtkBox, CheckButton, Dialog, FileChooserAction, FileChooserNative,
    Label, Orientation, ResponseType, ScrolledWindow, TextBuffer, TextView, WrapMode,
};

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

    let v = GtkBox::new(Orientation::Vertical, 4);
    v.set_hexpand(true);
    v.set_margin_top(8);
    v.set_margin_bottom(8);
    v.set_margin_start(8);
    v.set_margin_end(8);

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
        .justify(gtk::Justification::Fill)
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
            .justify(gtk::Justification::Fill)
            .build();
        extra.add_css_class("dim-label");
        v.append(&extra);
    }

    w.set_child(Some(&v));
    w.present();
}

pub fn present_activity_log_dialog(parent: &ApplicationWindow, log_text: &str) {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Activity log")
        .default_width(680)
        .default_height(460)
        .resizable(true)
        .build();
    let area = dialog.content_area();
    area.set_orientation(Orientation::Vertical);
    area.set_spacing(0);
    area.set_margin_top(0);
    area.set_margin_bottom(0);
    area.set_margin_start(0);
    area.set_margin_end(0);
    area.set_hexpand(true);
    area.set_vexpand(true);

    let outer = GtkBox::new(Orientation::Vertical, 12);
    outer.set_hexpand(true);
    outer.set_vexpand(true);
    outer.set_margin_top(24);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);

    let buf = TextBuffer::new(None);
    buf.set_text(log_text);
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
    let scroll = ScrolledWindow::builder()
        .child(&tv)
        .min_content_height(280)
        .vexpand(true)
        .hexpand(true)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();
    outer.append(&scroll);
    area.append(&outer);
    area.set_spacing(12);

    let close_btn = dialog.add_button("_Close", ResponseType::Close);
    close_btn.set_halign(gtk::Align::End);
    close_btn.set_margin_end(12);
    close_btn.set_margin_bottom(12);
    dialog.set_default_response(ResponseType::Close);
    dialog.connect_response(|d, _| {
        d.close();
    });
    dialog.present();
}

pub fn ask_confirmation(parent: &ApplicationWindow, title: &str, body: &str) -> bool {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title(title)
        .default_width(420)
        .build();
    dialog.add_button("_Cancel", ResponseType::Cancel);
    dialog.add_button("_Proceed", ResponseType::Ok);
    let area = dialog.content_area();
    let lbl = Label::new(Some(body));
    lbl.set_wrap(true);
    lbl.set_xalign(0.0);
    area.append(&lbl);

    let decided: Rc<RefCell<Option<bool>>> = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(move |d, r| {
        *decided_c.borrow_mut() = Some(r == ResponseType::Ok);
        d.close();
        loop_c.quit();
    });
    dialog.present();
    loop_.run();
    let out = decided.borrow().unwrap_or(false);
    out
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

pub fn choose_seed_settings(
    parent: &ApplicationWindow,
    installed_apps: &[(String, String, u32)],
) -> Option<(SeedPreset, Vec<String>)> {
    let dialog = Dialog::builder()
        .transient_for(parent)
        .modal(true)
        .title("Seed Settings")
        .default_width(420)
        .build();
    dialog.add_button("_Cancel", ResponseType::Cancel);
    dialog.add_button("_Seed", ResponseType::Ok);

    let area = dialog.content_area();
    area.set_spacing(12);
    area.set_margin_top(16);
    area.set_margin_bottom(16);
    area.set_margin_start(16);
    area.set_margin_end(16);

    let preset_label = Label::new(Some("Preset"));
    preset_label.set_xalign(0.0);
    let ubuntu = CheckButton::with_label("Ubuntu");
    ubuntu.set_active(true);
    let whonix = CheckButton::with_label("Whonix");
    whonix.set_group(Some(&ubuntu));

    let preset_box = GtkBox::new(Orientation::Vertical, 6);
    preset_box.append(&preset_label);
    preset_box.append(&ubuntu);
    preset_box.append(&whonix);
    area.append(&preset_box);

    let apps_label = Label::new(Some("Installed supported apps"));
    apps_label.set_xalign(0.0);
    area.append(&apps_label);

    let list_box = GtkBox::new(Orientation::Vertical, 6);
    let mut checks: Vec<(String, CheckButton)> = Vec::new();
    for (app_id, display_name, count) in installed_apps {
        let cb = CheckButton::with_label(&format!("{display_name} ({count} installed)"));
        cb.set_active(true);
        list_box.append(&cb);
        checks.push((app_id.clone(), cb));
    }
    let scroll = ScrolledWindow::builder()
        .child(&list_box)
        .min_content_height(120)
        .max_content_height(240)
        .build();
    area.append(&scroll);

    let decided: Rc<RefCell<Option<(SeedPreset, Vec<String>)>>> = Rc::new(RefCell::new(None));
    let decided_c = Rc::clone(&decided);
    let loop_ = glib::MainLoop::new(None, false);
    let loop_c = loop_.clone();
    dialog.connect_response(move |d, r| {
        if r == ResponseType::Ok {
            let preset = if whonix.is_active() {
                SeedPreset::Whonix
            } else {
                SeedPreset::Ubuntu
            };
            let selected: Vec<String> = checks
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
