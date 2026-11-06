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

A build under `target/` finds the repo's `data/catalog.json` by walking up from the binary (only builds under a cargo `target/` dir walk; the current directory is never searched), or set:

`export APPIMAGE_CONTROL_CATALOG=/path/to/catalog.json`

## Seeding app settings (architecture)

AppImage Control can **seed and patch app settings** on first run — write a config file the way you want by launching the AppImage once under `xvfb-run` and applying a declarative *seed pack* (a small JSON file) to whatever it produces.

This repository ships only the **architecture** (loader, interpreter, signature verifier, CLI surface). It does **not** ship any curated pack content. Actual seed packs are operator-curated and distributed to authorised users via a separate signed companion repository — see [`docs/seed-packs.md`](docs/seed-packs.md) for the schema and [`docs/seed-pack-companion-repo.md`](docs/seed-pack-companion-repo.md) for the distribution setup.

### Requirements

- `xvfb-run` available on `PATH` (e.g. on Debian/Ubuntu:

  ```bash
  sudo apt install xvfb
  ```

  When AppImage Control runs as a Flatpak the GNOME runtime does not ship
  `xvfb-run`, so the app uses `flatpak-spawn --host xvfb-run …` to invoke the
  host's `xvfb-run` (along with `fusermount3`, `appimageupdatetool`,
  `update-desktop-database`, and `gtk-update-icon-cache`). Install `xvfb` on
  the host, not inside the Flatpak.

- Network access to download and run the upstream AppImages at least once.

### CLI surface

General flags:

```
--catalog PATH       Load catalog JSON from PATH instead of the default discovery chain.
--list               Print catalog apps, installed instance counts, and version summary.
--list-seed-packs    Print one tab-separated row per loaded seed pack.
--update-seed-packs  Fetch + verify + install signed seed-pack tarball from the companion repo.
--repair-launchers   Recreate missing per-instance .desktop launchers and icons.
-update / --update   Update all installed instances from the catalog URL (sha256-checked); exits non-zero if any instance fails.
--check-updates      Print `<app-id>: <installed> -> <latest>` for each installed app with a newer release; downloads nothing. Runs on its own.
--notify             With --check-updates: also show one desktop notification naming those apps (the login autostart entry runs both).
--clean-cache        Delete the download cache dir.
--clean              Remove all installed catalog app instances and managed launchers (keeps cache).
--purge              Remove all installs, cache, and managed launcher files.
--migrate            Move legacy portable data dirs into catalog instance trees.
--on-conflict=X      With --migrate: replace | renumber | skip on collision.
--backup PATH        Create a manager-state backup archive (tar.zst recommended).
--restore-backup PATH
                     Restore manager state from PATH.
--force              With --restore-backup: overwrite existing managed state.
-y, --yes            Skip Yes/No confirmation prompts (for scripts).
```

Instance-count operators (can be combined; ops run in argv order; a mixed-op summary is printed when two or more ops are dispatched):

```
+APP             Add 1 instance of APP (capped by the catalog instance limit).
+APP:N           Add N instances of APP.
-APP             Remove all instances of APP.
-APP:N           Remove N instances (count not below 0).
=APP:N           Set APP to exactly N instances (absolute). =APP:0 removes all. =APP without :N is an error.
--seed-new       After all instance-adding ops (+APP/=APP:N), automatically seed newly added instances.
--allow-unverified  With +APP/=APP:N and -update: install a download that has no published sha256 to check; a known-but-different sha256 is always refused.
```

Seeding flags:

```
--seed-settings TARGETS
                     Seed supported app settings. TARGETS is `all`, one app id, or a comma-
                     separated list. There is no host switch.
--seed-save NAME     Save the current --seed-settings selection under NAME, then run the seed.
--seed-load NAME     Load a previously-saved seed form by NAME and run it.
--seed-list-forms    Print all saved seed-form names and exit.
--seed-delete-form NAME
                     Delete a saved seed form by name and exit.
```

