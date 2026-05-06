//! Install one instance each of selected catalog apps (for integration checks).
//! Run from the repo root: `cargo run -p appimage_control_core --bin install-test-desktop-apps`
//!
//! Optional: extra CLI args are catalog app ids (default: mattermost lm-studio anythingllm).
//! Per-app instance count: `id:N` (e.g. `mattermost:2`); default N is 1. Respects each app’s
//! catalog `max_instances` (e.g. LM Studio is capped at 1).

use std::process;

use appimage_control_core::{parse_app_instance_spec, reconcile_instance_count, AppPaths, Catalog};

fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let paths = AppPaths::from_env().map_err(|e| e.to_string())?;
    let catalog = Catalog::discover_load(None).map_err(|e| e.to_string())?;
    let by_id = catalog.by_id();

    let ids: Vec<String> = {
        let args: Vec<String> = std::env::args().skip(1).collect();
        if args.is_empty() {
            vec![
                "mattermost".into(),
                "lm-studio".into(),
                "anythingllm".into(),
            ]
        } else {
            args
        }
    };

    for spec in &ids {
        let (id, count) = parse_app_instance_spec(spec).map_err(|e| e.to_string())?;
        let app = by_id
            .get(id.as_str())
            .ok_or_else(|| format!("unknown catalog id: {id}"))?;
        eprintln!("Installing {id} ({count} instance(s)) …");
        reconcile_instance_count(&paths, app, count, &mut |m| eprintln!("  {m}"))
            .map_err(|e| format!("{id}: {e}"))?;
    }

    println!("OK: installed {:?}", ids);
    Ok(())
}
