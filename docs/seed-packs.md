# Seed packs

A *seed pack* is a small JSON file that tells AppImage Control how to
patch a supported app's first-run config to use Tor (and any other
manager-curated defaults). Packs live one-per-app under `data/seeds/`,
get baked into the binary at compile time by `crates/core/build.rs`,
and can be overridden on disk through the same fallback chain
`catalog.json` already uses.

If today's curated set (Feather, Electrum BTC, Electrum LTC) doesn't
cover an AppImage you'd like seeded, you add a pack — that's all. No
Rust changes, no rebuild.

## Authoring a new pack

Drop one file at `data/seeds/<app-id>.json`. The `<app-id>` must match
the `id` of an entry in `data/catalog.json` (the contract test
[`seeds_shipped_contract.rs`][contract] enforces this).

[contract]: ../crates/core/tests/seeds_shipped_contract.rs

The minimum a pack needs is a `config_path`:

```json
{
  "config_path": "{appimage}.config/<app>/settings.json"
}
```

`{appimage}` resolves to the AppImage filename at seed time (e.g.
`feather.AppImage`). After the AppImage's first-run write produces a
JSON config at that path, the manager reads it, applies the pack, and
writes it back.

Validate locally before committing:

```bash
cargo test -p appimage_control_core --test seeds_shipped_contract
```

## Schema — the closed three-verb vocabulary

Every supported transformation is one of three verbs. Adding a new
top-level key is a hard error (`#[serde(deny_unknown_fields)]`).

### `static` — literal JSON values

```json
{
  "config_path": "...",
  "static": {
    "proxy": 1,
    "torOnlyAllowOnion": true,
    "server": "x.onion:50002:s"
  }
}
```

Numbers, booleans, strings, arrays, nested objects — anything
`serde_json` accepts. Use this for values that don't depend on the
preset or the instance index.

### `preset` — values derived from the active preset

Two forms. `from` copies one named preset field directly:

```json
"preset": {
  "useLocalTor": { "from": "feather_use_local_tor" },
  "socks5Host":  { "from": "socks5_host", "drop_if": "feather_use_local_tor" }
}
```

`format` interpolates one or more scalar preset fields into a string:

```json
"preset": {
  "proxy": { "format": "socks5:{socks5_host}:{electrum_btc_port}" }
}
```

Both forms accept an optional `drop_if`: when the named **bool** preset
field is true, the key is omitted (and removed from any pre-existing
config). That's how Feather drops `socks5Host` under Ubuntu (where
`useLocalTor` is true) and keeps it under Whonix.

### `rotation` — per-instance values

Two forms. `values` rotates through an inline list, indexed by
`(instance_index - 1) mod len`:

```json
"rotation": {
  "skin": { "values": ["Breeze/Light", "Breeze/Dark", "Breeze/Light"] }
}
```

`from_preset_array` rotates through a named u16 preset array:

```json
"rotation": {
  "socks5Port": { "from_preset_array": "feather_ports", "as_string": true }
}
```

Set `as_string: true` when the receiving JSON schema expects a string
rather than a number (Feather's `socks5Port` is one such case).

## Legal preset field names

The accessor in `crates/core/src/seed.rs::SeedConfig::field` is the
single source of truth. Today:

| Name                     | Type        | Form factor              |
|--------------------------|-------------|--------------------------|
| `socks5_host`            | string      | `from`, `format`         |
| `feather_use_local_tor`  | bool        | `from`, `drop_if`, `format` |
| `feather_ports`          | u16 array   | `from_preset_array`      |
| `electrum_btc_port`      | u16 scalar  | `from`, `format`         |
| `electrum_ltc_port`      | u16 scalar  | `from`, `format`         |

`format` cannot inline an array field; use `rotation.from_preset_array`
for that. A typo'd field name fails pack-load (`SeedPackLibrary::validate`)
with the offending name in the message — never silently at seed time.

## Discovery — where the loader looks

Same fallback chain as `Catalog::discover_load`, in this order:

1. Explicit dir passed to `SeedPackLibrary::discover_load(Some(dir))`.
2. `APPIMAGE_CONTROL_SEEDS_DIR` env var, if non-empty and a directory.
3. Walk cwd ancestors (depth 32) for `data/seeds/`.
4. Walk `current_exe()` ancestors for `data/seeds/`.
5. `/app/share/appimage-control/seeds/` (Flatpak install path).
6. `/usr/share/appimage-control/seeds/`.
7. The slice baked in by `build.rs`.

For local iteration, point the env var at a working copy:

```bash
APPIMAGE_CONTROL_SEEDS_DIR=$PWD/data/seeds appimage-control --list-seed-packs
```

## Example: a brand-new pack

```jsonc
{
  // Where the AppImage will write its first-run config, relative to
  // the instance dir. {appimage} is substituted with the AppImage
  // filename at seed time.
  "config_path": "{appimage}.config/myapp/config.json",

  // Optional CLI args passed to the AppImage on the seed launch.
  // Often "--offline" for crypto wallets to skip network bootstrap.
  "first_run_args": [],

  // Literal JSON merged into the config map.
  "static": {
    "telemetry": false
  },

  // Values pulled from the active preset.
  "preset": {
    "proxyHost": { "from": "socks5_host" }
  },

  // Per-instance rotations.
  "rotation": {
    "color": { "values": ["red", "green", "blue"] }
  }
}
```

Save as `data/seeds/myapp.json`, add the matching app to
`data/catalog.json`, run:

```bash
cargo test -p appimage_control_core --test seeds_shipped_contract
```

If both pass, the new pack ships next build.

## Removing seed support for an app

Delete `data/seeds/<app-id>.json`. The shipped catalog contract test
will accept the absence (only the *presence* of a pack must match a
catalog id, not the other way round). Users with stale on-disk copies
keep working until they re-pull.
