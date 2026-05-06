#!/usr/bin/env bash
# Programmatic Stage 5 subset: shipped catalog loads and `appimage-control --list` mentions every app id.
# Requires: Python 3, a built `appimage-control` binary (this script builds it if missing).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

CARGO="${CARGO:-cargo}"
PROFILE="${PROFILE:-debug}"
if [[ "$PROFILE" == "release" ]]; then
  REL="release"
  $CARGO build -p appimage-control --release
else
  REL="debug"
  $CARGO build -p appimage-control
fi

BIN="${CARGO_TARGET_DIR:-$ROOT/target}/$REL/appimage-control"
if [[ ! -x "$BIN" ]]; then
  echo "missing binary: $BIN" >&2
  exit 1
fi

TMP_HOME="$(mktemp -d "${TMPDIR:-/tmp}/aic-stage5-home.XXXXXX")"
cleanup() { rm -rf "$TMP_HOME"; }
trap cleanup EXIT

export HOME="$TMP_HOME"
export APPIMAGE_CONTROL_CATALOG="$ROOT/data/catalog.json"

echo "==> appimage-control --list (HOME=$TMP_HOME)"
LIST_OUT="$("$BIN" --list 2>&1)" || {
  echo "$LIST_OUT" >&2
  echo "--list failed" >&2
  exit 1
}
export LIST_OUT
export ROOT

python3 - <<'PY'
import json, os, re, sys

root = os.environ["ROOT"]
out = os.environ["LIST_OUT"]
cat_path = os.path.join(root, "data", "catalog.json")
with open(cat_path, encoding="utf-8") as f:
    ids = [a["id"] for a in json.load(f)["apps"]]

missing = [i for i in ids if not re.search(rf"^{re.escape(i)}\s", out, re.MULTILINE)]

if missing:
    print("catalog id(s) not found in --list output:", ", ".join(missing), file=sys.stderr)
    print("---- list output ----", file=sys.stderr)
    print(out, file=sys.stderr)
    sys.exit(1)
print(f"OK: --list includes all {len(ids)} shipped catalog ids.")
PY
