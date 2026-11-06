# Close-out demo — 2026-11-06-gear-lever-gap-plan

Evaluator: a fresh close-out evaluator. I did not read the product source or the diff. I used the public core API, the CLI, the GUI and the store files, as their users meet them.
Tree: branch gear-lever-gap, HEAD b3becee9 ("chore: bump appimage-control to 0.4.0"). Workspace version 0.4.0. `git status` was clean before and after (this file is the only addition).

Environment:
- Host: core crate built and tested with cargo. The Stage 1 evaluator's public-API harness (`s1demo`, a separate crate that depends only on `appimage_control_core`) was copied to my scratchpad. I rebuilt it against the final core. One field had been renamed (`verification` → `cache_verification`), and `listing_error` was added. Fresh fake HOME.
- VM (user@192.168.122.204, Ubuntu, GNOME 50 Wayland, unlocked: `LockedHint=no`): built with `sh /tmp/aic-vm.sh 'cargo build -q -p appimage-control -p appimage_control_core --bin appimage-control --bin repair-launchers'`. Fixture installs: an executable shell script stood in for each AppImage, plus a `.appimage-control-version` file. Launchers were written by the product's own `repair-launchers`.
- GUI driven three ways: (1) the builder's `scripts/atspi-smoke.py` modes; (2) my own AT-SPI driver on the live Wayland session (open, dump, click); (3) a private Xvfb with xdotool mouse clicks and `import` screenshots, for the Preferences switch. On this VM's Wayland session, the AT-SPI `toggle` action on a GtkSwitch returned True but changed nothing. That happened for every switch, including unrelated ones ("Reload on focus"), so it is a harness limit, not the product.
- Notifications seen with `dbus-monitor` on org.gtk.Notifications / org.freedesktop.Notifications / portal Notification. "Click" = `gdbus call ... org.freedesktop.Application.ActivateAction show-updates [] {}`. That is what GNOME Shell sends for the notification's default action. For the "app not running" case, a temporary D-Bus service file in the real `~/.local/share/dbus-1/services` pointed at a wrapper that runs the dev binary on my fixture HOME. It was removed afterwards.
- Flatpak: the installed user Flatpak (origin aic-local, commit af697571…, exported 2026-11-06 20:27:37) is the Stage 3 build, not 0.4.0. At first it would not start (`bwrap: Can't find source path /run/user/1000/doc/by-app/...`). The VM's document-portal FUSE mount had gone away. `systemctl --user restart xdg-document-portal` restored the mount, and the Flatpak then ran. This was an environment fault, not the product.

## Plan goal check

`cargo test -p appimage_control_core -- app_details_every_catalog_app update_check_every_catalog_app` (host):
```
test app_details::tests::app_details_every_catalog_app ... ok
test update_check::tests::update_check_every_catalog_app ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 301 filtered out
```
rc=0. Both named tests ran; every other test binary filtered to 0. Full `cargo test -p appimage_control_core`: rc=0, lib 303 passed, all integration binaries ok (1 ignored in http_local_fixture).
Verdict: **met**.

---

## Stage 1 — Core

### Outcome 1
> Works: `app_details` returns, for any catalog app, its instances (index, folder, AppImage path, recorded version, size, launcher present or not), its update source and its cache verification state. `updates_available` returns each installed app whose newer release is known. `launch_instance` starts an instance's launcher command and returns at once.

