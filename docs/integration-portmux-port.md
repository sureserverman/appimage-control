# Integration: appimage-control ← portmux-port

Per-app SOCKS / HTTPTunnel port assignment for seed packs is
delegated to the host's `portmux-port` CLI (shipped by the
`portmux` `.deb`). This document is the consumer-side note;
the producer-side contract lives in
[`portmux/docs/integration-appimage-control.md`](../../portmux/docs/integration-appimage-control.md).

## Why a separate tool

- One source of truth across both desktop platforms:
  - **Ubuntu**: `/etc/tor/torrc` managed-block lines (created /
    edited by portmux when needed, with `systemctl reload tor`
    fired only when the listener fingerprint actually changes).
  - **Whonix-Workstation**: `~/.config/portmux/whonix-ports.conf`
    (annotation-only, never reloads anything — the gateway
    publishes listeners independently).
- Unbounded instance count. Feather 1 / 2 / 3 / … / 12+ each get
  their own port; the legacy `feather_ports: [u16; 3]` cap is
  gone.
- Cross-tool consistency. The portmux GUI's port editor sees the
  exact same annotated entries this pipeline writes.

## Required runtime dependency

`portmux-port` must be on the host's `PATH` (resolved through
`flatpak-spawn --host` when this app runs in a Flatpak sandbox).

On a host where portmux isn't installed, the first seed call
fails with:

> portmux-port is the host-side helper that allocates SOCKS /
> HTTPTunnel ports for seeded apps. Install with:
>   `sudo apt install portmux`
> or set `APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/path/to/portmux-port`
> to point at a non-default location.

The probe runs once per process; callers (the GUI startup or the
`appimage-control` CLI pre-flight) can invoke
[`portmux_port_probe_runtime`](../crates/core/src/portmux_port.rs)
before the seed loop starts so the error message lands in a single
top-level dialog instead of one-per-instance.

## Test override

Tests and ad-hoc smoke runs point at a stub binary via:

```bash
APPIMAGE_CONTROL_PORTMUX_PORT_BIN=/tmp/fake-portmux-port \
    cargo test -p appimage_control_core port_verb
```

The stub script must accept `portmux-port ensure --app … --json`
and emit a JSON document matching portmux's `EnsuredPort` schema
(`host`, `port`, `isolation`, `source_file`, `tor_reload_done`,
`already_assigned`). See `seed_pack.rs::port_verb_tests` for the
canonical fake.

## Pack-side wiring

A pack declares its port intent declaratively; see the
[`port` verb section in `docs/seed-packs.md`](seed-packs.md). The
three shipped packs (companion repo: `feather.json`,
`electrum-btc.json`, `electrum-ltc.json`) are the canonical
worked examples.

## Failure modes

| Symptom from the CLI                        | Cause                          | Fix |
|---------------------------------------------|--------------------------------|-----|
| `portmux-port not found on host PATH`      | portmux not installed         | `sudo apt install portmux` |
| `portmux-port exited 2`                    | gateway pool exhausted         | Disable an unused managed entry in `~/.config/portmux/whonix-ports.conf` or in `/etc/tor/torrc` |
| `portmux-port exited N` (other)            | helper internal error          | Check `journalctl -u portmux` or rerun with `--no-reload` for diagnostics |
| seed JSON missing `socks5Port`              | pack has `drop_if` that fired  | Expected when `feather_use_local_tor=true` on Ubuntu |

## Backwards compatibility

Users mid-flight (already-seeded Feather instances with old
hardcoded `9052 / 9053 / 9070` ports) will see their seeded
`socks5Port` change on the next reseed. Existing on-disk wallet
data is untouched — the seeder only rewrites `settings.json`'s
proxy fields.
