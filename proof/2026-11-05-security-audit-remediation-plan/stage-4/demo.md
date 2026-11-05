# Stage 4 demo — 2026-11-05-security-audit-remediation-plan

Environment: host at HEAD 43c50efb (`security-audit-remediation`). `aic-sign-pack` built with `cargo build -p appimage_control_core --bin aic-sign-pack`. The Ubuntu VM `user@192.168.122.204` has its tree at `~/aic`. Its `crates/**/*.rs`, `Cargo.toml` and `Cargo.lock` checksums matched the host (one combined sha256, 2b4ea060…). Its binary was older than the last commit, so I rebuilt it: `cargo build -p appimage-control`, finished in 12.94s, exit 0. Every run used a scratch HOME and XDG_DATA_HOME. The VM's umask was 0002. Scratch files were removed afterwards on both machines.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: a backup is written with mode 0600; a restore of an intact archive behaves as before.
Verdict: holds
Evidence:
- On the VM, with HOME=$T/home: `appimage-control +keepassxc:1` downloaded KeePassXC 2.7.12, printed `Installed keepassxc instance 1 …` and exited 0. `--list` showed `keepassxc  KeePassXC  1  installed 2.7.12`.
- `appimage-control --backup $T/bk.tar.zst` printed `Backup created: /tmp/aic-demo4.R4M4FB/bk.tar.zst (9 entries, 98609051 bytes)` and exited 0. `stat` gave `600 98040655 bk.tar.zst`, under umask 0002.
- I ran `--backup` onto a file that already existed with mode 0664. It exited 0, and `stat` gave `600` afterwards. The existing file's mode was tightened.
- I restored into a fresh HOME with `HOME=$T/home2 … --restore-backup $T/bk.tar.zst`. It exited 0 and printed nothing. All 9 managed files have the same sha256 as the originals. `--list` shows `keepassxc … 1 installed 2.7.12`. The restored `KeePassXC.AppImage --appimage-version` ran and printed `Version: 5735cc5`, exit 0.
- Modes after the restore: the AppImages and the launch script are `700`, so the owner can run them and others cannot read them. The .desktop file, icons and version/sha files are `600`. In the source install they were `755` and `664`.
- A second restore without `--force` printed `restore preflight refused: managed state already exists; re-run restore with --force to overwrite` and exited 1. With `--force` it exited 0, and the contents were identical again.

## Outcome 2
> A restored archive has an entry whose bytes do not match its manifest sha256 → restore fails in phase 1 naming the entry; managed state is byte-identical to before.
Verdict: holds
Evidence:
- I rebuilt bk.tar.zst with one byte flipped in `manager-data/cache/keepassxc/KeePassXC.AppImage`. A Python tarfile pass kept the same `manifest.json`.
- Restore over existing state, `--restore-backup bad-appimage.tar.zst --force`: printed `backup error: archive entry manager-data/cache/keepassxc/KeePassXC.AppImage does not match its recorded sha256 (expected 564fe8b7…ae49, got 64c35837…7607); nothing was restored`, exit 1.
- I recorded path, type, mode, size, mtime and sha256 for every entry under HOME2, before and after. Every file and subdirectory was identical. The only difference was the mtime of the `~/.local/share` directory itself (see Minor 1).
- The same archive restored into a fresh HOME3 gave the same message and exit 1. HOME3 contained only the empty `.local/share` I had created.
- A one-byte flip in `applications/appimage-control-keepassxc-1.desktop`, restored with `--force`: `backup error: archive entry applications/appimage-control-keepassxc-1.desktop does not match its recorded sha256 (…); nothing was restored`, exit 1, state identical.
- An extra entry `applications/evil.desktop` that the manifest does not list: `backup error: archive entry applications/evil.desktop is not listed in the manifest; nothing was restored`, exit 1. No evil.desktop was written.

## Outcome 3
> Works: `aic-sign-pack sign --sequence N <key> <tarball>` writes a v2 signature; the updater's install path (`install_verified_release`, shown by its tests while the updater ships dormant) accepts it and records N.
Verdict: holds
Evidence:
- Signing tool, on the host: `aic-sign-pack keygen key.bin` printed the public key and `Private key written to key.bin (mode 0600).` I made a tarball of 2 real seed packs (electrum-btc.json, feather.json). `aic-sign-pack sign --sequence 7 key.bin release.tar.gz` printed `Signature (v2, release 7) written to release.tar.gz.sig.`, exit 0.
- The .sig file is 77 bytes. In xxd it reads `AICS`, then version byte `02`, then the big-endian u64 `00…07`, then the 64-byte signature.
- `aic-sign-pack verify <pub> release.tar.gz` printed `7` and `OK: release.tar.gz verifies against the supplied public key (release 7).`, exit 0.
- With one byte appended to the tarball, verify printed `ed25519 signature does not verify`, exit 1. With the sequence byte in the .sig changed from 07 to 08, verify gave the same error, exit 1.
- `sign` without `--sequence` was refused with `sign --sequence N <KEYFILE> <TARBALL>: N is this release's sequence number, higher than every earlier release`. `abc` and `-1` were refused with `--sequence: not a number: …`.
- Updater half, by its tests: `cargo test -p appimage_control_core seed_pack_update::` gave `23 passed; 0 failed`. The tests that cover this clause are `v2_signature_verifies_and_returns_its_sequence`, `equal_release_reinstalls_and_newer_records_its_sequence`, `release_sequence_record_lives_outside_the_backed_up_dir` and `install_atomically_swaps_and_keeps_no_backup_on_success`. All are ok.
- `appimage-control --update-seed-packs` on the VM printed `this build has no seed-pack signing key configured (placeholder still in source)`, exit 1. This confirms the updater ships dormant.

