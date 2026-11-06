# Stage 3 demo — 2026-11-06-gear-lever-gap-plan

Environment: Test VM user@192.168.122.204, live GNOME 50 Wayland session (locked screen), ssh with
XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus.
Native: dev build of branch gear-lever-gap (HEAD 040b7211 + untracked demo.md), built with
`sh /tmp/aic-vm.sh 'cargo build -q -p appimage-control -p appimage_control_core --bin appimage-control --bin repair-launchers'`,
run as /tmp/aic-src/target/debug/appimage-control with a throwaway HOME (/tmp/aic-smoke-home-XXXX, XDG_* under it,
APPIMAGE_CONTROL_DEV_LOGIN_CHECK=1 for GUI runs). Fixture installs were written as
`$HOME/.local/share/appimages/apps/<id>/1/<id>.AppImage` (a script) + `.appimage-control-version`, then launchers were written
by `repair-launchers`. The fixture catalog had entries like
`{"id":"fixture","display_name":"Fixture App","download_url":"https://example.invalid/releases/fixture-2.0.0.AppImage",...}`.
Flatpak: user install io.github.sureserverman.AppImageControl from aic-local (exported 2026-11-06 20:27:37), run against the real home.
GUI driven with pyatspi (GTK_A11Y=atspi). Native runs turned animations off with an isolated GSettings keyfile
(GSETTINGS_BACKEND=keyfile, GDK_DEBUG=no-portals). The Flatpak run used `gsettings set ... enable-animations false`, set back to true afterwards.
Notifications seen with `dbus-monitor --session "type=method_call,interface=org.gtk.Notifications"` (plus the Notify and portal Notification interfaces).
"Click" = `gdbus call --session --dest io.github.sureserverman.AppImageControl --object-path /io/github/sureserverman/AppImageControl --method org.freedesktop.Application.ActivateAction show-updates [] {}`.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: at login `appimage-control --check-updates --notify` runs once. If any installed app has a newer release, one notification (id `updates`) says "Updates available" and names up to three apps ("Feather Wallet, Joplin and 2 more"). Clicking it opens the window with the Updates filter on. Nothing is downloaded (DEC-001).
Verdict: holds
Evidence:
- Registration. Fresh HOME with Fixture App 1.0.0 installed and no `check_updates_at_login` key in config.json. Before the first launch, `$HOME/.config/autostart` and `$HOME/.local/share/dbus-1/services` did not exist. After the GUI was launched once, both existed. Autostart file:
  ```
  [Desktop Entry]
  Type=Application
  Name=AppImage Control update check
  Comment=Tells you at login which installed AppImages have newer releases
  TryExec=/tmp/aic-src/target/debug/appimage-control
  Exec=/tmp/aic-src/target/debug/appimage-control --check-updates --notify
  ...
  X-GNOME-Autostart-enabled=true
  X-GNOME-Autostart-Delay=30
  ```
  Service file: `Exec=/tmp/aic-src/target/debug/appimage-control --gapplication-service`.
- Login run, window closed (no owner of the bus name before or after). Ran the autostart file's Exec line with the test HOME:
  `$ /tmp/aic-src/target/debug/appimage-control --check-updates --notify` → exit=0, stdout `fixture: 1.0.0 -> 2.0.0`, stderr empty.
  dbus-monitor saw exactly 1 call: `AddNotification` to org.gtk.Notifications with app id `"io.github.sureserverman.AppImageControl"`, id `"updates"`, title `"Updates available"`, body `"Fixture App"`, priority `"normal"`, default-action `"app.show-updates"`. No D-Bus error reply was seen.
- How many apps are named. Same command, with N outdated fixture apps. Each run: exit 0, one AddNotification. Bodies:
  - N=2: `"Fixture App and Beta Tool"`
  - N=3: `"Fixture App, Beta Tool and Gamma Editor"`
  - N=4: `"Fixture App, Beta Tool and 2 more"`
  - N=5: `"Fixture App, Beta Tool and 3 more"`
  stdout listed every app (`<id>: 1.0.0 -> 2.0.0`).
