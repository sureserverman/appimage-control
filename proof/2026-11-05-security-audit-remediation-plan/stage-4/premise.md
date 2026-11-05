# Stage 4 premises — 2026-11-05-security-audit-remediation-plan

Goal: Protect the maintainer's backups and signing key, and make the seed-pack updater refuse old or oversized releases, before OTA is ever switched on.
Risk: MEDIUM — the signature format changes; acceptable because no signed release exists (OTA dormant)

Outcomes:
- Works: a backup is written with mode 0600; a restore of an intact archive behaves as before.
- A restored archive has an entry whose bytes do not match its manifest sha256 → restore fails in phase 1 naming the entry; managed state is byte-identical to before.
- Works: `aic-sign-pack sign --sequence N <key> <tarball>` writes a v2 signature; the updater accepts it and records N.
- A signed release with a sequence lower than the last accepted one → refused with `seed-pack release N is older than installed release M`; installed packs untouched. Equal N re-installs.
- A v1 signature file → refused, naming the v2 format and `aic-sign-pack sign --sequence`.
- A tarball entry over 4 MiB, a total over 32 MiB, or more than 512 entries → refused, nothing installed.
- `aic-sign-pack keygen` onto an existing file → refuses and leaves it; a new key file is never readable by others at any moment.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: no signed seed-pack release exists, so the signature format can change with no compatibility cost.
Checked: `grep -n 'const DEFAULT_RELEASE_URL\|const SEED_PACKS_PUB_KEY' crates/core/src/seed_pack_update.rs`; `ls ~/dev/infra/installers/linux/appimage-control-seed-packs/`; `gh release list -R sureserverman/appimage-control-seed-packs`
Result: DEFAULT_RELEASE_URL = "<unset-placeholder>", SEED_PACKS_PUB_KEY = all-`A` placeholder; the companion dir holds only *.json and PNGs (no tarball, no .sig); no GitHub repo of that name exists.
Verdict: holds.

## Premise 2
Assumption: the live check's VM can install and run KeePassXC and AnythingLLM in a scratch HOME (OD-4).
Checked: VM download sanity at Preflight (curl 200, 49 MB) and the Stage 3 demo's real KeePassXC install through the allowed dev binary.
Result: KeePassXC 2.7.12 installed and verified (`verified github-digest`); the VM firewall entry for ~/aic/target/debug/appimage-control is in place.
Verdict: holds (VM disk is tight, ~1.2 G free — clean build output first).
