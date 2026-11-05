//! User-visible one-line summaries and row hints (unit-testable without widgets).

use appimage_control_core::{CatalogApp, UpdateAllReport};

pub fn format_update_status_summary(rep: &UpdateAllReport) -> String {
    let total = rep.updated + rep.unchanged + rep.skipped_up_to_date + rep.errors;
    if total == 0 {
        return "No installed instances to update.".to_string();
    }
    if rep.errors > 0 {
        return format!(
            "{} failed, {} updated, {} unchanged, {} skipped as already catalog (see log).",
            rep.errors, rep.updated, rep.unchanged, rep.skipped_up_to_date
        );
    }
    if rep.updated > 0 {
        let mut parts = vec![format!("Updated {} instance(s)", rep.updated)];
        if rep.unchanged > 0 {
            parts.push(format!("{} unchanged after update", rep.unchanged));
        }
        if rep.skipped_up_to_date > 0 {
            parts.push(format!(
                "{} skipped (already catalog)",
                rep.skipped_up_to_date
            ));
        }
        return format!("{}.", parts.join("; "));
    }
    if rep.skipped_up_to_date > 0 && rep.unchanged > 0 {
        return format!(
            "{} skipped (already catalog); {} checked with no binary change.",
            rep.skipped_up_to_date, rep.unchanged
        );
    }
    if rep.skipped_up_to_date > 0 {
        return format!(
            "All {} instance(s) already matched the catalog (nothing to fetch).",
            rep.skipped_up_to_date
        );
    }
    format!(
        "Checked {} instance(s); no AppImage replacement was needed.",
        rep.unchanged
    )
}

pub fn format_seed_status_summary(seeded: u32, failed: u32) -> String {
    if failed > 0 {
        format!("{failed} seed(s) failed, {seeded} completed.")
    } else if seeded > 0 {
        format!("Seeded {seeded} instance(s).")
    } else {
        "No supported installed instances to seed.".to_string()
    }
}

/// One Apply failure as written to the activity log. The leading `error:` is what the log's
/// summary counts; the app id is not repeated when the message already starts with it.
pub fn apply_error_log_line(app_id: &str, err: &str) -> String {
    if err.starts_with(&format!("{app_id}: ")) {
        format!("error: {err}")
    } else {
        format!("error: {app_id}: {err}")
    }
}

/// Status-bar text after an Apply run: `failed` apps whose reconcile returned an error (a refused
/// download included), `not_seeded` instances installed but not seeded.
pub fn format_apply_status(failed: u32, not_seeded: u32) -> String {
    let apps = |n: u32| format!("{n} app{} failed", if n == 1 { "" } else { "s" });
    match (failed, not_seeded) {
        (0, 0) => "Applied.".to_string(),
        (0, n) => format!("Applied — {n} instance(s) NOT seeded (see activity log)"),
        (f, 0) => format!("Applied — {} (see the activity log)", apps(f)),
        (f, n) => format!(
            "Applied — {}, {n} instance(s) NOT seeded (see the activity log)",
            apps(f)
        ),
    }
}

/// Bottom-bar lines when nothing is pending: after an Apply with failures, say so rather than
/// "All changes applied" (the failed targets were dropped back to the installed counts).
pub fn nothing_pending_summary(last_apply_failed: u32) -> (String, String) {
    if last_apply_failed == 0 {
        (
            "All changes applied".to_string(),
            "Nothing pending — you're up to date".to_string(),
        )
    } else {
        (
            format!(
                "Last Apply: {last_apply_failed} app{} not installed",
                if last_apply_failed == 1 { "" } else { "s" }
            ),
            "See the activity log for why".to_string(),
        )
    }
}

pub fn catalog_row_hint(app: &CatalogApp) -> Option<String> {
    let has_any_url = app.has_any_download_url();
    let can_install = app.resolved_download_url().is_ok();
    if !has_any_url {
        Some("Add a download_url in the catalog to enable installs.".to_string())
    } else if !can_install {
        Some(format!(
            "No AppImage URL for this system ({}) — installs are disabled for this app.",
            std::env::consts::ARCH
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_summary_empty_work_queue() {
        let s = format_update_status_summary(&UpdateAllReport::default());
        assert_eq!(s, "No installed instances to update.");
    }

    #[test]
    fn update_summary_errors_first() {
        let s = format_update_status_summary(&UpdateAllReport {
            updated: 0,
            unchanged: 1,
            skipped_up_to_date: 0,
            errors: 2,
            unverified_apps: vec![],
        });
        assert!(s.contains("failed"));
        assert!(s.contains("2"));
    }

    #[test]
    fn seed_summary_happy_path() {
        assert_eq!(
            format_seed_status_summary(3, 0).as_str(),
            "Seeded 3 instance(s)."
        );
    }

    #[test]
    fn catalog_row_hint_no_url() {
        let app = appimage_control_core::CatalogApp {
            id: "x".into(),
            display_name: None,
            homepage: None,
            download_url: None,
            download_url_aarch64: None,
            appimage_filename: None,
            instance_strategy: appimage_control_core::InstanceStrategy::None,
            instance_exec_args_template: None,
            instance_name_template: None,
            desktop_name_template: None,
            reset_config_dirs: vec![],
            reset_cache_dirs: vec![],
            reset_data_dirs: vec![],
            icon_url: None,
            wayland_app_id: None,
            desktop_pass_file_urls: None,
            instance_subdir: None,
            isolate_dbus_session: None,
            max_instances: None,
            max_instances_tooltip: None,
            category: None,
            sha256: None,
            sha256_aarch64: None,
        };
        assert!(catalog_row_hint(&app).unwrap().contains("download_url"));
    }

    #[test]
    fn apply_failure_log_line_counts_as_an_error() {
        // The activity log counts a line as an error when it starts with "error".
        let l = super::apply_error_log_line(
            "demo-a",
            "demo-a: download does not match the expected sha256 (expected a, got b); nothing was installed",
        );
        assert!(l.starts_with("error: "), "{l}");
        assert!(!l.contains("demo-a: demo-a:"), "app id not repeated: {l}");
        let l = super::apply_error_log_line("jan", "download failed: connection refused");
        assert_eq!(l, "error: jan: download failed: connection refused");
    }

    #[test]
    fn apply_failure_status_names_the_failed_count() {
        assert_eq!(super::format_apply_status(0, 0), "Applied.");
        assert_eq!(
            super::format_apply_status(2, 0),
            "Applied — 2 apps failed (see the activity log)"
        );
        assert_eq!(
            super::format_apply_status(1, 3),
            "Applied — 1 app failed, 3 instance(s) NOT seeded (see the activity log)"
        );
        assert_eq!(
            super::format_apply_status(0, 3),
            "Applied — 3 instance(s) NOT seeded (see activity log)"
        );
    }

    #[test]
    fn apply_failure_keeps_the_summary_from_saying_all_applied() {
        assert_eq!(
            super::nothing_pending_summary(0),
            (
                "All changes applied".to_string(),
                "Nothing pending — you're up to date".to_string()
            )
        );
        let (a, b) = super::nothing_pending_summary(1);
        assert_eq!(a, "Last Apply: 1 app not installed");
        assert!(b.contains("activity log"), "{b}");
    }
}
