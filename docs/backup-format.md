# AppImage Control Backup Format

This document defines the backup/restore contract for manager-owned state.

## Scope

Backups include only AppImage Control-managed files:

- `~/.local/share/appimages/`
- `~/.local/share/applications/appimage-control-*.desktop`
- `~/.local/share/icons/hicolor/*/apps/appimage-control-*.png`

Backups intentionally do not claim to capture arbitrary unmanaged AppImages.

## CLI Contract

- `appimage-control --backup <archive-path>`
- `appimage-control --restore-backup <archive-path>`
- `appimage-control --restore-backup <archive-path> --force`

## Archive Contract

Target artifact is a single `tar.zst` archive containing:

- `manifest.json`
- `catalog-snapshot.json`
- managed data files listed in the manifest inventory

`manifest.json` is versioned by `format_version` to support future compatibility checks.

## Restore Policy

- Restore must validate archive metadata before any writes.
- Restore refuses by default when manager-owned state already exists.
- `--force` is required to overwrite existing manager-owned state.
- Restore is designed to be deterministic from the archive and catalog snapshot (no live network dependency).
