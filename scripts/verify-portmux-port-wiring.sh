#!/usr/bin/env bash
# Verify the portmux-port wiring on a GTK-capable host.
#
# Runs a sequence of compile + behaviour checks for seeding's hand-off to
# `portmux-port configure-app`. Captures full output to a log file you can
# paste back for review.
#
# Usage:
#   ./scripts/verify-portmux-port-wiring.sh
#   LOG=/tmp/my.log ./scripts/verify-portmux-port-wiring.sh
#   PORTMUX_REPO=/some/path/portmux ./scripts/verify-portmux-port-wiring.sh
#
# Non-destructive: does not write to /etc/tor/torrc, does not install
# anything, does not touch your real Feather/Electrum instances. The
# only filesystem writes are inside a private tmp dir that gets cleaned
# up at the end (or after CTRL-C).
#
# Exit code: 0 if every check produced a green status line, non-zero
# otherwise. Specific failures are surfaced inline in the log.

set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || exit 1

# --- Configuration -----------------------------------------------------------

LOG="${LOG:-/tmp/portmux-port-wiring-verify.log}"
CARGO="${CARGO:-cargo}"
PORTMUX_REPO="${PORTMUX_REPO:-$ROOT/../portmux}"
PASS_COUNT=0
FAIL_COUNT=0
WARN_COUNT=0

# --- Logging helpers ---------------------------------------------------------

# Re-open all script output through `tee` so the user sees progress live and
# the same bytes also land in $LOG.
exec > >(tee "$LOG") 2>&1

hdr() {
    echo
    echo "================================================================"
    echo "  $1"
    echo "================================================================"
}

pass() { echo "  [PASS] $1"; PASS_COUNT=$((PASS_COUNT + 1)); }
fail() { echo "  [FAIL] $1"; FAIL_COUNT=$((FAIL_COUNT + 1)); }
warn() { echo "  [WARN] $1"; WARN_COUNT=$((WARN_COUNT + 1)); }
info() { echo "  [info] $1"; }

# Run a command, capture status, and stream its output indented for readability.
run() {
    local label="$1"
    shift
    info "→ $label"
    info "  \$ $*"
    if "$@" 2>&1 | sed 's/^/    | /'; then
        pass "$label"
        return 0
    else
        local rc=$?
        fail "$label (exit $rc)"
        return $rc
    fi
}

# --- Setup -------------------------------------------------------------------

TMPDIR="$(mktemp -d -t aic-verify-XXXXXX)"
mkdir -p "$TMPDIR/home"
# shellcheck disable=SC2329  # invoked by the trap below
cleanup() { rm -rf "$TMPDIR" || true; }
trap cleanup EXIT INT TERM

echo "appimage-control × portmux-port wiring verification"
echo "Repo:           $ROOT"
echo "Portmux repo:  $PORTMUX_REPO"
echo "Log file:       $LOG"
echo "Tmp work dir:   $TMPDIR"
echo "Date:           $(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo "Host kernel:    $(uname -srm)"

# --- Section 1: environment probes -------------------------------------------

hdr "1. Environment probes"

if command -v portmux-port >/dev/null 2>&1; then
    info "portmux-port found at $(command -v portmux-port)"
    portmux-port configure-app --help 2>&1 | head -3 | sed 's/^/    | /' || true
else
    info "portmux-port not on PATH — the checks below use stubs"
fi

if pkg-config --atleast-version=4.21 gtk4 2>/dev/null; then
    info "gtk4 dev libs present: $(pkg-config --modversion gtk4) (>= 4.21 required)"
    HAVE_GTK=1
elif pkg-config --exists gtk4 2>/dev/null; then
    fail "gtk4 dev libs too old: have $(pkg-config --modversion gtk4), \
need >= 4.21 (pinned by gtk4-sys 0.11.x in this workspace). \
On Ubuntu < 25.04 you'll need a backport or to build GTK4 from source."
    HAVE_GTK=0
else
    fail "gtk4 dev libs missing — appimage-control GUI crate cannot link. \
Install with: sudo apt install libgtk-4-dev libadwaita-1-dev"
    HAVE_GTK=0
fi

