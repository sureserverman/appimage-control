# Stage 1 demo — 2026-11-06-gear-lever-gap-plan

Environment: host (not the VM). Scratch binary crate `/tmp/claude-1000/s1-demo/demo` depends on
`appimage_control_core = { path = ".../crates/core" }` and calls only its public API
(`AppPaths::new`, `parse_catalog_json`, `app_details`, `fetch_remote_latest_versions`,
`latest_known_version(s)`, `updates_available`, `update_available_for`, `launch_instance`).
API learned from `cargo doc -p appimage_control_core --no-deps` only; no source or diff read.
Catalog: the shipped `data/catalog.json` (14 apps). Fake HOME: `/tmp/claude-1000/s1-demo/home`.
Fixtures were laid at the paths `app_details` reported. Real `~/.local` untouched; nothing run on the VM.
Commands below are `s1demo <cmd>` = `/tmp/claude-1000/s1-demo/demo/target/debug/s1demo <cmd>`.

Fixture install (feather):
- #1: `feather.AppImage` + `.appimage-control-version` = 2.7.0 + launcher `appimage-control-feather-1.desktop`, `Exec=<dir>/appimage-control-launch.sh %U`; the script appends `started ... args=[...]` to a marker, sleeps 5 s, appends `finished`.
- #2: launcher + script + version 2.7.0, no AppImage.
- #3: AppImage + version 2.6.0, no launcher.
- #4: folder with only `.appimage-control-version`.
- #5: launcher only; folder absent.
- Also: electrum-btc #1 (`apps/ebtc/1/eb.AppImage`, 4.7.0), anythingllm #1 (1.0.0), keepassxc #1 (2.7.12).

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: `app_details` returns, for any catalog app, its instances (index, folder, AppImage path, recorded version, size, launcher present or not), its update source and its cache verification state. `updates_available` returns each installed app whose newer release is known. `launch_instance` starts an instance's launcher command and returns at once.
Verdict: holds
Evidence:
- `s1demo details` over all 14 catalog apps: every one returns an `AppDetails` with source and verification (GithubReleases for feather/session/keepassxc/jan/cryptomator/joplin/standard-notes/qnote/zulip; PinnedUrl for mattermost/electrum-btc/electrum-ltc; UnpinnedUrl for anythingllm/lm-studio). Exit 0, no panic.
- `s1demo details feather` after the fixture install:
  ```
  #1 dir=.../apps/feather/1 dir_present=true
     appimage=.../apps/feather/1/feather.AppImage present=true size=Some(29)
     launcher=.../applications/appimage-control-feather-1.desktop present=true
     version=Some("2.7.0")
  ```
  `s1demo details electrum-btc` reports the short slug folder `.../apps/ebtc/1/eb.AppImage`, version `4.7.0`.
- Live network: `s1demo updates live` (fetch_remote_latest_versions took 9.19 s; real GitHub tags, e.g. feather 2.9.1, keepassxc 2.8.0) →
  ```
  updates_available -> 3 entries
    AvailableUpdate { app_id: "feather", installed: "2.6.0-2.7.0", latest: "2.9.1" }
    AvailableUpdate { app_id: "keepassxc", installed: "2.7.12", latest: "2.8.0" }
    AvailableUpdate { app_id: "electrum-btc", installed: "4.7.0", latest: "4.7.2" }
  ```
  Not-installed apps (jan, joplin, zulip, ...) are not listed. `update_available_for` agrees for each app. `s1demo update_for jan 0.8.8` (not installed) → `None`.
- `s1demo launch feather 1` → `returned after 333.786µs`, `Ok(())`. 1 s later the marker shows `started ... pid=1419309 args=[]` (the `%U` was dropped), and `ps` shows the script still running in its own process group (pgid 1419309 = pid). After 5 s the marker shows `finished`. With the caller kept alive 8 s (`s1demo launch feather 1 8`), `ps --ppid <caller>` after the script ended shows no children (no zombie).
- A quoted program path (`Exec="<own script>" %U`), no field code, and a legacy `Exec=<own AppImage> %U` all launch (`Ok(())`; the AppImage stub logged `appimage ran args=[]`).