- Nothing downloaded. A `find` of the whole test HOME before and after each N=2..5 run showed no new path: no cache dir, no AppImage, nothing.
- Click, app not running. A service file pointing at the dev binary was put in the real `~/.local/share/dbus-1/services` (the dir was empty before), and the click call was made. `-> rc=0 out='()'`. D-Bus activation started pid 3201767. The "AppImage Control" window came up with `Updates checked=True All checked=False`. The Fixture App card ("Details for Fixture App") was present and showing. No CRITICAL or panic lines in the GUI stderr. The service file was removed afterwards; the dir is empty again.
- Flatpak, same click on the shipped build. Nothing owned the bus name. The click call D-Bus-activated the Flatpak through its exported service file (`Exec=/usr/bin/flatpak run ... --command=/app/bin/appimage-control io.github.sureserverman.AppImageControl --gapplication-service`). The window showed `'All' checked=False`, `'Installed' checked=False`, `'Updates' checked=True`.
- Flatpak registration: on its first launch the Flatpak sent `RequestBackground` with `autostart: true` and `commandline ["appimage-control","--check-updates","--notify"]`. The portal answered `background true, autostart true` and wrote `~/.config/autostart/io.github.sureserverman.AppImageControl.desktop` with `Exec=flatpak run --command=appimage-control io.github.sureserverman.AppImageControl --check-updates --notify`.
- Not observed: GNOME actually drawing the banner. The screen is locked, so I relied on the AddNotification call and the lack of an error reply.


## Outcome 2
> Nothing to update → no notification. Exit code 0.
Verdict: holds
Evidence:
- HOME with Fixture App and Beta Tool installed at 2.0.0. The catalog URLs also carry 2.0.0.
  `$ /tmp/aic-src/target/debug/appimage-control --check-updates --notify` → exit=0. stdout and stderr were empty (0 lines). dbus-monitor saw 0 AddNotification/Notify calls.


## Outcome 3
> Offline or rate-limited → no notification, one line on stderr, exit code 0, so login is never disturbed.
Verdict: holds (offline shown; rate-limited could not be caused here and was not tested separately)
Evidence:
- Native, offline (the VM firewall blocks the dev binary). HOME with only `feather` installed at 0.0.1. The catalog entry was copied from data/catalog.json (`https://github.com/feather-wallet/feather/releases/download/2.8.1/feather-2.8.1.AppImage`).
  `$ appimage-control --check-updates --notify` → exit=0. stdout was empty. stderr was 1 line: `check-updates: could not look up the latest GitHub release of 1 installed app (offline, rate-limited, or no release); they were not checked.` 0 notification calls. The same result came with `--catalog <file>`.
