# Stage 3 demo — 2026-11-05-security-audit-remediation-plan

Environment: (see below)

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

Setup for every run (round 2, after gate round 1; Ubuntu VM `user@192.168.122.204`, x86_64):
- The VM tree `~/aic` was checked first. The sha256 of every git-tracked file under `crates/`, plus `Cargo.toml` and `Cargo.lock`, equals the host branch at `f3daaa2d`. Built as shipped for dev: `cd ~/aic && bash -lc 'cargo build -p appimage-control'` → `~/aic/target/debug/appimage-control`. It was always run from that path.
- Every CLI run used a fresh `HOME=$(mktemp -d /tmp/aicdemo-*)` with `XDG_DATA_HOME=$HOME/.local/share`. Scratch one-app catalogs were given with `APPIMAGE_CONTROL_CATALOG=<abs path>`. Each is the real `keepassxc` entry with `download_url`, `sha256` or `sha256_aarch64` changed.
- Loopback stand-ins (`standin.py`) are a file server plus a fake GitHub API. They ran on 127.0.0.1:18080 (real digest), :18081 (digest replaced by `sha256:abab…ab`), :18082 (digest `null`) and :18083 (HTTP 500), and each logs the requests it gets. `openssl s_server` with a self-signed cert ran on :18443.
- Real upstream digest, from `api.github.com` on the host today: `KeePassXC-2.7.12-x86_64.AppImage sha256:564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49`.
- In the output, `BAD` = `cdcd…cd` (64 hex characters).

## Outcome 1
> Works: a GitHub-hosted app downloads, its bytes match GitHub's `digest`, and it installs as before; the cache dir gains a `.appimage-control-sha256` sidecar reading `<hex> verified github-digest`.
Verdict: holds
Evidence:
Built-in catalog, real GitHub download, real GitHub API (no overrides apart from the scratch HOME):
```
$ HOME=/tmp/aicdemo-o1.51Ow … appimage-control +keepassxc
Downloading https://github.com/keepassxreboot/keepassxc/releases/download/2.7.12/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0
-rwxr-xr-x 2332     apps/keepassxc/1/appimage-control-launch.sh
-rw-rw-r-- 7        apps/keepassxc/1/.appimage-control-version
-rwxr-xr-x 49247424 apps/keepassxc/1/KeePassXC.AppImage
-rw-rw-r-- 7        cache/keepassxc/.appimage-control-version
-rw-rw-r-- 88       cache/keepassxc/.appimage-control-sha256
-rwxr-xr-x 49247424 cache/keepassxc/KeePassXC.AppImage
sidecar: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49 verified github-digest
sha256sum cache file: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49
$ appimage-control --list   →   keepassxc  KeePassXC  1  installed 2.7.12
.local/share/applications/appimage-control-keepassxc-1.desktop present
```


## Outcome 2
> Works: a catalog entry with `sha256` (or `sha256_aarch64` on arm64) downloads and is checked against it; a catalog hash takes precedence over GitHub's.
Verdict: holds on x86_64. The arm64 variant (`sha256_aarch64` on arm64) cannot reach by the user's route: no arm64 host is available.
Evidence:
```
# Precedence 1: catalog sha256 = correct; GitHub API stand-in :18081 serves a WRONG digest; real GitHub download
$ … APPIMAGE_CONTROL_CATALOG=/tmp/aicscripts/cat-gh-good.json APPIMAGE_CONTROL_GITHUB_API_BASE=http://127.0.0.1:18081 appimage-control +keepassxc
Downloading https://github.com/keepassxreboot/keepassxc/releases/download/2.7.12/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0
sidecar: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49 verified catalog
stand-in :18081 log: (empty — the API was not asked)

# Precedence 2: catalog sha256 = BAD; GitHub API stand-in :18080 serves the REAL digest
$ … cat-gh-bad.json … API_BASE=http://127.0.0.1:18080 appimage-control +keepassxc
Downloading https://github.com/…/KeePassXC-2.7.12-x86_64.AppImage …
keepassxc: download does not match the expected sha256 (expected cdcd…cd, got 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49); nothing was installed
exit=1   files: none   stand-in log: (empty)

# Non-GitHub URL, catalog sha256 = correct
$ … cat-loc-good.json appimage-control +keepassxc
Downloading http://127.0.0.1:18080/files/KeePassXC-2.7.12-x86_64.AppImage …
Installed keepassxc instance 1 …
exit=0   sidecar: 564fe8…ae49 verified catalog   stand-in log: GET /files/KeePassXC-2.7.12-x86_64.AppImage

# x86_64 ignores sha256_aarch64: sha256 = correct, sha256_aarch64 = efef…ef
$ … cat-arch.json appimage-control +keepassxc
Installed keepassxc instance 1 …   exit=0   sidecar: c5d5fbed…daaa6 verified catalog
```
Control showing that the stand-in API is really consulted when the catalog has no hash: built-in catalog with API :18081 gave `keepassxc: download does not match the expected sha256 (expected abab…ab, got 564fe8…ae49); nothing was installed`, exit 1. The stand-in logged `GET /repos/keepassxreboot/keepassxc/releases/tags/2.7.12`.


