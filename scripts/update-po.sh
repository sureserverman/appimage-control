#!/bin/sh
# Regenerates po/appimage-control.pot from the GUI sources and merges it into
# every po/<lang>.po listed in po/LINGUAS, keeping existing translations.
#
#   scripts/update-po.sh          update the template and the catalogs
#   scripts/update-po.sh --check  fail if the template is out of date (CI)
#
# xgettext has no Rust mode in older gettext releases, so the sources are read
# as C. That works because every string passed to tr/trc/ntr is a plain
# one-line literal: no raw strings, no `\` line continuations, no \u{...}.
set -eu

cd "$(dirname "$0")/.."
pot=po/appimage-control.pot
domain=appimage-control

generate() {
    xgettext --language=C --from-code=UTF-8 \
        --keyword= --keyword=tr --keyword=trc:1c,2 --keyword=ntr:1,2 \
        --add-comments=TRANSLATORS: --sort-by-file \
        --package-name="$domain" \
        --msgid-bugs-address="https://github.com/sureserverman/appimage-control/issues" \
        --files-from=po/POTFILES --output="$1"
}

# The messages of a template, in a form that does not depend on the gettext
# version that wrote it, on line numbers, or on the creation date.
normalize() {
    msgcat --no-location --no-wrap --sort-output "$1" | grep -v '^"POT-Creation-Date:'
}

if [ "${1:-}" = --check ]; then
    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT
    generate "$tmp/fresh.pot"
    normalize "$pot" > "$tmp/committed"
    normalize "$tmp/fresh.pot" > "$tmp/fresh"
    if ! diff -q "$tmp/committed" "$tmp/fresh" >/dev/null; then
        echo "po/appimage-control.pot is out of date. Run scripts/update-po.sh and commit the result." >&2
        diff -u "$tmp/committed" "$tmp/fresh" | head -40 >&2
        exit 1
    fi
    echo "OK: po/appimage-control.pot matches the sources."
    exit 0
fi

generate "$pot"
for lang in $(cat po/LINGUAS); do
    po="po/$lang.po"
    if [ -f "$po" ]; then
        msgmerge --quiet --update --backup=none --no-fuzzy-matching "$po" "$pot"
    else
        msginit --no-translator --locale="$lang.UTF-8" --input="$pot" --output-file="$po"
    fi
done
echo "Updated $pot and $(wc -w < po/LINGUAS) catalogs."
