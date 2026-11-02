# Stage 1 demo — 2026-11-02-portmux-owns-proxy-setup-sub-02-appimage-control-delegates-plan

Environment: Ubuntu test VM (user@192.168.122.204), `cargo build -p appimage-control` run once from the synced tree, binary `~/aic/target/debug/appimage-control`, driven from a shell. No source or diff read.

Common set-up for every run: a `mktemp -d` sandbox `$T` with `HOME=$T/home`, `TORRC=$T/etc/torrc` (empty, mode 664), `PORTMUX_DISABLE_RELOAD=1`, `PORTMUX_DROPIN_LOCK=$T/lock`, `APPIMAGE_CONTROL_SEEDS_DIR=$T/seeds`, `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/tmp/mp-sub1` unless stated. Never sudo; `/etc/tor/torrc` mtime stayed at 2026-10-17. Test data removed from the VM afterwards.

Stand-ins and limits, stated up front:
- Wallets were real: Feather 2.8.1 and Electrum 4.7.2 AppImages, under the real `xvfb-run`. No shell-script stand-in was used for any result below.
- I placed the wallet in the app's download cache (`~/.local/share/appimages/cache/<app>/<file>`, fetched with curl), because the app's own download failed (see Minor 5).
- Route was the CLI only. The GUI was not driven.
- Packs: `$T/seeds/feather.json` as written below, or the companion repo's real packs copied from `~/dev/infra/installers/linux/appimage-control-seed-packs/`.
- tor was not reloaded (safety rule), so nothing listened on a seeded port.

## Outcome 1
> Works: the seeded wallet is proxied through its own port, and every non-proxy setting from the pack still lands.
Verdict: holds
Evidence:
Limit: this is shown by the settings the wallet reads and by the tor config, not by observed traffic (see "Live traffic" below).

The real feather pack with `preset` swapped for `"proxy_setup":{"app":"feather"}`, unchanged otherwise (`static` proxy/torOnlyAllowOnion/torPrivacyLevel, `rotation` skin, `icons`):
```
$ appimage-control +feather:2 --seed-new
Using cached feather.AppImage
Installed feather instance 1 …
Installed feather instance 2 …
feather: auto-seeding 2 new instance(s)
feather instance 1: seeding settings …
feather instance 1: settings seeded
feather instance 2: seeding settings …
feather instance 2: settings seeded
rc=0
```
Instance 1 `feather.AppImage.config/feather/settings.json`: `"proxy": 1, "skin": "Breeze/Light", "socks5Port": "9084", "torOnlyAllowOnion": true, "torPrivacyLevel": 2, "useLocalTor": true`.
Instance 2: `'proxy': 1, 'skin': 'Breeze/Dark', 'socks5Port': '9085', 'torOnlyAllowOnion': True, 'torPrivacyLevel': 2, 'useLocalTor': True`.
`$T/etc/torrc.d/50-portmux.conf`: `SocksPort 9084 IsolateDestAddr` for feather #1 and `SocksPort 9085 IsolateDestAddr` for feather #2.

After launching instance 1 by its launcher for 45 s under xvfb, the real wallet kept the values: `{'proxy': 1, 'socks5Port': '9084', 'useLocalTor': True, 'torOnlyAllowOnion': True, 'torPrivacyLevel': 2, 'skin': 'Breeze/Light'}`.

Other settings, other pack: `"static":{"checkForUpdates":false,"darkMode":true,"lang":"de","nested":{"a":1,"b":[1,2]}, …}` with `"rotation":{"color":{"values":["red","green","blue"]}}` landed whole, including the nested object, with color red on instance 1 and green on instance 2.

Re-seed: `appimage-control --seed-settings feather` → `Seed settings summary: 2 seeded, 0 failed, 0 unsupported, 0 missing.`, rc=0, same two SocksPort lines, no new port. It printed `[portmux-port] WARNING: feather is assigned port 9084 but nothing listens on it…`, which is expected with reload disabled.

Second real wallet, the real electrum-btc pack with `preset` swapped for `proxy_setup`:
`appimage-control +electrum-btc:2 --seed-new` → rc=0. Config 1: `'proxy': 'socks5:127.0.0.1:9084', 'oneserver': True, 'server': 'nuzzg3…onion:50002:s'`, 9 bookmarks. Config 2: the same with `socks5:127.0.0.1:9085`.

Live traffic: not observed. A local listener on the seeded ports saw no connection from Feather (45 s, first-run wizard) or Electrum (50 s, twice, with and without `oneserver`). A wallet at its first-run screen may simply not connect yet. I did not create a wallet, so this neither confirms nor refutes the settings above.