## Outcome 3
> The hash does not match → the `.partdownload` is deleted, nothing is made executable, the cached copy (if any) is left as it was, and the user sees `<app>: download does not match the expected sha256 (expected <a>, got <b>); nothing was installed`. No opt-in overrides a mismatch.
Verdict: holds
Evidence:
No cached copy: see Outcome 2, precedence 2 (catalog mismatch), and the control (GitHub-digest mismatch). Both exit 1 with nothing under `appimages/`.

Cached copy present. This is the HOME from Outcome 1, where the cache holds the verified 2.7.12 build. The catalog URL now points at loopback, with sha256 = BAD. A poller watched for `*partdownload*` every 50 ms:
```
before: cache/keepassxc/KeePassXC.AppImage 49247424 mtime 19:47:03.6282037620, sha 564fe8…ae49, sidecar "564fe8…ae49 verified github-digest"
$ … cat-loc-bad.json appimage-control +keepassxc
Cached KeePassXC.AppImage does not match the catalog sha256; downloading again …
Downloading http://127.0.0.1:18080/files/KeePassXC-2.7.12-x86_64.AppImage …
keepassxc: download does not match the expected sha256 (expected cdcd…cd, got 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49); nothing was installed
exit=1
poller: .KeePassXC.AppImage.beb4e618-….unverified.partdownload 16811643 … 47685632
after: no *partdownload; cache, instance 1, version files, sidecar: same sizes, same mtimes, same sha
$ … same + --allow-unverified  →  same mismatch line, exit=1; partdownload seen then gone; nothing changed
```
Update path with an older cached build. 2.7.11 was installed and verified (`c5d5fbed… verified catalog`). Then the catalog moved to the 2.7.12 URL with sha256 = BAD:
```
$ … cat-loc-bad.json appimage-control -update
Cache KeePassXC.AppImage is outdated for this catalog; replacing …
Downloading http://127.0.0.1:18080/files/KeePassXC-2.7.12-x86_64.AppImage …
keepassxc instance 1: keepassxc: download does not match the expected sha256 (expected cdcd…cd, got 564fe8…ae49); nothing was installed
Summary: 0 updated, 0 unchanged, 0 skipped (already catalog), 1 failed.
1 instance update(s) failed (see above)
exit=1
poller: -rw-rw-r-- .KeePassXC.AppImage.94ade991-….unverified.partdownload (never executable), gone after the run
after: cache + instance 1 unchanged (200000 bytes, mtime 19:49:05.9046811950, version 2.7.11, sidecar unchanged)
$ … -update --allow-unverified  →  same mismatch line, "1 failed", exit=1, nothing changed
```
GUI, Demo Delta (catalog sha256 = BAD): `+` then `Apply (1)`. No confirm dialog was offered. The new Activity log window read `Downloading http://127.0.0.1:18080/files/d.AppImage …` / `demo-d: demo-d: download does not match the expected sha256 (expected cdcd…cd, got 1b4d8780…a960); nothing was installed`. No AppImage was written for demo-d; only an empty `cache/demo-d/` directory was left. The main window's status line contradicts this (see M1).


