# Stage 3 demo — 2026-11-05-security-audit-remediation-plan

Environment: (see below)

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

Setup used for every run (Ubuntu VM `user@192.168.122.204`, x86_64):
- Built as shipped for dev: `cd ~/aic && bash -lc 'cargo build -p appimage-control'` → `~/aic/target/debug/appimage-control`, always run from that path. VM tree `crates/**/*.rs|*.json` checksum equals the host branch tree (`7c0d89a7…`).
- Every run: fresh `HOME=$(mktemp -d /tmp/aicdemo-*)`, `XDG_DATA_HOME=$HOME/.local/share`; scratch one-app catalogs via `APPIMAGE_CONTROL_CATALOG=` (written by a small generator; entry = the real keepassxc entry with `download_url` / `sha256` changed).
- Loopback stand-ins (`python3 standin.py`): file server + fake GitHub API on 127.0.0.1:18080 (real digest), :18081 (digest replaced by `sha256:abab…ab`), :18082 (digest `null`), :18083 (HTTP 500). Each logs the requests it receives. `openssl s_server` with a self-signed cert on :18443 for the TLS case.
- Real upstream digest (fetched from api.github.com on the host): `KeePassXC-2.7.12-x86_64.AppImage sha256:564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49`.

## Outcome 1
> Works: a GitHub-hosted app downloads, its bytes match GitHub's `digest`, and it installs as before; the cache dir gains a `.appimage-control-sha256` sidecar reading `<hex> verified github-digest`.
Verdict: holds
Evidence:
Built-in catalog, real GitHub, real API:
```
$ HOME=/tmp/aicdemo-o1.ywaO … appimage-control +keepassxc
Downloading https://github.com/keepassxreboot/keepassxc/releases/download/2.7.12/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0
-rw-rw-r-- 88 …/cache/keepassxc/.appimage-control-sha256
-rwxr-xr-x 49247424 …/cache/keepassxc/KeePassXC.AppImage
sidecar: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49 verified github-digest
sha256sum cache file: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49
$ appimage-control --list   →   keepassxc  KeePassXC  1  installed 2.7.12
apps/keepassxc/1/KeePassXC.AppImage (49247424, executable), applications/appimage-control-keepassxc-1.desktop
```
Extra check (same HOME): appended one byte to the cached file, then `+keepassxc` again → `Cached KeePassXC.AppImage changed since it was checked; downloading again …` / `Installed keepassxc instance 2 …`; cache back to `564fe8…`, sidecar unchanged. (Instance 1 also carried the appended byte because instances are hard links of the cache file — that came from my tampering, not from the app.)


