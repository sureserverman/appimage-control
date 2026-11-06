//! Per-app detail page: where an app's updates come from, how its download was checked, and
//! each installed instance with Launch and Open folder. It never removes anything (OD-1):
//! removal stays on the card tiles and Apply.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use appimage_control_core::{
    app_details, launch_instance, AppDetails, AppPaths, CatalogApp, UpdateSource, Verification,
};
use gtk::{gio, glib, Align, Button};

use crate::i18n::{fill, tr};

/// Everything the dialog shows, as text and button states (no widgets, so it is testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsView {
    pub title: String,
    pub latest_line: String,
    /// Said under the latest release when GitHub could not be asked, so the version shown is
    /// only the newest one the catalog or the installs know.
    pub latest_note: Option<String>,
    pub source_line: String,
    /// The URL for a non-GitHub source, shown under the source line.
    pub source_detail: Option<String>,
    pub verification_line: String,
    /// Shown instead of the instance list: nothing installed, or the folders could not be read.
    pub empty_text: Option<String>,
    pub rows: Vec<InstanceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceRow {
    pub index: u32,
    pub title: String,
    pub subtitle: String,
    pub dir: PathBuf,
    pub launch_sensitive: bool,
    pub launch_tooltip: Option<String>,
    /// Accessible name of the Launch button ("Launch instance #2"): rows repeat the label.
    pub launch_name: String,
    pub open_sensitive: bool,
    pub open_tooltip: Option<String>,
    pub open_name: String,
}

/// Build the dialog's content from `details`, the newest release known (the card's "Latest"
/// label: the GitHub tag, the catalog version or an install's version, whichever is newest;
/// `None` when none is known) and whether GitHub answered for this app.
pub fn details_view(
    details: &AppDetails,
    latest: Option<&str>,
    github_answered: bool,
) -> DetailsView {
    let latest_line = match latest.map(str::trim).filter(|l| !l.is_empty() && *l != "—") {
        // A leading "v" is dropped only before a digit ("v2.8.0", not "vim-9").
        Some(v) => match v.strip_prefix('v') {
            Some(rest) if rest.starts_with(|c: char| c.is_ascii_digit()) => rest.to_string(),
            _ => v.to_string(),
        },
        None => tr("Not known yet").to_string(),
    };
    let latest_note = (matches!(details.source, UpdateSource::GithubReleases { .. })
        && !github_answered)
        .then(|| tr("GitHub could not be checked, so newer releases may exist").to_string());
    let (source_line, source_detail) = match &details.source {
        UpdateSource::GithubReleases { owner, repo } => (
            // TRANSLATORS: {repo} is a GitHub project, like "keepassxreboot/keepassxc".
            fill(
                tr("GitHub releases: {repo}"),
                &[("repo", &format!("{owner}/{repo}"))],
            ),
            None,
        ),
        UpdateSource::PinnedUrl { url } => (tr("Pinned download").to_string(), Some(url.clone())),
        UpdateSource::UnpinnedUrl { url } => (
            tr("Download without a published checksum").to_string(),
            Some(url.clone()),
        ),
        UpdateSource::NoDownload => (tr("No download for this system").to_string(), None),
    };
    let verification_line = verification_text(&details.cache_verification).to_string();
    let empty_text = if let Some(err) = &details.listing_error {
        Some(fill(
            tr("Could not read the install folders: {error}"),
            &[("error", err)],
        ))
    } else if details.instances.is_empty() {
        Some(tr("No instances installed — add one on the card").to_string())
    } else {
        None
    };
    let rows = details
        .instances
        .iter()
        .map(|i| {
            let version = match &i.version {
                // TRANSLATORS: {version} is an app version, like "2.8.1".
                Some(v) => fill(tr("Version {version}"), &[("version", v)]),
                None => tr("Version unknown").to_string(),
            };
            let mut parts = vec![version];
            if let Some(sz) = i.size {
                parts.push(crate::format_size(sz));
            }
            parts.push(i.dir.display().to_string());
            let launch_tooltip = if !i.appimage_present {
                Some(tr("The AppImage is missing — reinstall this instance").to_string())
            } else if !i.launcher_present {
                Some(tr("The launcher is missing — use Repair Launchers").to_string())
            } else {
                None
            };
            InstanceRow {
                index: i.index,
                // TRANSLATORS: an instance of an app, like "#2".
                title: fill(tr("Instance #{index}"), &[("index", &i.index)]),
                subtitle: parts.join("  ·  "),
                dir: i.dir.clone(),
                launch_sensitive: launch_tooltip.is_none(),
                launch_tooltip,
                // TRANSLATORS: accessible name of a Launch button; {index} is an instance number.
                launch_name: fill(tr("Launch instance #{index}"), &[("index", &i.index)]),
                open_sensitive: i.dir_present,
                open_tooltip: (!i.dir_present).then(|| tr("The folder is missing").to_string()),
                // TRANSLATORS: accessible name of an Open folder button.
                open_name: fill(
                    tr("Open the folder of instance #{index}"),
                    &[("index", &i.index)],
                ),
            }
        })
        .collect();
    DetailsView {
        title: details.display_name.clone(),
        latest_line,
        latest_note,
        source_line,
        source_detail,
        verification_line,
        empty_text,
        rows,
    }
}

