# Stage 2 demo — 2026-11-06-gear-lever-gap-plan

Environment: Ubuntu 26.04 test VM (user@192.168.122.204), live GNOME Wayland session on seat0. Built at branch `gear-lever-gap` HEAD 8ca6a2d4 with `sh /tmp/aic-vm.sh 'cargo build -p appimage-control -p appimage_control_core --bin appimage-control --bin repair-launchers'`. The GUI ran from /tmp/aic-src/target/debug/appimage-control with `GTK_A11Y=atspi LANGUAGE=en`, `APPIMAGE_CONTROL_CATALOG=/tmp/aic-src/data/catalog.json` and a throwaway HOME `/tmp/aic-demo-home` (XDG_CONFIG/DATA/CACHE_HOME inside it). Clicks were made with pyatspi: each button was found by its accessible name and pressed. Screenshots came from a second GUI instance on a private Xvfb :7 (`GDK_BACKEND=x11 GSK_RENDERER=cairo dbus-run-session`), clicked with xdotool. The evaluator did not read the source or the diff.

Fixture installs: an executable shell script stood in for each AppImage. It writes `$HOME/launched.log` and sleeps. It was placed at `apps/<id>/<n>/<catalog filename>`: keepassxc 1+2, mattermost 1, lm-studio 1, feather 1, jan 1, joplin 1+2 and cryptomator 1. Most got a `.appimage-control-version` file; lm-studio did not. Then the product's own `repair-launchers` wrote the launchers. It reported "launcher repaired" for feather, keepassxc 1/2, mattermost, jan and lm-studio. It wrote none for joplin or cryptomator. Cache sidecar `cache/mattermost/.appimage-control-sha256` = `98c6…8533 unverified user-allowed`. No sidecar for keepassxc.

Failure states, caused on disk:
- `rm appimage-control-feather-1.desktop` (launcher deleted)
- jan launcher `Exec=/bin/sh -c "touch /tmp/aic-demo-home/PWNED"` (refused Exec)
- `chmod -x apps/lm-studio/1/appimage-control-launch.sh` (spawn error)
- `rm -rf apps/keepassxc/2` with its launcher left behind (folder missing)

Note: on startup the GUI ran its own launcher repair. The Activity log said "feather instance 1: launcher repaired", "jan instance 1: launcher repaired" and "keepassxc instance 2: skipped (missing AppImage …)". So the launcher-deleted and refused-Exec states were re-caused while the details dialog was open, before **Launch** was pressed.

## Outcome 1
> Works: each card has a details button. It opens a dialog titled with the app's name. The dialog shows the latest version, the update source ("GitHub releases: owner/repo", "Pinned download", "Download without a published checksum") and the verification state, then one row per instance with its version, size and folder, plus **Launch** and **Open folder** buttons.
Verdict: does not hold
Evidence:
- Accessibility tree of the main window: 14 `[table cell]` cards and 14 buttons, one per card. Each is named `Details for <name>`: AnythingLLM, Cryptomator, Electrum-BTC, Electrum-LTC, Feather Wallet, Jan, Joplin, KeePassXC, LM Studio, Mattermost, qnote, Session, Standard Notes, Zulip. Visually it is the (i) icon at the card's top right (screenshot main.png).
- Every one of the 14 was pressed. Each opened a new toplevel `[frame]` named exactly after the app ('KeePassXC', 'Zulip', 'Standard Notes', …), and each closed again with its Close button.
- KeePassXC dialog (AT-SPI dump, matched by the Xvfb screenshot):
  `Latest release: Not known yet` / `Updates from: GitHub releases: keepassxreboot/keepassxc` / `Download check: Not checked yet` / Instances: `Instance #1  Version 2.7.12 · 3.0 MB · /tmp/aic-demo-home/.local/share/appimages/apps/keepassxc/1  [Launch] [Open folder]`, `Instance #2  Version unknown · /tmp/aic-demo-home/.local/share/appimages/apps/keepassxc/2  [Launch] [Open folder]`.