Verdict: **holds**
Evidence:
- `s1demo details` over all 14 catalog apps (empty HOME): each returns source + `cache_verification=NotChecked` + `listing_error=None` + `instances=0`, exit 0. Sources: GithubReleases (feather, session, keepassxc, jan, cryptomator, joplin, standard-notes, qnote, zulip), PinnedUrl (mattermost, electrum-btc, electrum-ltc), UnpinnedUrl (anythingllm, lm-studio).
- Fixtures: feather #1 full (2.7.0), #2 launcher + script without an AppImage (2.7.0), #3 AppImage without a launcher (2.6.0), #5 launcher only, with no folder. Also electrum-btc #1 at `apps/ebtc/1/eb.AppImage` (4.7.0), keepassxc #1 (2.7.12) and mattermost #1 (6.0.4). `s1demo details feather` lists #1, #2, #3 and #5, each with its folder, AppImage path, `present`, `size=Some(173)`/None, launcher path, `present` and version.
- `s1demo updates live` (real network on host, 8.3 s): feather 2.9.1, keepassxc 2.8.0, … →
  ```
  updates_available -> 3 entries
    AvailableUpdate { app_id: "feather", display_name: "Feather Wallet", installed: "2.6.0-2.7.0", latest: "2.9.1" }
    AvailableUpdate { app_id: "keepassxc", display_name: "KeePassXC", installed: "2.7.12", latest: "2.8.0" }
    AvailableUpdate { app_id: "electrum-btc", display_name: "Electrum-BTC", installed: "4.7.0", latest: "4.7.2" }
  ```
  mattermost (installed 6.0.4 = pinned 6.0.4) is not listed. `update_available_for` agrees for each.
- Stage 1's Material finding (a lagging instance was missed) is **fixed**. `s1demo updates set feather=2.7.0` now reports `feather installed "2.6.0-2.7.0" latest "2.7.0"`, because instance #3 is at 2.6.0. Stage 1's Minor 2 (a non-version tag counted as an update) is also fixed. `feather=latest` is no longer reported.
- `s1demo launch feather 1` → `returned after 595.689µs`, `Ok(())`. The marker shows `started … args=[] who=f1`, so `%U` was dropped. With a caller kept alive 8 s, `ps` showed the launch script with pgid = its own pid (1895877), in a group separate from the caller's. The caller exited 0. No launch scripts were left running.

### Outcome 2
> An app with no instances → `app_details` returns an empty instance list, not an error.

Verdict: **holds**. On the empty HOME, all 14 apps gave `instances=0`, `listing_error=None`, exit 0. After the fixtures, the not-installed apps (jan, zulip, …) still gave `instances=0`.

### Outcome 3
> An instance folder without its AppImage or launcher → that instance is listed with the missing piece marked, not dropped.

Verdict: **holds**.
```
#2 dir_present=true  appimage …/feather/2/feather.AppImage present=false size=None  launcher present=true  version=Some("2.7.0")
#3 dir_present=true  appimage present=true size=Some(173)  launcher …feather-3.desktop present=false  version=Some("2.6.0")
#5 dir_present=false appimage present=false size=None  launcher present=true  version=None
```

### Outcome 4
> No remote version known (offline, GitHub rate limit) → `updates_available` leaves that app out. A failed lookup never counts as "update available".

Verdict: **holds** (offline caused; a rate limit was not caused separately)
- `HTTPS_PROXY=http://127.0.0.1:9 … s1demo updates live` → the fetch took 7.4 ms. Tags for feather and keepassxc were `None`, so `latest_known=None`. Result: only `electrum-btc 4.7.0 -> 4.7.2`, whose version comes from the pinned catalog URL. feather and keepassxc, reported online, are left out.
- Empty tag `keepassxc=` → `latest_known=None`, left out.

### Outcome 5
> Launcher missing or its Exec unparseable → `launch_instance` returns an error naming the app and instance. It never runs a partial command.

Verdict: **holds**. Every refused case ran nothing: the `PWNED` marker never appeared, and the launch log stayed empty apart from the earlier run's own `finished` line.

| launcher state | error |
|---|---|
| `Exec=/usr/bin/touch …/PWNED` | `feather instance 1: cannot launch: Exec runs /usr/bin/touch, not this instance's own launcher` |
| unclosed quote | `… cannot parse Exec in …/appimage-control-feather-1.desktop` |
| `<own script> --extra %U` | `… Exec passes extra arguments (--extra); repair the launchers and try again` |
| `Exec=` | `… cannot parse Exec in …` |
| no Exec line | `… launcher …feather-1.desktop has no Exec` |
| `sh -c '<own script>; touch PWNED'` | `… cannot parse Exec in …` |
| `env FOO=1 <own script> %U` | `… Exec runs env, not this instance's own launcher` |
| `<own script>;touch PWNED` | `… Exec runs …/appimage-control-launch.sh;touch, not this instance's own launcher` |
| instance 2's script | `… Exec runs …/feather/2/appimage-control-launch.sh, not this instance's own launcher` |
| `…/feather/2/../1/<script>` | refused, `not this instance's own launcher` |
| launcher deleted | `feather instance 1: cannot launch: launcher …: No such file or directory (os error 2)` |
| #3 (no launcher), #9, jan 1 | `feather instance 3: …`, `feather instance 9: …`, `jan instance 1: …` launcher not found |
| #5 (launcher, no script) | `feather instance 5: cannot launch: …/feather/5/appimage-control-launch.sh is missing` |

