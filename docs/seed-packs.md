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

## Schema — the closed verb vocabulary

Every supported transformation is one of a fixed set of verbs.
Adding a new top-level key is a hard error
(`#[serde(deny_unknown_fields)]`).

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
instance index.

### `proxy_setup` — hand proxy settings to portmux

```json
"proxy_setup": { "app": "feather" }
```

`app` is a recipe id for `portmux-port configure-app --app`. portmux
allocates this instance's port and writes the proxy settings into the app's
config. The pack says nothing about ports, hosts or isolation. Its key is not
`proxy` because Feather and Electrum both have a config key of that name.

portmux runs before the pack's other keys are written, so a failure leaves the
first-run config byte-identical. It runs again after them, so its settings win
over a `static` or `rotation` key of the same name. A pack with no `proxy_setup`
gets no `configure-app` call, though seeding's pre-flight probe still needs
portmux. See
[`docs/integration-portmux-port.md`](integration-portmux-port.md).

The removed `preset` section and `port` verb are refused by name at load time,
because ignoring them would leave a wallet with no proxy and no warning. That
document has the migration steps.

### `rotation` — per-instance values

One form: `values` rotates through an inline list, indexed by
`(instance_index - 1) mod len`:

```json
"rotation": {
  "themeRotation": { "values": ["Light", "Dark", "Light"] }
}
```

(Per-instance ports do not belong here: they are portmux's, through
`proxy_setup`.)

## Discovery chain

`SeedPackLibrary::discover_load` resolves packs in this order; first
non-empty match wins:

1. Explicit dir passed to `discover_load(Some(dir))`.
2. `APPIMAGE_CONTROL_SEEDS_DIR` env var, if non-empty and a directory.
3. `~/.local/share/appimage-control/seeds/` — per-user, where
   `--update-seed-packs` installs companion-repo content.
4. Walk `current_exe()` ancestors (depth 32) for `data/seeds/`. The
   current directory is never searched: the GUI starts in `$HOME`,
   where a planted `data/seeds/` would otherwise win.
5. `/app/share/appimage-control/seeds/` (Flatpak install path —
   empty by default in this repo's Flatpak builds).
6. `/usr/share/appimage-control/seeds/`.
7. Slice baked in by `build.rs` from `data/seeds/*.json` at compile
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
`proxy_setup` / `static` / `rotation` verbs to it.

Validate locally before shipping:

```bash
appimage-control --list-seed-packs   # confirms which packs are loaded
                                     # and where each came from
```

Validation errors include the pack id and the offending field name
or unknown placeholder — typos fail loud at load time, never
silently at first seed.

## Authoring by recording (diff-based)

Instead of hand-writing the `static` block, you can let the manager
capture it for you — install the app, let the manager snapshot its
untouched first-run config, configure the app by hand, and have the
manager diff the two into a `static`-only pack. This is the seed-pack
*recorder*; see [`docs/workflows/recording.md`](workflows/recording.md)
(WF-RECORD-001) for the full contract.

GUI: **Record Seed Pack…** in the main menu walks the three steps
(capture baseline → configure → review the diff → save).

CLI:

```bash
# 1. Capture a baseline of feather instance 1's first-run config.
#    The config path comes from an existing pack if one ships; otherwise
#    pass --pack-config-path (a single {appimage} is substituted).
appimage-control --record-pack-baseline feather \
  --pack-config-path '{appimage}.config/feather/settings.json'

# 2. Launch feather, change the settings you want the pack to carry, quit.

# 3. Diff against the baseline and write a static-only pack (unsigned).
appimage-control --record-pack-capture feather --out ./feather.json
```

The recorder only emits `static` literals (the added/changed top-level
keys). It does **not** infer `proxy_setup` or `rotation`. Delete any proxy or
port values it captured and add `proxy_setup` by hand afterwards, because those
values are portmux's. Removed keys
are reported but not encoded (a `static` pack cannot delete keys). The
written file is not signed; the signed companion-repo delivery
(`--update-seed-packs`) stays separate.

## Example template

`data/seeds/example.json.txt` ships a copy-paste template. The
`.txt` suffix keeps `build.rs` from picking it up as a real pack.