- Source strings seen: "GitHub releases: owner/repo" (KeePassXC, Zulip, Session, Joplin, Cryptomator, qnote, Standard Notes, Feather `feather-wallet/feather`). "Pinned download\n<url>" (Mattermost, Electrum-BTC, Electrum-LTC). "Download without a published checksum\n<url>" (LM Studio, AnythingLLM).
- **Launch** on KeePassXC #1: `launched.log` = "fake keepassxc instance 1 started", and the process `/bin/sh …/keepassxc/1/KeePassXC.AppImage` was running. The dialog stayed open.
- **Open folder** on KeePassXC #1: `/usr/bin/nautilus --gapplication-service` started. Its AT-SPI window was `[frame] '1'`, which is the keepassxc/1 folder.
- **Why it does not hold — the latest version.** "Latest release" shows a version only for the 3 pinned apps (Mattermost `6.0.4`, Electrum-BTC `4.7.2`, Electrum-LTC `4.2.2.1`). For the other 11 apps it reads `Not known yet`: every GitHub-releases app, every no-checksum app, and Jan (source not captured). This happens even though their cards show a version: KeePassXC's card says `v2.7.12`, and its dialog says `Latest release: Not known yet`. The 11 are listed under Finding B1.
  - Waiting does not fill it in. The KeePassXC dialog was left open for 30 s on the Xvfb instance, and the row still read `Not known yet` (keepassxc-30s.png). The VM can reach GitHub: `curl https://api.github.com/repos/keepassxreboot/keepassxc/releases/latest` returned 200.
  - The GUI offers no route to fill it in. The main menu has: Update All Installed, Seed Settings…, Record Seed Pack…, Repair Launchers, Back Up…, Restore…, Clean Download Cache, Remove All Instances…, Purge Everything…, Preferences, Keyboard Shortcuts, About (menu.png). None of them is a "check for updates". The "Updates" filter showed no cards and "Nothing pending — you're up to date".
  - No later stage covers it. The plan's Stage 3 adds a CLI/login update check and notification. It does not mention the detail page's latest-version row.
- Everything else in this Outcome works.

## Outcome 2
> App not installed → the dialog shows the source and verification sections and says "No instances installed — add one on the card".
Verdict: holds
Evidence:
- Pressed `Details for Zulip`. Frame 'Zulip': `Latest release: Not known yet`, `Updates from: GitHub releases: zulip/zulip-desktop`, `Download check: Not checked yet`, Instances list holds one item `No instances installed — add one on the card`. No Launch or Open folder buttons.
- Same empty-state text for Electrum-BTC (Pinned download, `Latest release 4.7.2`), Electrum-LTC, Session, qnote, Standard Notes and AnythingLLM ("Download without a published checksum").

## Outcome 3
> Launch fails (launcher missing, refused Exec, spawn error) → a toast names the app and instance, and the full error goes to the activity log. The dialog stays open.
Verdict: holds
Evidence:
- Launcher missing. I opened `Details for Feather Wallet`; Launch was sensitive. Then `rm appimage-control-feather-1.desktop` and pressed **Launch**.
  - Within 1.2 s a toast appeared in the Feather Wallet frame: label `Could not launch Feather Wallet #1`, buttons `Show log` and `Dismiss`.
  - The frame 'Feather Wallet' was still showing.
  - Pressing `Show log` opened an `Activity log` dialog ("1 error · 0 warnings · 0 successes", filter "Errors"). Its text: `error: feather: feather instance 1: cannot launch: launcher /tmp/aic-demo-home/.local/share/applications/appimage-control-feather-1.desktop: No such file or directory (os error 2)`.
  - No `launched.log` was written.
- Refused Exec. I opened `Details for Jan`. Then I set the launcher to `Exec=/bin/sh -c "touch /tmp/aic-demo-home/PWNED"` and pressed **Launch**.
  - The toast read `Could not launch Jan #1`, and the frame 'Jan' stayed open.
  - The log read `error: jan: jan instance 1: cannot launch: Exec runs /bin/sh, not this instance's own launcher`.
  - `/tmp/aic-demo-home/PWNED` does not exist, so the refused command did not run.
