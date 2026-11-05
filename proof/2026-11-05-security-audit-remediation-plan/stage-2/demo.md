# Stage 2 demo — 2026-11-05-security-audit-remediation-plan

Environment: Ubuntu 26.04 test VM (user@192.168.122.204), branch tree synced at `~/aic`, built with `cd ~/aic && bash -lc 'cargo build -p appimage-control'` (debug, 10.8 s). Two binaries used: the dev build `~/aic/target/debug/appimage-control`, and a copy of it outside `target/` (`$S/bin/appimage-control`) standing in for an installed binary (no dev-tree walk). Every run used a scratch `HOME` under `/tmp/aicdemo.uj1a/` and `https_proxy/ALL_PROXY=http://127.0.0.1:9` so nothing could download. Fake AppImages are small `#!/bin/sh` scripts that log `$0` and args, placed in `~/.local/share/appimages/cache/<id>/` so installs use the cache. GUI driven on the live Wayland session over AT-SPI (pyatspi). Outcome 6 was run as a Rust program outside the repo depending on `crates/core` by path (built on the host, run on the VM). `/usr/share/appimage-control/catalog.json` and `~/aic/data/catalog.json` were changed during Outcome 2/3 and restored byte-identical (`cmp` ok, owner/mode/mtime kept). VM scratch removed at the end; no GUI process left running.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: the shipped catalog and every shipped and companion seed pack load unchanged; installs, resets, uninstalls and updates behave as before.
Verdict: holds
Evidence:
- Shipped catalog: `HOME=$(mktemp -d) appimage-control --list` → 14 apps (feather … zulip), exit 0, no warnings. `/usr/share/appimage-control/catalog.json` on the VM is byte-identical to `data/catalog.json`; loading it shows `Feather Wallet` with no warnings.
- Seed packs: the repo ships no pack (`data/seeds/` holds only `example.json.txt`). Companion packs (copied from `~/dev/infra/installers/linux/appimage-control-seed-packs/`) via `APPIMAGE_CONTROL_SEEDS_DIR=$S/companion appimage-control --list-seed-packs` → `electrum-btc`, `electrum-ltc`, `feather` rows, exit 0. The same three load when installed into `~/.local/share/appimage-control/seeds/`.
- Seeding with the companion feather pack: `appimage-control --seed-settings feather` → `feather instance 1: settings seeded`, `1 seeded, 0 failed`. The written `settings.json` has `torOnlyAllowOnion: true`, `torPrivacyLevel: 2`, `skin: Breeze/Light`, and portmux's `socks5Port`/`useLocalTor`/`proxy`. The electrum-btc and electrum-ltc packs also seed (see Outcome 5).
- CLI install: `appimage-control +feather:2` → `Using cached feather.AppImage`, `Installed feather instance 1 …`, `… 2 …`, exit 0. It writes `apps/feather/{1,2}/feather.AppImage`, `appimage-control-launch.sh`, `.desktop` launchers and icons. `desktop-file-validate` passes. `--list` shows `feather … 2  installed 2.8.1`.
- GUI install: launched the GUI, clicked Feather `+`, then `Apply (1)`. The Activity log shows `Using cached feather.AppImage / Installed feather instance 1 … / auto-seeding …`. The card reads `Installed 1`. The `.desktop` is written with `Exec=…/appimage-control-launch.sh %U`.
- Uninstall: `appimage-control -feather:1` → `Removed feather instance 2 …`. `-feather` → `Removed feather instance 1 …`. Each removal deletes the instance dir and its `.desktop`.
  - GUI route: click chip `1`, then `Apply (1)`, then the dialog "Remove instance data? … Feather Wallet: instance 2", then `Remove and Apply`. The instance is removed.
  - `--clean -y` and `--purge -y` work as their help says. Neither touches `~/.config/<app>`.
