//! Manage AppImage installs under `~/.local/share/appimages` with `.desktop` integration.

mod backup;
mod batch;
mod catalog;
mod desktop;
mod download;
mod error;
mod host_cmd;
mod icons;
mod install;
mod instances;
mod list;
mod migrate;
mod model;
mod portmux_port;
mod paths;
mod reset;
mod seed;
mod seed_pack;
mod seed_pack_update;
mod update;
mod version_info;

pub use backup::{
    collect_backup_inventory, create_backup_archive, restore_backup_archive, BackupCatalogSnapshot,
    BackupEntry, BackupManifest, BackupOptions, BackupReport, BACKUP_FORMAT_VERSION,
};
pub use batch::install_batch;
pub use catalog::{Catalog, CatalogApp, InstanceStrategy};
pub use desktop::{
    app_id_from_appimage_path, build_desktop_entry, parse_desktop_file, parse_desktop_str,
    DesktopEntry, APP_ID_KEY, DESKTOP_PREFIX, MANAGED_KEY,
};
pub use download::download_to_file;
pub use error::{Error, Result};
pub use icons::{
    fetch_url_bytes, instance_icon_theme_name, resolve_icon_image_url, write_instance_icon_png,
};
pub use install::{install_appimage, uninstall_app, InstallOptions, InstallResult};
pub use instances::{
    appimage_file_identity_unchanged, catalog_row_latest_version_label,
    clean_catalog_download_cache, clean_installed_catalog_apps_keep_cache,
    installed_version_fleet_display, installed_version_fleet_has_distinct_recorded_versions,
    instance_count, instance_desktop_display_name, instance_exec_suffix, instance_startup_wm_class,
    list_instance_indices, list_managed_instance_indices, parse_app_instance_spec,
    purge_appimage_control_state, read_installed_version, read_installed_version_fleet_max_semver,
    read_installed_version_fleet_per_instance_breakdown, read_installed_version_fleet_summary,
    read_instance_recorded_version, reconcile_catalog_install_version_metadata,
    reconcile_instance_count, reconcile_instance_count_for_portable_migration,
    reconcile_instance_count_with_report,
    reconcile_instance_version_metadata_if_binary_matches_cache, repair_instance_launchers,
    update_all_installed_instances, update_instance_appimage, FleetInstalledDisplay,
    InstanceUpdateOutcome, ReconcileReport, UpdateAllReport, MAX_INSTANCES_PER_APP,
};
pub use list::list_installed;
pub use migrate::{
    discover_migration_candidates, run_portable_dir_migration, MigrationCandidate,
    MigrationConflict, MigrationResolution,
};
pub use model::InstalledApp;
pub use paths::AppPaths;
pub use reset::{delete_paths, execute_reset, preview_reset_paths, ResetPreview};
pub use seed::{
    recipe_for_supported_app, seed_instance_settings, seed_instance_settings_with_recipe,
    seed_new_instances_after_reconcile, seed_supported_installed_apps, SeedConfig, SeedOutcome,
    SeedPreset, SeedRecipe, SeedReport, SeedTarget, SupportedSeedApp,
};
pub use seed_pack::{user_seeds_dir, SeedPack, SeedPackLibrary, ENV_SEEDS_DIR};
pub use seed_pack_update::{
    update_seed_packs, UpdateReport as SeedPackUpdateReport, DEFAULT_RELEASE_URL as SEED_PACKS_DEFAULT_RELEASE_URL,
    ENV_RELEASE_URL as SEED_PACKS_ENV_RELEASE_URL,
};
pub use portmux_port::{
    probe_runtime as portmux_port_probe_runtime, ProbeOutcome as PortmuxPortProbeOutcome,
    ENV_PORTMUX_PORT_BIN, INSTALL_HINT as PORTMUX_PORT_INSTALL_HINT,
};
pub use update::{appimageupdatetool_available, run_appimageupdatetool, update_from_file};
pub use version_info::{
    catalog_cache_fresh_for_pinned, catalog_pin_matches_recorded, catalog_pinned_version,
    fetch_github_release_hints, fetch_remote_latest_versions, github_owner_repo_from_url,
    recorded_release_matches_tag, remote_release_newer_than_installed,
    should_use_github_latest_asset, version_from_appimage_filename, version_from_download_url,
    GithubReleaseHint,
};
