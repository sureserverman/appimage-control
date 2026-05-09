#!/usr/bin/env bash
# Validate desktop entry, AppStream metainfo, and (optionally) Flatpak manifest build.
# Full Stage 5 automation (tests + this script + catalog/--list smoke): ./scripts/stage5-validate.sh
# Usage:
#   ./scripts/validate-packaging.sh
#   RUN_FLATPAK_BUILD=1 ./scripts/validate-packaging.sh   # full flatpak-builder (slow)
#
# Requires: desktop-file-validate, appstreamcli, flatpak-builder.
# Full build additionally requires: org.gnome.Platform and org.gnome.Sdk for the runtime-version in the manifest.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

DESKTOP="data/io.github.sureserverman.AppImageControl.desktop"
METAINFO="data/io.github.sureserverman.AppImageControl.metainfo.xml"
MANIFEST="flatpak/io.github.sureserverman.AppImageControl.yml"

echo "==> desktop-file-validate $DESKTOP"
desktop-file-validate "$DESKTOP"

echo "==> appstreamcli validate $METAINFO"
# --no-net skips URL-reachability checks (repo URL and screenshot may not be live yet pre-Flathub-PR);
# Flathub's own CI runs reachability at submission time when the repo and assets are public.
appstreamcli validate --no-net "$METAINFO"

echo "==> flatpak-builder --show-manifest $MANIFEST"
flatpak-builder --show-manifest "$MANIFEST" >/dev/null

echo "==> flatpak-builder --show-deps $MANIFEST"
flatpak-builder --show-deps "$MANIFEST"

if [[ "${RUN_FLATPAK_BUILD:-0}" == "1" ]]; then
  BUILD_DIR="${FLATPAK_BUILD_DIR:-/tmp/appimage-control-flatpak-build}"
  REPO_DIR="${FLATPAK_REPO_DIR:-/tmp/appimage-control-flatpak-repo}"
  echo "==> flatpak-builder (full build; set FLATPAK_BUILD_DIR / FLATPAK_REPO_DIR to override)"
  rm -rf "$BUILD_DIR"
  flatpak-builder --force-clean --user \
    --repo="$REPO_DIR" \
    "$BUILD_DIR" \
    "$MANIFEST"
  echo "Flatpak build finished; repo: $REPO_DIR"
else
  echo "Skipping flatpak-builder compile (set RUN_FLATPAK_BUILD=1 to build)."
fi

echo "Packaging checks OK."
