#!/usr/bin/env bash
# Programmatic Stage 5 gate: workspace tests + packaging/metadata tools + catalog/--list smoke.
# Same commands work locally and in CI (install host deps for validate-packaging.sh).
#
# Usage:
#   ./scripts/stage5-validate.sh
#   PROFILE=release ./scripts/stage5-validate.sh
#   RUN_FLATPAK_BUILD=1 ./scripts/stage5-validate.sh   # also runs a full flatpak build (slow)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CARGO="${CARGO:-cargo}"
export PROFILE="${PROFILE:-debug}"

echo "==> [1/3] cargo test --workspace (locked)"
$CARGO test --workspace --locked -- --nocapture

echo "==> [2/3] Packaging & metadata (desktop-file-validate, appstreamcli, flatpak manifest)"
# Reuses RUN_FLATPAK_BUILD if you export it before calling this script.
./scripts/validate-packaging.sh

echo "==> [3/3] Shipped catalog + CLI --list smoke"
./scripts/stage5-catalog-list-smoke.sh

echo ""
echo "Stage 5 programmatic validation finished successfully."
