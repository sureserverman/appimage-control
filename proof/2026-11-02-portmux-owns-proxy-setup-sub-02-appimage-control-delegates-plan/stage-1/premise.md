# Stage 1 premises — 2026-11-02-portmux-owns-proxy-setup-sub-02-appimage-control-delegates-plan

Goal: Make the seeding core hand all proxy work to portmux, for the person seeding a wallet.
Risk: HIGH — seeding wallets is this app's sensitive path, and this removes a pack verb, so older packs stop loading.

Outcomes:
- Works: the seeded wallet is proxied through its own port, and every non-proxy setting from the pack still lands.
- portmux is missing or older than `configure-app` → the install hint appears once, seeding is skipped, and installed instances stay installed.
- Group membership is not active yet → the instance is reported failed with portmux's own message about logging out and in, and its first-run config is unchanged.
- Pool exhausted → the instance is reported failed with portmux's message, and its first-run config is unchanged.
- The pack still uses `port` or `preset` → the pack is refused by name, with a message saying how to migrate it.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: The real `portmux-port configure-app` writes a wallet's proxy settings into an existing JSON config and keeps its other keys.
Checked: On the Ubuntu VM, the Sub-plan 1 binary with `TORRC` at a temp file: `configure-app --app feather --instance 4 --config settings.json --desktop-app-id appimage-control-feather-4 --json-v2`, config `{"keep":"me","proxy":0}`.
Result: Exit 0. Config became `{"keep":"me","proxy":1, "socks5Port": "9084", "useLocalTor": true}`. JSON output carries `manual_action`.
Verdict: holds

## Premise 2
Assumption: A portmux older than `configure-app` fails the probe `configure-app --help`.
Checked: On the VM, the installed older `/usr/bin/portmux-port configure-app --help`.
Result: Exit 1.
Verdict: holds

## Premise 3
Assumption: When allocation is denied, portmux's own message tells the user to log out and in, on stderr, and the config is unchanged.
Checked: On the host, the Sub-plan 1 binary, where this session cannot read the temp torrc, so allocation is refused.
Result: Exit 1. stderr: "If you were just added to the portmux group, log out and in." Config still `{"keep":"me","proxy":0}`. stdout also carries the JSON error object, so the runner reads stderr for the message.
Verdict: holds

## Premise 4
Assumption: Pool exhaustion exits 2 with portmux's message.
Checked: `configure-app --help` on the Sub-plan 1 binary documents exit 2, and Sub-plan 1's `configure_app_cli` suite covers it. Not exhausted live, because that would need filling the VM's real pool.
Result: Documented as "2 pool exhausted". The runner's mapping is proven against a stub.
Verdict: holds

## Premise 5
Assumption: `podman` and the `aic-gtk-build` container are available for the GUI checks.
Checked: `podman images` in this session.
Result: False in a remote-agents session (`NoNewPrivs=1`, single-uid namespace). Owner chose the Ubuntu VM (OD-4). This changes where GUI checks run, not the Goal.
Verdict: holds, as amended by OD-4
