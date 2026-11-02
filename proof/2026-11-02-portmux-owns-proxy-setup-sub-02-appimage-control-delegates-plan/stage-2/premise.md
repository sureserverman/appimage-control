# Stage 2 premises — 2026-11-02-portmux-owns-proxy-setup-sub-02-appimage-control-delegates-plan

Goal: Make the GUI, CLI, CI and docs match what seeding now does, for the people reading and running them.
Risk: MEDIUM — the GUI build needs the container, and the doc-sync gate is strict (print_help escaping, every `--token` in README).

Outcomes:
- Works: Preferences shows whether portmux is available for proxy setup, and the docs describe `proxy_setup`.
- portmux is missing or old → the Preferences row reads "not detected" with the "missing" pill, as it does today.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: With no container in this session, the GUI crate builds and its tests run on the Ubuntu VM, as OD-4 says.
Checked: `ssh user@192.168.122.204 'cd ~/aic && cargo test -p appimage-control --test cli_subprocess'`
Result: Built and ran, 11 passed.
Verdict: holds

## Premise 2
Assumption: A real portmux older than `configure-app` fails the probe, so Preferences reads "not detected" for it, and a current one passes.
Checked: On the VM, `/usr/bin/portmux-port configure-app --help` and `/tmp/mp-sub1 configure-app --help`.
Result: Old exits 1, Sub-plan 1 build exits 0.
Verdict: holds

## Premise 3
Assumption: The GitHub runner's GTK and CI environment runs the wiring script the same way the VM does.
Checked: Not run: the runner is not reachable from here. Task 2.2 runs the script on the VM.
Result: None.
Verdict: not checkable here — the CI runner only exists on GitHub; it runs on the next push
