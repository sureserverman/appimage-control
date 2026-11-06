# Stage 1 premises — 2026-11-06-gear-lever-gap-plan

Goal: Give the GUI and the CLI one core source for an app's details, for the list of installed apps with newer releases, and for launching one instance.
Risk: MEDIUM — 1.3 executes a command read from a launcher file. The Exec comes from launchers this app wrote, but the file sits in a user-writable folder.

Outcomes:
- Works: `app_details` returns, for any catalog app, its instances (index, folder, AppImage path, recorded version, size, launcher present or not), its update source and its cache verification state. `updates_available` returns each installed app whose newer release is known. `launch_instance` starts an instance's launcher command and returns at once.
- An app with no instances → `app_details` returns an empty instance list, not an error.
- An instance folder without its AppImage or launcher → that instance is listed with the missing piece marked, not dropped.
- No remote version known (offline, GitHub rate limit) → `updates_available` leaves that app out. A failed lookup never counts as "update available".
- Launcher missing or its Exec unparseable → `launch_instance` returns an error naming the app and instance. It never runs a partial command.
- Unverified cache (no sidecar, or `unverified user-allowed`) → the verification state says so in plain words.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: An instance launcher's Exec runs the instance's own AppImage, so Launch can accept only that program.
Checked: `grep -h '^Exec=' ~/.local/share/applications/appimage-control-*.desktop` on the VM (real installs), plus `crates/core/src/instances.rs:1310` and `desktop.rs:74`.
Result: `Exec=/home/user/.local/share/appimages/apps/feather/1/appimage-control-launch.sh %U`. Every launcher runs the instance's own launch script, which sets up HOME/PATH/D-Bus and then runs the AppImage.
Verdict: false as written. Stopped and asked the owner, who answered "Own launch script only (Recommended)" (OD-8). Task 1.3's test was amended under OD-8. With that change the premise holds.

## Premise 2
Assumption: The cache sidecar records one of `verified catalog`, `verified github-digest` or `unverified user-allowed`, and a damaged sidecar is distinguishable from a missing one.
Checked: `crates/core/src/integrity.rs:184,204-234,277-286` (the only writer labels and `read_cache_sidecar`).
Result: exactly those three labels are written; `CacheSidecar::{Absent, Present, Corrupt}`.
Verdict: holds.

## Premise 3
Assumption: A remote version is known only from a successful lookup, so a failed lookup yields "unknown" rather than a version.
Checked: `crates/core/src/version_info.rs:260-279` (`fetch_github_release_hints` maps a failed fetch or a non-GitHub URL to `None`).
Result: GitHub apps get `Some(tag)` only on success. Non-GitHub apps always get `None` here; their catalog pin (`catalog_pinned_version`, local data, no lookup) is the only "latest" for them.
Verdict: holds. Task 1.2 combines the GitHub tag with the catalog pin for non-GitHub URLs.

## Premise 4
Assumption: Inside the Flatpak, `flatpak-spawn --host` can start the instance's launch script on the host.
Checked: not run. No Flatpak build of this branch is installed here.
Result: —
Verdict: not checkable here — needs the CI Flatpak bundle installed on a desktop. Listed for the owner at close-out.