- Native, mixed case: feather is offline and Fixture App is outdated. Exit 0, stdout `fixture: 1.0.0 -> 2.0.0`, the same single stderr line, and 1 AddNotification with body `"Fixture App"`. The failed lookup does not block the other apps.
- Flatpak, real home: `$ flatpak run --command=appimage-control io.github.sureserverman.AppImageControl --check-updates --notify` (the portal's autostart Exec line) → exit=0. stdout was empty. stderr: `check-updates: could not look up the latest GitHub release of 2 installed apps (offline, rate-limited, or no release); they were not checked.` No calls were seen on org.gtk.Notifications, org.freedesktop.portal.Notification or org.freedesktop.Notifications.


## Outcome 4
> Catalog unreadable → no notification, the error on stderr, exit code 1.
Verdict: holds
Evidence (HOME has an outdated Fixture App install; each run recorded 0 notification calls and empty stdout):
- `--check-updates --notify --catalog $H/missing.json` → exit=1, stderr `--catalog '/tmp/aic-smoke-home-iSGP/missing.json': file not found or not readable`
- `--catalog $H/bad.json` (contents `{not json`) → exit=1, stderr `--catalog '.../bad.json': invalid catalog JSON: JSON error: key must be a string at line 1 column 2`
- `--catalog $H/noperm.json` (chmod 000) → exit=1, stderr `--catalog '.../noperm.json': invalid catalog JSON: IO error: Permission denied (os error 13)`


## Outcome 5
> The setting is turned off in Preferences → the autostart entry is removed (native) or `RequestBackground` is sent with `autostart: false` (Flatpak). Nothing runs at the next login.
Verdict: holds
Evidence:
- Native. GUI on the registered test HOME. Main Menu → the menu item with shortcut Ctrl+, opened the "Preferences" window. Under ON LAUNCH: switch "Check for updates at login", subtitle "Notify me at login when installed apps have newer releases", CHECKED. Clicking the switch gave `now checked=False; autostart exists=False service exists=True`. config.json gained `"check_updates_at_login": false`. On relaunch, the switch was still off and no autostart file was recreated (`after launch: autostart exists=False`). Turning it back on recreated `io.github.sureserverman.AppImageControl.desktop`. The D-Bus service file stays, but it only runs on activation, not at login.
- Flatpak (stored permission "yes"). Before the change, the autostart file existed and the switch was CHECKED. After the click: `now checked=False; autostart file exists=False`. The portal monitor saw `RequestBackground` with `handle_token "aic_login_2_1"`, `autostart: false` and the same commandline. The answer was `Response 0 {background: true, autostart: false}`. Only `mullvad-vpn.desktop` was left in `~/.config/autostart`. The Flatpak config has `"check_updates_at_login": false`.
- Flatpak relaunch with the setting off: the autostart file was absent before and after. The one `RequestBackground` sent at launch had `autostart: false`. The switch showed off.


## Outcome 6
> Flatpak: the desktop refuses background permission → Preferences shows the switch off, with the subtitle "Your desktop did not allow running at login".
Verdict: holds
Evidence:
- `flatpak permission-set background background io.github.sureserverman.AppImageControl no` (shown as `background background io.github.sureserverman.AppImageControl no 0x00`).
- Case A: the setting was off and the user turned it on. Clicking the switch sent `RequestBackground` with autostart true. The answer was `Response uint32 1`, background false, autostart false. The switch went back to off (`now checked=False`). The row subtitle became the label `'Your desktop did not allow running at login'`. No autostart file was created.
- Case B: the setting was on, then the desktop refused. With permission "yes", I turned the switch on: the autostart file was created and config said `check_updates_at_login: true`. I then set permission "no" and relaunched. At launch the app sent `RequestBackground` with autostart true and got `Response 1`, false/false. Preferences showed the switch off with the subtitle `'Your desktop did not allow running at login'`. The portal-written autostart file was gone after launch. config.json now reads `"check_updates_at_login": false`.
- No stderr output from the Flatpak in either case.


## Outcome 7
> The window is already open when the check runs → the same notification is sent. Clicking it raises the open window on the Updates filter.
Verdict: holds (the "raise" part was not observable on the locked screen; see Minor 2)
Evidence:
- The GUI was running (D-Bus-activated instance pid 3201767, test HOME). The user clicked the "All" filter: `Updates checked=False All checked=True`.
- The autostart Exec line ran: `appimage-control --check-updates --notify` → exit=0, stdout `fixture: 1.0.0 -> 2.0.0`, stderr empty. dbus-monitor saw 1 `AddNotification` with the same content as Outcome 1 (id `"updates"`, `"Updates available"`, body `"Fixture App"`, default-action `"app.show-updates"`). The bus owner was still pid 3201767.
- The click call: `-> rc=0`. The owner was still 3201767, with `Updates checked=True All checked=False`. There was 1 main window. Window state: showing=True, visible=True, active=False. No CRITICAL or panic lines.


## Findings

Blocking: none.

Material: none.

Minor:
1. Outcome 4: an unreadable catalog file (chmod 000) is reported as `invalid catalog JSON: IO error: Permission denied (os error 13)`. The file is not invalid JSON, it cannot be read. A missing file gets the clearer `file not found or not readable`. The exit code and the no-notification behaviour are correct.
2. Outcome 7, "raises the open window": on the locked session the open window stayed `active=False` after the click. It did switch to the Updates filter, and no second window opened. I cannot tell whether the window would come to the front on an unlocked desktop.
3. Outcome 6 Case B: the desktop's refusal turns the user's own setting off (config `check_updates_at_login: false`). If the user later allows background in GNOME Settings, the app will not ask again until they turn the switch back on. This matches the Outcome text ("shows the switch off"); it is noted only as a behaviour to be aware of.
4. Not covered (premises the plan lists as unchecked): the GNOME banner itself, and the click on it, were not seen on screen because of the lock. "Rate-limited" was not caused separately from "offline"; both give the same stderr line. LXQt/Whonix was not tested.

Background work: none left running. Every GUI, dbus-monitor and Flatpak instance I started was stopped. The temporary service file in the real ~/.local/share/dbus-1/services was removed (the dir is empty, as before). ~/.config/autostart holds only mullvad-vpn.desktop. The background permission entry the run created was removed (`flatpak permission-show` is empty). ~/.var/app/io.github.sureserverman.AppImageControl was restored from its backup. enable-animations is back to true. My test HOMEs and helper files on the VM are deleted.