## Outcome 2
> portmux is missing or older than `configure-app` → the install hint appears once, seeding is skipped, and installed instances stay installed.
Verdict: holds
Evidence:
Older real portmux, `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/usr/bin/portmux-port` (its answer to `configure-app --help`: `unknown subcommand`):
```
$ appimage-control +feather:2 --seed-new
Using cached feather.AppImage
Installed feather instance 1 …
Installed feather instance 2 …
Seeding helper exited 1 on the host.
A required system helper for seeding is not installed, or is too old. Install or upgrade the support package with:
sudo apt install portmux
Power users can point at a non-default location by setting APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/binary.
Seeding skipped for 2 new instance(s): required system helper unavailable.
rc=1
```
`--list` then shows `feather  Feather Wallet  2  installed 2.8.1`. Both instance dirs keep their AppImage and launcher. No `settings.json` exists. Torrc size is 0.

Same hint, once, rc=1, for `--seed-settings feather`, for `--seed-settings all`, and for `--seed-settings feather,electrum-btc` (`apt-hint count: 1` with two apps selected).

Missing: `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=$T/nonexistent`:
`--seed-settings feather` → `Could not start seeding helper: No such file or directory (os error 2).`, then the same hint once, rc=1.
`+feather --seed-new` → `Installed feather instance 3 …`, hint once, `Seeding skipped for 1 new instance(s)…`, rc=1. `--list` shows 3 instances.

Missing by PATH: variable unset, PATH without `portmux-port` (`which` finds none): same `Could not start…` line and hint once, rc=1. With variable unset and default PATH it finds the older `/usr/bin/portmux-port` and gives the older-version output.

## Outcome 3
> Group membership is not active yet → the instance is reported failed with portmux's own message about logging out and in, and its first-run config is unchanged.
Verdict: holds
Evidence:
Caused by `chmod 000 $TORRC` with the real portmux `/tmp/mp-sub1`. This is the route the brief names, not an actual group change. Config made by running the real wallet once by its launcher: only `walletDirectory`.
```
$ sha256sum settings.json   → 7da8e38f…7621, mtime 18:36:35.380492692
$ appimage-control --seed-settings feather
feather instance 1: seeding settings …
feather instance 1: seeding failed: invalid path: Seeding helper exited 1: portmux-port: cannot change tor's config: main torrc at …/etc/torrc exists but is unreadable — refusing to treat it as absent (DEC-MT-013): Permission denied (os error 13). If you were just added to the portmux group, log out and in. Otherwise run `sudo portmux-port configure-app --app feather --instance 1 --allocate-only`, then this command with --write-only
Seed settings summary: 0 seeded, 1 failed, 0 unsupported, 0 missing.
rc=1
$ sha256sum settings.json   → 7da8e38f…7621, mtime 18:36:35.380492692 (same)
```
Running `/tmp/mp-sub1 configure-app … --json-v2` directly gives the same message text, so the wording is portmux's.

Fresh instance: `+feather --seed-new` under the same denial → same failure for instance 2, rc=1, instance stays installed, config holds only `walletDirectory` (no `lang`, `proxy` or `socks5Port`).

Recovery: after `chmod 664 $TORRC`, `--seed-settings feather` → `2 seeded, 0 failed`, rc=0, with ports 9084 and 9085 and the pack's settings in place.

## Outcome 4
> Pool exhausted → the instance is reported failed with portmux's message, and its first-run config is unchanged.
Verdict: holds
Evidence:
Real exhaustion, no stub: I filled the pool in the sandbox `TORRC` only, using the real portmux (`ensure --app filler$n --purpose socks --isolation none --no-reload`). It stopped at n=17: `portmux-port: no free port in pool 9050-9099 for filler17`.
```
$ sha256sum settings.json   → 956f26b6…f71c, mtime 18:37:19.203075687
$ appimage-control --seed-settings feather
feather instance 1: seeding settings …
feather instance 1: seeding failed: invalid path: Seeding helper: no free port for feather#1 — portmux-port: no free port in pool 9050-9099 for feather instance 1
Seed settings summary: 0 seeded, 1 failed, 0 unsupported, 0 missing.
rc=1
$ sha256sum settings.json   → 956f26b6…f71c, same mtime
```
`+feather --seed-new` under the same full pool: same message for feather#2, rc=1, config holds only `walletDirectory`, no feather line in the drop-in.

