# Flathub Publish Guide

This guide is the operator runbook for validating, building, and submitting AppImage Control to Flathub.

App ID: `io.github.sureserverman.AppImageControl`

## Prerequisites

- Ubuntu/Debian packages:
  - `flatpak`
  - `flatpak-builder`
  - `desktop-file-utils`
  - `appstream`
- Flathub remote configured:
  - `flatpak remotes` includes `flathub`
- Required runtimes/extensions installed for local builds:
  - `org.gnome.Platform//50`
  - `org.gnome.Sdk//50`
  - `org.freedesktop.Sdk.Extension.rust-stable//24.08`

Install prerequisites:

```bash
sudo apt-get update
sudo apt-get install -y flatpak flatpak-builder desktop-file-utils appstream
sudo flatpak install -y flathub org.gnome.Platform//50 org.gnome.Sdk//50 org.freedesktop.Sdk.Extension.rust-stable//24.08
```

## Metadata and Manifest Validation

Run from repository root:

```bash
desktop-file-validate data/io.github.sureserverman.AppImageControl.desktop
appstreamcli validate data/io.github.sureserverman.AppImageControl.metainfo.xml
flatpak-builder --show-manifest flatpak/io.github.sureserverman.AppImageControl.yml >/dev/null
```

Expected result:

- desktop validation: no errors (hints are acceptable)
- appstream validation: success
- manifest parsing succeeds

## Local Flatpak Build

```bash
flatpak-builder --force-clean build-flatpak flatpak/io.github.sureserverman.AppImageControl.yml
```

Expected result:

- build exits `0`
- exports:
  - `io.github.sureserverman.AppImageControl.desktop`
  - `io.github.sureserverman.AppImageControl.metainfo.xml`
  - `io.github.sureserverman.AppImageControl.png` (128/256/512 hicolor sizes)

## Local Smoke Test

1) CLI starts inside sandbox:

```bash
flatpak-builder --run build-flatpak flatpak/io.github.sureserverman.AppImageControl.yml appimage-control --help
```

2) Backup/restore smoke flow (clean synthetic home):

```bash
flatpak-builder --run build-flatpak flatpak/io.github.sureserverman.AppImageControl.yml sh -lc '
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

## Pre-submission Checklist (manual steps not handled by the validation script)

The repo and manifest are structurally ready for Flathub. The remaining steps are
content/process work the operator must do before opening the submission PR:

1) **Push the source repo to GitHub.** The metainfo and Cargo workspace point at
   `https://github.com/sureserverman/appimage-control`. That URL must resolve (200 OK)
   for Flathub's reviewer CI to validate the metainfo. If the repo is currently local
   or private, push it public:
   ```bash
   git push -u origin main
   ```

2) **Capture and upload at least one screenshot.** The metainfo references:
   ```
   https://raw.githubusercontent.com/sureserverman/appimage-control/main/docs/screenshots/catalog.png
   ```
   Run the installed Flatpak, capture the catalog window, save it to
   `docs/screenshots/catalog.png`, commit, and push. (Local validation passes
   because the script uses `--no-net`; Flathub's reviewer CI does fetch the URL.)

3) **Create and push the `v0.1.0` tag.** Flathub builds from a tag, not a branch:
   ```bash
   git tag -a v0.1.0 -m "AppImage Control 0.1.0 — initial Flathub release"
   git push origin v0.1.0
   ```
   Note the commit SHA the tag points to — you'll paste it into the manifest below.

4) **Swap the manifest source to `type: git`.** In
   `flatpak/io.github.sureserverman.AppImageControl.yml`, the `sources:` block
   currently uses `type: dir, path: ..` for local builds. Replace it with the
   commented-out `type: git` block (`url`, `tag: v0.1.0`, `commit: <sha-from-step-3>`)
   when preparing the Flathub PR. Keep the local-dir block in your local working
   tree if you'll continue iterating; the Flathub fork is the only place that
   needs the `type: git` form.

## Flathub Submission Workflow

1) Fork `flathub/flathub`.
2) Create a branch named `io.github.sureserverman.AppImageControl`.
3) Add the manifest at the repo root: `io.github.sureserverman.AppImageControl.yml`
   (using the `type: git` source from the pre-submission step).
4) Open PR with:
   - summary of finish-args and why each permission is needed
     (network for catalog downloads; xdg-data/{appimages,applications,icons} for
     managed AppImage installs and launcher integration)
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