- Update: `appimage-control -update` → `feather instance 1: already at catalog version 2.8.1; skipping … Summary: 0 updated, 0 unchanged, 2 skipped`, exit 0. With the version marker forced to 2.8.0 it prints `reconciled install metadata to 2.8.1 (AppImage size matches catalog cache)` and skips. The AppImage md5 is unchanged. A real download update was not run: the VM is offline by design.
- Reset: no CLI or GUI control triggers it. I checked `--help`, all 22 `win.*` actions, the remove dialog, the purge dialog and Preferences→General. Markers in `~/.config/feather`, `~/.cache/feather`, `~/.config/{session,Zulip,joplin-desktop,keepassxc}` survived install, remove (CLI and GUI), `--clean`, `--purge`, `--record-pack-baseline` and `--seed-settings`. Reset is reachable only through the library API (`preview_reset_paths`/`execute_reset`). From the Outcome 6 program, with the shipped catalog: preview for feather = `[~/.cache/feather, ~/.config/feather]`, and `execute_reset` → Ok. Afterwards `~/.config/feather` is gone, while `~/.config` and the sibling `~/.config/keepme` remain.

## Outcome 2
> A catalog file has an `id` or `appimage_filename` that is not one plain name (contains `/`, `..`, is empty, `.`, or has characters outside `A-Za-z0-9._-`) → that catalog file is refused with an error naming the app and field, and loading falls through to the next source (ending at the embedded catalog).
Verdict: holds
Evidence:
- I built 16 bad catalogs from the shipped one, each changing only feather:
  - `id`: `../x`, `/abs`, `a/b`, `""`, `.`, `..`, `x y`, `x$y`
  - `appimage_filename`: `../evil.AppImage`, `/tmp/evil.AppImage`, `a/b.AppImage`, `""`, `.`, `..`, `x y.AppImage`, ``x`y`.AppImage``
- To show the fall-through, `/usr/share/appimage-control/catalog.json` was temporarily set to a valid catalog whose feather display_name is `Feather USRSHARE-MARK`. Then for each bad file: `APPIMAGE_CONTROL_CATALOG=bad.json appimage-control --list` (installed-style binary, cwd = scratch HOME). Every one exits 0, its row 2 is `feather  Feather USRSHARE-MARK …`, and stderr names the app and field. Examples:
  - `warning: APPIMAGE_CONTROL_CATALOG=…/bad00.json failed to parse (invalid catalog: app "../x": field `id` = "../x" must be one plain name (A-Za-z0-9._-, not . or ..)); falling through to next source`
  - `… app "": field `id` = "" gives an empty instance directory name); falling through to next source` (same message for `.` and `..`)
  - `… app "feather": field `appimage_filename` = "../evil.AppImage" must be one plain name …; falling through to next source` (same form for all 8 filename cases, including ``x`y`.AppImage``)
- Bad catalog planted at `/usr/share/appimage-control/catalog.json` (bad00, bad08): `--list` → row 2 `Feather Wallet` (the embedded catalog), exit 0. stderr: `warning: catalog candidate /usr/share/appimage-control/catalog.json failed to parse (invalid catalog: app "feather": field `appimage_filename` = "../evil.AppImage" …); skipping`.
- Maintainer route: bad08 copied over `~/aic/data/catalog.json`, then the dev build runs `--list`. Result: `catalog candidate /home/user/aic/data/catalog.json failed to parse (… field `appimage_filename` = "../evil.AppImage" …); skipping`. Loading falls through to the next source, exit 0. The file was restored afterwards.
- Explicit `--catalog bad08.json --list` → `--catalog '…/bad08.json': invalid catalog JSON: invalid catalog: app "feather": field `appimage_filename` = "../evil.AppImage" must be one plain name …`, exit 1. This matches the documented `--catalog` contract: no fall-through.
- GUI with `APPIMAGE_CONTROL_CATALOG=bad08.json`: the same warning goes to the GUI's stderr, and the window shows the full fallback catalog (`Feather Wallet v2.8.1 Available`). No message appears in the window (see Minor 1).

