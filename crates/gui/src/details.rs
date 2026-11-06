//! Per-app detail page: where an app's updates come from, how its download was checked, and
//! each installed instance with Launch and Open folder. It never removes anything (OD-1):
//! removal stays on the card tiles and Apply.

use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use appimage_control_core::{
    app_details, launch_instance, AppDetails, AppPaths, CatalogApp, UpdateSource, Verification,
};
use gtk::{gio, Align, Button};

use crate::i18n::{fill, tr};

/// Everything the dialog shows, as text and button states (no widgets, so it is testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetailsView {
    pub title: String,
    pub latest_line: String,
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
    pub open_sensitive: bool,
    pub open_tooltip: Option<String>,
}

/// Build the dialog's content from `details` and the newest known release (`None` when not
/// known yet, e.g. offline).
pub fn details_view(details: &AppDetails, latest: Option<&str>) -> DetailsView {
    let latest_line = match latest.map(str::trim).filter(|l| !l.is_empty()) {
        Some(v) => v.trim_start_matches('v').to_string(),
        None => tr("Not known yet").to_string(),
    };
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
                open_sensitive: i.dir_present,
                open_tooltip: (!i.dir_present).then(|| tr("The folder is missing").to_string()),
            }
        })
        .collect();
    DetailsView {
        title: details.display_name.clone(),
        latest_line,
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

/// Show the detail dialog for `app` over `parent`. Reads the app's state now; Launch and
/// Open folder act on it. Launch failures show a toast whose button opens the full error in
/// the activity log; the dialog stays open.
pub fn present_details(
    parent: &adw::ApplicationWindow,
    paths: Rc<AppPaths>,
    app: &CatalogApp,
    latest: Option<&str>,
) {
    let view = details_view(&app_details(&paths, app), latest);
    // A modal window like Preferences: AdwDialog's content is not exposed to AT-SPI in this
    // libadwaita (1.9.1), which would leave screen readers and the smoke test with nothing.
    let dialog = adw::Window::builder()
        .title(view.title.as_str())
        .modal(true)
        .transient_for(parent)
        .default_width(600)
        .default_height(520)
        .build();
    let toasts = adw::ToastOverlay::new();
    let page = adw::PreferencesPage::new();

    let about = adw::PreferencesGroup::new();
    let latest_row = adw::ActionRow::builder()
        .title(tr("Latest release"))
        .subtitle(view.latest_line.as_str())
        .build();
    let source_row = adw::ActionRow::builder()
        .title(tr("Updates from"))
        .subtitle(view.source_line.as_str())
        .subtitle_selectable(true)
        .build();
    if let Some(url) = &view.source_detail {
        source_row.set_subtitle(&format!("{}\n{url}", view.source_line));
    }
    let verify_row = adw::ActionRow::builder()
        .title(tr("Download check"))
        .subtitle(view.verification_line.as_str())
        .build();
    about.add(&latest_row);
    about.add(&source_row);
    about.add(&verify_row);
    page.add(&about);

    let instances = adw::PreferencesGroup::builder()
        .title(tr("Instances"))
        .build();
    if let Some(text) = &view.empty_text {
        let row = adw::ActionRow::builder().title(text.as_str()).build();
        instances.add(&row);
    }
    for r in &view.rows {
        let row = adw::ActionRow::builder()
            .title(r.title.as_str())
            .subtitle(r.subtitle.as_str())
            .subtitle_selectable(true)
            .build();

        let launch = Button::with_label(tr("Launch"));
        launch.set_valign(Align::Center);
        launch.set_sensitive(r.launch_sensitive);
        launch.set_tooltip_text(r.launch_tooltip.as_deref());
        {
            let paths = Rc::clone(&paths);
            let app = app.clone();
            let index = r.index;
            let toasts = toasts.clone();
            let parent = parent.clone();
            launch.connect_clicked(move |_| {
                if let Err(e) = launch_instance(&paths, &app, index) {
                    let toast = adw::Toast::builder()
                        .title(fill(
                            // TRANSLATORS: {app} is an app name, {index} an instance number.
                            tr("Could not launch {app} #{index}"),
                            &[("app", &app.display_name_resolved()), ("index", &index)],
                        ))
                        .button_label(tr("Show log"))
                        .timeout(8)
                        .build();
                    let log = crate::status_text::apply_error_log_line(&app.id, &e.to_string());
                    let parent = parent.clone();
                    toast.connect_button_clicked(move |_| {
                        let w: gtk::ApplicationWindow = parent.clone().upcast();
                        crate::dialogs::present_activity_log_dialog(&w, &log);
                    });
                    toasts.add_toast(toast);
                }
            });
        }

        let open = Button::with_label(tr("Open folder"));
        open.set_valign(Align::Center);
        open.set_sensitive(r.open_sensitive);
        open.set_tooltip_text(r.open_tooltip.as_deref());
        {
            let dir = r.dir.clone();
            let parent = parent.clone();
            open.connect_clicked(move |_| {
                gtk::FileLauncher::new(Some(&gio::File::for_path(&dir))).launch(
                    Some(&parent),
                    gio::Cancellable::NONE,
                    |_| {},
                );
            });
        }

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.append(&launch);
        buttons.append(&open);
        row.add_suffix(&buttons);
        instances.add(&row);
    }
    page.add(&instances);

    toasts.set_child(Some(&page));
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&toasts));
    dialog.set_content(Some(&toolbar));
    dialog.present();
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
        let v = details_view(&details(vec![inst(1), inst(2)]), Some("v2.9.0"));
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
    fn details_no_instances_shows_the_empty_text() {
        let v = details_view(&details(vec![]), None);
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
        let v = details_view(&d, None);
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
        let v = details_view(&details(vec![no_dir, no_bin, no_launcher]), None);
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
        let v = details_view(&d, None);
        assert_eq!(v.source_line, "Pinned download");
        assert_eq!(v.source_detail.as_deref(), Some("https://e/p.AppImage"));
        d.source = UpdateSource::UnpinnedUrl {
            url: "https://e/u.AppImage".into(),
        };
        let v = details_view(&d, None);
        assert_eq!(v.source_line, "Download without a published checksum");
        assert_eq!(v.source_detail.as_deref(), Some("https://e/u.AppImage"));
        d.source = UpdateSource::NoDownload;
        assert_eq!(
            details_view(&d, None).source_line,
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
            let line = details_view(&d, None).verification_line;
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
