# Seed packs — architecture

A *seed pack* is a small declarative JSON file describing how to
patch one supported app's first-run configuration after the manager
has launched it under `xvfb-run`. This document covers the
**architecture** that lives in this repository — schema, loader,
interpreter, discovery chain, signature verifier. It does **not**
ship any curated pack content.

The actual pack files are operator-curated and distributed
separately to authorised users via a companion repository. See
[`docs/seed-pack-companion-repo.md`](seed-pack-companion-repo.md)
for how to set one up and how the `--update-seed-packs` flow
delivers signed updates.

If you are reading this and there are no `*.json` files under
`data/seeds/`: that is correct. The architecture is public; the
content is not. An authorised build replaces the placeholder
`SEED_PACKS_PUB_KEY` and `DEFAULT_RELEASE_URL` constants in
`crates/core/src/seed_pack_update.rs` before publishing the binary
to its select audience.

## Schema — the closed three-verb vocabulary

Every supported transformation is one of three verbs. Adding a new
top-level key is a hard error (`#[serde(deny_unknown_fields)]`).

### `static` — literal JSON values

```json
{
  "config_path": "...",
  "static": {
    "telemetry": false,
    "compactMode": true,
    "themeId": "dark"
  }
}
```

Numbers, booleans, strings, arrays, nested objects — anything
`serde_json` accepts. Use this for values that don't depend on the
active preset or the instance index.

### `preset` — values derived from the active preset

Two forms. `from` copies one named preset field directly:

```json
"preset": {
  "useLocalProxy": { "from": "feather_use_local_tor" },
  "proxyHost":     { "from": "socks5_host", "drop_if": "feather_use_local_tor" }
}
```

`format` interpolates one or more scalar preset fields into a string:

```json
"preset": {
  "proxyUrl": { "format": "socks5:{socks5_host}:{electrum_btc_port}" }
}
```

Both forms accept an optional `drop_if`: when the named **bool**
preset field is true, the key is omitted (and removed from any
pre-existing config). Use this to drop a key conditionally based on
the active preset.

### `rotation` — per-instance values

Two forms. `values` rotates through an inline list, indexed by
`(instance_index - 1) mod len`:

```json
"rotation": {
  "themeRotation": { "values": ["Light", "Dark", "Light"] }
}
```

`from_preset_array` rotates through a named u16 preset array:

```json
"rotation": {
  "proxyPort": { "from_preset_array": "feather_ports", "as_string": true }
}
```

Set `as_string: true` when the receiving JSON schema expects a
string rather than a number.

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

(Field names are legacy — they predate the data-driven schema.
Extending the preset surface to a new field is a one-line change in
`SeedConfig::field`.)

`format` cannot inline an array field; use
`rotation.from_preset_array` for that. A typo'd field name fails
pack-load (`SeedPackLibrary::validate`) with the offending name in
the message — never silently at seed time.

## Discovery chain

`SeedPackLibrary::discover_load` resolves packs in this order; first
non-empty match wins:

1. Explicit dir passed to `discover_load(Some(dir))`.
2. `APPIMAGE_CONTROL_SEEDS_DIR` env var, if non-empty and a directory.
3. `~/.local/share/appimage-control/seeds/` — per-user, where
   `--update-seed-packs` installs companion-repo content.
4. Walk cwd ancestors (depth 32) for `data/seeds/`.
5. Walk `current_exe()` ancestors for `data/seeds/`.
6. `/app/share/appimage-control/seeds/` (Flatpak install path —
   empty by default in this repo's Flatpak builds).
7. `/usr/share/appimage-control/seeds/`.
8. Slice baked in by `build.rs` from `data/seeds/*.json` at compile
   time (empty by default in this repo).

For local iteration on a pack you're authoring:

```bash
APPIMAGE_CONTROL_SEEDS_DIR=$PWD/my-packs appimage-control --list-seed-packs
```

## Signed delivery: `--update-seed-packs`

Authorised users get a binary built with a real
`SEED_PACKS_PUB_KEY` + `DEFAULT_RELEASE_URL`. Running:

```bash
appimage-control --update-seed-packs
```

fetches the latest release from the configured companion repo,
verifies its 69-byte AICSIG-1 signature against the embedded
ed25519 public key, unpacks the tarball (rejecting any non-flat /
non-`*.json` entry), validates every pack via the same loader the
binary uses at startup, and atomically installs into the per-user
seeds dir.

The full operator workflow (keygen on cold storage, companion-repo
layout, release pipeline) lives in
[`docs/seed-pack-companion-repo.md`](seed-pack-companion-repo.md).

## Authoring a new pack

Drop one file at `<seeds-dir>/<app-id>.json`. The `<app-id>` must
match an entry in `data/catalog.json` for the manager to recognise
the app as installable. Minimum schema:

```json
{
  "config_path": "{appimage}.config/<app>/settings.json"
}
```

`{appimage}` is substituted at seed time with the AppImage filename.
The manager will launch the AppImage once under `xvfb-run`, wait for
the config file to appear at that path, then apply the pack's
`static` / `preset` / `rotation` verbs to it.

Validate locally before shipping:

```bash
appimage-control --list-seed-packs   # confirms which packs are loaded
                                     # and where each came from
```

Validation errors include the pack id and the offending field name
or unknown placeholder — typos fail loud at load time, never
silently at first seed.

## Example template

`data/seeds/example.json.txt` ships a copy-paste template. The
`.txt` suffix keeps `build.rs` from picking it up as a real pack.