if command -v xvfb-run >/dev/null 2>&1; then
    info "xvfb-run present"
    HAVE_XVFB=1
else
    warn "xvfb-run missing — GUI smoke test will be skipped. \
Install with: sudo apt install xvfb"
    HAVE_XVFB=0
fi

# --- Section 2: compile checks -----------------------------------------------

hdr "2. Compile checks"

run "cargo check -p appimage_control_core" \
    "$CARGO" check -p appimage_control_core --tests

if [ "$HAVE_GTK" = "1" ]; then
    run "cargo check -p appimage-control (GUI crate)" \
        "$CARGO" check -p appimage-control --tests
else
    warn "skipping GUI compile check (no gtk4)"
fi

if [ -d "$PORTMUX_REPO/rust" ]; then
    info "checking portmux sibling repo at $PORTMUX_REPO"
    (cd "$PORTMUX_REPO/rust" && "$CARGO" check --bin portmux-port 2>&1) | sed 's/^/    | /'
    if [ "${PIPESTATUS[0]}" -eq 0 ]; then
        pass "portmux-port binary builds against sibling repo"
    else
        fail "portmux-port binary failed to build — check log"
    fi
else
    warn "no portmux sibling at $PORTMUX_REPO — skipping portmux-port build"
fi

# --- Section 3: core lib tests ------------------------------------------------

hdr "3. Core library tests"

run "cargo test -p appimage_control_core --lib" \
    "$CARGO" test -p appimage_control_core --lib

# --- Section 4: CLI pre-flight behaviour --------------------------------------

hdr "4. CLI pre-flight: probe surfaces install hint"

# 4a. portmux-port missing → exit non-zero with install hint
info "4a. point APPIMAGE_CONTROL_PORTMUX_PORT_BIN at a nonexistent path \
and confirm the CLI seed surface bails with the install hint."

# Build the CLI binary once so we can drive it without recompiling per test.
if [ "$HAVE_GTK" = "1" ]; then
    run "build appimage-control binary" \
        "$CARGO" build -p appimage-control --bin appimage-control
    BIN="$ROOT/target/debug/appimage-control"

    if [ -x "$BIN" ]; then
        info "→ run with APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/nonexistent/port-helper"
        OUT_FILE="$TMPDIR/missing-mp.log"
        HOME="$TMPDIR/home" APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/nonexistent/port-helper \
            "$BIN" --seed-settings feather \
            > "$OUT_FILE" 2>&1
        RC=$?
        info "  exit code: $RC"
        sed 's/^/    | /' < "$OUT_FILE"
        # Assertion targets the neutralized install-hint wording in
        # PORTMUX_PORT_INSTALL_HINT (core): the user must learn the
        # helper is missing AND learn the install command. The
        # `system helper` phrasing is shared by the probe-error path
        # and the install-hint body.
        if [ "$RC" -ne 0 ] \
                && grep -qE "Seeding helper|system helper" "$OUT_FILE" \
                && grep -q "sudo apt install portmux" "$OUT_FILE"; then
            pass "4a: pre-flight bailed with non-zero exit + install hint"
        else
            fail "4a: expected non-zero exit and install-hint message; \
got exit=$RC, see log above"
        fi
    else
        fail "4a: $BIN not built — cannot run CLI"
    fi
else
    warn "4a skipped: GUI crate did not compile (no gtk4)"
fi

# 4b. With a stub portmux-port that succeeds, the seed pipeline should
# reach the per-instance loop. We don't actually have any Feather
# instances installed under the test user's $HOME, so the run should
# end with "No supported installed instances to seed." and exit 0.
hdr "4b. CLI pre-flight: probe passes when portmux-port is reachable"

STUB="$TMPDIR/portmux-port"
cat > "$STUB" <<'STUB_EOF'
#!/bin/sh
# Stub portmux-port for the appimage-control wiring smoke test. Answers
# `configure-app` only, so a probe that asks for anything else fails.
[ "${1:-}" = configure-app ] || exit 1
[ "${2:-}" = --help ] && exit 0
printf '{"version":2}\n'
STUB_EOF
chmod +x "$STUB"

