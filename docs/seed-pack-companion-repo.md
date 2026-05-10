# Companion repo: `appimage-control-seed-packs`

This doc describes how to set up the companion GitHub repository
that distributes signed seed-pack updates separately from the
`appimage-control` binary. It is an authoring/operator guide, not a
specification of the binary's behavior — for that, see
`crates/core/src/seed_pack_update.rs`.

## Why a separate repo

Seed packs (especially the BTC `.onion` server list) need updates
faster than the binary. Bundling them into the binary couples
release cadence to onion-server churn. Splitting lets you publish a
new pack release the same hour an onion goes dark, without users
needing to upgrade the manager.

The trust property is preserved by ed25519 signing — the matching
public key is committed in `crates/core/src/seed_pack_update.rs` as
`SEED_PACKS_PUB_KEY`, so an attacker who compromises GitHub cannot
ship malicious packs without also stealing the offline private key.

## One-time setup (on a cold-storage machine)

Generate the signing keypair. The private half **must never enter
GitHub, CI, or any network-connected machine**.

```sh
# Build the helper binary on the cold-storage machine.
cargo build --release --bin aic-sign-pack -p appimage_control_core

# Generate the keypair. Stores the 32-byte raw seed at KEYFILE
# (mode 0600); prints the matching base64 public key on stdout.
target/release/aic-sign-pack keygen ~/.aic-keys/seed-packs.key
# Example output: HaCk2Kt9IBFmrMS3kF9uHZTO3uxUYShJy51Vg6uCrpU=

# Back up the keyfile to a separate offline medium. Anyone with this
# 32-byte file can sign a release tarball that every appimage-control
# user will install.
```

Open `crates/core/src/seed_pack_update.rs` and replace the
placeholder constant:

```rust
pub const SEED_PACKS_PUB_KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";
```

with the base64 string `keygen` printed. Commit and push. Until you
do this, `--update-seed-packs` refuses every invocation with a clear
error pointing to this doc, so an unkeyed build is still safe to ship.

## Companion-repo layout

Create a new GitHub repo named `appimage-control-seed-packs` under the
same org/user as `appimage-control` (the embedded URL hardcodes this
path):

```
DEFAULT_RELEASE_URL =
  https://github.com/sureserverman/appimage-control-seed-packs/releases/latest/download
```

Repo contents:

```
appimage-control-seed-packs/
├── README.md
├── LICENSE
├── packs/
│   ├── feather.json
│   ├── electrum-btc.json
│   └── electrum-ltc.json
└── .github/
    └── workflows/
        └── release.yml
```

Each pack file follows the schema in `docs/seed-packs.md` of this repo.
The companion repo is a *pure-data* repo — no Rust code, no build
scripts, no CI that touches the signing key.

## Cutting a release

The release flow has two stages: build the tarball anywhere, sign it
on the cold-storage machine, then upload.

```sh
# On any machine: build the tarball.
cd appimage-control-seed-packs
tar --create --zstd --file seed-packs.tar.zst -C packs/ \
    feather.json electrum-btc.json electrum-ltc.json

# Move the tarball to the cold-storage machine (USB / SD card).

# On the cold-storage machine: sign it.
aic-sign-pack sign ~/.aic-keys/seed-packs.key seed-packs.tar.zst
# Produces seed-packs.tar.zst.sig (69 bytes).

# Sanity-check the signature against the embedded pubkey before upload.
aic-sign-pack verify "<EMBEDDED-PUBKEY-FROM-MAIN-REPO>" seed-packs.tar.zst

# Move both files back to the network-connected machine, attach to a
# GitHub release.
gh release create v2026.05.10 \
    --repo sureserverman/appimage-control-seed-packs \
    --title "Seed packs 2026-05-10" \
    --notes "Refreshed BTC server list." \
    seed-packs.tar.zst \
    seed-packs.tar.zst.sig
```

GitHub's "latest release" alias resolves to whichever release has the
`latest` flag set, so `appimage-control --update-seed-packs` always
fetches the newest tagged release without needing to know the version
string.

## What the user sees

```sh
$ appimage-control --update-seed-packs
downloading seed-packs.tar.zst
downloading signature
verifying signature
unpacking
validating packs
installing
Installed 3 seed packs to /home/alice/.local/share/appimage-control/seeds
  from https://github.com/sureserverman/appimage-control-seed-packs/releases/latest/download
```

After the install, the per-user dir wins over the embedded slice for
every subsequent seed call (per the discovery chain in
`crates/core/src/seed_pack.rs`).

## Refusing on placeholder key

Until `SEED_PACKS_PUB_KEY` is replaced with a real key:

```sh
$ appimage-control --update-seed-packs
seed-pack signature verification failed: this build has no seed-pack
signing key configured (placeholder still in source). See
docs/seed-pack-companion-repo.md for the keygen workflow.
```

No network requests are made. Safe to ship.

## Threat model

| Attacker capability | Defended? | How |
|---------------------|-----------|-----|
| MITM the GitHub-releases download | ✓ | TLS via reqwest+rustls; the signature catches body tampering anyway |
| Compromise GitHub releases (push a new tagged release) | ✓ | Tarball must verify against the embedded ed25519 key; attacker has no private key |
| Compromise the maintainer's online machine | ✓ | Private key stays on cold storage; CI does not have it |
| Compromise the maintainer's cold-storage machine | ✗ | Out of scope — equivalent to source-code compromise |
| Replay an old signed release | partially | GitHub `latest` only points forward; attackers who can rewrite GitHub state are covered by row 2 |
| Smuggle malicious JSON into a pack file | ✓ | Each pack runs through `SeedPackLibrary::validate` after unpack; bad shapes fail before install |
| Path traversal in the tarball | ✓ | `unpack_tar_zst` rejects any entry whose path has more than one component or doesn't end in `.json` |

## File-format spec ("AICSIG-1")

Reference: `crates/core/src/seed_pack_update.rs` const `SIG_FILE_LEN`.

```
offset  bytes  meaning
0       4      magic = b"AICS"
4       1      version = 0x01
5       64     ed25519 signature over SHA-512(seed-packs.tar.zst)
```

Total 69 bytes. The signature is verified with the embedded base64
ed25519 public key. Future versions can bump the version byte.

## Rotating the signing key

Lost key, suspected compromise, or scheduled rotation:

1. On the cold-storage machine, generate a new keypair with
   `aic-sign-pack keygen`.
2. Update `SEED_PACKS_PUB_KEY` in this repo, ship a new
   `appimage-control` release.
3. Resign the latest seed-pack tarball with the new key, push as a
   new companion-repo release.
4. Optionally: revoke the old key by burning the file. Old releases
   stay verifiable against the old key but the manager only honors
   the new one once users upgrade.

There is no in-band "key rotation" message — users on the old binary
keep using the old key until they upgrade. Acceptable because the
trust root (the embedded const) is the manager binary itself.