Accepted (OD-8): a quoted own-script path, and a legacy `Exec=<own AppImage> %U`. Both ran with `args=[]`.

### Outcome 6
> Unverified cache (no sidecar, or `unverified user-allowed`) → the verification state says so in plain words.

Verdict: **holds**. Core states: absent → `NotChecked`; `verified catalog` → `CatalogHash`; `verified github-digest` → `GithubDigest`; `unverified user-allowed` → `UnverifiedAllowed`; bad tag, non-hex hash, empty file or a directory → `Unreadable`. The plain words are given by the GUI (Stage 2 Outcome 5 below): "Not checked yet" and "Not verified — installed with your permission".

---

## Stage 2 — GUI detail page

### Outcome 1
> Works: each card has a details button. It opens a dialog titled with the app's name. The dialog shows the latest version, the update source ("GitHub releases: owner/repo", "Pinned download", "Download without a published checksum") and the verification state, then one row per instance with its version, size and folder, plus **Launch** and **Open folder** buttons.

Verdict: **holds**
- `python3 scripts/atspi-smoke.py details` → PASS. Its checks included: 'Latest release' shows a version; no removal button (OD-1); Launch started the instance; a failed launch shows a toast; no CRITICAL. `details-all` → PASS: one details button per app (14 of 14), and each window opens.
- By hand (my AT-SPI driver, fixture HOME). Main window: 14 cards, each with a `Details for <name>` button.
  - KeePassXC dialog (frame 'KeePassXC'): `Latest release` = `2.7.12\nGitHub could not be checked, so newer releases may exist`. The VM firewall blocks the dev binary. `Updates from` = `GitHub releases: keepassxreboot/keepassxc`. `Download check` = `Not checked yet`.
  - The KeePassXC instance rows read `Instance #1 / Version 2.7.12 · 0.0 MB · /tmp/aic-eval/h2/.local/share/appimages/apps/keepassxc/1` and `Instance #2 …`. Each row has the buttons `Launch instance #N` ("Launch") and `Open the folder of instance #N` ("Open folder").
  - Mattermost: `6.0.4` / `Pinned download\nhttps://releases.mattermost.com/…6.0.4…AppImage`. LM Studio: `Not known yet` / `Download without a published checksum\nhttps://lmstudio.ai/download/latest/linux/x64`. Electrum-BTC: `4.7.2` / `Pinned download\n…`; its instance row says `Version 4.7.0`.
- **Launch** on KeePassXC #1 wrote `keepassxc 1 started args=[]`, so no arguments were passed (OD-8). The process `/bin/bash …/keepassxc/1/appimage-control-launch.sh` was running, and the dialog stayed open.
- **Open folder** on #1 started `/usr/bin/nautilus --gapplication-service`. Its AT-SPI window was named `1`, the instance folder.
- No word "remove", "delete" or "uninstall" appears anywhere in the dialog tree (OD-1).
- Mouse route (Xvfb): clicking the (i) icon on the Electrum-BTC card opened the "Electrum-BTC" window. It showed Latest release 4.7.2, Pinned download + URL, Download check, and Instance #1 with Version 4.7.0 · 0.0 MB · folder, and the Launch and Open folder buttons.

### Outcome 2
> App not installed → the dialog shows the source and verification sections and says "No instances installed — add one on the card".

