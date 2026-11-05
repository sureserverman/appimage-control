# Companion repo: distributing seed packs to a select audience

This document describes how to set up a **private** companion
repository that distributes signed seed-pack updates separately
from the `appimage-control` binary. It is an operator guide for the
person who curates seed packs and decides who gets them. The public
`appimage-control` repo ships only the architecture (loader,
interpreter, signature verifier) — no curated pack content.

For the binary-side specification of what `--update-seed-packs`
expects on the wire, see `crates/core/src/seed_pack_update.rs`.

## Why a separate repo

Seed packs encode operational choices that are not for general
public consumption — pinned `.onion` server addresses, trust
assumptions about specific wallet builds.
Curating them as content rather than code lets you:

- Ship updates the same hour a pinned server changes, without
  rebuilding or redistributing the binary.
- Restrict access to vetted users (a private GitHub repo whose
  releases are visible only to invited collaborators).
- Sign the artifacts so even if the hosting GitHub account is
  compromised, an attacker can't substitute malicious packs without
  also stealing the offline private key.

## One-time setup (on a cold-storage machine)

Generate the signing keypair. The private half **must never enter
GitHub, CI, or any network-connected machine.**

```sh
# Build the helper binary on the cold-storage machine.
cargo build --release --bin aic-sign-pack -p appimage_control_core

# Generate the keypair. Stores the 32-byte raw seed at KEYFILE
# (mode 0600); prints the matching base64 public key on stdout.
target/release/aic-sign-pack keygen ~/.aic-keys/seed-packs.key
# Example output: HaCk2Kt9IBFmrMS3kF9uHZTO3uxUYShJy51Vg6uCrpU=

# Back up the keyfile to a separate offline medium. Anyone with this
# 32-byte file can sign a release tarball that every appimage-control
# user who trusts your build will install.
```

In a **private fork** of `appimage-control` (or a build branch your
authorised users pull from), replace the two placeholders in
`crates/core/src/seed_pack_update.rs`:

```rust
// Before
pub const DEFAULT_RELEASE_URL: &str = "<unset-placeholder>";
pub const SEED_PACKS_PUB_KEY: &str  = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

// After (with your actual values)
pub const DEFAULT_RELEASE_URL: &str =
    "https://github.com/your-org/your-private-pack-repo/releases/latest/download";
pub const SEED_PACKS_PUB_KEY: &str = "HaCk2Kt9IBFmrMS3kF9uHZTO3uxUYShJy51Vg6uCrpU=";
```

Build the binary from that branch. Distribute the binary to your
select audience through whatever channel they use (private apt
repo, signed release page, hand-out). Do **not** push these values
back to the public `appimage-control` repo — they encode your trust
choices, not the architecture.

Until both placeholders are replaced, `--update-seed-packs` on a
public build refuses every invocation without touching the network,
so an unkeyed binary remains safe in general distribution.

## Companion-repo layout

Create a new private GitHub repo (any name you like — the binary
points at whatever URL is baked into its `DEFAULT_RELEASE_URL`).
Contents:

```
your-private-pack-repo/
├── README.md           (private; document your decisions here)
├── packs/
│   ├── feather.json
│   ├── electrum-btc.json
│   └── electrum-ltc.json
└── .github/
    └── workflows/
        └── release.yml (optional; CI to assemble the tarball)
```

Each pack file follows the schema in `docs/seed-packs.md` of the
public repo. The companion repo is a *pure-data* repo — no Rust
code, no build scripts, no CI that touches the signing key.

## Cutting a release

The release flow has two stages: build the tarball on any machine,
sign it on the cold-storage machine, then upload.

```sh
# On any machine: build the tarball.
cd your-private-pack-repo
tar --create --zstd --file seed-packs.tar.zst -C packs/ \
    feather.json electrum-btc.json electrum-ltc.json

# Move the tarball to the cold-storage machine (USB / SD card).

# On the cold-storage machine: sign it with this release's sequence number.
# N must be higher than every earlier release's (keep a counter next to the key);
# installs refuse a release older than the last one they accepted.
aic-sign-pack sign --sequence N ~/.aic-keys/seed-packs.key seed-packs.tar.zst
# Produces seed-packs.tar.zst.sig (77 bytes, format v2).

# Sanity-check the signature against the embedded pubkey before upload;
# it prints the signed release sequence.
aic-sign-pack verify "<EMBEDDED-PUBKEY-FROM-YOUR-FORK>" seed-packs.tar.zst

# Move both files back to the network-connected machine, attach to a
# GitHub release. Mark the repo private; only invited collaborators
# and authorised users can pull.
gh release create v2026.05.10 \
    --repo your-org/your-private-pack-repo \
    --title "Seed packs 2026-05-10" \
    --notes "Refreshed BTC server list." \
    seed-packs.tar.zst \
    seed-packs.tar.zst.sig
```