## Outcome 4
> No hash is available (catalog has none and GitHub returns `null`, the URL is not a GitHub asset, or the digest lookup failed) → CLI: the op fails with `<app>: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway`. GUI: a dialog names the apps and offers Cancel (default) and "Install unverified"; Cancel leaves everything untouched, "Install unverified" re-runs Apply for those apps only. An opted-in install writes the sidecar as `<hex> unverified user-allowed`.
Verdict: holds
Evidence:
CLI, every cause (all exit=1, no files written):
```
# GitHub digest null (:18082)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github has no digest for this asset)
# digest lookup failed, HTTP 500 (:18083)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github api returned HTTP 500 Internal Server Error)
# digest lookup failed, nothing listening (:18099)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (github api request failed: error sending request for url (http://127.0.0.1:18099/repos/keepassxreboot/keepassxc/releases/tags/2.7.12))
# not a GitHub asset, no catalog sha256
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (no catalog sha256 and not a GitHub asset)
  file server log: no request
# -update, older build installed, catalog moved to a 2.7.12 loopback URL with no sha256
keepassxc instance 1: keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (no catalog sha256 and not a GitHub asset)
1 instance update(s) failed (see above); no published sha256 for keepassxc: re-run with --allow-unverified to install anyway
exit=1, instance still 2.7.11, cache unchanged, file server: no request
```
CLI opt-in:
```
$ … cat-small.json appimage-control +keepassxc --allow-unverified
keepassxc: no published sha256 (no catalog sha256 and not a GitHub asset); installing unverified as allowed
Downloading http://127.0.0.1:18080/files/small.AppImage …
Installed keepassxc instance 1 …   exit=0
sidecar: c5d5fbed8d28f989e252f910831082a244a224370244e70d46a822713f3daaa6 unverified user-allowed
$ … cat-gh-none.json API :18082 (null) appimage-control +keepassxc --allow-unverified
keepassxc: no published sha256 (github has no digest for this asset); installing unverified as allowed
Installed keepassxc instance 1 …   exit=0
sidecar: 564fe8b751b9ef7aa057e4d3d0b2878db24eaa0f6b1c855c82e699ab0913ae49 unverified user-allowed
$ … cat-loc-none.json appimage-control -update --allow-unverified     (the 2.7.11 install above)
keepassxc instance 1: applying build from catalog cache …   Summary: 1 updated …   exit=0
instance now 49247424 bytes, version 2.7.12; sidecar "564fe8…ae49 unverified user-allowed"
```
GUI: started on the VM's live Wayland session with `GTK_A11Y=atspi`, scratch HOME/XDG dirs and a 4-app catalog. Demo Alpha and Demo Bravo have no sha256. Demo Charlie has a correct sha256. Demo Delta has a BAD sha256. All are served from loopback. The GUI was driven over AT-SPI.
```
Apply 1: + Alpha, + Bravo, + Charlie → "3 apps changing" → click 'Apply (3)'
  t=1s: new toplevel dialog 'Install without verification?'
    label: 'No published sha256 is available to check these downloads, so they were not installed. Install them anyway only if you trust where they come from.'
    label: 'Demo Alpha: no catalog sha256 and not a GitHub asset\nDemo Bravo: no catalog sha256 and not a GitHub asset'
    button: 'Cancel' [focused]
    button: 'Install unverified'
  stand-in log: GET /files/c.AppImage only; files: demo-c installed, sidecar "7e08acf6…fb28 verified catalog"
  click Cancel → dialog closes; GUI alive; Alpha/Bravo 'Available + no instances'
    file tree (names, modes, mtimes) identical before and after Cancel; stand-in log: no request
Apply 2: + Alpha, + Bravo, + Charlie (2nd instance) → click 'Apply (3)'
  t=1s: same dialog naming Demo Alpha and Demo Bravo, Cancel focused
  Charlie instance 2 installed from cache ("Using cached DemoCharlie.AppImage"); no request
  click 'Install unverified' →
    stand-in log: GET /files/a.AppImage, GET /files/b.AppImage   (nothing for c)
    Activity log: "demo-a: no published sha256 (…); installing unverified as allowed" / "Installed demo-a instance 1 …" / same for demo-b
    cells: Alpha 'Installed 1', Bravo 'Installed 1', Charlie 'Installed 2 instances' (not 3)
    sidecars: demo-a "bf30c344…6d4ed0 unverified user-allowed"
              demo-b "79952caf…7c834f unverified user-allowed"
              demo-c "7e08acf6…dfb28 verified catalog"
  gui.log: empty (no panic) across all GUI runs
```
Round 1's Blocking crash on Apply did not happen in any GUI run here.


