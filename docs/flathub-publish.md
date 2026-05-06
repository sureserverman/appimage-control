# Flathub Publish Guide

This guide is the operator runbook for validating, building, and submitting AppImage Control to Flathub.

App ID: `io.github.appimagecontrol.AppImageControl`

## Prerequisites

- Ubuntu/Debian packages:
  - `flatpak`
  - `flatpak-builder`
  - `desktop-file-utils`
  - `appstream`
- Flathub remote configured:
  - `flatpak remotes` includes `flathub`
- Required runtimes/extensions installed for local builds:
  - `org.gnome.Platform//47`
  - `org.gnome.Sdk//47`
  - `org.freedesktop.Sdk.Extension.rust-stable//24.08`

Install prerequisites:

```bash
sudo apt-get update
sudo apt-get install -y flatpak flatpak-builder desktop-file-utils appstream
sudo flatpak install -y flathub org.gnome.Platform//47 org.gnome.Sdk//47 org.freedesktop.Sdk.Extension.rust-stable//24.08
```

## Metadata and Manifest Validation

Run from repository root:

```bash
desktop-file-validate data/io.github.appimagecontrol.AppImageControl.desktop
appstreamcli validate data/io.github.appimagecontrol.AppImageControl.metainfo.xml
flatpak-builder --show-manifest flatpak/io.github.appimagecontrol.AppImageControl.yml >/dev/null
```

Expected result:

- desktop validation: no errors (hints are acceptable)
- appstream validation: success
- manifest parsing succeeds

## Local Flatpak Build

```bash
flatpak-builder --force-clean build-flatpak flatpak/io.github.appimagecontrol.AppImageControl.yml
```

Expected result:

- build exits `0`
- exports:
  - `io.github.appimagecontrol.AppImageControl.desktop`
  - `io.github.appimagecontrol.AppImageControl.metainfo.xml`
  - `io.github.appimagecontrol.AppImageControl.svg`

## Local Smoke Test

1) CLI starts inside sandbox:

```bash
flatpak-builder --run build-flatpak flatpak/io.github.appimagecontrol.AppImageControl.yml appimage-control --help
```

2) Backup/restore smoke flow (clean synthetic home):

```bash
flatpak-builder --run build-flatpak flatpak/io.github.appimagecontrol.AppImageControl.yml sh -lc '
set -eux
export HOME=/tmp/aic-smoke-home
export XDG_DATA_HOME=$HOME/.local/share
export XDG_CONFIG_HOME=$HOME/.config
mkdir -p "$XDG_DATA_HOME/appimages/apps/smoke/1" "$XDG_DATA_HOME/applications" "$XDG_DATA_HOME/icons/hicolor/64x64/apps"
printf test-appimage > "$XDG_DATA_HOME/appimages/apps/smoke/1/smoke.AppImage"
printf "[Desktop Entry]\nType=Application\nName=Smoke\nExec=true\n" > "$XDG_DATA_HOME/applications/appimage-control-smoke-1.desktop"
printf fakepng > "$XDG_DATA_HOME/icons/hicolor/64x64/apps/appimage-control-smoke-1.png"
appimage-control --backup /tmp/aic-smoke.tar.zst
appimage-control --restore-backup /tmp/aic-smoke.tar.zst || true
appimage-control --restore-backup /tmp/aic-smoke.tar.zst --force
test -f "$XDG_DATA_HOME/appimages/apps/smoke/1/smoke.AppImage"
test -f "$XDG_DATA_HOME/applications/appimage-control-smoke-1.desktop"
test -f "$XDG_DATA_HOME/icons/hicolor/64x64/apps/appimage-control-smoke-1.png"
'
```

Expected result:

- non-forced restore refuses when managed state exists
- forced restore succeeds
- restored files exist

## Flathub Submission Workflow

1) Fork `flathub/flathub`.
2) Add/update manifest:
   - `io.github.appimagecontrol.AppImageControl.yml`
3) Add/update metadata assets referenced by metainfo/screenshots.
4) Open PR with:
   - summary of finish-args and why each permission is needed
   - local build + smoke command outputs
   - known limitations (if any)

## Review Iteration Loop

For each reviewer comment:

1) Apply fix in this repo first.
2) Re-run:
   - metadata validation
   - full flatpak build
   - smoke flow
3) Update Flathub PR manifest/assets.
4) Reply with exact command output evidence.

Never bypass a permission/metadata issue with docs-only wording; fix manifest or app behavior.

## Release Update Workflow

When publishing a new release:

1) Update code and metadata in this repo.
2) Rebuild and run smoke checks from this guide.
3) Sync manifest changes into Flathub PR/branch.
4) Ensure appstream metadata and screenshots remain current.
5) After Flathub merge, install from flathub and rerun backup/restore smoke test.