if [ "$HAVE_GTK" = "1" ] && [ -x "${BIN:-}" ]; then
    info "→ run with APPIMAGE_CONTROL_PORTMUX_PORT_BIN=$STUB"
    OUT_FILE="$TMPDIR/stub-mp.log"
    HOME="$TMPDIR/home" APPIMAGE_CONTROL_PORTMUX_PORT_BIN="$STUB" \
        "$BIN" --seed-settings feather \
        > "$OUT_FILE" 2>&1
    RC=$?
    info "  exit code: $RC"
    sed 's/^/    | /' < "$OUT_FILE"
    if [ "$RC" -eq 0 ]; then
        pass "4b: pre-flight passed; seed loop completed (no instances to seed)"
    else
        # A non-zero exit is acceptable iff the failure is about
        # missing instances rather than missing portmux-port. The
        # report-summary line is the discriminator.
        if grep -q "Seed settings summary" "$OUT_FILE"; then
            pass "4b: pre-flight passed; seed loop ran (saw summary)"
        else
            fail "4b: unexpected non-zero exit when portmux-port stub is OK"
        fi
    fi
else
    warn "4b skipped: GUI crate or binary unavailable"
fi

# 4c. A portmux older than configure-app answers plain --help but not the
# subcommand. It must get the same install hint as a missing one.
hdr "4c. CLI pre-flight: a portmux without configure-app gets the hint"

OLD_STUB="$TMPDIR/old-portmux-port"
cat > "$OLD_STUB" <<'STUB_EOF'
#!/bin/sh
[ "${1:-}" = --help ] && exit 0
exit 1
STUB_EOF
chmod +x "$OLD_STUB"

if [ "$HAVE_GTK" = "1" ] && [ -x "${BIN:-}" ]; then
    OUT_FILE="$TMPDIR/old-mp.log"
    HOME="$TMPDIR/home" APPIMAGE_CONTROL_PORTMUX_PORT_BIN="$OLD_STUB" \
        "$BIN" --seed-settings feather \
        > "$OUT_FILE" 2>&1
    RC=$?
    info "  exit code: $RC"
    sed 's/^/    | /' < "$OUT_FILE"
    if [ "$RC" -ne 0 ] && grep -q "sudo apt install portmux" "$OUT_FILE"; then
        pass "4c: old portmux bailed with non-zero exit + install hint"
    else
        fail "4c: expected non-zero exit and install hint; got exit=$RC"
    fi
else
    warn "4c skipped: GUI crate or binary unavailable"
fi

# --- Section 5: GUI smoke (optional) -----------------------------------------

hdr "5. GUI startup smoke (xvfb-run)"

if [ "$HAVE_GTK" = "1" ] && [ "$HAVE_XVFB" = "1" ] && [ -x "${BIN:-}" ]; then
    info "→ launch the GUI under xvfb-run with the stub portmux-port; \
let it idle for 3s; then SIGTERM and confirm there were no panics."
    OUT_FILE="$TMPDIR/gui-smoke.log"
    APPIMAGE_CONTROL_PORTMUX_PORT_BIN="$STUB" \
        timeout --signal=TERM 5s \
        xvfb-run -a "$BIN" > "$OUT_FILE" 2>&1
    RC=$?
    info "  exit code: $RC (124 = timeout-after-3s is normal, 143 = SIGTERM)"
    sed 's/^/    | /' < "$OUT_FILE" | tail -40
    if grep -qE "thread .* panicked|RUST_BACKTRACE|fatal:" "$OUT_FILE"; then
        fail "5: GUI showed a panic or fatal during startup — see log above"
    else
        pass "5: GUI started under xvfb and idled without panic"
    fi
else
    warn "5 skipped (need gtk4 + xvfb-run + built binary)"
fi

# --- Summary -----------------------------------------------------------------

hdr "Summary"

echo "  passes:    $PASS_COUNT"
echo "  failures:  $FAIL_COUNT"
echo "  warnings:  $WARN_COUNT"
echo
echo "Full log saved to: $LOG"
echo
echo "If you want me (Claude) to look at this run, paste the contents of"
echo "$LOG back into the conversation (or just say 'check the verify log')."

if [ "$FAIL_COUNT" -gt 0 ]; then
    exit 1
fi
exit 0