/// Plain words for how the cached download was checked. Only the two verified states say
/// "Verified".
pub fn verification_text(v: &Verification) -> &'static str {
    match v {
        Verification::CatalogHash => tr("Verified — matches the checksum in the catalog"),
        Verification::GithubDigest => tr("Verified — matches the checksum GitHub publishes"),
        Verification::UnverifiedAllowed => tr("Not verified — installed with your permission"),
        Verification::Unreadable => tr("Unknown — the checksum record cannot be read"),
        Verification::NotChecked => tr("Not checked yet"),
    }
}

/// Show the detail window for `app` over `parent`. Reads the app's state now; Launch and
/// Open folder act on it. A failed action shows a toast; Launch's toast has a button that
/// opens the full error in the activity log. The window stays open.
pub fn present_details(
    parent: &adw::ApplicationWindow,
    paths: Rc<AppPaths>,
    app: &CatalogApp,
    latest: Option<&str>,
    github_answered: bool,
) {
    let view = details_view(&app_details(&paths, app), latest, github_answered);
    // A modal window like Preferences: AdwDialog's content is not exposed to AT-SPI in this
    // libadwaita (1.9.1), which would leave screen readers and the smoke test with nothing.
    let window = adw::Window::builder()
        .title(view.title.as_str())
        .modal(true)
        .transient_for(parent)
        .default_width(600)
        .default_height(520)
        .build();
    // Escape closes it, as it would an AdwDialog.
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    let toasts = adw::ToastOverlay::new();
    let page = adw::PreferencesPage::new();

    // Plain text everywhere: paths, versions and errors may hold `&` or `<`.
    let row = |title: &str, subtitle: &str| {
        adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .use_markup(false)
            .subtitle_selectable(true)
            .build()
    };
    let about = adw::PreferencesGroup::new();
    let latest_sub = match &view.latest_note {
        Some(note) => format!("{}\n{note}", view.latest_line),
        None => view.latest_line.clone(),
    };
    about.add(&row(tr("Latest release"), &latest_sub));
    let source_sub = match &view.source_detail {
        Some(url) => format!("{}\n{url}", view.source_line),
        None => view.source_line.clone(),
    };
    about.add(&row(tr("Updates from"), &source_sub));
    about.add(&row(tr("Download check"), &view.verification_line));
    page.add(&about);

    let instances = adw::PreferencesGroup::builder()
        .title(tr("Instances"))
        .build();
    if let Some(text) = &view.empty_text {
        instances.add(&row(text, ""));
    }
    let toast = |toasts: &adw::ToastOverlay, title: &str| {
        let t = adw::Toast::builder()
            .title(title)
            .use_markup(false)
            .timeout(6)
            .build();
        toasts.add_toast(t.clone());
        t
    };
    for r in &view.rows {
        let action_row = row(&r.title, &r.subtitle);

        let launch = Button::with_label(tr("Launch"));
        launch.set_valign(Align::Center);
        launch.set_sensitive(r.launch_sensitive);
        launch.set_tooltip_text(r.launch_tooltip.as_deref());
        // A labelled button is named by its label child; drop that link so the
        // per-instance name is what assistive technology reads.
        launch.reset_relation(gtk::AccessibleRelation::LabelledBy);
        launch.update_property(&[gtk::accessible::Property::Label(&r.launch_name)]);
        {
            let paths = Rc::clone(&paths);
            let app = app.clone();
            let index = r.index;
            let toasts = toasts.downgrade();
            let window = window.downgrade();
            launch.connect_clicked(move |btn| {
                let (Some(toasts), Some(window)) = (toasts.upgrade(), window.upgrade()) else {
                    return;
                };
                let name = app.display_name_resolved();
                match launch_instance(&paths, &app, index) {
                    Ok(()) => {
                        // TRANSLATORS: {app} is an app name, {index} an instance number.
                        toast(
                            &toasts,
                            &fill(
                                tr("Starting {app} #{index}…"),
                                &[("app", &name), ("index", &index)],
                            ),
                        );
                        // A slow start invites a second click; hold the button briefly.
                        btn.set_sensitive(false);
                        let btn = btn.downgrade();
                        glib::timeout_add_seconds_local_once(3, move || {
                            if let Some(b) = btn.upgrade() {
                                b.set_sensitive(true);
                            }
                        });
                    }
                    Err(e) => {
                        let t = toast(
                            &toasts,
                            &fill(
                                tr("Could not launch {app} #{index}"),
                                &[("app", &name), ("index", &index)],
                            ),
                        );
                        t.set_timeout(10);
                        t.set_button_label(Some(tr("Show log")));
                        // The activity log counts lines that start with "error".
                        let log = format!("error: {e}");
                        let window = window.downgrade();
                        t.connect_button_clicked(move |_| {
                            if let Some(w) = window.upgrade() {
                                crate::dialogs::present_activity_log_dialog(&w, &log);
                            }
                        });
                    }
                }
            });
        }

        let open = Button::with_label(tr("Open folder"));
        open.set_valign(Align::Center);
        open.set_sensitive(r.open_sensitive);
        open.set_tooltip_text(r.open_tooltip.as_deref());
        open.reset_relation(gtk::AccessibleRelation::LabelledBy);
        open.update_property(&[gtk::accessible::Property::Label(&r.open_name)]);
        {
            let dir = r.dir.clone();
            let toasts = toasts.downgrade();
            let window = window.downgrade();
            open.connect_clicked(move |_| {
                let Some(w) = window.upgrade() else {
                    return;
                };
                let toasts = toasts.clone();
                gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
                    Some(&w),
                    gio::Cancellable::NONE,
                    move |res| {
                        let dismissed = matches!(
                            &res,
                            Err(e) if e.matches(gtk::DialogError::Dismissed)
                                || e.matches(gtk::DialogError::Cancelled)
                        );
                        if let (Err(e), false, Some(t)) = (res, dismissed, toasts.upgrade()) {
                            toast(
                                &t,
                                &fill(
                                    tr("Could not open the folder: {error}"),
                                    &[("error", &e.message())],
                                ),
                            );
                        }
                    },
                );
            });
        }

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.append(&launch);
        buttons.append(&open);
        action_row.add_suffix(&buttons);
        instances.add(&action_row);
    }
    page.add(&instances);

    toasts.set_child(Some(&page));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&toasts));
    window.set_content(Some(&toolbar));
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use appimage_control_core::InstanceDetails;

    fn inst(index: u32) -> InstanceDetails {
        InstanceDetails {
            index,
            dir: PathBuf::from(format!("/h/apps/demo/{index}")),
            dir_present: true,
            appimage: PathBuf::from(format!("/h/apps/demo/{index}/demo.AppImage")),
            appimage_present: true,
            launcher: PathBuf::from(format!("/h/apps/appimage-control-demo-{index}.desktop")),
            launcher_present: true,
            version: Some("2.8.1".into()),
            size: Some(38_000_000),
        }
    }

    fn details(instances: Vec<InstanceDetails>) -> AppDetails {
        AppDetails {
            app_id: "demo".into(),
            display_name: "Demo App".into(),
            instances,
            source: UpdateSource::GithubReleases {
                owner: "o".into(),
                repo: "r".into(),
            },
            cache_verification: Verification::CatalogHash,
            listing_error: None,
        }
    }

    #[test]
    fn details_two_instances() {
        let v = details_view(&details(vec![inst(1), inst(2)]), Some("v2.9.0"), true);
        assert_eq!(v.title, "Demo App");
        assert_eq!(v.latest_line, "2.9.0");
        assert_eq!(v.source_line, "GitHub releases: o/r");
        assert_eq!(v.source_detail, None);
        assert_eq!(v.empty_text, None);
        assert_eq!(v.rows.len(), 2);
        let r = &v.rows[1];
        assert_eq!(r.title, "Instance #2");
        assert_eq!(r.subtitle, "Version 2.8.1  ·  38 MB  ·  /h/apps/demo/2");
        assert!(r.launch_sensitive && r.open_sensitive);
        assert_eq!(
            (r.launch_tooltip.as_ref(), r.open_tooltip.as_ref()),
            (None, None)
        );
    }

    #[test]
    fn details_latest_release_falls_back_and_says_when_github_was_not_asked() {
        // The card's label (catalog or install version) is shown, never "Not known yet".
        let v = details_view(&details(vec![inst(1)]), Some("2.7.12"), false);
        assert_eq!(v.latest_line, "2.7.12");
        assert_eq!(
            v.latest_note.as_deref(),
            Some("GitHub could not be checked, so newer releases may exist")
        );
        let v = details_view(&details(vec![inst(1)]), Some("2.8.0"), true);
        assert_eq!((v.latest_line.as_str(), v.latest_note), ("2.8.0", None));
        // The card's "no version" dash and blanks read "Not known yet".
        for none in [None, Some("—"), Some("  ")] {
            assert_eq!(
                details_view(&details(vec![]), none, true).latest_line,
                "Not known yet"
            );
        }
        // A leading "v" is dropped only before a digit.
        assert_eq!(
            details_view(&details(vec![]), Some("vim-9"), true).latest_line,
            "vim-9"
        );
        assert_eq!(
            details_view(&details(vec![]), Some("v1.2"), true).latest_line,
            "1.2"
        );
        // A non-GitHub source never claims GitHub was skipped.
        let mut d = details(vec![]);
        d.source = UpdateSource::PinnedUrl {
            url: "https://e/p-2.0.0.AppImage".into(),
        };
        assert_eq!(details_view(&d, Some("2.0.0"), false).latest_note, None);
    }

    #[test]
    fn details_row_buttons_have_distinct_accessible_names() {
        let v = details_view(&details(vec![inst(1), inst(2)]), None, true);
        assert_eq!(v.rows[1].launch_name, "Launch instance #2");
        assert_eq!(v.rows[1].open_name, "Open the folder of instance #2");
        assert_ne!(v.rows[0].launch_name, v.rows[1].launch_name);
    }

    #[test]
    fn details_no_instances_shows_the_empty_text() {
        let v = details_view(&details(vec![]), None, true);
        assert!(v.rows.is_empty());
        assert_eq!(
            v.empty_text.as_deref(),
            Some("No instances installed — add one on the card")
        );
        assert_eq!(v.latest_line, "Not known yet");
        // Source and verification are still shown.
        assert_eq!(v.source_line, "GitHub releases: o/r");
        assert!(v.verification_line.starts_with("Verified"));
    }

    #[test]
    fn details_listing_error_is_not_the_empty_state() {
        let mut d = details(vec![]);
        d.listing_error = Some("permission denied".into());
        let v = details_view(&d, None, true);
        assert_eq!(
            v.empty_text.as_deref(),
            Some("Could not read the install folders: permission denied")
        );
    }

    #[test]
    fn details_missing_pieces_disable_their_buttons() {
        let mut no_dir = inst(1);
        no_dir.dir_present = false;
        let mut no_bin = inst(2);
        no_bin.appimage_present = false;
        no_bin.size = None;
        no_bin.version = None;
        let mut no_launcher = inst(3);
        no_launcher.launcher_present = false;
        let v = details_view(&details(vec![no_dir, no_bin, no_launcher]), None, true);
        assert!(!v.rows[0].open_sensitive);
        assert_eq!(
            v.rows[0].open_tooltip.as_deref(),
            Some("The folder is missing")
        );
        assert!(!v.rows[1].launch_sensitive);
        assert!(v.rows[1]
            .launch_tooltip
            .as_deref()
            .unwrap()
            .contains("AppImage is missing"));
        assert_eq!(v.rows[1].subtitle, "Version unknown  ·  /h/apps/demo/2");
        assert!(v.rows[1].open_sensitive);
        assert!(!v.rows[2].launch_sensitive);
        assert!(v.rows[2]
            .launch_tooltip
            .as_deref()
            .unwrap()
            .contains("Repair Launchers"));
    }

    #[test]
    fn details_each_update_source() {
        let mut d = details(vec![]);
        d.source = UpdateSource::PinnedUrl {
            url: "https://e/p.AppImage".into(),
        };
        let v = details_view(&d, None, true);
        assert_eq!(v.source_line, "Pinned download");
        assert_eq!(v.source_detail.as_deref(), Some("https://e/p.AppImage"));
        d.source = UpdateSource::UnpinnedUrl {
            url: "https://e/u.AppImage".into(),
        };
        let v = details_view(&d, None, true);
        assert_eq!(v.source_line, "Download without a published checksum");
        assert_eq!(v.source_detail.as_deref(), Some("https://e/u.AppImage"));
        d.source = UpdateSource::NoDownload;
        assert_eq!(
            details_view(&d, None, true).source_line,
            "No download for this system"
        );
    }

    #[test]
    fn details_no_unverified_state_reads_as_verified() {
        for (v, verified) in [
            (Verification::CatalogHash, true),
            (Verification::GithubDigest, true),
            (Verification::UnverifiedAllowed, false),
            (Verification::Unreadable, false),
            (Verification::NotChecked, false),
        ] {
            let mut d = details(vec![]);
            d.cache_verification = v.clone();
            let line = details_view(&d, None, true).verification_line;
            assert_eq!(line.starts_with("Verified"), verified, "{v:?}: {line}");
        }
        assert_eq!(
            verification_text(&Verification::UnverifiedAllowed),
            "Not verified — installed with your permission"
        );
        assert_eq!(
            verification_text(&Verification::NotChecked),
            "Not checked yet"
        );
    }
}