## Outcome 2
> Works: a catalog entry with `sha256` (or `sha256_aarch64` on arm64) downloads and is checked against it; a catalog hash takes precedence over GitHub's.
Verdict: holds (x86_64 half; the `sha256_aarch64` half cannot reach by the user's route — no arm64 host available)
Evidence:
Non-GitHub URL with catalog `sha256` = real hash:
```
$ HOME=… APPIMAGE_CONTROL_CATALOG=/tmp/aicscripts/cat-o2c.json appimage-control +keepassxc
Downloading http://127.0.0.1:18080/files/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0
sidecar: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49 verified catalog
```
Precedence, real GitHub download, API pointed at stand-ins:
```
# catalog sha256 = correct, GitHub stand-in serves a WRONG digest (:18081)
$ … cat-o2a.json APPIMAGE_CONTROL_GITHUB_API_BASE=http://127.0.0.1:18081 appimage-control +keepassxc
Downloading https://github.com/keepassxreboot/keepassxc/releases/download/2.7.12/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0
sidecar: 564fe8…ae49 verified catalog
stand-in log: (empty — API not asked)

# catalog sha256 = WRONG (cdcd…), GitHub stand-in serves the REAL digest (:18080)
$ … cat-o2b.json APPIMAGE_CONTROL_GITHUB_API_BASE=http://127.0.0.1:18080 appimage-control +keepassxc
keepassxc: download does not match the expected sha256 (expected cdcdcdcd…cdcd, got 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49); nothing was installed
exit=1
files: (none)
```
Control showing the stand-in API is really used when the catalog has no hash: catalog without `sha256`, API :18081 → `expected abab…ab, got 564fe8…`; stand-in log `GET /repos/keepassxreboot/keepassxc/releases/tags/2.7.12`.
`sha256_aarch64`: the VM and host are x86_64; arm64 multiarch is off-limits on this host. Not exercised.


## Outcome 3
> The hash does not match → the `.partdownload` is deleted, nothing is made executable, the cached copy (if any) is left as it was, and the user sees `<app>: download does not match the expected sha256 (expected <a>, got <b>); nothing was installed`. No opt-in overrides a mismatch.
Verdict: holds
Evidence:
No cached copy: see Outcome 2 (catalog mismatch) and the GitHub-digest mismatch (`expected abab…`): exit 1, nothing under `appimages/`.
Cached copy present (`+keepassxc` again, catalog sha256 now wrong):
```
cache before: KeePassXC.AppImage 200000 mtime 19:25:38.311349349, sha 8f3d8183…c9e2, sidecar "8f3d…c9e2 unverified user-allowed"
$ … cat-small-bad.json appimage-control +keepassxc
Cached KeePassXC.AppImage does not match the catalog sha256; downloading again …
Downloading http://127.0.0.1:18080/files/small.AppImage …
keepassxc: download does not match the expected sha256 (expected cdcd…cdcd, got 8f3d81833cef9e2ecf370fa9c74b16c7062c3bd258238afbeee53f0b5479c9e2); nothing was installed
exit=1
$ … same + --allow-unverified        → same message, exit=1
cache after both: same size, same mtime, same sha, same sidecar
```
Update path with a cached older build (2.7.11 installed, catalog moved to a 2.7.12 build with a wrong sha256), with a poller watching for `*.partdownload`:
```
$ … cat-kp-bad.json appimage-control -update
Cached KeePassXC.AppImage does not match the catalog sha256; downloading again …
Downloading http://127.0.0.1:18080/files/KeePassXC-2.7.12-x86_64.AppImage …
keepassxc instance 1: keepassxc: download does not match the expected sha256 (expected cdcd…cdcd, got 564fe8…ae49); nothing was installed
Summary: 0 updated, 0 unchanged, 0 skipped (already catalog), 1 failed.
exit=0
poller during run: .local/share/appimages/cache/keepassxc/.KeePassXC.AppImage.0406300d-….unverified.partdownload 3137536 … 38232064
after run: no *.partdownload
cache + instance 1: unchanged (200000 bytes, mtime 19:26:07.0156, version file 2.7.11, sidecar unchanged)
$ … -update --allow-unverified       → same mismatch line, 1 failed, nothing changed
```
(`-update` exits 0 with "1 failed" — see Findings.)


## Outcome 4
> No hash is available (catalog has none and GitHub returns `null`, the URL is not a GitHub asset, or the digest lookup failed) → CLI: the op fails with `<app>: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway`. GUI: a dialog names the apps and offers Cancel (default) and "Install unverified"; Cancel leaves everything untouched, "Install unverified" re-runs Apply for those apps only. An opted-in install writes the sidecar as `<hex> unverified user-allowed`.
Verdict: does not hold — the CLI half holds; the GUI crashes on Apply before any dialog appears (Blocking B1)
Evidence:
CLI, all three causes:
```
# GitHub digest null (:18082)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github has no digest for this asset)
exit=1, files: none, stand-in log: GET /repos/keepassxreboot/keepassxc/releases/tags/2.7.12
# lookup failed, HTTP 500 (:18083)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github api returned HTTP 500 Internal Server Error)
exit=1
# lookup failed, nothing listening (:18099)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github api request failed: error sending request for url (http://127.0.0.1:18099/repos/keepassxreboot/keepassxc/releases/tags/2.7.12))
exit=1
# not a GitHub asset, no catalog sha256
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (no catalog sha256 and not a GitHub asset)
exit=1, files: none, file server log: no request for the file
```
CLI opt-in:
```
$ … cat-small.json appimage-control +keepassxc --allow-unverified
keepassxc: no published sha256 (no catalog sha256 and not a GitHub asset); installing unverified as allowed
Downloading http://127.0.0.1:18080/files/small.AppImage …
Installed keepassxc instance 1 …
exit=0
sidecar: 8f3d81833cef9e2ecf370fa9c74b16c7062c3bd258238afbeee53f0b5479c9e2 unverified user-allowed
```
Mixed CLI op: `+demo-c +demo-a` (demo-c has a catalog sha256, demo-a none) → `Installed demo-c instance 1 …`, then the demo-a refusal; `Summary: +demo-c OK; +demo-a FAILED (…)`, exit=1; demo-c sidecar `… verified catalog`.

GUI: the GUI was started on the VM's live Wayland session (`GTK_A11Y=atspi`, scratch HOME/XDG dirs, a 3-app catalog: Demo Alpha and Demo Bravo with no sha256 served from loopback, Demo Charlie with a correct catalog sha256). It was driven over AT-SPI: click the card's `+` tile, then click `Apply (N)`, then poll the app's toplevel windows and the process.
```
Run 1 — + Alpha, + Bravo, + Charlie → "3 apps changing", click 'Apply (3)'
  within 4 s the app vanished from the a11y bus; process gone
  gui.log: thread 'main' panicked at crates/gui/src/main.rs:870:27:
           RefCell already mutably borrowed
           … panic in a function that cannot unwind … thread caused non-unwinding panic. aborting.
           (stack: gtk4 SpinButton::connect_value_changed trampoline ← gtk_spin_button_set_value
            ← appimage_control::mount_catalog_view::{{closure}}::{{closure}} at crates/gui/src/main.rs:1254:36
            ← glib source trampoline ← g_application_run)
  stand-in log: GET /files/c.AppImage only; files: demo-c installed, sidecar "2509b5f9…45e2 verified catalog"; nothing for Alpha/Bravo
Run 2 — control, + Charlie only (verified) → click 'Apply (1)': no crash over 15 s; demo-c installed, "verified catalog"
Run 3 — + Alpha only (no hash) → click 'Apply (1)'
  t=1s: no toplevel windows; t=2s: GUI process GONE
  thread 'main' (1125127) panicked at crates/gui/src/main.rs:870:27: RefCell already mutably borrowed
  stand-in log: empty; files: none
Run 4 — + Alpha, + Bravo → click 'Apply (2)'
  t=2s: GUI process GONE; same panic at main.rs:870:27 via main.rs:1254:36; files: none
```
No "Install unverified" dialog appeared in any run. Cancel, its default state, and the "re-run Apply for those apps only" path could not be reached. A GUI user cannot install an app that has no published hash. The GUI aborts every time they try.


## Outcome 5
> A download URL is plain http (not loopback), or redirects to one → refused with `<app>: refusing non-https download URL <url>`.
Verdict: holds (redirect case refused with different wording — see Findings)
Evidence:
```
# http URL, catalog sha256 present
$ … cat-http-sha.json appimage-control +keepassxc
Downloading http://example.com/files/KeePassXC.AppImage …
download failed: keepassxc: refusing non-https download URL http://example.com/files/KeePassXC.AppImage
exit=1, files: none
# http URL, no hash, opted in
$ … cat-http.json appimage-control +keepassxc --allow-unverified
download failed: keepassxc: refusing non-https download URL http://example.com/files/KeePassXC.AppImage
exit=1
# http URL, no hash, not opted in → the no-hash refusal comes first and tells the user to re-run with --allow-unverified
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (no catalog sha256 and not a GitHub asset)
# loopback URL that answers 302 → http://example.com/files/x.AppImage
$ … cat-redir.json appimage-control +keepassxc --allow-unverified
download failed: keepassxc: error following redirect for url (http://127.0.0.1:18080/redirect-http): refusing redirect to non-https URL http://example.com/files/x.AppImage
exit=1, files: none, stand-in log: GET /redirect-http
```


## Outcome 6
> A download exceeds 4 GiB (Content-Length or streamed) → stopped, partial file removed, `<app>: download exceeds the 4 GiB limit`.
Verdict: holds
Evidence:
Content-Length 5 GiB (server sends 64 MiB then stalls):
```
$ … cat-hugecl.json appimage-control +keepassxc --allow-unverified
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1   real 0m0.108s   files: none, no *partdownload
$ … cat-hugecl-sha.json (catalog sha256 set) appimage-control +keepassxc
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1
stand-in log: huge-cl stopped: BrokenPipeError / ConnectionResetError (client hung up at once)
```
Streamed (chunked, no Content-Length, 6 GiB of zeros). /tmp (3.7 G) and / (1 G free) cannot hold 4 GiB, so HOME was put on a temporary `tmpfs size=4700M` at /mnt/aicbig (unmounted and removed afterwards):
```
$ HOME=/mnt/aicbig/aicdemo-run.qBRZ … cat-hugestream.json appimage-control +keepassxc --allow-unverified
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1   real 0m9.485s
poller: partdownload size 2218786816 … 3230281728 … 4067191654
after: no *partdownload, files: none
stand-in log: huge-stream stopped after 4299161600 bytes: BrokenPipeError
```


## Outcome 7
> A download fails for a network reason → the message carries the whole error chain (resolve, connect, TLS), not just reqwest's top line.
Verdict: holds
Evidence:
```
# resolve
download failed: keepassxc: error sending request for url (https://nonexistent-host.invalid/KeePassXC.AppImage): client error (Connect): dns error: failed to lookup address information: Name or service not known
# connect
download failed: keepassxc: error sending request for url (https://127.0.0.1:18099/KeePassXC.AppImage): client error (Connect): tcp connect error: Connection refused (os error 111)
# TLS, plain-http server on the https port
download failed: keepassxc: error sending request for url (https://127.0.0.1:18080/files/small.AppImage): client error (Connect): tls handshake eof
# TLS, self-signed cert
download failed: keepassxc: error sending request for url (https://127.0.0.1:18443/KeePassXC.AppImage): client error (Connect): invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))
```
All exit=1, files: none. (The digest-lookup failure reason in Outcome 4 is still only reqwest's top line — see Findings.)


## Findings

| Outcome | Verdict |
|---|---|
| 1 GitHub digest verified, sidecar | holds |
| 2 catalog sha256, precedence | holds (x86_64); `sha256_aarch64` cannot reach by the user's route (no arm64 host) |
| 3 mismatch refused | holds |
| 4 no hash: CLI refuse / GUI dialog / opt-in sidecar | does not hold — CLI and opt-in sidecar hold, GUI crashes |
| 5 non-https refused | holds (redirect wording differs) |
| 6 4 GiB limit | holds (Content-Length and streamed) |
| 7 full error chain | holds |

Plan goal check (host, at evaluation time): `cargo audit --json` → `vulnerabilities.count` = 0.

### Blocking
- **B1. The GUI aborts on Apply when any pending add has no published hash.** This reproduced in 3 of 3 runs: Alpha+Bravo+Charlie, Alpha alone, and Alpha+Bravo. The panic is `crates/gui/src/main.rs:870:27: RefCell already mutably borrowed`. It is raised inside a SpinButton `value-changed` handler, called through `set_value` from a glib source closure in `mount_catalog_view` (`main.rs:1254:36`). That handler cannot unwind, so the process aborts. The unverified-confirm dialog never shows. The GUI half of Outcome 4 cannot be used at all. A verified app in the same Apply (Charlie) was installed before the abort. An Apply that has only verified apps does not crash.

### Material
- **M1. `-update` relabels a stale cached build as the new version when no hash is available.** Setup: keepassxc 2.7.11 was installed with `--allow-unverified`. Then the catalog moved to a `…-2.7.12-…` URL with no sha256, and the URL is not a GitHub asset. Plain `-update` (no `--allow-unverified`) printed `Using cached KeePassXC.AppImage` / `applying build from catalog cache …` / `Summary: 1 updated`. It made no request to the server. The instance version file now says `2.7.12`, but the bytes are still the 200000-byte 2.7.11 file. The user is told they updated when they did not. The new build's missing hash is never surfaced. A similar thing happened when only the URL changed: `reconciled install metadata to 2.7.12 (AppImage size matches catalog cache)`, then "already at catalog version; skipping". The cache does not seem to be tied to the URL or version. It may predate this stage, but it undercuts the stage goal.
- **M2. `-update` exits 0 when an update is refused for a hash mismatch** (`Summary: … 1 failed.` / `exit=0`). A script cannot tell that a tampered download was refused. `+APP` exits 1 for the same refusal.

### Minor
- m1. The redirect-to-http refusal reads `download failed: keepassxc: error following redirect for url (http://127.0.0.1:18080/redirect-http): refusing redirect to non-https URL http://example.com/files/x.AppImage`. The Outcome specifies `<app>: refusing non-https download URL <url>`. The download is refused, but the wording differs. Several messages also carry a `download failed: ` prefix before `<app>:`.
- m2. For an `http://` URL with no hash, the first refusal tells the user to "re-run with --allow-unverified". Re-running with that flag then fails on the non-https check. The no-hash check runs before the https check.
- m3. `Downloading <url> …` is printed before refusals that make no request at all: the no-hash refusal (the file server logged nothing) and the non-https refusal.
- m4. The reason given for a failed digest lookup is only reqwest's top line: `(github api request failed: error sending request for url (http://127.0.0.1:18099/…))`. It has no `tcp connect error: Connection refused`. Outcome 7's full error chain is applied to downloads but not to this lookup.
- m5. The temporary file is named `.KeePassXC.AppImage.<uuid>.unverified.partdownload` even when the download has a catalog sha256 that is about to be checked.

### Not reached
- `sha256_aarch64` on arm64: no arm64 host is available (arm64 multiarch on this host is off-limits).

### Housekeeping
All background jobs finished on their own: the two GUI scenario runs and both waiters. Afterwards the stand-in servers (python, `openssl s_server`) were killed. No appimage-control process is left running; the GUI aborted in each crashing run, and the control run's GUI was killed by the next scenario. The temporary tmpfs `/mnt/aicbig` was unmounted and removed. `/tmp/aicdemo-*`, `/tmp/aicsrv` and `/tmp/aicscripts` were deleted. No system file was changed. The real HOME was only read once, by a stray `--list`, which is read-only.