## Outcome 5
> A download URL is plain http (not loopback), or redirects to one → refused with `<app>: refusing non-https download URL <url>`.
Verdict: holds. The redirect case is refused with different wording; see m1.
Evidence:
```
# http URL, catalog sha256 present
$ … cat-http-sha.json appimage-control +keepassxc
Downloading http://example.com/files/KeePassXC.AppImage …
download failed: keepassxc: refusing non-https download URL http://example.com/files/KeePassXC.AppImage
exit=1, files: none
# http URL, no hash, --allow-unverified
download failed: keepassxc: refusing non-https download URL http://example.com/files/KeePassXC.AppImage   exit=1
# http URL, no hash, no flag → the no-hash refusal comes first (see m3)
keepassxc: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway (no catalog sha256 and not a GitHub asset)   exit=1
# loopback URL answering 302 → http://example.com/files/x.AppImage (with a catalog sha256, and again with no hash + --allow-unverified)
download failed: keepassxc: error following redirect for url (http://127.0.0.1:18080/redirect-http): refusing redirect to non-https URL http://example.com/files/x.AppImage
exit=1, files: none; stand-in log: GET /redirect-http (×2), nothing fetched from example.com
```


## Outcome 6
> A download exceeds 4 GiB (Content-Length or streamed) → stopped, partial file removed, `<app>: download exceeds the 4 GiB limit`.
Verdict: holds
Evidence:
Content-Length 5 GiB. The server sends 64 MiB, then stalls:
```
$ … cat-hugecl.json (catalog sha256) appimage-control +keepassxc
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1   real 0m0.123s   files: none, no *partdownload
$ … cat-hugecl-none.json appimage-control +keepassxc --allow-unverified
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1   real 0m0.109s
stand-in log: huge-cl stopped: BrokenPipeError / ConnectionResetError (client hung up at once)
```
Streamed download: chunked, with no Content-Length, 6 GiB of zeros. The VM's disk cannot hold 4 GiB, so HOME was put on a temporary `tmpfs size=4700M` at `/mnt/aicbig-d3`, which was unmounted and removed afterwards:
```
$ HOME=/mnt/aicbig-d3/aicdemo-run.Fu5d … cat-hugestream.json appimage-control +keepassxc
Downloading http://127.0.0.1:18080/huge-stream …
download failed: keepassxc: download exceeds the 4 GiB limit
exit=1   real 0m9.621s
poller: -rw-rw-r-- partdownload 41461397 … 4265431040
after: no *partdownload, HOME 0 bytes
stand-in log: huge-stream stopped after 4299161600 bytes: ConnectionResetError
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
# TLS: plain-http server on the https port
download failed: keepassxc: error sending request for url (https://127.0.0.1:18080/files/small.AppImage): client error (Connect): tls handshake eof
# TLS: self-signed cert (openssl s_server)
download failed: keepassxc: error sending request for url (https://127.0.0.1:18443/KeePassXC.AppImage): client error (Connect): invalid peer certificate: Other(OtherError(CaUsedAsEndEntity))
```
All exit=1, files: none. The GitHub digest lookup's failure reason still shows only reqwest's top line; see m5.


## Findings

| Outcome | Verdict |
|---|---|
| 1 GitHub digest verified, sidecar | holds |
| 2 catalog sha256, precedence | holds on x86_64; the `sha256_aarch64`-on-arm64 variant cannot reach by the user's route (no arm64 host) |
| 3 mismatch refused, cache kept, no opt-in override | holds |
| 4 no hash: CLI refusal, GUI dialog, opt-in sidecar | holds |
| 5 non-https refused | holds (redirect wording differs, m1) |
| 6 4 GiB limit | holds (Content-Length and streamed) |
| 7 full error chain | holds |

