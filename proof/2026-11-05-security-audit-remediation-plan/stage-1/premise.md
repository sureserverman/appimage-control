# Stage 1 premises — 2026-11-05-security-audit-remediation-plan

Goal: Clear every advisory-bearing crate and every GitHub Actions and shell finding, so the maintainer's next audit starts from those lanes clean.
Risk: MEDIUM — re-vendoring rewrites thousands of files under `vendor/`, and a TLS-stack bump touches every download

Outcomes:
- Works: `cargo audit` reports none of the 13 crates above; zizmor and actionlint report nothing on `.github/workflows/`; shellcheck reports nothing on `scripts/`; CI builds offline from `vendor/` as before.
- A bumped crate has no fixed version inside its current major → the task stops and names the crate and advisory; nothing is half-vendored.
- The re-vendored tree carries the `CLAUDE.md`/`AGENTS.md` checksum entries again → `cargo build --offline` fails with "failed to calculate checksum"; Task 1.1's test catches it before commit.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: every advisory-bearing crate has a fixed release inside its current major on crates.io.
Checked: `cargo info <crate> --registry crates-io` for all 13 crates (2026-11-05, host).
Result: latest rustls 0.24.0-dev.2 (0.23.x line exists; plan targets 0.23.46), rustls-webpki 0.104 alpha (0.103.16 target), webpki-roots 1.0.11, tar 0.4.46, zstd 0.14.3 (0.13.4 target), zstd-safe 8.2.0 (7.4.0 target), zstd-sys 2.1.2, tokio 1.54.1, anyhow 1.0.104, rand 0.10.3 (0.9.3 target), crossbeam-epoch 0.9.21, quinn-proto 0.11.19, fastrand 2.5.0.
Verdict: holds — confirmed per crate by `cargo update -p` in Task 1.1, which resolves within the semver range.

## Premise 2
Assumption: GitHub resolves each action tag to a commit SHA.
Checked: `gh api repos/<r>/git/ref/tags/<tag>`
Result: actions/checkout v7.0.1 -> 3d3c42e5aac5ba805825da76410c181273ba90b1 (commit); actions/cache v6.1.0 -> 55cc8345863c7cc4c66a329aec7e433d2d1c52a9; actions/upload-artifact v7.0.2 -> cf430e030ddbb5b0abf93d22962f4752f3646cd9.
Verdict: holds.

## Premise 3
Assumption: the archlinux container image has a resolvable digest to pin.
Checked: Docker Hub registry `HEAD /v2/library/archlinux/manifests/latest` (podman cannot run in this session).
Result: docker-content-digest sha256:ece6c971fec3c532fd508bce299a446deaf469feecbfe7c353e995bc1a5e7bba
Verdict: holds.

## Premise 4
Assumption: the bumped TLS stack still reaches GitHub release assets.
Checked: Task 1.1's ignored test `live_github_download` from the host.
Result: see Task 1.1 commit.
Verdict: checked in Task 1.1.

Note (Stage 1 gate, 2026-11-05): Premise 2 checked that the newest tags resolve. Task 1.2 pinned the newest tag of the major the workflows already used (checkout v4.4.0, cache v4.3.0, upload-artifact v4.6.2); moving to the current majors is a separate change. Outcome 2 above shows its Step 3.0 wording; the plan amended it at the gate with the owner's words.
