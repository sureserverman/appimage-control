# Stage 3 premises — 2026-11-05-security-audit-remediation-plan

Goal: Make sure the user never runs an AppImage whose bytes differ from what upstream published, and never runs one with no hash to check unless they chose to.
Risk: HIGH — this changes the main install path for every app; a wrong digest parser or tag decoder would block every install

Outcomes:
- Works: a GitHub-hosted app downloads, its bytes match GitHub's `digest`, and it installs as before; the cache dir gains a `.appimage-control-sha256` sidecar reading `<hex> verified github-digest`.
- Works: a catalog entry with `sha256` (or `sha256_aarch64` on arm64) downloads and is checked against it; a catalog hash takes precedence over GitHub's.
- The hash does not match → the `.partdownload` is deleted, nothing is made executable, the cached copy (if any) is left as it was, and the user sees `<app>: download does not match the expected sha256 (expected <a>, got <b>); nothing was installed`. No opt-in overrides a mismatch.
- No hash is available (catalog has none and GitHub returns `null`, the URL is not a GitHub asset, or the digest lookup failed) → CLI: the op fails with `<app>: no published sha256 to verify this download; re-run with --allow-unverified to install it anyway`. GUI: a dialog names the apps and offers Cancel (default) and "Install unverified"; Cancel leaves everything untouched, "Install unverified" re-runs Apply for those apps only. An opted-in install writes the sidecar as `<hex> unverified user-allowed`.
- A download URL is plain http (not loopback), or redirects to one → refused with `<app>: refusing non-https download URL <url>`.
- A download exceeds 4 GiB (Content-Length or streamed) → stopped, partial file removed, `<app>: download exceeds the 4 GiB limit`.
- A download fails for a network reason → the message carries the whole error chain (resolve, connect, TLS), not just reqwest's top line.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: GitHub's release-asset `digest` (sha256) is published for the catalog's GitHub-hosted AppImages, so they verify with no maintainer work.
Checked: `gh api repos/<o>/<r>/releases/tags/<tag> --jq '.assets[] | select(.name==<asset>) | .digest'` for every GitHub URL in data/catalog.json (2026-11-05).
Result: digest present for session v1.17.17, keepassxc 2.7.12 (sha256:564fe8b7…), jan v0.7.9, cryptomator 1.19.2 (both arches), joplin v3.5.13, zulip v5.12.3, standard-notes 3.201.21 (both arches; tag holds `/`). `null` for feather 2.8.1 (both arches: uploaded before GitHub published digests). qnote v0.2.11: the release itself returns 404 (catalog URL already dead, unrelated to this plan).
Verdict: holds — as the Research Summary predicted, older assets have `null`; feather therefore goes through the opt-in path unless Task 3.6 or a later change pins it.

## Premise 2
Assumption: a failed digest lookup (rate limit, network) is common enough to matter and must map to "unavailable", never "verified".
Checked: `curl -s https://api.github.com/rate_limit`
Result: anonymous limit 60 requests/hour (51 left at check time). One lookup per GitHub download, so a busy session can exhaust it.
Verdict: holds — the design's Unavailable path covers it.

## Premise 3
Assumption: the vendored reqwest 0.12.28 supports a custom redirect policy and a connect timeout; sha2 is already in the lockfile.
Checked: `grep -n 'pub fn custom\|pub fn connect_timeout' vendor/reqwest/src/redirect.rs vendor/reqwest/src/blocking/client.rs`; `grep -n 'name = "sha2"' Cargo.lock`
Result: redirect.rs:102 `pub fn custom`, blocking/client.rs:399 `pub fn connect_timeout`; sha2 present.
Verdict: holds.

## Premise 4
Assumption: mattermost, electrum-btc and electrum-ltc publish a checksum or signature the catalog hash can be taken from.
Checked: in Task 3.6.
Result: see Task 3.6 commit.
Verdict: not checkable here — Task 3.6 owns it, and its plan text already allows "left without a hash" when no source exists.
