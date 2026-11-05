# Stage 1 demo — 2026-11-05-security-audit-remediation-plan

Environment: host Linux 7.0.0-42 (Ubuntu), branch `security-audit-remediation` at c33ad416; cargo-audit 0.22.1 (advisory-db 26fd2d43, 1409 advisories, fetched 2026-11-05), zizmor 1.24.1, actionlint 1.7.12, shellcheck 0.11.0. Host gtk4 is 4.14.5 (GUI crate needs >= 4.21); podman is unusable in this session ("cannot re-exec process to join the existing user namespace"), so the CI archlinux container and the GUI crate's compile could not be run. All destructive steps ran in a throwaway clone under the session scratchpad, since deleted.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: `cargo audit` reports none of the 13 crates above; zizmor and actionlint report nothing on `.github/workflows/`; shellcheck reports nothing on `scripts/`; CI builds offline from `vendor/` as before.
Verdict: holds
Evidence:
- `cargo audit --json | python3 -c "...gate one-liner..."` → `0 []`, exit 0. Full JSON: `vulnerabilities.count` 0, `warnings` `{}`, 270 crate dependencies scanned.
- Lockfile versions of the audited crates: rustls 0.23.46, rustls-webpki 0.103.16, webpki-roots 1.0.11, tar 0.4.46, zstd 0.13.4, zstd-safe 7.4.0, zstd-sys 2.1.2+zstd.1.5.7, tokio 1.54.1, anyhow 1.0.104, rand 0.10.3, crossbeam-epoch 0.9.21, quinn-proto 0.11.19, fastrand 2.5.0 (block-buffer 0.12.1).
- `zizmor .github/workflows/` → "No findings to report. Good job! (4 suppressed)", exit 0. Same result with `--gh-token "$(gh auth token)"`, which turns on the online audits (impostor-commit, ref-version-mismatch, known-vulnerable-actions).
- `actionlint` → no output, exit 0.
- `shellcheck scripts/*.sh` (all 5 files; `scripts/` has no other files) → no output, exit 0.
- Pins checked with `gh api repos/<r>/commits/<tag> --jq .sha`: checkout v4.4.0 → 11d5960a…, cache v4.3.0 → 0057852b…, upload-artifact v4.6.2 → ea165f8d…. Each matches the SHA and tag comment in the workflows. The container is pinned `archlinux@sha256:ece6c971…`. `persist-credentials: false` is set on both checkouts. `permissions: contents: read` is set in ci.yml.
- Offline build from a fresh clone (the gate command, clone placed in the scratchpad): `git clone --branch security-audit-remediation . $D/aic && cargo build --offline -p appimage_control_core` → "Finished `dev` profile", exit 0.
- Whole workspace in the same fresh clone: `cargo check --offline --workspace --all-targets` loaded every vendored package with no checksum or missing-crate error. It stopped only at `gdk4-sys`'s pkg-config probe (`gtk4 >= 4.21`; the host has 4.14.5). This is a host limit, not a vendor gap.
- Host-runnable CI steps: `cargo clippy --offline -p appimage_control_core --all-targets -- -D warnings` → exit 0. `sh scripts/verify-cli-doc-sync.sh` → "Flags verified: 29", exit 0. `cargo test -p appimage_control_core` → 231 lib tests plus every integration-test binary ok, 0 failed. **`cargo fmt --all -- --check` → exit 1.** See Material finding M1: CI's "Format check" step will fail.
- Not reached: CI's own archlinux container run and the GUI crate compile (podman broken here; host gtk4 too old).

