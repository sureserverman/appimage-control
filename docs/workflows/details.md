# Details — one app's detail page

ID: WF-DETAILS-001

The detail page shows where one catalog app's updates come from, how its cached download was
checked, and each installed instance. From there an instance can be started or its folder
opened. It reads the app's state when it opens and changes nothing on disk.

Entry points:
- GUI: the info button at the right end of each card's header row (tooltip **Details**,
  accessible name "Details for <app>") → `details::present_details` in
  `crates/gui/src/details.rs`. The content comes from `app_details` in
  `crates/core/src/app_details.rs`. The latest release is the card's own "Latest" label
  (`catalog_row_latest_version_label`), given the GitHub tag the window last fetched.

Steps:
1. **Open.** Clicking the card's info button opens a modal window titled with the app's name.
   Escape or the window's close button closes it.
2. **About the app.** Three rows:
   - **Latest release:** the same version the card shows: the newest of the GitHub
     `releases/latest` tag, the version in the catalog URL, and any install's recorded
     version. For a GitHub app whose lookup has not answered (still running, offline, rate
     limited) it adds "GitHub could not be checked, so newer releases may exist". "Not known
     yet" only when no version is known at all.
   - **Updates from:** "GitHub releases: owner/repo", "Pinned download" (the catalog
     publishes a sha256), "Download without a published checksum", or "No download for this
     system". The URL is shown under the two download kinds.
   - **Download check:** how the download in the cache was accepted, read from the record
     the download wrote beside it (`.appimage-control-sha256`). It describes the cached
     download, not each instance, and the record's hash is not re-checked against the file
     when the page opens. The texts are "Verified — matches the checksum in
     the catalog", "Verified — matches the checksum GitHub publishes", "Not verified —
     installed with your permission", "Unknown — the checksum record cannot be read", and
     "Not checked yet".
3. **Instances.** One row per managed instance, "Instance #N":
   - The row shows the recorded version (or "Version unknown"), the AppImage size, and the
     instance folder. An AppImage left under an older catalog file name still counts.
   - With no instances, the page says "No instances installed — add one on the card".
   - If the install folders cannot be read, it says "Could not read the install folders:
     <error>" instead.
4. **Launch.** Starts the instance the way its menu launcher does (`launch_instance` in
   `crates/core/src/instances.rs`). It runs detached, on the host from inside a Flatpak, and
   its output goes to `<instance folder>/.appimage-control-launch.log` when that file can be
   written.
   - On success a toast says "Starting <app> #N…", and the button rests for 3 seconds so a
     slow start is not launched twice.
   - If launching fails, a toast says "Could not launch <app> #N". Its **Show log** button
     opens the full error in the activity log, and the window stays open.
   - The button is off, with a tooltip saying why, when the AppImage or the launcher is
     missing.
5. **Open folder.** Opens the instance folder in the file manager. The button is off when
   the folder is missing; if the desktop cannot open it, a toast says why.
   - Each row's buttons have their own accessible names ("Launch instance #N", "Open the
     folder of instance #N"), so a screen reader can tell the rows apart.
6. **Close.** The window's close button. Nothing was changed.

Invariants:
- The detail page never removes an instance (OD-1). Removal stays on the card tiles and
  **Apply** (WF-APPLY-001), so instances are still numbered `1..=target`.
- Only the catalog and GitHub-digest states read as "Verified". A missing, unreadable or
  user-allowed record never does (DEC-001).
- Launch runs only the instance's own `appimage-control-launch.sh`, or its own AppImage for
  launchers written before the script existed, with no extra arguments. Any other `Exec=`
  in the `.desktop` file is refused and nothing runs (OD-8).
- Opening the page downloads nothing and writes nothing. Launch writes only its log.

Tests:
- `crates/gui/src/details.rs` unit tests (`cargo test -p appimage-control --bins details`):
  texts, the empty and unreadable states, disabled buttons, and that no unverified state
  says "Verified".
- `crates/core/src/app_details.rs`, `crates/core/src/update_check.rs`, and the
  `launch_instance` tests in `crates/core/src/instances.rs`.
- Live, on a logged-in desktop session: `python3 scripts/atspi-smoke.py details` (the page
  shows a version, offers no removal, Launch starts a fixture instance, and a failed launch
  shows the toast whose Show log opens the activity log) and `details-all` (every catalog
  card opens its window). Each run fails on any GTK or Adwaita CRITICAL message.