Verdict: **holds**. Zulip, not installed: `5.12.3\nGitHub could not be checked…` / `GitHub releases: zulip/zulip-desktop` / `Not checked yet` / `No instances installed — add one on the card`. No Launch or Open folder buttons. `details-all` reported the empty state for all 14 on an empty HOME.

### Outcome 3
> Launch fails (launcher missing, refused Exec, spawn error) → a toast names the app and instance, and the full error goes to the activity log. The dialog stays open.

Verdict: **holds**. Each failure was caused after the dialog opened, then I pressed **Launch**:
- Launcher deleted (Feather Wallet). Toast `Could not launch Feather Wallet #1` with the buttons `Show log` and `Dismiss`. Frame 'Feather Wallet' still open. The log read `error: feather instance 1: cannot launch: launcher /tmp/aic-eval/h2/.local/share/applications/appimage-control-feather-1.desktop: No such file or directory (os error 2)`.
- Refused Exec (Jan, `Exec=/bin/sh -c "touch /tmp/aic-eval/PWNED"`). Toast `Could not launch Jan #1`; frame open. Log: `error: jan instance 1: cannot launch: Exec runs /bin/sh, not this instance's own launcher`. PWNED does not exist.
- Spawn error (LM Studio, `chmod -x` on its launch script). Toast `Could not launch LM Studio #1`; frame open. Log: `error: lm-studio instance 1: cannot launch: could not start …/lm-studio/1/appimage-control-launch.sh: Permission denied (os error 13)`.
- If the launcher is already missing when the dialog opens, Launch is insensitive, with the tooltip `The launcher is missing — use Repair Launchers`.
- The GUI stayed up the whole time.

### Outcome 4
> Instance folder missing → **Open folder** is insensitive, with a tooltip saying the folder is missing.

Verdict: **holds**. I ran `rm -rf apps/keepassxc/2` and kept its launcher. Row `Instance #2 / Version unknown · …/keepassxc/2`: `[button] 'Open the folder of instance #2' desc='The folder is missing' INSENSITIVE` and `[button] 'Launch instance #2' desc='The AppImage is missing — reinstall this instance' INSENSITIVE`. Instance #1's buttons stayed sensitive.

### Outcome 5
> Unverified cache → the verification row says "Not verified — installed with your permission" or "Not checked yet", never "Verified".

Verdict: **holds**. Download check row by sidecar:
- mattermost `<hash> unverified user-allowed` → `Not verified — installed with your permission`
- no sidecar (keepassxc, LM Studio, Zulip) → `Not checked yet`
- `verified catalog` (electrum-btc) → `Verified — matches the checksum in the catalog`
- `verified github-digest` (keepassxc) → `Verified — matches the checksum GitHub publishes`
- `nothex verified catalog` (feather), or a sidecar that is a directory (jan) → `Unknown — the checksum record cannot be read`. A record that cannot be read is never shown as Verified.

---

## Stage 3 — Update check at login

### Outcome 1
> Works: at login `appimage-control --check-updates --notify` runs once. If any installed app has a newer release, one notification (id `updates`) says "Updates available" and names up to three apps ("Feather Wallet, Joplin and 2 more"). Clicking it opens the window with the Updates filter on. Nothing is downloaded (DEC-001).