## Outcome 2
> Works: `Cargo.lock` and `vendor/` agree package for package, and no crate the build uses changed its version line.
Verdict: holds
Evidence:
- A Python `tomllib` script read the fresh clone's `Cargo.lock` and every `vendor/*/Cargo.toml` plus `.cargo-checksum.json`. Result: "lock registry pkgs: 268 workspace pkgs: 2 vendor dirs: 268". Missing from vendor: `[]`. In vendor but not in the lock: `[]`. Lock checksum vs vendor `package` checksum mismatches: `[]`. CLAUDE.md/AGENTS.md checksum entries: `[]`.
- Version lines, `Cargo.lock` at preflight 22ae1db7 vs HEAD (package name and version only). Same line: anyhow, block-buffer, crossbeam-epoch, fastrand, quinn-proto, rustls, rustls-webpki, tar, tokio, webpki-roots, zstd, zstd-safe, zstd-sys. Line changed: rand 0.9.2 → 0.10.3 and rand_core 0.9.5 → 0.10.1. Added: chacha20 0.10.2 and rand_pcg 0.10.2. Removed: rand_chacha, ppv-lite86, zerocopy, zerocopy-derive.
- None of the changed-line crates is used by the build. `cargo tree --offline --workspace -i rand` returns "nothing to print". The same holds for rand_core, chacha20 and rand_pcg, with `-e normal,build`, with `-e dev`, and with `--target all -e all`. `cargo tree -i quinn-proto --target all -e all` also prints nothing. The rand family lives in the lockfile only, behind quinn-proto.

## Outcome 3
> The re-vendored tree carries the `CLAUDE.md`/`AGENTS.md` checksum entries again → `cargo build --offline` fails with "failed to calculate checksum"; Task 1.1's test catches it before commit.
Verdict: holds
Evidence:
- Baseline in the fresh clone: Task 1.1's Test chain → exit 0. It ran the build excluding the GUI crate, core tests, the audit filter `[]`, the `! grep` guard and `live_github_download ... ok`.
- Failure caused the way a maintainer would: `cargo vendor vendor` in the clone → exit 0. `git diff --stat` shows `vendor/system-deps/.cargo-checksum.json | 2 +-`. `grep -lE '"(CLAUDE|AGENTS)\.md"' vendor/*/.cargo-checksum.json` → `vendor/system-deps/.cargo-checksum.json`. The `vendor/system-deps/CLAUDE.md` file itself is ignored (`git status --ignored` shows `!! vendor/system-deps/CLAUDE.md`, from `.gitignore:14:CLAUDE.md`).
- Task 1.1's Test on that tree: the build passes locally, the tests pass and the audit prints `[]`. Then the guard prints `vendor/system-deps/.cargo-checksum.json`, and the chain exits **1** before the live test. The bad tree is caught before commit.
- Then the bad tree was committed anyway, and a fresh clone of it was built. `cargo check --offline --workspace` → `error: failed to calculate checksum of: …/aic2/vendor/system-deps/CLAUDE.md … No such file or directory (os error 2)`, exit 101.
- In the same broken clone, `cargo build --offline -p appimage_control_core` → "Finished", exit 0. See M2.

## Findings

Blocking: none.

- **M1 (Material): `cargo fmt --all -- --check` fails. CI's "Format check" step will go red on the next push.** rustfmt wants `crates/core/tests/http_local_fixture.rs:104` split across lines: `assert!(head.len() > 1_000_000, "suspiciously small: {} bytes", head.len());`. A detached worktree of preflight 22ae1db7 passes the same check (exit 0), so Stage 1 introduced this. The fix is `cargo fmt` on that file.
- **M2 (Material): the stage gate's Integration check cannot see the Outcome 3 failure.** It runs `cargo build --offline -p appimage_control_core` on a fresh clone. system-deps is only in the GUI crate's graph, so a clone with the dangling `system-deps` CLAUDE.md checksum entry still passes that check (exit 0). It fails only with `--workspace`. Task 1.1's build also uses `--exclude appimage-control`, so its `grep` guard is the only thing that catches this.
- m1 (Minor): Premise 2 in `premise.md` records actions/checkout v7.0.1, cache v6.1.0 and upload-artifact v7.0.2 with their SHAs. The workflows pin v4.4.0, v4.3.0 and v4.6.2 instead. Those pins are valid and match their comments, but the premise record does not describe what shipped.
- m2 (Minor): zizmor's "(4 suppressed)" are persona-filtered, not inline ignores. `--persona=auditor` shows 2 low and 2 informational findings: `concurrency-limits` on ci.yml and flathub.yml, and `anonymous-definition` on flathub.yml job `flatpak`. The default persona reports nothing.
- m3 (Minor): `premise.md` still carries the old Outcome 2 wording. The plan's 2026-11-05 amendment replaced it, and this demo uses the amended text.

Background work: none started. All commands ran in the foreground. The scratch clones and the temporary worktree were removed.