## Outcome 3
> A catalog `reset_*_dirs` entry is empty, `.`, absolute or contains `..` → the catalog file is refused the same way; reset never deletes a base directory.
Verdict: holds
Evidence:
- 15 bad catalogs: `reset_config_dirs`, `reset_cache_dirs` and `reset_data_dirs` each set to `[""]`, `["."]`, `["/etc"]`, `["a/../.."]` and `[".."]`.
- Via `APPIMAGE_CONTROL_CATALOG`: all 15 are refused and fall through to `/usr/share` (row 2 `Feather USRSHARE-MARK`, exit 0). Example: `… app "feather": field `reset_config_dirs` = "." must be a relative path with a name in it and no `..`); falling through to next source`.
- Planted at `/usr/share` (bad17 `reset_config_dirs ["."]`, bad23 `reset_cache_dirs ["/etc"]`): `… catalog candidate /usr/share/appimage-control/catalog.json failed to parse (… field `reset_cache_dirs` = "/etc" …); skipping`. Row 2 is the embedded `Feather Wallet`.
- Maintainer route: bad19 (`a/../..`) in `~/aic/data/catalog.json` is refused the same way. `--catalog bad20.json` → error naming `reset_config_dirs` = `..`, exit 1.
- "Reset never deletes a base directory": no CLI/GUI trigger exists (Outcome 1), so this was checked through the library (Outcome 6 program).
  - `parse_catalog_json` refuses each of `""`, `.`, `<abs path>`, `a/../..`, `..`, `./`.
  - A caller that bypasses it with `serde_json::from_str::<Catalog>` gets this from `preview_reset_paths`: `warning: reset_config_dirs entry "." for app 'feather' is empty, '.', absolute or contains '..'; skipping`, and preview = `[]`. `execute_reset` → `Err("invalid path: no paths matched catalog hints for this app")`.
  - A snapshot of the whole scratch HOME before and after shows no change. `~/.config`, `~/.cache`, `~/.local/share` and their `keepme` files survive, and so does the absolute target dir.

## Outcome 4
> A `data/catalog.json` or `data/seeds/` in the current directory or its parents → ignored; the GUI launched from `$HOME` uses the installed or embedded catalog.
Verdict: holds
Evidence:
- Planted `$HOME/data/catalog.json` (valid, feather renamed `PLANTED-IN-CWD`) and `$HOME/data/seeds/feather.json` (valid pack with `static: {planted: true}`).
- Positive control: the same files are good input. `APPIMAGE_CONTROL_CATALOG=$HOME/data/catalog.json --list` → `feather  PLANTED-IN-CWD`. `APPIMAGE_CONTROL_SEEDS_DIR=$HOME/data/seeds --list-seed-packs` → `feather  …/home4/data/seeds/feather.json`.
- I ran both the installed-style binary and the dev build, from cwd `$HOME` and from `$HOME/a/b/c`. In all 4 runs `--list` shows `feather  Feather Wallet`, and `--list-seed-packs` prints nothing (exit 0).
- From cwd `$HOME`: `+feather:1` then `--seed-settings feather` → `feather: seeding skipped (unsupported app)`. The planted pack was not used and no `settings.json` was written.
- GUI started with cwd = `$HOME` (confirmed via `/proc/<pid>/cwd -> /tmp/aicdemo.uj1a/home4`), installed-style binary. The first card reads `Feather Wallet v2.8.1 · 0.0 MB Installed 1`. An AT-SPI search for `PLANTED` finds nothing. Main-menu action `win.list_seed_packs` → Activity log `No seed packs loaded.`