## Outcome 4
> Works: the updater's install path (`install_verified_release`, shown by its tests while the updater ships dormant) refuses a signed release with a sequence lower than the last accepted one with `seed-pack release N is older than installed release M`, leaving installed packs untouched; an equal N re-installs.
Verdict: holds
Evidence:
- `cargo test -p appimage_control_core seed_pack_update::` gave `23 passed; 0 failed`. The tests that cover this clause are `older_release_is_refused_and_installed_packs_untouched` and `equal_release_reinstalls_and_newer_records_its_sequence`, both ok. The related tests `corrupt_sequence_record_says_how_to_recover` and `unwritable_sequence_record_refuses_with_nothing_changed` are also ok.
- I ran `cargo test … older_release_is_refused -- --nocapture`. The test prints nothing, so the exact text `seed-pack release N is older than installed release M` could not be seen this way. It rests on the test name and the test passing.

## Outcome 5
> A v1 signature file → refused, naming the v2 format and `aic-sign-pack sign --sequence`.
Verdict: holds
Evidence:
- I made a real v1 signature with the pre-v2 tool. It was built from `git archive caaa7952` into a scratch directory with its own target dir. `old aic-sign-pack sign key.bin real-v1.tar.gz` wrote a 69-byte .sig (`AICS 01` plus the signature). The old tool verified it: `OK: real-v1.tar.gz verifies…`.
- The new tool, `aic-sign-pack verify <pub> real-v1.tar.gz`, printed `seed-pack signature verification failed: signature format v1 is no longer accepted: it binds no release sequence, so an older release could be replayed. Re-sign with `aic-sign-pack sign --sequence N` (format v2)`, exit 1.
- A bare 64-byte signature and a base64 signature were both refused with `signature file is missing AICS magic bytes`, exit 1.
- The updater test `v1_signature_is_refused_naming_v2` is ok.

## Outcome 6
> Works: the updater's install path (`install_verified_release`, shown by its tests while the updater ships dormant) refuses a tarball entry over 4 MiB, a total over 32 MiB, or more than 512 entries, and installs nothing.
Verdict: holds
Evidence:
- `cargo test -p appimage_control_core seed_pack_update::` gave `23 passed; 0 failed`. The tests that cover this clause are `unpack_caps_refuse_large_entry_total_and_count` and `capped_release_installs_nothing`, both ok. The neighbouring tests `unpack_bounds_decompressed_bytes_and_refuses_non_regular_entries`, `unpack_rejects_path_traversal`, `unpack_rejects_subdirectory_entry` and `unpack_rejects_non_json_entries` are also ok.

## Outcome 7
> `aic-sign-pack keygen` onto an existing file → refuses and leaves it; a new key file is never readable by others at any moment.
Verdict: holds
Evidence:
- Run under `umask 022` on the host.
- `aic-sign-pack keygen key.bin` onto an existing key printed `key.bin already exists; refusing to overwrite`, exit 1. The sha256 (df9779e6…), mode 600, size 32 and mtime were all unchanged.
- `keygen notes.txt` onto an existing 0644 text file was refused the same way, exit 1. Its content (`precious`) and mode 644 were unchanged.
- `keygen link.bin` onto a dangling symlink printed `link.bin: File exists (os error 17); refusing to overwrite`, exit 1. The symlink target was not created.
- I traced a fresh key with `strace -e trace=openat,…,chmod,fchmod,rename… aic-sign-pack keygen new.bin`. The trace shows `openat(AT_FDCWD, "new.bin", O_WRONLY|O_CREAT|O_EXCL|O_CLOEXEC, 0600) = 3`, with no chmod or rename after it. The file is created 0600 in one step, and `stat` gave `600 32`.

## Plan goal
- Stage 4 is the plan's last stage. On the host at 43c50efb, the `cargo audit --json | python3 … count` check exits 0, `zizmor .github/workflows/ci.yml .github/workflows/flathub.yml` exits 0, and `shellcheck -S style scripts/*.sh` exits 0.

## Findings
Blocking: none.

Material: none.

Minor:
1. A refused restore still changes the mtime of `~/.local/share`. Every managed file and subdirectory stays identical, but the directory's own mtime changes on each refused restore (1793911736 → 1793911760). This suggests a temporary entry is created there and removed. "Byte-identical" holds for the managed state but not for that directory's metadata.
2. Restore messages read oddly. A restore refusal starts with `backup error:`. A successful `--restore-backup` prints nothing, while `--backup` prints `Backup created: …`.
3. Restore changes file modes from the original install. Installed files were 755/664 and restored files are 700/600. This is probably intended, since the brief asks for not-readable-by-others, but it is a visible difference from the original install.
4. `aic-sign-pack --help` is rejected as `unknown subcommand`. The bare usage line does not list each subcommand's arguments.
5. `aic-sign-pack sign` accepts sequence `0` and `18446744073709551615` with no warning. A typo that produces a huge N would leave only equal-N re-installs possible from then on. It also overwrites an existing `.sig` silently, and it signs with a 0644 key file without a warning.
6. For Outcome 4, the exact refusal wording could not be seen by the prescribed route. The test prints nothing, so it rests on the test name and the test passing.
