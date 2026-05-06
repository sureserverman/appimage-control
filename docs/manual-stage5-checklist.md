# Stage 5 manual / system validation checklist

**Automated subset (programmatic Stage 5):** run from repo root:

```bash
./scripts/stage5-validate.sh
```

This runs `cargo test --workspace --locked`, `./scripts/validate-packaging.sh`, and `./scripts/stage5-catalog-list-smoke.sh` (builds `appimage-control`, isolated `HOME`, `APPIMAGE_CONTROL_CATALOG=data/catalog.json`, asserts `--list` includes every shipped catalog id).

Optional: `PROFILE=release ./scripts/stage5-validate.sh` for a release binary in the list smoke. Optional full Flatpak compile: `RUN_FLATPAK_BUILD=1 ./scripts/stage5-validate.sh` (slow).

Use the sections below for checks that still need a real desktop session or real upstream AppImages.

See `docs/plans/2026-04-17-app-functionality-test-plan.md` for context.

**Environment:** record GNOME/Wayland vs X11, session type, and display.

## Packaging (repeatable commands)

| Step | Command | Pass |
|------|---------|------|
| All automated packaging + tests | `./scripts/stage5-validate.sh` | |
| Desktop entry | `./scripts/validate-packaging.sh` (includes `desktop-file-validate`) | |
| AppStream | (same script, includes `appstreamcli validate`) | |
| Flatpak manifest parse | (same script, `flatpak-builder --show-manifest`) | |
| Full Flatpak build | `RUN_FLATPAK_BUILD=1 ./scripts/validate-packaging.sh` | |

Notes: `desktop-file-validate` may emit a **hint** about multiple main `Categories`; exit code should still be 0.

## Real AppImage matrix (shipped `data/catalog.json`)

For each app: install → multi-instance (if allowed) → launch from menu → verify isolation → spot-check icon / WM class where relevant.

| id | Install 1 | Multi-instance | Launch `.desktop` | Isolated dirs OK | Icon / WM class notes |
|----|-------------|------------------|-------------------|------------------|------------------------|
| feather | | | | | Qt/X11, dock icon theme name |
| session | | | | | Electron class |
| keepassxc | | | | | Isolated launcher env |
| mattermost | | | | | Electron isolated |
| anythingllm | | | | | Electron isolated |
| jan | | | | | Electron isolated, dbus isolate flag |
| lm-studio | | n/a (max 1) | | | Electron isolated, max_instances=1 |
| electrum-btc | | | | | instance_subdir `ebtc` |
| electrum-ltc | | | | | instance_subdir `eltc` |
| cryptomator | | | | | extract strategy; squashfs icon path |
| joplin | | | | | alt instance strategy |
| standard-notes | | | | | Electron class |
| zulip | | | | | Electron isolated; absolute desktop icon |

## Backup / restore / migration drills

| Drill | Pass |
|-------|------|
| Create backup after installs (GUI menu or `--backup PATH`) | |
| Restore into **empty** state | |
| Restore **refused** when state exists without `--force` | |
| Forced restore overwrites only manager-owned state | |
| Migrate legacy dirs (if applicable): dry-run expectations, no unrelated file damage | |

## Desktop shell UX (manual)

| Check | Pass |
|-------|------|
| Launcher repair on next startup (if you delete a managed binary) | |
| Icons in app menu / dock resolve | |
| Narrow window: list usable, no clipped primary actions | |
| Icon-only header buttons: tooltips visible | |

## Sign-off

- Date:
- Tester:
- Blockers / follow-ups:
