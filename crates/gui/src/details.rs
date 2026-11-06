//! Per-app detail page: where an app's updates come from, how its download was checked, and
//! each installed instance with Launch and Open folder. It never removes anything (OD-1):
//! removal stays on the card tiles and Apply.

use std::path::PathBuf;

use appimage_control_core::{AppDetails, UpdateSource, Verification};

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
                Some(tr("The launcher is missing — use Repair launchers").to_string())
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
            .contains("Repair launchers"));
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
