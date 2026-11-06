# Stage 4 premises — 2026-11-06-gear-lever-gap-plan

Goal: Make the store listing say first what this app does that Gear Lever does not, and meet Flathub's quality guidelines that can be met from here.
Risk: LOW — metadata and images only.

Outcomes:
- Works: the summary is 35 characters or fewer, starts with a verb and does not repeat the name. The description leads with isolated copies of curated apps whose downloads are checked against a known hash, then settings seeding and backup, in 3 to 6 lines plus at most 6 list items. Light and dark brand colors are set. Three captioned 1000x700 screenshots (catalog, detail page, Preferences) show default light settings.
- A screenshot URL does not resolve before the push → local validation still passes (`--no-net`). Close-out lists checking the URLs after the push.
- Screenshots lack native decorations → close-out lists retaking them on the owner's desktop as ACTION NEEDED (OD-7).

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: the AppStream validator Flathub uses accepts `<branding>` with light and dark `<color type="primary">` entries.
Checked: `appstreamcli validate --no-net --pedantic` (AppStream 1.0.2) on a scratch copy of the metainfo with a `<branding>` block added.
Result: "Validation was successful: pedantic: 1" (only the existing cid-contains-uppercase-letter note).
Verdict: holds

## Premise 2
Assumption: the VM can capture a 1000x700 screen of the GUI (private Xvfb, `import -window root`).
Checked: `ssh user@192.168.122.204 'Xvfb :8 -screen 0 1000x700x24 & ...; DISPLAY=:8 import -window root /tmp/probe.png; identify -format "%w %h"'`
Result: `1000 700`. The GUI-in-Xvfb recipe (GSK_RENDERER=cairo + dbus-run-session) produced the current catalog.png on 2026-11-06.
Verdict: holds

## Premise 3
Assumption: the screenshot URLs on raw.githubusercontent.com resolve after the push.
Checked: none possible before the push (the files are not on main yet).
Result: —
Verdict: not checkable here — the files only exist upstream after close-out's push; close-out checks each URL for 200.
