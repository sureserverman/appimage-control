# Details — one app's detail page

ID: WF-DETAILS-001

The detail page shows where one catalog app's updates come from, how its cached download was
checked, and each installed instance. From there an instance can be started or its folder
opened. It reads the app's state when it opens and changes nothing on disk.

Entry points:
- GUI: the info button at the right of each card's name (tooltip **Details**, accessible
  name "Details for <app>") → `details::present_details` in `crates/gui/src/details.rs`.
  The content comes from `app_details` in `crates/core/src/app_details.rs`. The latest
  release comes from `latest_known_version` in `crates/core/src/update_check.rs`, given the
  GitHub tag the window last fetched.

Steps:
1. **Open.** Clicking the card's info button opens a modal window titled with the app's name.
2. **About the app.** Three rows:
   - **Latest release:** the GitHub `releases/latest` tag for an app downloaded from GitHub,
     or the version in the catalog URL otherwise. "Not known yet" when the lookup has not
     run or failed.
   - **Updates from:** "GitHub releases: owner/repo", "Pinned download" (the catalog
     publishes a sha256), "Download without a published checksum", or "No download for this
     system". The URL is shown under the two download kinds.
   - **Download check:** how the download in the cache was accepted, read from the cache
     sidecar (`.appimage-control-sha256`). The texts are "Verified — matches the checksum in
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
   its output goes to `<instance folder>/.appimage-control-launch.log`.
   - If launching fails, a toast says "Could not launch <app> #N". Its **Show log** button
     opens the full error in the activity log, and the window stays open.
   - The button is off, with a tooltip saying why, when the AppImage or the launcher is
     missing.
5. **Open folder.** Opens the instance folder in the file manager. The button is off when
   the folder is missing.
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
- Live: `python3 scripts/atspi-smoke.py details` (Launch from the dialog starts a fixture
  instance) and `details-all` (every catalog card opens its dialog), on a logged-in desktop
  session.