GitHub's "latest release" alias resolves to whichever release has
the `latest` flag set, so `appimage-control --update-seed-packs`
always fetches the newest tagged release without needing to know the
version string. Private repo releases require an authenticated
fetch — your authorised users provide their own GitHub credentials
to reqwest (via standard environment / netrc), or you host the
artifacts on a static webserver of your own with whatever access
control fits.

## What an authorised user sees

```sh
$ appimage-control --update-seed-packs
downloading seed-packs.tar.zst
downloading signature
verifying signature, unpacking and validating
Installed 3 seed packs to /home/alice/.local/share/appimage-control/seeds
  from https://github.com/your-org/your-private-pack-repo/releases/latest/download
```

After the install, the per-user dir wins over the embedded slice
for every subsequent seed call (per the discovery chain in
`crates/core/src/seed_pack.rs`).

## What an unauthorised user sees

On a public build with placeholders intact:

```sh
$ appimage-control --update-seed-packs
seed-pack signature verification failed: this build has no seed-pack
signing key configured (placeholder still in source). See
docs/seed-pack-companion-repo.md for the keygen workflow.
```

No network requests are made. Distribution-safe.

## Threat model

| Attacker capability | Defended? | How |
|---------------------|-----------|-----|
| MITM the release download | ✓ | TLS via reqwest+rustls; the signature catches body tampering anyway |
| Compromise the companion-repo GitHub account (push a release) | ✓ | Tarball must verify against the embedded ed25519 key; attacker has no private key |
| Compromise the binary-distribution channel | ✓ | The signing pub-key is part of the binary; substituting the binary substitutes the trust root, but that's equivalent to source-code compromise |
| Compromise the maintainer's online machine | ✓ | Private key stays on cold storage; CI does not have it |
| Compromise the maintainer's cold-storage machine | ✗ | Out of scope — equivalent to source-code compromise |
| Replay an old signed release | ✓ | The v2 signature binds a release sequence; an install records the last accepted one (`~/.local/share/appimage-control/seed-pack-release-seq`, outside what backup archives and restore clears) and refuses an older release ("seed-pack release N is older than installed release M") |
| Oversized or overlong tarball | ✓ | The tarball download is capped at 32 MiB and the signature at 77 bytes; unpack bounds the decompressed stream, accepts regular files only, and refuses an entry over 4 MiB, a total over 32 MiB, or more than 512 entries; nothing is installed |
| Replay after a fresh install | partially | A fresh install (or a deleted record) has no floor and accepts any validly signed release once |
| Smuggle malicious JSON into a pack file | ✓ | Each pack runs through `SeedPackLibrary::validate` after unpack; bad shapes fail before install |
| Path traversal in the tarball | ✓ | `unpack_tar_zst_bytes` rejects any entry whose path has more than one component or doesn't end in `.json` |
| Distribute the binary to an unauthorised user | ✓ | Without a real `DEFAULT_RELEASE_URL` and `SEED_PACKS_PUB_KEY`, `--update-seed-packs` refuses to fetch anything |

## File-format spec (signature v2)

Reference: `crates/core/src/seed_pack_update.rs` (`SIG_FILE_LEN`,
`signed_message_v2`).

```
offset  bytes  meaning
0       4      magic = b"AICS"
4       1      version = 0x02
5       8      release sequence, u64 big-endian
13      64     ed25519 signature over
               b"AICS-v2\0" || sequence (8 bytes BE) || SHA-512(seed-packs.tar.zst)
```

Total 77 bytes. The signature is verified with the embedded base64
ed25519 public key. A v1 file (69 bytes, no sequence) is refused with a
message naming v2 and `aic-sign-pack sign --sequence`. The tarball is read
once, and the same bytes are verified and unpacked.

## Rotating the signing key

Lost key, suspected compromise, or scheduled rotation:

1. On the cold-storage machine, generate a new keypair with
   `aic-sign-pack keygen`.
2. Update `SEED_PACKS_PUB_KEY` in your private fork, build a new
   binary, distribute to authorised users.
3. Resign the latest seed-pack tarball with the new key, push as a
   new companion-repo release.
4. Optionally: revoke the old key by burning the file. Old releases
   stay verifiable against the old key but the new binary only
   honors the new one.

There is no in-band "key rotation" message — users on the old
binary keep using the old key until they take the new binary.
Acceptable because the trust root (the embedded const) is the
binary itself.