## Outcome 5
> A seed pack's `config_path` resolves to an absolute path or contains `..` → the pack is refused at load with an error naming the pack and field; other packs still load.
Verdict: holds
Evidence:
- Each test dir holds the companion `electrum-btc.json` and `electrum-ltc.json`, plus a `feather.json` whose `config_path` is one of: `/etc/aic-evil.json`, `../../../.bashrc`, `{appimage}.config/../../../../evil.json`, `a/../b.json`, `""`.
- `APPIMAGE_CONTROL_SEEDS_DIR=<dir> appimage-control --list-seed-packs` → stdout lists `electrum-btc` and `electrum-ltc`. stderr: `refused	feather	seed pack `feather` (…/feather.json): field `config_path` = "/etc/aic-evil.json" must be a relative path inside the instance directory (not absolute, no `..`, not empty)`. Exit 1. All 5 values behave the same.
- (`~/x.json` is accepted as a plain relative path: it is not expanded, so it is not absolute.)
- Seeding: installed feather, electrum-btc and electrum-ltc, then used a feather pack with `config_path = ../../../../../../escape-target.json` (resolves to `$HOME/escape-target.json`). `--seed-settings feather,electrum-btc,electrum-ltc` → `feather: seeding failed: seed pack `feather` … field `config_path` = "../../../../../../escape-target.json" must be …`. Then `electrum-btc instance 1: settings seeded` and `electrum-ltc instance 1: settings seeded`. Summary `2 seeded, 1 failed`, exit 1. `$HOME/escape-target.json` does not exist. The electrum configs contain the pack's onion server list and proxy.
- GUI with that seeds dir, `win.list_seed_packs` → Activity log: `Loaded seed packs: electrum-btc … electrum-ltc … Refused seed packs (skipped): feather  seed pack `feather` (…): field `config_path` = "../../../../../../escape-target.json" must be …`.

