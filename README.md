# AppImage Control

Rust + GTK 4 utility that shows a **catalog of apps** from JSON: for each entry you set how many **isolated instances** you want (separate directories, like `~/feather1` / `~/feather2`). **Apply** downloads the AppImage once (cached under `~/.local/share/appimages/cache/<id>/`), copies or hardlinks it into each instance dir, and writes a `.desktop` launcher per instance.

Feather-style setups use optional `instance_exec_args_template` (e.g. `--name {instance_name}`) and `instance_name_template` (e.g. `{Id}{index}` → `Feather1`, `Feather2`) so each instance gets its own window class, matching the pattern in `~/dev/installers/linux/files/feather*.sh`.

## Layout

- **Per-catalog copies:** `~/.local/share/appimages/apps/<catalog_id>/<1…n>/<appimage_filename>`  
  Portable app data (e.g. Feather’s `feather.AppImage.config`) stays next to that copy.
- **CLI `install` copies:** `~/.local/share/appimages/files/` (standalone AppImages not tied to the catalog layout).
- **Download cache:** `~/.local/share/appimages/cache/<catalog_id>/` (one binary per catalog id; reused for every instance).
- **Launchers:** `~/.local/share/applications/appimage-control-<id>-<n>.desktop`

On **Whonix**, keep `$HOME` on persistent storage.

## Security

- **Trust** every `download_url` in your catalog (verify upstream URLs, signatures, and checksums yourself).
- **Tor / Whonix:** `reqwest` uses normal TLS; traffic follows your environment (e.g. system proxy). Large downloads over Tor can be slow or fragile.
- **Apply** removes instances (dirs + `.desktop`) when you lower the count.

## Build

```bash
sudo apt install build-essential pkg-config libgtk-4-dev
cargo build --release -p appimage-control
./target/release/appimage-control
```

Run from the repo root so `./data/catalog.json` is found, or set:

`export APPIMAGE_CONTROL_CATALOG=/path/to/catalog.json`

## Seeding Feather and Electrum settings

For Feather and Electrum (BTC/LTC), AppImage Control can **seed and patch app settings** so they match your installer scripts (ports, Tor use, etc.). Seeding is implemented entirely in `appimage_control_core` and works the same from CLI and GUI.

### Requirements