Verdict: **holds** (native; GNOME's on-screen banner not watched)
- `atspi-smoke.py login-check` → PASS. The first launch wrote the autostart entry (OD-2) and the D-Bus service file. The login entry exited 0 and sent the notification with id 'updates'. The click opened the window on 'Updates', and Launch worked from the detail page. `activation` → PASS.
- By hand, registration. Fresh HOME with electrum-btc 4.7.0. Before the launch, neither `.config/autostart` nor `.local/share/dbus-1/services` existed. After the first GUI launch: `.config/autostart/io.github.sureserverman.AppImageControl.desktop` with `Exec=/tmp/aic-src/target/debug/appimage-control --check-updates --notify`, `X-GNOME-Autostart-Delay=30`, and a service file with `Exec=… --gapplication-service`.
- By hand, login run. Window closed (`NameHasOwner` → false). I ran the autostart Exec line under `env -i` with only PATH, the session bus and the HOME/XDG variables. No catalog override, so the built-in catalog was used.
  - N=1: exit 0, stdout `electrum-btc: 4.7.0 -> 4.7.2`, stderr empty. One `AddNotification` to org.gtk.Notifications: app `io.github.sureserverman.AppImageControl`, id `"updates"`, title `"Updates available"`, body `"Electrum-BTC"`, priority normal, default-action `app.show-updates`.
  - N=2: body `"Electrum-BTC and Electrum-LTC"`.
  - N=3: body `"Mattermost, Electrum-BTC and Electrum-LTC"`.
  - N=4 (fixture catalog via `--catalog`): `"Fixture App, Beta Tool and 2 more"`.
  - N=5: `"Fixture App, Beta Tool and 3 more"`.
  - Every run was exit 0 with exactly 1 notification call.
- Nothing downloaded. A `find` of the whole HOME before and after each run showed 0 new paths, and no `appimages/cache` directory was ever created.
- Click with the app not running: `ActivateAction show-updates` → `()`. D-Bus activation started pid 3287862 (`appimage-control --gapplication-service`). Window 'AppImage Control': `Updates` PRESSED, `All` and `Installed` not. Showing cards: Electrum-BTC, Electrum-LTC, Mattermost, which are the three outdated apps.

### Outcome 2
> Nothing to update → no notification. Exit code 0.

Verdict: **holds**. electrum-btc 4.7.2 and mattermost 6.0.4 installed (both current), built-in catalog: exit 0, stdout and stderr empty, 0 notification calls.

### Outcome 3
> Offline or rate-limited → no notification, one line on stderr, exit code 0, so login is never disturbed.

Verdict: **holds** (offline; a rate limit was not caused separately)
- feather 0.0.1 and keepassxc 2.7.12 installed. The VM firewall gives the dev binary no network. Result: exit 0, stdout empty, stderr 1 line `check-updates: could not look up the latest GitHub release of 2 installed apps (offline, rate-limited, or no release); they were not checked.`, 0 notifications.
- Mixed (plus electrum-btc 4.7.0): exit 0, stdout `electrum-btc: 4.7.0 -> 4.7.2`, the same single stderr line, and 1 notification with body `"Electrum-BTC"`.

### Outcome 4
> Catalog unreadable → no notification, the error on stderr, exit code 1.

Verdict: **holds**. With `--catalog`, every case gave exit 1, empty stdout, 1 stderr line and 0 notifications:
- missing → `--catalog '…/missing.json': file not found or not readable`
- `{not json` → `invalid catalog JSON: JSON error: key must be a string at line 1 column 2`
- chmod 000 → `invalid catalog JSON: IO error: Permission denied (os error 13)`
- a directory → `file not found or not readable`
- `{"apps": 5}` → `invalid catalog JSON: JSON error: invalid type: integer \`5\`, expected a sequence …`

The real login line has no `--catalog`. A bad `APPIMAGE_CONTROL_CATALOG` printed a `warning: … falling through to next source`, then the check ran on the built-in catalog with exit 0. The `--help` text documents this.

### Outcome 5
> The setting is turned off in Preferences → the autostart entry is removed (native) or `RequestBackground` is sent with `autostart: false` (Flatpak). Nothing runs at the next login.

Verdict: **holds**. Native was checked on 0.4.0. The Flatpak half was checked on the installed Stage 3 Flatpak build, the only Flatpak reachable here.
- Native (Xvfb, mouse). Fresh-registered HOME. Ctrl+, opened "Preferences" → General → ON LAUNCH: "Check for updates at login", subtitle "Notify me at login when installed apps have newer releases", on. I clicked the switch, and it showed off. `.config/autostart` was then empty, and the service file stayed. `config.json` had `"check_updates_at_login": false`. After a relaunch, autostart was still empty and the switch was off. Clicking it on recreated `io.github.sureserverman.AppImageControl.desktop` with the same Exec line, and config became `true`.
- Flatpak (Stage 3 build), setting off, `background` permission "yes". The launch sent `RequestBackground` with `reason "Check installed apps for newer releases at login"`, `dbus-activatable false`, `autostart false` and `commandline ["appimage-control","--check-updates","--notify"]`. `~/.config/autostart` held only `mullvad-vpn.desktop`. The switch showed off. I could not toggle the switch in the Flatpak (see Environment).

### Outcome 6
> Flatpak: the desktop refuses background permission → Preferences shows the switch off, with the subtitle "Your desktop did not allow running at login".

Verdict: **holds on the Stage 3 Flatpak build**. The 0.4.0 Flatpak was not built here.
- Setup: `flatpak permission-set background background io.github.sureserverman.AppImageControl no`. The Flatpak config had no `check_updates_at_login` key, so the setting was on by default.
- Launch: `RequestBackground` with `autostart true` and the same commandline. The portal answered `Response uint32 1`, `background false`, `autostart false`.
- Preferences (opened through the main menu over AT-SPI): `[switch] 'Check for updates at login'`, not CHECKED. The subtitle label read `'Your desktop did not allow running at login'`.
- No autostart file was created. The Flatpak config now read `"check_updates_at_login": false` (see Minor m5).
- Afterwards: the permission entry was removed (`permission-show` is empty again), and `~/.var/app/io.github.sureserverman.AppImageControl` was restored from a tar backup.

### Outcome 7
> The window is already open when the check runs → the same notification is sent. Clicking it raises the open window on the Updates filter.

Verdict: **holds** ("raise" not shown; see m2)
- I clicked `All` in the open window (pid 3287862), giving `All=on Updates=off`, with all 14 cards showing.
- The login run then gave exit 0, stdout with three updates, and 1 `AddNotification` with id `updates`, "Updates available" and body `"Mattermost, Electrum-BTC and Electrum-LTC"` (the same as with the window closed). The bus owner stayed 3287862, and the filter stayed on All.
- Click: `ActivateAction show-updates` → owner still 3287862, `All=off Installed=off Updates=on`, 1 main window, showing cards Electrum-BTC, Electrum-LTC and Mattermost. No CRITICAL or panic lines.
- The frame was not reported ACTIVE after the click.
- `activation` smoke mode → PASS: the filter moved to All, the open instance handled the second call, the window switched back to Updates, and there was still one main window.

---

## Stage 4 — Store listing

### Outcome 1
> Works: the summary is 35 characters or fewer, starts with a verb and does not repeat the name. The description leads with isolated copies of curated apps whose downloads are checked against a known hash, then settings seeding and backup, in 3 to 6 lines plus at most 6 list items. Light and dark brand colors are set. Three captioned 1000x700 screenshots (catalog, detail page, Preferences) show default light settings.

Verdict: **holds**
- `<summary>Run apps as isolated copies</summary>`: 27 characters, starts with the verb "Run", no "AppImage Control", no period. The desktop file has the same text: `Comment=Run apps as isolated copies`.
- Description paragraph 1: "Install curated apps as separate, isolated copies, each with its own folder, settings and launcher. Downloads are checked against a known SHA-256 hash before they are made executable, and one without a published hash is installed only if you confirm." That is 250 characters, 4 lines at 70 columns.
- Paragraph 2 covers ready-made settings (seeding), then backup and restore: 157 characters, 3 lines. Together that is 7 lines at 70 columns, or about 5–6 at Flathub's wider page. 4 `<li>` items.
- Branding: light `#b9e4cf`, dark `#1e7a57`. The dark color was changed since the Stage 4 demo (it was `#20476e`). It is now the same hue family as the light one (hue 157° vs 151°). Its contrast against the icon's dark body (#30384B / #374257 sampled from the 256 px PNG) is about 2.0–2.3, up from about 1.1–1.3. A mock banner with the icon on #1e7a57 shows the icon outline clearly. This resolves the Stage 4 demo's M2 and m1.
- `identify`: catalog.png, details.png and preferences.png are each `1000 700`. Captions: "Catalog of apps with their installed copies", "Detail page with each copy's version, size and folder", "Preferences with the update check at login". Catalog is `type="default"`.
- Default light settings. preferences.png matches exactly what a fresh HOME shows on Xvfb: Theme = System, **Compact rows on**, Reload on focus off, the three ON LAUNCH switches on. So "Compact rows" on is the default, which resolves the Stage 4 demo's open nit m2.
- `appstreamcli validate --no-net --pedantic` → only `P: cid-contains-uppercase-letter`, "Validation was successful: pedantic: 1", rc 0. `desktop-file-validate` → rc 0, no output. `python3 scripts/check-metainfo-quality.py …` → `OK: metainfo meets the checked quality guidelines`.
- The checker can fail. Copies with a long summary + name + period, no dark color, dark = light, or a caption with a period each printed `FAIL: …` lines. It does not check "starts with a verb" or the list-item cap (a summary of "Isolated copies of apps" and a 7-item list both passed). I checked those two by eye (m6).

### Outcome 2
> A screenshot URL does not resolve before the push → local validation still passes (`--no-net`). Close-out lists checking the URLs after the push.

Verdict: **holds**
- `curl` of the three raw.githubusercontent.com URLs → `404` for each. `gh repo view` → `"visibility":"PRIVATE"`.
- `--no-net` validation passed (above). Networked `appstreamcli validate --pedantic` → rc 3: three `url-not-reachable` (lines 34–36) and three `screenshot-image-not-found` (lines 52, 56, 60), and nothing else.
- Plan close-out notes (vault plan, line 349): "After `git push origin main` and `git push origin v0.4.0`: confirm … that the three screenshot URLs return 200." Line 338 records that the repo is private and that close-out must list "make it public" as ACTION NEEDED.

### Outcome 3
> Screenshots lack native decorations → close-out lists retaking them on the owner's desktop as ACTION NEEDED (OD-7).

Verdict: **holds**. Viewed: all three are plain captures with no window manager (square frame, no shadow, no title-bar decorations beyond the app's own header buttons). The failure case is present. Plan line 350: "ACTION NEEDED (owner): retake the three screenshots on your desktop with native decorations at 1000x700 or smaller (OD-7)." `docs/flathub-publish.md` step 2 says a private X server has no window manager and to retake the screenshots before submitting.

---

## Verdict table

| Stage | Outcome | Verdict |
|---|---|---|
| Goal | `app_details_every_catalog_app`, `update_check_every_catalog_app` | met (2 passed) |
| 1 | O1 details / updates / launch | holds |
| 1 | O2 no instances → empty list | holds |
| 1 | O3 missing piece marked | holds |
| 1 | O4 failed lookup never an update | holds (offline; rate limit not caused) |
| 1 | O5 refused launch names app+instance, runs nothing | holds |
| 1 | O6 unverified cache state | holds |
| 2 | O1 details button, dialog, source, verification, rows, Launch/Open folder | holds |
| 2 | O2 not installed → empty-state text | holds |
| 2 | O3 failed launch → toast + log, dialog stays | holds |
| 2 | O4 folder missing → Open folder insensitive + tooltip | holds |
| 2 | O5 unverified → never "Verified" | holds |
| 3 | O1 login run, one notification, names, click → Updates, no download | holds (banner not watched on screen) |
| 3 | O2 nothing to update | holds |
| 3 | O3 offline | holds (rate limit not caused) |
| 3 | O4 catalog unreadable → exit 1 | holds |
| 3 | O5 setting off | holds (native on 0.4.0; Flatpak half on the Stage 3 Flatpak build) |
| 3 | O6 Flatpak refusal | holds on the Stage 3 Flatpak build; 0.4.0 Flatpak not built here |
| 3 | O7 window open → same notice, click → Updates | holds (raise not shown) |
| 4 | O1 summary, description, branding, screenshots | holds |
| 4 | O2 URLs 404 → `--no-net` passes, close-out lists URL check | holds |
| 4 | O3 no decorations → ACTION NEEDED (OD-7) | holds |

## Findings

Blocking: none.

Material: none.

Minor:
- m1. The detail page's "Download check" row covers the app's cached download, not each instance. Fixture: electrum-btc instance #1 at 4.7.0, with a cache sidecar `verified catalog` for the 4.7.2 hash. The page shows `Download check: Verified — matches the checksum in the catalog` above `Instance #1 Version 4.7.0`. An instance at a different version from the cached download did not come from that download. With mixed instances (say, one installed before 0.3.0 and one added later), the app-level "Verified" sits next to an install that was never checked. My fixture was hand-built. I could not reproduce this through real downloads, because the VM firewall blocks the dev binary. This brushes against DEC-001's "never present an unverified install as verified"; the owner should judge whether the row needs a per-instance scope or a note.
- m2. Stage 3 O7 "raises the open window": after `ActivateAction show-updates` on the unlocked session, the window switched to Updates but was not reported ACTIVE. My call carried no activation token, as GNOME Shell's real click would, so GNOME's focus-stealing prevention may explain it. I did not see the on-screen banner or a real click.
- m3. Cards show the catalog version, not the installed one. Electrum-BTC's card reads `v4.7.2 · 0.0 MB` with an "Update" badge, while its detail page shows `Version 4.7.0`; Mattermost's card shows `v6.0.4` for an installed 6.0.0. This is card behaviour outside the plan, but it now sits next to the detail page, which tells the truth.
- m4. Carried from Stage 3 m1: a chmod-000 catalog is reported as `invalid catalog JSON: IO error: Permission denied`. The file is unreadable, not invalid. The exit code and the absence of a notification are correct.
- m5. Carried from Stage 3 m3, seen again on the Stage 3 Flatpak: when the desktop refuses, the app turns the user's own setting off (`check_updates_at_login: false`). If background is later allowed, the app will not ask again until the user turns the switch back on.
- m6. `scripts/check-metainfo-quality.py` does not check that the summary starts with a verb, or that the list has at most 6 items. Both are met today; I checked them by eye.
- m7. Plan line 350 (the owner's ACTION NEEDED list) and `docs/flathub-publish.md` do not include "make the GitHub repo public". Line 338 says close-out must list it. Until it is public, the post-push URL check at line 349 will fail. This is a known owner action; noted only so close-out does not drop it.
- m8. Carried from Stage 1 m5 (core only): `launch_instance` on an instance whose AppImage is missing but whose launcher and script exist returns `Ok(())` and runs the script (feather #2). The GUI is safe: Launch is insensitive there ("The AppImage is missing — reinstall this instance").
- m9. Carried from Stage 2 M1: the activity log reached from a toast holds only the latest failure ("1 line · 1 error" each time).

Resolved since the stage demos: Stage 1 Material 1 (lagging instance) and Minor 2 (non-version tag); Stage 4 M2 and m1 (dark brand color); Stage 4 m2 (Compact rows is the default).

Not reached: the 0.4.0 Flatpak (only the Stage 3 export is installed); GNOME drawing the banner and a real click on it; a GitHub rate limit separate from offline; GitHub-fetched versions in the GUI (VM firewall); LXQt/Whonix.

## Cleanup and background work

- Host: two background `cargo test` runs; both finished on their own (rc 0). The `s1demo` launch scripts all finished (none left running). The repo tree is clean apart from this file.
- VM: every GUI I started was stopped: the live-session GUIs, the D-Bus-activated instance pid 3287862, the Xvfb GUI and Xvfb :7, and both Flatpak runs (`flatpak kill`). Also stopped: Nautilus (started by Open folder), the fixture `sleep 300` and every dbus-monitor.
- The temporary service file in `~/.local/share/dbus-1/services` was removed and ReloadConfig called; that directory is empty, as before. `~/.config/autostart` holds only `mullvad-vpn.desktop`. The Flatpak background permission entry was removed. `~/.var/app/io.github.sureserverman.AppImageControl` was restored from backup. `enable-animations` is still `true`. `/tmp/aic-eval` was deleted.
- One environment change was left in place: `xdg-document-portal` was restarted to restore its FUSE mount.
- Four `/tmp/aic-smoke-home-*` dirs (18:25, before this session) were left as found.
- The smoke script's own runs cleaned up after themselves.