## Outcome 6
> Works: the library's `uninstall_app` and `update_from_file` refuse a launcher whose `Exec=` leaves the managed files dir (through `..` or a symlink) and touch nothing; no CLI or GUI command calls them today.
Verdict: holds
Evidence:
- Library user: a Rust program outside the repo (`libuser`, depends on `appimage_control_core` by path, built with the repo's vendored sources). It runs with `AppPaths::new(<scratch home>)`. Before and after each call it snapshots the whole scratch tree (paths, symlink targets, file contents).
- Control: `install_appimage(Legit.AppImage)` → `Exec=…/appimages/files/Legit.AppImage %U`. `update_from_file(…, new-2.0.AppImage)` → Ok, and the file content is now `echo NEW-VERSION`. `uninstall_app` → Ok; the AppImage and `.desktop` are gone.
- Attacks: install a bait AppImage, then rewrite its launcher's `Exec=`. Victim is `$HOME/victim/victim.AppImage`. Both call orders were run.
  - `..`: `Exec=…/appimages/files/../../../../victim/victim.AppImage`. `uninstall_app` refused: `invalid path: refusing to remove AppImage outside manager directory: …/files/../../../../victim/victim.AppImage`. `update_from_file` refused: `invalid path: managed AppImage is outside manager directory`. Filesystem changes: none, both times.
  - Same with Desktop-Entry quoting (`Exec="…/files/../../../../victim/victim.AppImage" %U`): both refused, no changes.
  - Symlinked dir: `files/linkdir -> $HOME/victim`, `Exec=…/files/linkdir/victim.AppImage`. Both refused, no changes.
  - Symlinked file: `files/linkfile.AppImage -> victim`, `Exec=…/files/linkfile.AppImage`. Both refused, no changes.
  - The victim content stays `echo VICTIM-ORIGINAL` throughout.
- No CLI/GUI caller:
  - `--help` lists no install-file, uninstall-file or update-from-file operation.
  - `appimage-control --uninstall x` → `unknown option '--uninstall' (see --help)`, exit 1.
  - `appimage-control install /tmp/x.AppImage` → `GLib-GIO-CRITICAL **: This application can not open files.`, exit 1.
  - The GUI's 22 window actions contain none (record_seed_pack, backup, repair, about, purge, refresh, preferences, clean_cache, clean, restore_backup, apply, list_seed_packs, update, migrate, shortcuts, update_seed_packs, seed_settings, open_catalog, plus window close/minimize/maximize/default).
- See Minor 2 for a launcher that points directly at an outside file (no `..`, no symlink).

## Outcome 7
> An AppImage path containing a space, `%`, `$`, a quote or a backtick → the written `Exec=` is quoted and escaped per the Desktop Entry spec, and launches that exact file.
Verdict: holds
Evidence:
- The scratch HOME was named literally `/tmp/aicdemo.uj1a/we ird %u %% $HOME 'sq' "dq" `id` \bs`. Every managed path lives under HOME, and `appimage_filename` can no longer carry these characters.
- `appimage-control +feather:1 +electrum-btc:1 +keepassxc:1 +session:1 +qnote:1` → all OK. That covers 5 instance strategies.
- Each `.desktop` gets, for example: `Exec=env "/tmp/aicdemo.uj1a/we ird %%u %%%% \\$HOME 'sq' \\"dq\\" \\`id\\` \\\\bs/.local/share/appimages/apps/feather/1/appimage-control-launch.sh" %U`. `%` is doubled, and `"`, `` ` ``, `$` and `\` are escaped inside the quoted argument, then escaped again for the string level.
- `desktop-file-validate` → 0 on all 5.
- `gio launch <file.desktop>` on each → exit 0. The fake AppImage logged its own `$0` as the exact file, for example `launched argv0=[/tmp/aicdemo.uj1a/we ird %u %% $HOME 'sq' "dq" `id` \bs/.local/share/appimages/apps/feather/1/feather.AppImage] nargs=0`.
  - The literal `%u` in the path did not expand into a URL argument (nargs=0).
  - `` `id` `` was not executed.
  - session got exactly its 3 strategy args.
- With a file argument: `gio launch feather-1.desktop "/tmp/aicdemo.uj1a/doc with space.txt"` → `nargs=1 args=[/tmp/aicdemo.uj1a/doc with space.txt]`.
- `--repair-launchers` after deleting the feather `.desktop` writes an `Exec=` byte-identical to the install-time one.
- For contrast, an ordinary path stays unquoted: `Exec=/tmp/aicdemo.uj1a/home/…/appimage-control-launch.sh %U`.

## Findings

No Blocking findings. All 7 Outcomes hold. Plan goal: `cargo audit --json` on the host reports `vulnerabilities.count = 0` (pipeline exit 0).

- Minor 1 — A refused catalog is reported only on stderr. When the GUI falls through past a refused catalog, the window shows nothing. A user who starts the GUI from the desktop never sees the "refused" error that names the app and field. They just get the next catalog. Refused seed packs, by contrast, do show in the Activity log.
- Minor 2 — `uninstall_app` handles a launcher with a plain absolute outside path inconsistently. Case: `Exec=` points straight at a file outside `files/`, with no `..` and no symlink (`Exec=$HOME/victim/victim.AppImage`). `update_from_file` refuses it ("managed AppImage is outside manager directory"). `uninstall_app` instead returns Ok and deletes the `.desktop` launcher. The outside AppImage is left intact, so nothing outside the app's dirs is harmed. This case is outside the Outcome's `..`/symlink wording, and no CLI/GUI command calls it.
- Minor 3 — Repeated warning. One `--seed-settings feather,electrum-btc,electrum-ltc` run prints the same "seed pack `feather` … skipping this pack" warning 7 times.
- Minor 4 (pre-existing, outside this stage) — A bare non-flag argument such as `appimage-control install /tmp/x.AppImage` prints `GLib-GIO-CRITICAL **: This application can not open files.` and exits 1. `--help` promises that unrecognized arguments get a message pointing to `--help`.
- Note — The README still mentions "CLI `install` copies: `~/.local/share/appimages/files/`", but there is no such CLI command (see Outcome 6).
- Note — Another session was driving the VM's GNOME session at the same time: a portmux closeout run (`closeout-act.sh`, `portmux-gui`). It wrote `/var/lib/portmux/profiles/profiles.json` and `~/closeout-req/*` at 18:10, which this demo did not cause. This demo used `xdotool` once (window activate plus one right-click on the AppImage Control window, about 18:05). That could have shifted focus for that session.
