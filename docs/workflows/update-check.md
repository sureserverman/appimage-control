# Update check — tell the user at login which apps have newer releases

ID: WF-UPDATE-CHECK-001

At login, a short run of `appimage-control --check-updates --notify` lists the installed apps
whose newer release is known. If there are any, it shows one desktop notification. Clicking it
opens the window with the **Updates** filter on. The check reads release versions only. It
never downloads or installs anything.

Entry points:
- Login: the autostart entry `~/.config/autostart/io.github.sureserverman.AppImageControl.desktop`
  (native), or the Background portal's autostart registration (Flatpak). Both run
  `appimage-control --check-updates --notify`. They are written by `crates/gui/src/login_check.rs`.
- CLI: `appimage-control --check-updates [--notify] [--catalog PATH]` →
  `run_check_updates` in `crates/gui/src/cli.rs`. The verdict comes from `updates_available`
  in `crates/core/src/update_check.rs`, the same rule as the card's Update badge.
- Notification click: the app's `show-updates` action, called on its bus name. When the app
  is not running, the bus starts it from its D-Bus service file (`--gapplication-service`;
  `~/.local/share/dbus-1/services/io.github.sureserverman.AppImageControl.service`, or
  `/app/share/dbus-1/services/` in the Flatpak). GNOME Shell makes that call itself; on other
  desktops the `--check-updates` process makes it (step 3).
- Preferences → General → ON LAUNCH → **Check for updates at login**. The choice is saved in
  `config.json` (`check_updates_at_login`): `~/.config/appimage-control/` natively,
  `~/.var/app/io.github.sureserverman.AppImageControl/config/appimage-control/` in the
  Flatpak, which cannot write `~/.config`.

Steps:
1. **Registration.** On the first start of the window, the check is registered (on by default,
   OD-2). Natively the app writes the autostart entry and the D-Bus service file; a repeat
   start leaves identical files untouched. Inside Flatpak it asks the Background portal for
   autostart. Registration runs once per process.
   - If the user turned the entry off outside the app (`Hidden=true` or
     `X-GNOME-Autostart-enabled=false`), the app keeps that choice: the preference turns off
     and the entry is left as it is.
   - A program path that will not exist at the next login (a replaced binary, an AppImage's
     temporary mount, a path with characters an Exec line cannot carry) is refused, and
     Preferences shows "Could not set up the login check: <error>".
   - A debug build registers only with `APPIMAGE_CONTROL_DEV_LOGIN_CHECK=1`.
2. **Check.** At login the entry runs `--check-updates --notify`:
   - It looks up the GitHub `releases/latest` tag for installed GitHub-hosted apps only, and
     uses the version in the catalog URL for the others.
   - It prints `<app-id>: <installed> -> <latest>` for each app with a newer release.
   - If any GitHub lookup failed (offline, rate limited), it prints one line on stderr and
     leaves those apps out.
   - Exit 0 in all of these cases. Exit 1 only when a `--catalog` file cannot be read or the
     home folder is unknown. The login entry passes no `--catalog`: discovery then skips an
     unreadable `APPIMAGE_CONTROL_CATALOG` or installed catalog with a warning and ends at the
     catalog built into the binary, so a login run checks against that one and exits 0.
3. **Notify.** With `--notify` and at least one update, it sends one notification with id
   `updates` and the title "Updates available". The body names up to three apps ("Feather
   Wallet, Joplin and 2 more"). With no updates, nothing is shown. If the notification cannot
   be sent, one line goes to stderr and the exit code stays 0. The route depends on the
   desktop's notification server, the way GLib picks it:
   - GNOME (`org.gtk.Notifications`) and Flatpak (the notification portal): a GNotification.
     A newer one replaces the older one, and the process exits at once.
   - Others (LXQt, KDE, Xfce: `org.freedesktop.Notifications` only): the server reports a
     click to the sending process alone, so the process stays until the notice is clicked or
     closed, at most one hour. On a click it asks the app to run `show-updates`. A server
     without clickable actions gets a plain notice and the process exits.
4. **Click.** The notification's default action is `app.show-updates`. If the app is not
   running, D-Bus activation starts it and builds the window. If it is running, the open window
   is used. Either way, the **Updates** filter is turned on and the window is raised.
5. **Turn it off.** Switching **Check for updates at login** off removes the autostart entry
   (native) or sends `RequestBackground` with `autostart: false` (Flatpak). The D-Bus service
   file stays, so a notification already on screen still opens the app.
   - Flatpak: if the desktop refuses permission, the switch turns off and its subtitle says
     "Your desktop did not allow running at login".
   - Only the newest switch change counts; a late answer to an older one is ignored.

Invariants:
- The check downloads nothing (DEC-001). It reads release versions and the installed
  instances' recorded versions only; no file under the home folder changes.
- A failed lookup never produces a notification. An app whose newest release is unknown is
  left out, never counted as an update.
- No recorded version, a downgrade, or a version that cannot be ordered never counts as an
  update.
- The login entry's Exec names one absolute program with no characters that need escaping;
  anything else is refused, never escaped.
- `--check-updates` runs on its own: only `--notify` and `--catalog` may go with it.

Tests:
- `crates/gui/tests/cli_subprocess.rs` (`cargo test -p appimage-control --test cli_subprocess
  check_updates`): the newer pinned release is printed, a current install prints nothing,
  neither changes any file under HOME, a missing catalog exits 1, `--notify` alone is refused.
- `crates/gui/src/cli.rs` unit tests: the notification body and the allowed companion flags.
- `crates/gui/src/login_check.rs` unit tests (`cargo test -p appimage-control --bins
  login_check`): file paths and texts, byte-identical reconcile, disable, refused paths and
  characters, the user's own off switch, the portal options, request path and answers.
- `crates/core/src/update_check.rs`: the update rule for every catalog app.
- Live, on a logged-in desktop session: `python3 scripts/atspi-smoke.py activation` (D-Bus
  activation of `show-updates`, with and without an open window) and `login-check` (the
  first launch registers both files; the login entry sends the notification; the click opens
  the Updates filter; Launch from the detail page works).
- Other desktops: `dbus-run-session -- python3 scripts/notify-fdo-check.py` plays a
  freedesktop-only notification server on a private bus: the notice is sent, a click runs
  `show-updates` on the app, and a server without actions gets a plain notice.
- The Flatpak portal path cannot be driven from the test VM; it is checked by hand on a
  Flatpak install.
