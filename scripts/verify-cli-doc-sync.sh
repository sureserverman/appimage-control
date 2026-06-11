#!/bin/sh
# Verify that every --flag and instance-spec token form documented in
# README.md's CLI sections also appears in the print_help() text in
# crates/gui/src/cli.rs, and vice versa.
#
# Does NOT require a built binary or GTK — pure static source grep.
#
# Exit 0 when README and print_help are in sync.
# Exit 1 with a list of mismatches when they diverge.
#
# Usage:
#   sh scripts/verify-cli-doc-sync.sh

set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

README="$ROOT/README.md"
CLI_RS="$ROOT/crates/gui/src/cli.rs"

if [ ! -f "$README" ]; then
    printf 'ERROR: README.md not found at %s\n' "$README" >&2
    exit 1
fi
if [ ! -f "$CLI_RS" ]; then
    printf 'ERROR: cli.rs not found at %s\n' "$CLI_RS" >&2
    exit 1
fi

# ---------------------------------------------------------------------------
# Helper: check whether a literal string appears as a complete line in a
# newline-separated list stored in a variable.  Uses a temp file to avoid
# passing flag-like tokens (--foo) as grep options.
# ---------------------------------------------------------------------------
list_contains() {
    # $1 = the newline-separated list (variable value)
    # $2 = the literal token to search for
    printf '%s\n' "$1" | grep -qxF -- "$2"
}

# ---------------------------------------------------------------------------
# 1. Extract flags from print_help() in cli.rs.
#
#    Scope extraction to the print_help function body by selecting lines
#    between the `pub fn print_help()` marker and the closing `);` line
#    that ends the print! macro.
# ---------------------------------------------------------------------------

extract_help_flags() {
    sed -n '/pub fn print_help()/,/^[[:space:]]*);/p' "$CLI_RS" |
        grep -oE '\-\-[a-z][a-z0-9-]+' |
        sort -u
}

HELP_FLAGS="$(extract_help_flags)"

# ---------------------------------------------------------------------------
# 2. Extract --flags from README's CLI surface section only.
#
#    Scope: from "### CLI surface" heading to the next heading at the
#    same (###) or higher (##) level.  We strip lines that are inside the
#    Build section (which contains --release as a cargo flag) or the
#    Catalog schema table (which contains no CLI flags).  In practice it
#    is simplest and correct to scope to the seeding architecture section.
#
#    Strategy: extract the block between "### CLI surface" and the next
#    line that starts with "## " (two hashes, space) which terminates the
#    seeding architecture section.
# ---------------------------------------------------------------------------

extract_readme_flags() {
    awk '
        /^### CLI surface/        { in_section=1; next }
        in_section && /^## /      { in_section=0 }
        in_section                { print }
    ' "$README" |
        grep -oE '\-\-[a-z][a-z0-9-]+' |
        sort -u
}

README_FLAGS="$(extract_readme_flags)"

# ---------------------------------------------------------------------------
# 3. Check instance-spec token sigil forms (+APP, -APP, =APP) in README's
#    CLI surface section and in print_help.
# ---------------------------------------------------------------------------

README_CLI_BLOCK="$(awk '
    /^### CLI surface/    { in_section=1; next }
    in_section && /^## /  { in_section=0 }
    in_section            { print }
' "$README")"

HELP_BLOCK="$(sed -n '/pub fn print_help()/,/^[[:space:]]*);/p' "$CLI_RS")"

# ---------------------------------------------------------------------------
# 4. Compare the two flag sets.
# ---------------------------------------------------------------------------

MISMATCHES=0

# Flags in print_help but missing from README's CLI surface section
for flag in $HELP_FLAGS; do
    if ! list_contains "$README_FLAGS" "$flag"; then
        printf 'MISSING from README CLI section  : %s  (in print_help in cli.rs)\n' "$flag"
        MISMATCHES=$((MISMATCHES + 1))
    fi
done

# Flags in README CLI section but missing from print_help
for flag in $README_FLAGS; do
    if ! list_contains "$HELP_FLAGS" "$flag"; then
        printf 'MISSING from print_help          : %s  (in README.md CLI section)\n' "$flag"
        MISMATCHES=$((MISMATCHES + 1))
    fi
done

# Instance-spec sigils: each must appear in both print_help and README CLI section
for sigil in '+APP' '-APP' '=APP'; do
    in_help=0
    in_readme=0
    printf '%s\n' "$HELP_BLOCK"   | grep -qF -- "$sigil" && in_help=1   || true
    printf '%s\n' "$README_CLI_BLOCK" | grep -qF -- "$sigil" && in_readme=1 || true
    if [ "$in_help" -eq 1 ] && [ "$in_readme" -eq 0 ]; then
        printf 'MISSING from README CLI section  : %s token form  (in print_help)\n' "$sigil"
        MISMATCHES=$((MISMATCHES + 1))
    fi
    if [ "$in_readme" -eq 1 ] && [ "$in_help" -eq 0 ]; then
        printf 'MISSING from print_help          : %s token form  (in README)\n' "$sigil"
        MISMATCHES=$((MISMATCHES + 1))
    fi
done

# ---------------------------------------------------------------------------
# 5. Report
# ---------------------------------------------------------------------------

if [ "$MISMATCHES" -eq 0 ]; then
    NFLAGS="$(printf '%s\n' "$HELP_FLAGS" | grep -c . || true)"
    printf 'OK: README CLI section and print_help are in sync.\n'
    printf '    Flags verified: %s\n' "$NFLAGS"
    exit 0
else
    printf '\nFAIL: %d mismatch(es) between README CLI section and print_help.\n' "$MISMATCHES"
    exit 1
fi
