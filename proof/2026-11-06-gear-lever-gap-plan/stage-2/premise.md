# Stage 2 premises — 2026-11-06-gear-lever-gap-plan

Goal: Let a person open any app's card and see its versions, sizes, folders, update source and verification state, and launch an instance or open its folder from there.
Risk: MEDIUM — new widgets in the 1,800-line `main.rs` card builder, testable only on the VM.

Outcomes:
- Works: each card has a details button. It opens a dialog titled with the app's name. The dialog shows the latest version, the update source ("GitHub releases: owner/repo", "Pinned download", "Download without a published checksum") and the verification state, then one row per instance with its version, size and folder, plus **Launch** and **Open folder** buttons.
- App not installed → the dialog shows the source and verification sections and says "No instances installed — add one on the card".
- Launch fails (launcher missing, refused Exec, spawn error) → a toast names the app and instance, and the full error goes to the activity log. The dialog stays open.
- Instance folder missing → **Open folder** is insensitive, with a tooltip saying the folder is missing.
- Unverified cache → the verification row says "Not verified — installed with your permission" or "Not checked yet", never "Verified".

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: The VM runtime has libadwaita ≥ 1.5 (AdwDialog, ToastOverlay) and GTK ≥ 4.10 (FileLauncher).
Checked: `pkg-config --modversion libadwaita-1 gtk4` on the VM.
Result: `1.9.1`, `4.22.4`.
Verdict: holds.

## Premise 2
Assumption: "Open folder" has a handler: the desktop opens `inode/directory`.
Checked: `gio mime inode/directory` on the VM.
Result: `Default application for "inode/directory": org.gnome.Nautilus.desktop`.
Verdict: holds.

## Premise 3
Assumption: A live GNOME session exists for the AT-SPI smoke test (memory: project_atspi_gui_harness).
Checked: `loginctl list-sessions --no-legend` and `ls /run/user/1000` on the VM.
Result: session 3 for user on seat0, tty2, active; `wayland-0` and `bus` present.
Verdict: holds.
