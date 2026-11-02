# Integration: appimage-control ← portmux-port

Seeding hands all proxy setup to the host's `portmux-port configure-app`
command (shipped by the `portmux` `.deb`). portmux picks the port, writes it
into the app's own config, and keeps its own record of the assignment. This app
holds no port, proxy host or isolation class. This document is the consumer-side
note; the producer-side contract lives in
[`portmux/docs/integration-appimage-control.md`](../../portmux/docs/integration-appimage-control.md).

## What a seed runs

For Feather instance 4, after the first run has stopped and created its config:

```bash
portmux-port configure-app --app feather --instance 4 \
    --config <instance dir>/<config_path> \
    --desktop-app-id appimage-control-feather-4 --json-v2
```

Then the pack's `static` and `rotation` keys are written, and portmux runs once
more with `--write-only`, so its proxy settings win over a pack key of the same
name. Any failure restores the config the first run left.

A pack with no `proxy_setup` never calls portmux, and that app gets no proxy
from seeding.

## Required runtime dependency

`portmux-port` with the `configure-app` command must be on the host's `PATH`
(resolved through `flatpak-spawn --host` when this app runs in a Flatpak
sandbox). `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/binary` overrides the
location.

The probe is `portmux-port configure-app --help`. It runs once per process,
before the seed loop. A missing portmux and one too old to have the command get
the same message:

> A required system helper for seeding is not installed, or is too old. Install
> or upgrade the support package with:
>   `sudo apt install portmux`

Seeding is then skipped, and instances already installed stay installed.
Preferences shows the same probe's result: the **Proxy setup via** row reads
`portmux-port · not detected` with a `missing` pill.

## Migrating a private seed pack

Packs that still use `preset` or `port` are refused, and while one is refused no
pack in that directory loads. This is deliberate: ignoring those keys would leave
a wallet with no proxy and no warning.

1. **Upgrade portmux**, so that `portmux-port configure-app --help` exits 0.
2. **Find the pack.** Run `appimage-control --list-seed-packs`. The error names
   the file. Per-user packs live in `~/.local/share/appimage-control/seeds/`.
3. **Sort the keys under `preset`.** Each key is one of two kinds:
   - A proxy setting (host, port, "use local Tor", or any key with a `port`
     body). Delete it. portmux writes these now.
   - Anything else. Move it to `static` as a literal value. A `from` or `format`
     value has no literal form in the old pack, so write the value you want.
4. **Delete the `preset` section.**
5. **Add `proxy_setup`** with the recipe for the app:

   | Pack's app | `proxy_setup.app` |
   |---|---|
   | Feather | `feather` |
   | Electrum (Bitcoin) | `electrum-btc` |
   | Electrum (Litecoin) | `electrum-ltc` |

   Seeding patches a JSON config, so these three are the recipes a pack can use
   today. portmux's other recipes write other file formats.
6. **Check it loads.** `appimage-control --list-seed-packs` lists the pack with
   no error.
7. **Reseed one instance** with `appimage-control --seed-settings <app id>`, and
   check that the config carries portmux's proxy settings and your `static`
   keys.

For example, this pack:

```json
{
  "config_path": "{appimage}.config/feather/settings.json",
  "static": { "compactMode": true },
  "preset": {
    "socks5Port": { "port": { "isolation": "dest-addr", "field": "port", "as_string": true } },
    "themeId": { "format": "dark" }
  }
}
```

becomes:

```json
{
  "config_path": "{appimage}.config/feather/settings.json",
  "static": { "compactMode": true, "themeId": "dark" },
  "proxy_setup": { "app": "feather" }
}
```

## Test override

Tests and ad-hoc smoke runs point at a stub binary through
`APPIMAGE_CONTROL_PORTMUX_PORT_BIN`. The stub must exit 0 on
`configure-app --help`, and print a JSON object on stdout for a real call. An
optional `manual_action` string in it is shown to the user. See `portmux_port.rs`
and the `proxy_setup` tests in `seed.rs` for the canonical fakes.

`crates/core/tests/real_portmux_configure_app.rs` seeds through a real binary. It
runs only when that variable is set, and reports itself skipped otherwise.

## Failure modes

An instance that fails keeps its first-run config byte-identical, and the seed
report counts it as failed.

| Message | Cause | Fix |
|---|---|---|
| The install hint above | portmux missing, or older than `configure-app` | `sudo apt install portmux` |
| `Seeding helper: no free port for <app>#<n> — …` | Exit 2: the port pool is exhausted | Disable an unused managed entry in portmux |
| `Seeding helper exited <n>: …` | Any other failure. portmux's own message follows, for example about logging out and in after joining the `portmux` group | Do what the message says, then reseed |
| `seed pack <file>: uses the removed …` | The pack has not been migrated | See **Migrating a private seed pack** |

## Backwards compatibility

Instances seeded before this change keep the proxy settings they have until they
are next reseeded. A reseed asks portmux, which keeps an existing assignment for
the same app and instance. Existing on-disk wallet data is untouched.
