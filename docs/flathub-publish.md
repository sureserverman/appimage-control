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
  - `org.freedesktop.Sdk.Extension.rust-stable//25.08`

Install prerequisites:

```bash
sudo apt-get update
sudo apt-get install -y flatpak flatpak-builder desktop-file-utils appstream
sudo flatpak install -y flathub org.gnome.Platform//50 org.gnome.Sdk//50 org.freedesktop.Sdk.Extension.rust-stable//25.08
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
  - `io.github.sureserverman.AppImageControl.service` (D-Bus activation)
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

## Flathub Rules That Shape This Submission

Checked against <https://docs.flathub.org/docs/for-app-authors/requirements>
on 2026-11-06. Re-read that page before submitting; it changes.

1) **AI-generated material must be disclosed.** Most of this codebase, its
   docs and the in-repo Flatpak files were written with an AI coding assistant
   (most commits carry a `Co-Authored-By: Claude` line). The submission must say
   which parts and roughly how much. Reviewers may reject on the extent alone.

2) **The Flathub manifest must contain no AI-generated or AI-assisted content,
   even if disclosed.** Write the submission manifest yourself. Do not copy
   `flatpak/io.github.sureserverman.AppImageControl.yml` or
   `flatpak/cargo-sources.json` into the submission: both were AI-written and
   are kept only for local and CI builds.

3) **AI tools must not open the submission PR or write its description,
   commit messages, review replies or linter exception requests.**

4) **No machine translations.** The `po/` catalogs are machine-made, so the
   Flathub build must pass `--no-default-features` to the cargo build. That
   drops the `translations` feature and the GUI stays English. The desktop
   file and metainfo carry English only.

5) **Host-dependent apps are accepted only case by case.** The app runs
   AppImages, `xvfb-run`, `fusermount3` and the desktop-database tools on the
   host through `flatpak-spawn --host`. Gear Lever (`it.mijorus.gearlever`) is
   on Flathub with the same `--talk-name=org.freedesktop.Flatpak` permission,
   so there is precedent, but expect to justify it.

## What the Hand-Written Manifest Needs

These are requirements, not manifest text:

- Top-level file named `io.github.sureserverman.AppImageControl.yml` (or
  `.json`), with the latest GNOME runtime on Flathub (50 at the time of writing)
  and the `rust-stable` SDK extension.
- A `git` source with both `tag` and `commit` pointing at a release tag of
  this repo. `type: dir` sources are refused by the linter.
- An offline build: Flathub builds have no network. This repo vendors every
  crate in `vendor/` and `.cargo/config.toml` points cargo at it, so
  `cargo build --release --offline` works from the git source alone. Flathub
  also asks for a dependency manifest; `flatpak-cargo-generator.py` from
  `flatpak/flatpak-builder-tools` produces one from `Cargo.lock`. Ask the
  reviewers which of the two they want.
- `--no-default-features` on the cargo build (rule 4).
- `APPIMAGE_CONTROL_CATALOG` set to the installed catalog
  (`/app/share/appimage-control/catalog.json`), the way the local manifest's
  wrapper script does it.
- Installed files: the binary under `/app/bin`, the desktop file, the metainfo,
  `data/catalog.json`, the SVG icon in `hicolor/scalable/apps` and the PNG
  icons in their sized `hicolor` folders, all named after the app ID.
- The D-Bus service file `data/io.github.sureserverman.AppImageControl.service`
  installed to `/app/share/dbus-1/services/`, named after the app ID. Clicking
  the login update notification starts the app through this file. The desktop
  file does not set `DBusActivatable`: GNOME Shell needs only the service file
  for the click, and the key would make a launcher without the service file
  start nothing. Its `Exec` names
  `/app/bin/appimage-control`; Flatpak rewrites it on export.
- `LICENSE` installed to `$FLATPAK_DEST/share/licenses/$FLATPAK_ID/`.
- Permissions, each with a reason in the PR:
  - `--share=network`: catalog, icon and AppImage downloads.
  - `--socket=wayland`, `--socket=fallback-x11`, `--share=ipc`, `--device=dri`:
    the GTK 4 window.
  - `--filesystem=xdg-data/appimages`, `xdg-data/applications`,
    `xdg-data/icons`: the managed AppImages and their launchers and icons.
    The linter grades `xdg-data` access as "explain".
  - `--talk-name=org.freedesktop.Flatpak`: `flatpak-spawn --host` (rule 5).
    The linter reports it as a warning, not an error.
- `flathub.json` only if the app must be limited to one architecture.
  `data/catalog.json` has `download_url_aarch64` entries, so building both
  x86_64 and aarch64 is expected.

## Pre-submission Checklist

1) **Cut a release that contains the current metadata.** Flathub builds from
   a tag. The metainfo must list that release with its date, and the
   screenshot it names must exist on the default branch. v0.3.0 predates the
   screenshot and the English-only switch, so the submission needs a newer tag.

2) **Check the screenshot.** `docs/screenshots/catalog.png` is referenced from
   the metainfo through its `raw.githubusercontent.com` URL on `main`. Flathub
   mirrors it at build time. To retake it on a machine with GTK 4, render into
   a private X server, which avoids compositor screenshot restrictions:
   ```bash
   Xvfb :7 -screen 0 1000x760x24 &
   DISPLAY=:7 GDK_BACKEND=x11 GSK_RENDERER=cairo LANGUAGE=en \
     dbus-run-session -- appimage-control &
   # after the icons load:
   W=$(DISPLAY=:7 xdotool search --name "AppImage Control" | tail -1)
   DISPLAY=:7 xdotool windowmove $W 0 0 windowsize --sync $W 1000 760
   DISPLAY=:7 import -window root docs/screenshots/catalog.png
   ```
   Use an isolated `HOME` with a few installed instances so the cards show
   both states.

3) **Run the linter** on your manifest and on the built repo:
   ```bash
   flatpak install -y flathub org.flatpak.Builder
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder manifest io.github.sureserverman.AppImageControl.yml
   flatpak run --command=flathub-build org.flatpak.Builder --install io.github.sureserverman.AppImageControl.yml
   flatpak run --command=flatpak-builder-lint org.flatpak.Builder repo repo
   ```

4) **Write the AI disclosure** for the PR in your own words (rule 1).

## Flathub Submission Workflow

1) Fork `flathub/flathub` with "Copy the master branch only" unchecked, and
   branch from `new-pr`.
2) Add your manifest at the repo root.
3) Open the PR yourself against `new-pr` (not `master`), titled
   "Add io.github.sureserverman.AppImageControl". Include the permission
   reasons, the AI disclosure, and the local build and smoke-test results.

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