## Outcome 2
> An app with no instances → `app_details` returns an empty instance list, not an error.
Verdict: holds
Evidence:
- Empty fake HOME: `s1demo details` → all 14 apps `instances=0`, `verification=NotChecked`, exit 0.
- Not-installed apps after the fixture install (e.g. jan, joplin) still `instances=0`.

## Outcome 3
> An instance folder without its AppImage or launcher → that instance is listed with the missing piece marked, not dropped.
Verdict: holds
Evidence: `s1demo details feather`:
```
#2 dir_present=true  appimage=.../feather/2/feather.AppImage present=false size=None
   launcher=.../appimage-control-feather-2.desktop present=true  version=Some("2.7.0")
#3 dir_present=true  appimage=.../feather/3/feather.AppImage present=true size=Some(10)
   launcher=.../appimage-control-feather-3.desktop present=false  version=Some("2.6.0")
#5 dir=.../feather/5 dir_present=false  appimage present=false size=None
   launcher=.../appimage-control-feather-5.desktop present=true  version=None
```
Missing AppImage (#2), missing launcher (#3) and missing folder (#5) are each listed and marked.
Not listed: #4, a folder holding only a version file (no AppImage and no launcher), and bare empty folders. This matches the crate's documented instance definition (`list_managed_instance_indices`: "instance dirs (with an AppImage) and managed .desktop entries"). See Minor finding 4.

## Outcome 4
> No remote version known (offline, GitHub rate limit) → `updates_available` leaves that app out. A failed lookup never counts as "update available".
Verdict: holds
Evidence:
- Real offline: `HTTPS_PROXY=http://127.0.0.1:9 HTTP_PROXY=... s1demo updates live` (dead proxy; `unshare -rn` is not permitted on this host) → fetch took 5.6 ms. Every GitHub tag is `None` and `latest_known=None` for every GitHub-sourced app. Result:
  ```
  updates_available -> 1 entries
    AvailableUpdate { app_id: "electrum-btc", installed: "4.7.0", latest: "4.7.2" }
  ```
  feather and keepassxc, both reported online, are left out. electrum-btc stays because its version comes from the catalog's pinned URL, not a lookup.
- `s1demo updates none` (all lookups failed) → same single electrum-btc entry.
- `s1demo update_for feather NONE` → `None`. An unpinned app with no version in its URL (anythingllm, installed 1.0.0) → `latest_known=None` even when handed tag `9.9.9`, and is never reported.
- An empty tag (`keepassxc=`) → `latest_known=None`, left out.
- GitHub rate limit was not caused directly. It reaches `updates_available` through the same `None` input as offline.

## Outcome 5
> Launcher missing or its Exec unparseable → `launch_instance` returns an error naming the app and instance. It never runs a partial command.
Verdict: holds
Evidence: `s1demo launch feather 1` with the .desktop edited, marker dir emptied first and checked 1 s after; no marker, no new process in every refused case:
| Exec / state | Error (Display) |
|---|---|
| `Exec=/usr/bin/touch <marker>` | `feather instance 1: cannot launch: Exec runs /usr/bin/touch, not this instance's own launcher` |
| `Exec="<own script> %U` (unclosed quote) | `feather instance 1: cannot launch: cannot parse Exec in .../appimage-control-feather-1.desktop` |
| `Exec=<own script> --extra %U` (and without `%U`) | `feather instance 1: cannot launch: Exec passes extra arguments (--extra); repair the launchers and try again` |
| `Exec=` | `... cannot parse Exec in ...` |
| no Exec line | `... launcher .../appimage-control-feather-1.desktop has no Exec` |
| `Exec=sh -c '<own script>; touch ...'` | `... cannot parse Exec in ...` |
| `Exec=env FOO=1 <own script> %U` | `... Exec runs env, not this instance's own launcher` |
| `Exec=<own script>;touch <marker>` | `... Exec runs <own script>;touch, not this instance's own launcher` |
| `Exec=<instance 2's script> %U` | `... Exec runs .../feather/2/appimage-control-launch.sh, not this instance's own launcher` |
| `Exec=.../feather/2/../1/<script> %U` | refused, `not this instance's own launcher` |
| launcher deleted | `feather instance 1: cannot launch: launcher .../appimage-control-feather-1.desktop: No such file or directory (os error 2)` |
| instance 3 (never had a launcher) | `feather instance 3: cannot launch: launcher ...feather-3.desktop: No such file or directory` |
| instance 5 (launcher, script missing) | `feather instance 5: cannot launch: .../feather/5/appimage-control-launch.sh is missing` |
| instance 9 / jan 1 (not installed) | `feather instance 9: ...` / `jan instance 1: ...` launcher not found |

Every error is `Error::LaunchRefused { app_id, index, reason }`, and its Display starts `<app> instance <n>:`. All return in under 0.25 ms.

## Outcome 6
> Unverified cache (no sidecar, or `unverified user-allowed`) → the verification state says so in plain words.
Verdict: holds
Evidence: `s1demo details feather | grep verification` with `.../appimages/cache/feather/.appimage-control-sha256` set to:
| sidecar | `verification` |
|---|---|
| absent | `NotChecked` (doc: "No sidecar: nothing cached yet, or cached before verification existed.") |
| `<hex> verified catalog` | `CatalogHash` |
| `<hex> verified github-digest` | `GithubDigest` |
| `<hex> unverified user-allowed` | `UnverifiedAllowed` (doc: "No published sha256; installed because the user allowed it.") |
| `<hex> something-else`, `nothex verified catalog`, empty file, sidecar is a directory | `Unreadable` |
Both unverified cases get their own state, distinct from the verified ones. The state is a named enum variant. The crate offers no Display or label text for it (see Minor finding 1).

## Findings

Blocking: none.

Material:
1. `updates_available` misses a lagging instance. feather has instances at 2.6.0 (#3) and 2.7.0 (#1, #2). With latest = 2.7.0, `s1demo updates set feather=2.7.0` reports no feather update, although instance #3 is behind. The installed label shows the range `2.6.0-2.7.0`, but the newer-than test seems to use the highest version only. A login checker built on this would not flag an old instance.

Minor:
1. `Verification` has only `Debug`/`Clone`/`PartialEq`. The plain-language wording exists only in rustdoc, so Stage 2 must write the user-facing words itself.
2. A non-version GitHub tag counts as an update. `s1demo updates set feather=latest` → `AvailableUpdate { app_id: "feather", installed: "2.6.0-2.7.0", latest: "latest" }`. This is not a failed lookup, but a repo whose latest release has a non-numeric tag would report "update available" forever. The live run showed no catalog repo doing this today.
3. An installed instance with no recorded version is reported as updatable: `s1demo update_for session 1.18.3` with no version file → `installed: "unknown", latest: "1.18.3"`. This looks deliberate (the docs mention `unknown`). Noted for Stage 3's wording.
4. A folder holding data but no AppImage and no launcher (#4) is not listed by `app_details`. This matches the crate's own instance definition, so it is not an Outcome 3 failure. The detail page will not show such a leftover folder.
5. `launch_instance` on an instance whose AppImage is missing but whose launcher and script exist (#2) returns `Ok(())` and runs the script. Because the call returns at once, the caller never learns that the AppImage was absent.

Not checked: the Flatpak premise (`flatpak-spawn --host`) cannot be checked on this host. GitHub rate limiting was not caused directly (see Outcome 4).

Background work: one backgrounded caller (`s1demo launch feather 1 8`) was waited on with `wait <pid>` (exit 0). Every launched script was confirmed finished by its `finished` marker line, or by its 5 s sleep ending inside a 10 s wait loop. No background jobs remain.