Seed-pack recorder flags (author a pack by diffing an app's config before/after manual setup — see [docs/workflows/recording.md](docs/workflows/recording.md)):

```
--record-pack-baseline APP[:N]
                     Phase 1: launch instance N (default 1) once headlessly and snapshot its
                     untouched first-run config as a baseline.
--pack-config-path PATH
                     With --record-pack-baseline: the config file the app writes, relative to the
                     instance dir (a single `{appimage}` placeholder is substituted).
--pack-first-run-args A,B
                     With --record-pack-baseline: comma-separated first-run AppImage args.
--record-pack-capture APP[:N]
                     Phase 3: diff the instance's current config against the baseline and write a
                     `static`-only seed pack of the added/changed top-level keys (unsigned).
--out PATH           With --record-pack-capture: output path (default per-user seeds dir).
--list-pack-recordings
                     Print one tab-separated row per stored baseline recording: `<app-id>\t<N>`.
--delete-pack-recording APP[:N]
                     Delete a stored baseline recording (idempotent).
```

**Strict parsing:** unrecognised arguments cause a non-zero exit with a message pointing to `--help`; they do not open the GUI. Exit 0 only when all dispatched ops succeed.

Examples:

```bash
appimage-control --list
appimage-control --catalog /path/to/catalog.json --list
appimage-control +feather:2 --seed-new
appimage-control =session:0
appimage-control --repair-launchers --list
appimage-control --seed-settings feather,electrum-btc --seed-save daily
appimage-control --update-seed-packs
appimage-control --check-updates --notify
```

The seeding command:

- Runs each AppImage under `xvfb-run` until its config file appears.
- Patches that config with the pack's settings. It writes no proxy settings.
- Logs per-instance results and a final summary, e.g.  
  `Seed settings summary: 3 seeded, 0 failed, 0 unsupported, 0 missing.`

### GUI workflow

- Use the main window to **Apply** instance counts for `feather`, `electrum-btc`, and `electrum-ltc`.
- Then use the **”Seed Settings…”** entry in the main menu:
  - Choose which installed supported apps to seed (checkboxes; no host picker).
  - Optionally save or recall a named app selection via the “Saved configurations” section.
  - Review the full seeding log in the activity dialog.
- Click the info button on a card to open the app's **detail page**: where its updates come from, whether its download was checked against a published checksum, and each instance with **Launch** and **Open folder**. It never removes anything; removal stays on the card. See [docs/workflows/details.md](docs/workflows/details.md).
- Use **”Record Seed Pack…”** to author a new pack by diffing: pick an installed app/instance, capture a baseline, configure the app by hand, then review the changed keys and save a `static`-only pack (unsigned). See [docs/workflows/recording.md](docs/workflows/recording.md).

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

## Translations

The GUI is translated into German, Spanish, French, Italian, Japanese, Polish,
Portuguese (Brazil), Russian, Turkish, Ukrainian and Chinese (Simplified). The
CLI and the activity log stay in English, so scripts can rely on their text.

The translations are machine-made and no native speaker has reviewed them yet.
Flathub does not accept machine translations, so the Flathub build passes
`--no-default-features` to leave them out and is English only. Corrections are
welcome.

The language follows `LANGUAGE`, then `LC_ALL`, `LC_MESSAGES` and `LANG`.
Portuguese from Portugal gets the Brazilian text. Chinese in Traditional script
(`zh_TW`, `zh_HK`) stays English. To try one:

```bash
LANGUAGE=uk appimage-control
```

The catalogs are standard gettext files in `po/`, built into the binary, so
nothing has to be installed next to it.

To change GUI text or a translation:

1. Wrap user-visible GUI text in `tr`, `trc` (with a context) or `ntr` (with a
   count), from `crates/gui/src/i18n.rs`. Pass plain one-line string literals.
2. Run `scripts/update-po.sh`. It refreshes `po/appimage-control.pot` and
   merges it into every `po/<lang>.po`.
3. Translate the new or changed entries in each `po/<lang>.po`.
4. Run `cargo test -p appimage-control`. A test fails if any catalog misses an
   entry, loses a `{placeholder}`, or has the wrong number of plural forms.

To add a language, add its code to `po/LINGUAS` and to `CATALOGS` and
`plural_index` in `crates/gui/src/i18n.rs`, then run `scripts/update-po.sh`.

## App ID and Desktop Metadata

Canonical application ID:

- `io.github.sureserverman.AppImageControl`

Canonical desktop/metainfo files:

- `data/io.github.sureserverman.AppImageControl.desktop`
- `data/io.github.sureserverman.AppImageControl.metainfo.xml`
- `data/io.github.sureserverman.AppImageControl.service` (D-Bus activation, for
  the Flatpak; the desktop file sets `DBusActivatable=true`)

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
- restore extracts fully to a staging area before clearing existing state (validate before any writes)

## Flatpak (Stage 6 WIP)

Flatpak manifest and dependency scaffold:

- `flatpak/io.github.sureserverman.AppImageControl.yml`
- `flatpak/cargo-sources.json` (populate with generator output before Flathub submission)

Local metadata validation:

```bash
desktop-file-validate data/io.github.sureserverman.AppImageControl.desktop
appstreamcli validate data/io.github.sureserverman.AppImageControl.metainfo.xml
```

## Flathub Publishing (Stage 7)

Operator documentation for local validation, smoke testing, submission, and review iteration:

- `docs/flathub-publish.md`

Submission asset checklist location:

- `assets/flathub/README.md`
