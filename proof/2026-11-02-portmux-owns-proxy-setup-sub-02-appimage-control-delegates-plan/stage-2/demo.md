# Stage 2 demo — 2026-11-02-portmux-owns-proxy-setup-sub-02-appimage-control-delegates-plan

Environment: Ubuntu 26.04 test VM (`user@192.168.122.204`). GNOME Wayland session 3 on tty2, active. App built from `~/aic` with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 cargo build -p appimage-control`. Checksums of README, the two docs and `crates/*/src/*.rs` on the VM match local f3827c38. Isolated `HOME` for every app run except the wiring script. GUI driven through AT-SPI: Main Menu > item 9 (Preferences) > Network.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: Preferences shows whether portmux is available for proxy setup, and the docs describe `proxy_setup`.
Verdict: holds
Evidence:
Preferences > Network > SEEDING, row text read from the running app:

| Helper | Row text | Pill |
|---|---|---|
| Current portmux as `portmux-port` on `PATH`, no override (the upgraded user's case) | `portmux-port · detected on host` | `connected` |
| `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/tmp/mp-sub1` | `/tmp/mp-sub1 · detected on host` | `connected` |

Docs read as a maintainer:
- `docs/seed-packs.md` has a `proxy_setup` section. `README.md` and `docs/workflows/seeding.md` describe it. `docs/integration-portmux-port.md` has the contract, failure table and migration steps. Its link to `../../portmux/docs/integration-appimage-control.md` resolves.
- Goal sweep `! grep -rnE 'PortLookup|EnsuredPort|socks5_host|feather_use_local_tor|ensure_port|portmux-port ensure' crates data scripts README.md docs/seed-packs.md docs/integration-portmux-port.md docs/workflows` exits 0.

Migration doc followed on the VM, using its own example pack under an isolated `HOME`:
- Old pack in `~/.local/share/appimage-control/seeds/feather.json`, plus a valid `electrum-btc.json` beside it. `appimage-control --list-seed-packs` gives exit 1: `invalid path: seed pack /tmp/demo-mig/.local/share/appimage-control/seeds/feather.json: uses the removed `port` verb at `preset.socks5Port`. portmux now writes proxy settings. Delete the `preset` section, move any non-proxy key to `static`, and add "proxy_setup": { "app": "<recipe id>" }. See docs/integration-portmux-port.md`. The file is named, and no pack loads, as the doc says.
- `--seed-settings feather` is refused the same way, exit 1, with `/tmp/mp-sub1` selected.
- Pack rewritten to the doc's "becomes" form. `--list-seed-packs` gives exit 0 and lists both packs.
- Steps 1 to 6 work as written. Step 7 (reseed one instance) was not run. See Findings.

## Outcome 2
> portmux is missing or old → the Preferences row reads "not detected" with the "missing" pill, as it does today.
Verdict: holds
Evidence:
Same route, row text read from the running app:

| Helper | Row text | Pill |
|---|---|---|
| `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/nonexistent/portmux-port` (missing) | `portmux-port · not detected` | `missing` |
| No override, `/usr/bin/portmux-port` on `PATH` (old) | `portmux-port · not detected` | `missing` |
| `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/usr/bin/portmux-port` (old) | `portmux-port · not detected` | `missing` |

`/usr/bin/portmux-port configure-app --help` gives exit 1: `portmux-port: unknown subcommand `configure-app` — see --help`. `/tmp/mp-sub1 configure-app --help` gives exit 0.

CLI agrees, `appimage-control --seed-settings feather` with a migrated pack:
- Missing: exit 1. `Could not start seeding helper: No such file or directory (os error 2).` then `A required system helper for seeding is not installed, or is too old. Install or upgrade the support package with:` / `sudo apt install portmux`.
- Old: exit 1. `Seeding helper exited 1 on the host.` then the same hint.

No crash or panic in any run.

## Findings

Blocking: none.

Material: none.

Minor:
1. A refused pack's error starts with `invalid path:`. The path is fine; the pack's content is the problem. The doc's failure table shows the message as starting `seed pack <file>:`.
2. The doc says a missing and an old portmux "get the same message". The hint is the same, but a different line comes first (`Could not start seeding helper: …` or `Seeding helper exited 1 on the host.`). The second looks like the table's `Seeding helper exited <n>: …` row, which has a different fix.

Not exercised:
- Migration step 7 and a real seed. `+feather` failed on the VM (`download failed: error sending request for url (https://github.com/feather-wallet/feather/releases/download/2.8.1/feather-2.8.1.AppImage)`), so no instance exists to reseed. The real binary's `configure-app` without `--write-only` allocates a Tor listener, which is off limits here. Neither Outcome names this, but the judgment gate on the migration doc rests on steps 1 to 6 only.
- Premise on the GitHub runner: still unchecked. `scripts/verify-portmux-port-wiring.sh` on the VM, run as CI runs it, gave exit 1 with 7 passes, 1 failure, 1 warning. The failure is `cargo test -p appimage_control_core --lib`: 230 passed, 1 failed. The failing test is `instances::tests::ensure_cached_replaces_stale_file_on_successful_download`, with `DownloadFailed("error sending request for url (http://127.0.0.1:36785/feather-2.8.1.AppImage)")`. Project notes already record that this VM's loopback refuses core's local-HTTP tests. I did not confirm that cause. The warning is the absent portmux sibling, which `ci.yml` expects. The wiring checks 4a, 4b, 4c and 5 all passed.
