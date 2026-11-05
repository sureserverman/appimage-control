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
}