- `xvfb-run` available on `PATH` (e.g. on Debian/Ubuntu:

  ```bash
  sudo apt install xvfb
  ```

- Network access to download and run the upstream AppImages at least once.

### CLI workflow (Feather + Electrum)

From the repo root:

```bash
# 1) Install instances from the bundled catalog
cargo run -p appimage-control --release -- \
  +feather +electrum-btc +electrum-ltc

# 2) Seed and patch their settings using the Ubuntu-style preset
cargo run -p appimage-control --release -- \
  --seed-settings feather,electrum-btc,electrum-ltc \
  --seed-preset ubuntu
```

The seeding command:

- Runs each AppImage under `xvfb-run` until its config file appears.
- Patches that config according to the chosen preset (`ubuntu` or `whonix`).
- Logs per-instance results and a final summary, e.g.  
  `Seed settings summary: 3 seeded, 0 failed, 0 unsupported, 0 missing.`

### GUI workflow

- Use the main window to **Apply** instance counts for `feather`, `electrum-btc`, and `electrum-ltc`.
- Then use the **“Seed Settings…”** entry in the main menu:
  - Choose which installed supported apps to seed.
  - Pick the preset (`ubuntu` or `whonix`).
  - Review the full seeding log in the activity dialog.

## Catalog schema

See [`data/catalog.example.json`](data/catalog.example.json). Important fields:

| Field | Purpose |
|--------|--------|
| `id` | Stable id; instance paths use this. |
| `download_url` | Direct HTTPS link to the `.AppImage` (empty = row shows hint only). |
| `appimage_filename` | Name inside each instance dir (default: URL basename). |
| `instance_exec_args_template` | Extra `Exec` args; `{instance_name}`, `{index}`, `{id}`, `{Id}`. |
| `instance_name_template` | Expands to `{instance_name}` (default `{Id}{index}`). |
| `desktop_name_template` | Menu name (default `{display_name} (#{index})`). |

Bundled [`data/catalog.json`](data/catalog.json) tracks **the same upstreams** as `~/dev/installers/linux/user.sh` and `whonix.sh` where possible, plus **GitHub `releases/latest`-verified** URLs (checked 2026-04-06):

| App | Source (aligned with `~/dev/installers/linux`) |
|-----|----------------------------------------|
| Feather x86_64 | Same URL as `user.sh` / `whonix.sh`: `github.com/feather-wallet/feather` release **2.8.1** `feather-2.8.1.AppImage` |
| Feather arm64 | Same as `whonix-aarch.sh`: `feather-2.8.1-arm64.AppImage` |
| Session | Same as `user.sh` / `whonix.sh`: `session-foundation/session-desktop` **v1.17.17** AppImage; launcher adds `--no-sandbox` like your `.desktop` |
| KeePassXC | GitHub `keepassxreboot/keepassxc` **2.7.12** x86_64 AppImage (verify `.sig` on the release page) |
| Electrum BTC | Same as scripts: `download.electrum.org` **4.7.2** (official site, not a GitHub asset) |
| Electrum LTC | Same as scripts: `electrum-ltc.org` **4.2.2.1** |

**Element Desktop:** current [`element-hq/element-desktop` GitHub releases](https://github.com/element-hq/element-desktop/releases) no longer attach a Linux `.AppImage` (only source tarballs/signatures). Use Flatpak from your scripts or add a URL yourself if upstream ships an AppImage elsewhere.

Always verify checksums/PGP for each release before relying on downloads.

## Legacy

The `appimage_control_core` library still exposes the older single-path helpers (`install_appimage`, `list_installed` under `appimages/`, etc.) for tests and scripting; the GTK UI uses only the catalog + instance flow above.

## Workspace

- `crates/core` — `appimage_control_core`
- `crates/gui` — `appimage-control`

## GUI Design Guideline

GTK interface work in this repository follows:

- `docs/gtk-ui-guidelines.md`

Treat this as the primary UI guideline for AppImage Control surfaces.

## App ID and Desktop Metadata

Canonical application ID:

- `io.github.appimagecontrol.AppImageControl`

Canonical desktop/metainfo files:

- `data/io.github.appimagecontrol.AppImageControl.desktop`
- `data/io.github.appimagecontrol.AppImageControl.metainfo.xml`

## Backup/Restore Contract (Stage 0)

Backup/restore scope is intentionally limited to **AppImage Control-managed state**:

- `~/.local/share/appimages/` (apps/cache/files/icons trees owned by this manager)
- managed desktop entries: `~/.local/share/applications/appimage-control-*.desktop`
- manager-owned icon tiles: `~/.local/share/icons/hicolor/*/apps/appimage-control-*.png`

Planned artifact format:

- single `tar.zst` archive
- includes `manifest.json` and `catalog-snapshot.json`
- deterministic restore metadata (format versioned)

CLI contract:

- `appimage-control --backup <archive-path>`
- `appimage-control --restore-backup <archive-path> [--force]`

Restore policy:

- default restore refuses when manager-owned state already exists
- `--force` is required to overwrite existing managed state
- restore must not depend on live network catalog data; backup snapshot is authoritative

## Flatpak (Stage 6 WIP)

Flatpak manifest and dependency scaffold:

- `flatpak/io.github.appimagecontrol.AppImageControl.yml`
- `flatpak/cargo-sources.json` (populate with generator output before Flathub submission)

Local metadata validation:

```bash
desktop-file-validate data/io.github.appimagecontrol.AppImageControl.desktop
appstreamcli validate data/io.github.appimagecontrol.AppImageControl.metainfo.xml
```

## Flathub Publishing (Stage 7)

Operator documentation for local validation, smoke testing, submission, and review iteration:

- `docs/flathub-publish.md`

Submission asset checklist location:

- `assets/flathub/README.md`