Plan goal (host, at evaluation time): `cargo audit --json | python3 … sys.exit(count)` → `vulnerabilities.count = 0`, exit 0.

Fixed since round 1, checked again: the GUI no longer aborts on Apply (round-1 B1). `-update` no longer relabels a stale cached build when no hash is available; it now refuses (round-1 M1). `-update` now exits 1 when an update is refused (round-1 M2).

### Blocking
- None among the Outcomes as reachable here.
- Not reached: the arm64 variant of Outcome 2 (`sha256_aarch64` checked on an arm64 host). The VM and host are x86_64, and arm64 multiarch is off-limits on this host. On x86_64 the product correctly ignores a wrong `sha256_aarch64`. Under the brief's rule, "cannot reach" is Blocking. The gate must decide whether this platform variant needs an arm64 run.

### Material
- **M1. The GUI says "All changes applied" after a download was refused.** Case 1: Demo Delta with a hash mismatch. After `Apply (1)`, the main window reads `All changes applied` / `Nothing pending — you're up to date`. The Activity log header reads `2 lines · 0 errors`. The only sign of the refusal is the mismatch line inside the log text. Case 2: after Cancel in the unverified dialog, the same status text shows, and the pending `+1` for Alpha and Bravo has been dropped. Files are handled correctly in both cases. But a GUI user who does not read the log is told a refused, possibly tampered download was "applied".

### Minor
- m1. The redirect-to-http refusal reads `download failed: keepassxc: error following redirect for url (http://127.0.0.1:18080/redirect-http): refusing redirect to non-https URL http://example.com/files/x.AppImage`. That is not the specified `<app>: refusing non-https download URL <url>`. The download is still refused. Several CLI messages also start with `download failed: ` before `<app>:`.
- m2. Two problems with the GUI Activity log lines. The app id is doubled: `demo-a: demo-a: no published sha256 …` and `demo-d: demo-d: download does not match …`. The no-hash line also tells a GUI user to "re-run with --allow-unverified", which is a CLI flag.
- m3. For an `http://` URL with no hash, the first refusal says "re-run with --allow-unverified". Re-running with that flag then fails on the non-https check.
- m4. `Downloading <url> …` is printed before refusals that make no request at all. This happens for the no-hash refusal, where the file server logged nothing, and for the non-https refusal.
- m5. When the digest lookup fails, the reason shown is only reqwest's top line: `(github api request failed: error sending request for url (http://127.0.0.1:18099/…))`. The underlying `tcp connect error: Connection refused` is not shown.
- m6. The temporary file is named `….unverified.partdownload` even when a catalog sha256 is about to be checked.
- m7. Each GUI Apply opens one more "Activity log" window. Four were stacked after four Applies.
- m8. A refused GUI download leaves an empty `cache/<app>/` directory (`cache/demo-d`).

### Housekeeping
- No background job is left running. One GUI step ran past the 120 s tool timeout: its AT-SPI `toplevels` query hung while the dialog was open. It was moved to the background and finished on its own. Its completion notice was awaited, and the output read.
- On the VM, the stand-in servers and `openssl s_server` were killed, and the GUI was closed with `kill`. Afterwards `pgrep` found no appimage-control, stand-in or s_server process.
- The temporary tmpfs `/mnt/aicbig-d3` was unmounted and removed. `/tmp/aicscripts`, `/tmp/aicsrv` and `/tmp/aicdemo-*` were deleted, and the `/tmp` listing matches the pre-run baseline. Two `nice-dns-*` files that disappeared belonged to another session and were not touched by this run.
- No system file was changed. `target/debug/appimage-control` was rebuilt by the instructed build command.
- One read-only `appimage-control --catalog /tmp/aicscripts/cat-gui.json --list` ran with the real HOME. It only listed four demo apps with 0 instances and wrote nothing.
