# Stage 3 premises — 2026-11-06-gear-lever-gap-plan

Goal: Tell a person at login, without opening the window, which installed apps have newer releases. Clicking the notice opens the window on the Updates tab.
Risk: HIGH — D-Bus activation and portal behaviour can differ between GNOME, LXQt and the Flatpak sandbox. The Flatpak path cannot be driven live in this session.

Outcomes:
- Works: at login `appimage-control --check-updates --notify` runs once. If any installed app has a newer release, one notification (id `updates`) says "Updates available" and names up to three apps ("Feather Wallet, Joplin and 2 more"). Clicking it opens the window with the Updates filter on. Nothing is downloaded (DEC-001).
- Nothing to update → no notification. Exit code 0.
- Offline or rate-limited → no notification, one line on stderr, exit code 0, so login is never disturbed.
- Catalog unreadable → no notification, the error on stderr, exit code 1.
- The setting is turned off in Preferences → the autostart entry is removed (native) or `RequestBackground` is sent with `autostart: false` (Flatpak). Nothing runs at the next login.
- Flatpak: the desktop refuses background permission → Preferences shows the switch off, with the subtitle "Your desktop did not allow running at login".
- The window is already open when the check runs → the same notification is sent. Clicking it raises the open window on the Updates filter.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: the GNOME session bus has a GNotification backend (`org.gtk.Notifications`), plus the freedesktop fallback and the desktop portal.
Checked: on the VM live session, `gdbus call --session --dest org.freedesktop.DBus --object-path /org/freedesktop/DBus --method org.freedesktop.DBus.ListNames` (2026-11-06).
Result: `org.gtk.Notifications`, `org.freedesktop.Notifications`, `org.gnome.Shell.Notifications` and `org.freedesktop.portal.Desktop` are all owned. GNOME Shell 50.1.
Verdict: holds.

## Premise 2
Assumption: the session bus D-Bus-activates a service file dropped into `~/.local/share/dbus-1/services/` without a re-login.
Checked: wrote `io.github.sureserverman.AicPremiseProbe.service` (`Exec=/bin/sh -c "touch /tmp/aic-probe-activated"`) there, then `gdbus call ... org.freedesktop.DBus.StartServiceByName io.github.sureserverman.AicPremiseProbe 0`; removed the probe after.
Result: the marker file appeared (the call itself timed out, as expected: the probe never claims the name). `dbus.service` (dbus-daemon) is the bus; session.conf uses `<standard_session_servicedirs />`.
Verdict: holds. Note: the bus reads the real user's XDG_DATA_HOME, not an isolated test HOME, so the live checks install the service file in the real home.

## Premise 3
Assumption: GNOME Shell shows a GNotification for our app id.
Checked: `gdbus call --session --dest org.gtk.Notifications --object-path /org/gtk/Notifications --method org.gtk.Notifications.AddNotification io.github.sureserverman.AppImageControl aic-premise "{'title': <'premise probe'>}"`, then the same for `io.github.sureserverman.NoSuchApp`.
Result: our id returns `()`. The unknown id fails with `org.gtk.Notifications.Error.InvalidApp: The app by ID ... could not be found`. A desktop file named after the app id is installed on the VM (`~/.local/share/applications/` and `/usr/share/applications/`).
Verdict: holds where the app's desktop file is installed (every package installs it). Without one, GNOME refuses the notification; the CLI must not treat that as fatal.

## Premise 4
Assumption: clicking a GNOME notification starts the app through D-Bus activation and calls `ActivateAction` on `/io/github/sureserverman/AppImageControl`.
Checked: by the Stage 3 gate's live `login-check` (it calls `ActivateAction show-updates` the way GNOME Shell's notification daemon does).
Result: see the gate.
Verdict: not checkable here before the code exists — Premise 2 proves the activation half now.

## Premise 5
Assumption: the Flatpak Background portal grants or refuses autostart, and the LXQt (Whonix) notification server keeps working after the sender exits.
Checked: not run.
Result: the Flatpak path and LXQt cannot be driven in this session (plan Risk line).
Verdict: not checkable here — no Flatpak install or LXQt session on the VM; close-out hands the Flatpak check to the owner.