Stub as well (exit 2, message on stderr, exit 0 for `configure-app --help`): `+feather --seed-new` → `seeding failed: invalid path: Seeding helper: no free port for feather#1 — portmux-port: STUB port pool exhausted: all ports in the pool are in use`, rc=1, config holds only `walletDirectory`. The stub recorded the call: `configure-app --app feather --instance 1 --config …/settings.json --desktop-app-id appimage-control-feather-1 --json-v2`.

## Outcome 5
> The pack still uses `port` or `preset` → the pack is refused by name, with a message saying how to migrate it.
Verdict: holds
Evidence:
The real, unmigrated `feather.json` from the companion repo (a `port` verb inside `preset`):
```
$ appimage-control --seed-settings feather
invalid path: seed pack …/seeds/feather.json: uses the removed `port` verb at `preset.socks5Host`. portmux now writes proxy settings. Delete the `preset` section, move any non-proxy key to `static`, and add "proxy_setup": { "app": "<recipe id>" }. See docs/integration-portmux-port.md
rc=1
```
`--list-seed-packs` gives the same line, rc=1. No config is written.

An empty `"preset":{}` beside `proxy_setup` → `uses the removed `preset` section…` with the same steps, rc=1.
`+feather --seed-new` with the real pack → instance 2 is installed, then `feather instance 2: seeding failed: invalid path: seed pack …: uses the removed `port` verb…`, rc=1. `--list` shows 2 instances.
The real `electrum-btc.json` → `uses the removed `port` verb at `preset.proxy`…`.
The same pack in the user dir `~/.local/share/appimage-control/seeds/`, with `APPIMAGE_CONTROL_SEEDS_DIR` unset → same refusal, naming that path. It does not fall back to anything else.
Plain keys named `port` and `preset` inside `static` are not refused and land as literal values, which is correct.

Two other shapes are refused without the migration steps: see Minor 2.

## Other findings

Blocking: none.

Material:
1. A pack's `static` silently overrides what portmux wrote. Pack with `"static":{…,"socks5Port":"1234","proxy":0}` and `proxy_setup`: `+feather:2 --seed-new` printed `settings seeded`, rc=0, and both instances ended with `"proxy": 0, "socks5Port": "1234"`, while torrc holds 9084 and 9085. The wallet is left with proxy mode 0 and a shared wrong port, and the user is told it worked. The real packs do not do this, apart from a harmless `"proxy": 1`.
2. One refused pack stops seeding for every app. Good feather pack beside the unmigrated `electrum-btc.json`: `--seed-settings feather` fails on the electrum pack, rc=1, and feather gets no config. `--list-seed-packs` prints only the error, rc=1. A person with one stale private pack cannot seed any wallet until they fix it.

Minor:
1. Every seeding failure starts with `invalid path:`, including pool exhaustion and group denial. Nothing is wrong with a path, and it sits in front of portmux's message.
2. Two other old-style shapes get a parser error, with no migration steps. Top-level `"port":{…}`: `unknown field `port`, expected one of …` (names it and lists `proxy_setup`, but gives no steps). `port` under `rotation`: `data did not match any variant of untagged enum RotationValue` (does not name it). I do not know that either shape was ever valid; the real packs use the `preset` form, which is handled.
3. The refusal sends the reader to `docs/integration-portmux-port.md`, which has no `proxy_setup` or `preset` text yet. Plan Task 2.3 covers it. Until then the message's own steps are all the reader has. The path is also relative to the repo, which an installed user does not have.
4. With the unchanged electrum-btc `static` (`oneserver`), Electrum logged on launch: `both "oneserver" and "auto_connect" options enabled, disabling "auto_connect" and resetting "server"` and `falling back to localhost:1:s`. The pack's onion server lands in the file but Electrum discards it at run time. Likely not caused by this stage, since these `static` values are the pack's own. I did not compare with the earlier build.
5. The app's built-in download failed in the sandbox: `download failed: error sending request for url (…)`, for both github.com and a local http server, where curl from the same shell succeeded and the local server logged no request from the app. Cause not established (`strace` is not on the VM), and I did not compare with the earlier build. It may belong to the sandbox or the VM. Worth one check in a real session, because a person hitting it cannot install a wallet at all.
6. The real packs in the companion repo still use `preset` (3 of 3, 0 use `proxy_setup`), so they are all refused today. Sub-plan 3 Stage 1 migrates them. Until it lands, anyone holding those packs cannot seed.

Not exercised: the GUI route (Who names it; Outcomes judged by the CLI), and live traffic through tor.