- Spawn error. The launch script was mode 0644. I opened `Details for LM Studio` and pressed **Launch**.
  - The toast read `Could not launch LM Studio #1`, and the frame 'LM Studio' stayed open.
  - The log read `error: lm-studio: lm-studio instance 1: cannot launch: could not start /tmp/aic-demo-home/.local/share/appimages/apps/lm-studio/1/appimage-control-launch.sh: Permission denied (os error 13)`.
- No crash. The GUI process (pid 2983794) stayed up the whole session.
- Related: if the launcher is already missing when the dialog opens (Joplin #1/#2, Cryptomator #1), **Launch** is insensitive, with the tooltip `The launcher is missing — use Repair Launchers`. Nothing to toast there.

## Outcome 4
> Instance folder missing → **Open folder** is insensitive, with a tooltip saying the folder is missing.
Verdict: holds
Evidence:
- `apps/keepassxc/2` was deleted, and its launcher `appimage-control-keepassxc-2.desktop` was left behind. The card still showed `1 2 + · 2 instances`.
- Details for KeePassXC, row `Instance #2  Version unknown · /tmp/aic-demo-home/.local/share/appimages/apps/keepassxc/2`:
  - `[button] 'Open folder' desc='The folder is missing' INSENSITIVE`
  - `[button] 'Launch' desc='The AppImage is missing — reinstall this instance' INSENSITIVE`
  - Both buttons are greyed out in keepassxc.png.
- Instance #1 in the same dialog kept both buttons sensitive.

## Outcome 5
> Unverified cache → the verification row says "Not verified — installed with your permission" or "Not checked yet", never "Verified".
Verdict: holds
Evidence:
- Mattermost has the sidecar `<64 hex> unverified user-allowed`. Its Download check row reads `Not verified — installed with your permission` (AT-SPI label, and mattermost.png).
- KeePassXC has no sidecar. Its Download check row reads `Not checked yet`. All other apps, which have no sidecar, also read `Not checked yet`.
- No dialog among the 14 showed the word "Verified" in its Download check row.

## Findings

Blocking
- B1 — Outcome 1: "Latest release" shows no version for 11 of the 14 catalog apps. The 11 are the 8 GitHub-releases apps (KeePassXC, Feather Wallet, Zulip, Session, Joplin, Cryptomator, qnote, Standard Notes), the 2 no-checksum apps (LM Studio, AnythingLLM) and Jan.
  - The row reads `Not known yet`, while the same app's card shows a version (KeePassXC: card `v2.7.12`).
  - It does not fill in while the dialog stays open, and the network works.
  - The GUI has no update-check action that could fill it.
  - No later stage in the plan changes the detail page.

Material
- none

Minor
- M1 — The Activity log reached through a toast's **Show log** keeps only the latest failure. After Feather's launch error, the Jan launch error's log read "1 line · 1 error" and held only the Jan line; Feather's error was gone. I did not determine whether this is intended (per-operation log). The main menu has no Activity log item, so the user cannot reach older errors.
- M2 — The row of a missing-folder instance shows no size ("Version unknown · <folder>"). That is reasonable, but it differs from the other rows' layout. Cosmetic only.
- M3 — `repair-launchers` and the GUI's startup repair wrote no launcher for the joplin and cryptomator fixture installs. Their Launch buttons then sit insensitive ("The launcher is missing — use Repair Launchers"). This is outside Stage 2's scope, but it means a user with those installs cannot launch from the detail page. I did not press the menu's Repair Launchers to check whether that repairs them.

Cleanup: both GUI instances stopped (live session and Xvfb :7). Xvfb was stopped. Fixture stub processes were killed (none remain). Removed `/tmp/aic-demo-home`, `/tmp/aic-demo-shots`, all `/tmp/aic-demo-*` scripts and any `/tmp/appimage-control-isolated.*` left on the VM. Nothing was added under /tmp/aic-src. Nautilus, started by Open folder, was stopped.

## Re-check after round 1 (Outcome 1)

Environment: the same Ubuntu 26.04 test VM and live GNOME Wayland session. Host HEAD was 56383c19 ("Stage 2 gate round 1: fix the review and demo findings"). Built with `sh /tmp/aic-vm.sh 'cargo build -p appimage-control -p appimage_control_core --bin appimage-control --bin repair-launchers'`, which finished without errors. The evaluator did not read the source or the diff.

Setup:
- Throwaway HOME `/tmp/aic-rc-home`, with XDG_CONFIG/DATA/CACHE_HOME inside it.
- The GUI ran with `XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus GTK_A11Y=atspi LANGUAGE=en APPIMAGE_CONTROL_CATALOG=/tmp/aic-src/data/catalog.json`.
- One fixture install: `apps/keepassxc/1/KeePassXC.AppImage`, an executable shell script that appends to `$HOME/launched.log` and runs `sleep 300`. A `.appimage-control-version` file holds `2.7.12`. `repair-launchers` then printed `keepassxc instance 1: launcher repaired`.
- Hands: a pyatspi script on the VM. It read every card's labels. It pressed each `Details for <name>` button and dumped the dialog's rows. Then it pressed the dialog's Close button.
- GitHub API rate limit before starting: 60/60 remaining.

### (a) GitHub unreachable
Command: the GUI started with `HTTPS_PROXY=http://127.0.0.1:9 https_proxy=http://127.0.0.1:9`, probed after 8 s.
Verdict: holds
What I saw. Card version label, then the dialog's "Latest release" row:

| App | Card | Latest release row | Updates from |
|---|---|---|---|
| Cryptomator | v1.19.2 | `1.19.2` + `GitHub could not be checked, so newer releases may exist` | GitHub releases: cryptomator/cryptomator |
| Feather Wallet | v2.8.1 | `2.8.1` + same note | GitHub releases: feather-wallet/feather |
| Jan | v0.7.9 | `0.7.9` + same note | GitHub releases: janhq/jan |
| Joplin | v3.5.13 | `3.5.13` + same note | GitHub releases: laurent22/joplin |
| KeePassXC | v2.7.12 · 0.0 MB | `2.7.12` + same note | GitHub releases: keepassxreboot/keepassxc |
| Session | v1.17.17 | `1.17.17` + same note | GitHub releases: session-foundation/session-desktop |
| Standard Notes | v3.201.21 | `3.201.21` + same note | GitHub releases: standardnotes/app |
| Zulip | v5.12.3 | `5.12.3` + same note | GitHub releases: zulip/zulip-desktop |
| qnote | v0.2.11 | `0.2.11` + same note | GitHub releases: Omibranch/qnote |
| Electrum-BTC | v4.7.2 | `4.7.2` (no note) | Pinned download + URL |
| Electrum-LTC | v4.2.2.1 | `4.2.2.1` (no note) | Pinned download + URL |
| Mattermost | v6.0.4 | `6.0.4` (no note) | Pinned download + URL |
| LM Studio | `v—  ·  single instance` | `Not known yet` | Download without a published checksum + URL |
| AnythingLLM | `v—` | `Not known yet` | Download without a published checksum + URL |

- All 14 cards have a `Details for <name>` button. Each opened a toplevel frame named exactly after the app, and each closed with Close.
- 12 of 14 rows match their card's version. All 9 GitHub-sourced apps carry the "GitHub could not be checked" note. Jan now shows its source (`GitHub releases: janhq/jan`).
- The 2 no-checksum apps read `Not known yet`. Their cards show no version either (`v—`). The card and the dialog agree, so the old mismatch is gone for them too.
- Every dialog's Download check row read `Not checked yet`.
- The GUI's stdout/stderr log was empty.

### (b) Normal network
Verdict: not checkable on this VM. The reason is the VM's per-app firewall, not the rate limit.
What I saw:
- Start 1 had no proxy variables. It was probed at about 4 s (KeePassXC only) and again at about 40 s (all 14). Every GitHub-sourced app still showed the card version with `GitHub could not be checked, so newer releases may exist`. The values were the same as in the (a) table.
- GitHub itself reports newer releases. `curl -sI https://github.com/<repo>/releases/latest` (no API quota) gave: keepassxc `2.8.0`, cryptomator `1.19.6`, feather `2.9.1`, jan `v0.8.8`, session-desktop `v1.18.3`, standardnotes `@standardnotes/desktop@3.202.11`, zulip-desktop `v5.14.2`. The API gave joplin `v3.7.22`, and `Not Found` for Omibranch/qnote's latest release.
- Rate limit after start 1: 60/60 remaining, 0 used. The GUI made no GitHub API call that counted.
- Start 2 put the GUI behind a pass-through logging CONNECT proxy (`HTTPS_PROXY=http://127.0.0.1:18080`). Its sockets were sampled with `ss -tnp` every 0.1 s for 40 s.
  - The proxy was healthy: a `curl` through it returned 200, and so did a python client.
  - The GUI opened 5 connections to `127.0.0.1:18080`, and all 5 stayed in `SYN-SENT`. The proxy never received a request from it.
  - The dialogs again showed the card version plus the note.
- Cause: the VM's nftables table of a per-app network firewall sends every new connection to NFQUEUE 77. That queue is read by the root daemon of that firewall. Its journal logged `new subject queued for classification: bin:/tmp/aic-src/target/debug/appimage-control` at 19:05:12 and 19:07:47, once per start. The unclassified dev binary gets no network on this VM, loopback included. I did not change the VM's firewall.
- So no run of the GUI on this VM can show a release fetched from GitHub. This also applies to the original demo's "the network works" claim: it was only true for curl.
- Rate limit at the end: 58/60 remaining. The 2 used were my own API curls for joplin and qnote.

### (c) Instance row, Launch, Open folder
Verdict: holds
What I saw (offline run):
- KeePassXC dialog, Instances: `Instance #1` / `Version 2.7.12  ·  0.0 MB  ·  /tmp/aic-rc-home/.local/share/appimages/apps/keepassxc/1`. It has buttons `Launch instance #1` (visible text "Launch") and `Open the folder of instance #1` (visible text "Open folder").
- **Launch**: the launcher script `appimage-control-launch.sh` started, and the fixture's `sleep 300` ran as its child. The fixture wrote `fake keepassxc instance 1 started`. The log landed in `apps/keepassxc/1/KeePassXC.AppImage.home/launched.log`, because the launcher points HOME at the instance's portable home. The dialog stayed open.
- **Open folder**: `/usr/bin/nautilus --gapplication-service` started with an AT-SPI frame named `1`, which is the instance folder.

### Findings (re-check)

Blocking
- none. B1 is fixed for the offline case. No dialog shows `Not known yet` while its card shows a version.

Material
- none from the product. Environment note: (b) is unverified. Seeing a GitHub-fetched release in the dialog needs a host where the firewall lets the dev binary through, or where the binary has been classified.

Minor
- M4: the note says "GitHub could not be checked" even while the lookup may still be running. At about 4 s into start 1, with the GUI's connections still pending (`SYN-SENT` all through the 40 s window in start 2), KeePassXC already showed the note. "Still checking" would be more accurate. Because the network was blocked, I could not see whether the dialog updates once a lookup succeeds.
- M5: the no-checksum apps' cards show `v—`, a "v" prefix in front of a dash. This is cosmetic.

Cleanup: the GUI was stopped after each run. The fixture's `sleep 300` and its launcher script were killed. Nautilus and the logging proxy were stopped. I removed `/tmp/aic-rc-home` and every `/tmp/aic-rc-*` script and log on the VM. No `/tmp/appimage-control-isolated.*` was left behind. Nothing was added under /tmp/aic-src beyond the rsync and build. No background job is still running.
