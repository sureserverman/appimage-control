//! The "Install without verification?" step after Apply or Update (WF-APPLY-001, DEC-001).
//!
//! The GTK side only shows a dialog and re-runs; everything it decides — which apps to list and
//! what to re-run — lives here, so it is testable without a display.

use appimage_control_core::Error;

/// One app whose download was refused because no sha256 is published for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnverifiedApp {
    pub app_index: usize,
    pub app_id: String,
    pub name: String,
    /// The instance count the user asked for (Apply), or the current count (Update).
    pub target: u32,
    pub reason: String,
}

/// One app's result from an Apply or Update worker.
pub struct AppOutcome<'a> {
    pub app_index: usize,
    pub app_id: &'a str,
    pub name: &'a str,
    pub target: u32,
    pub error: Option<&'a Error>,
}

/// The apps to list in the dialog: exactly those refused for lack of a published sha256.
/// Apps that succeeded, or failed for any other reason (a hash mismatch included), are not
/// offered — a mismatch is never overridable.
pub fn unverified_from_outcomes(outcomes: &[AppOutcome<'_>]) -> Vec<UnverifiedApp> {
    let mut out: Vec<UnverifiedApp> = Vec::new();
    for o in outcomes {
        if let Some(Error::UnverifiedDownload { reason, .. }) = o.error {
            if !out.iter().any(|u| u.app_index == o.app_index) {
                out.push(UnverifiedApp {
                    app_index: o.app_index,
                    app_id: o.app_id.to_string(),
                    name: o.name.to_string(),
                    target: o.target,
                    reason: reason.clone(),
                });
            }
        }
    }
    out
}

/// Body text listing the apps, one per line, with why each has no hash.
pub fn dialog_detail(apps: &[UnverifiedApp]) -> String {
    apps.iter()
        .map(|a| format!("{}: {}", a.name, a.reason))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What to run again once the user answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RerunRequest {
    /// `(app_index, target)` for each app to run again.
    pub apps: Vec<(usize, u32)>,
    pub allow_unverified: bool,
}

/// "Install unverified" re-runs with the opt-in set for exactly the listed apps (Apply re-runs
/// every card, but the others are already at their installed counts; Update is restricted to
/// the listed apps); Cancel runs nothing.
pub fn rerun_for_response(
    apps: &[UnverifiedApp],
    install_unverified: bool,
) -> Option<RerunRequest> {
    if !install_unverified || apps.is_empty() {
        return None;
    }
    Some(RerunRequest {
        apps: apps.iter().map(|a| (a.app_index, a.target)).collect(),
        allow_unverified: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unverified(id: &str) -> Error {
        Error::UnverifiedDownload {
            app: id.into(),
            reason: "no catalog sha256 and not a GitHub asset".into(),
        }
    }

    #[test]
    fn unverified_dialog_lists_only_apps_refused_for_lack_of_a_hash() {
        let e_unv_a = unverified("anythingllm");
        let e_mismatch = Error::DigestMismatch {
            app: "keepassxc".into(),
            expected: "a".repeat(64),
            actual: "b".repeat(64),
        };
        let e_net = Error::DownloadFailed("jan: connection refused".into());
        let e_unv_b = unverified("lm-studio");
        let outcomes = [
            AppOutcome {
                app_index: 0,
                app_id: "feather",
                name: "Feather",
                target: 1,
                error: None,
            },
            AppOutcome {
                app_index: 1,
                app_id: "anythingllm",
                name: "AnythingLLM",
                target: 2,
                error: Some(&e_unv_a),
            },
            AppOutcome {
                app_index: 2,
                app_id: "keepassxc",
                name: "KeePassXC",
                target: 1,
                error: Some(&e_mismatch),
            },
            AppOutcome {
                app_index: 3,
                app_id: "jan",
                name: "Jan",
                target: 1,
                error: Some(&e_net),
            },
            AppOutcome {
                app_index: 4,
                app_id: "lm-studio",
                name: "LM Studio",
                target: 1,
                error: Some(&e_unv_b),
            },
            // An Update run reports one error per instance: list the app once.
            AppOutcome {
                app_index: 4,
                app_id: "lm-studio",
                name: "LM Studio",
                target: 1,
                error: Some(&e_unv_b),
            },
        ];
        let got = unverified_from_outcomes(&outcomes);
        let ids: Vec<&str> = got.iter().map(|u| u.app_id.as_str()).collect();
        assert_eq!(ids, ["anythingllm", "lm-studio"]);
        assert_eq!(got[0].target, 2);
        let detail = dialog_detail(&got);
        assert!(detail.contains("AnythingLLM: no catalog sha256") && detail.contains("LM Studio"));
        assert!(!detail.contains("KeePassXC"), "a mismatch is never offered");
    }

    #[test]
    fn unverified_install_response_reruns_only_listed_apps_with_opt_in() {
        let apps = vec![
            UnverifiedApp {
                app_index: 1,
                app_id: "anythingllm".into(),
                name: "AnythingLLM".into(),
                target: 2,
                reason: "r".into(),
            },
            UnverifiedApp {
                app_index: 4,
                app_id: "lm-studio".into(),
                name: "LM Studio".into(),
                target: 1,
                reason: "r".into(),
            },
        ];
        assert_eq!(
            rerun_for_response(&apps, true),
            Some(RerunRequest {
                apps: vec![(1, 2), (4, 1)],
                allow_unverified: true
            })
        );
    }

    #[test]
    fn unverified_cancel_response_reruns_nothing() {
        let apps = vec![UnverifiedApp {
            app_index: 1,
            app_id: "anythingllm".into(),
            name: "AnythingLLM".into(),
            target: 2,
            reason: "r".into(),
        }];
        assert_eq!(rerun_for_response(&apps, false), None);
        assert_eq!(rerun_for_response(&[], true), None);
    }
}
