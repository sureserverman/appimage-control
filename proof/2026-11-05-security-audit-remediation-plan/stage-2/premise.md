# Stage 2 premises — 2026-11-05-security-audit-remediation-plan

Goal: Stop a planted or malformed catalog, seed pack or launcher from steering writes, deletes or launches outside the app's own directories, for the user running the GUI or CLI.
Risk: MEDIUM — removing the current-directory catalog walk changes which catalog a dev run from the repo root sees; the executable walk still finds it from `target/`

Outcomes:
- Works: the shipped catalog and every shipped and companion seed pack load unchanged; installs, resets, uninstalls and updates behave as before.
- A catalog file has an `id` or `appimage_filename` that is not one plain name (contains `/`, `..`, is empty, `.`, or has characters outside `A-Za-z0-9._-`) → that catalog file is refused with an error naming the app and field, and loading falls through to the next source (ending at the embedded catalog).
- A catalog `reset_*_dirs` entry is empty, `.`, absolute or contains `..` → the catalog file is refused the same way; reset never deletes a base directory.
- A `data/catalog.json` or `data/seeds/` in the current directory or its parents → ignored; the GUI launched from `$HOME` uses the installed or embedded catalog.
- A seed pack's `config_path` resolves to an absolute path or contains `..` → the pack is refused at load with an error naming the pack and field; other packs still load.
- A launcher's `Exec=` points through `..` out of the managed files dir → uninstall and update leave that target alone and report it.
- An AppImage path containing a space, `%`, `$`, a quote or a backtick → the written `Exec=` is quoted and escaped per the Desktop Entry spec, and launches that exact file.

These Outcomes are what `Stage N green` is checked against. Changing one after
this point needs the owner's words in its amendment note: owner, <date>: "<words>".

One entry per thing the Goal, Outcomes or Risk assume about what the code does
not control (the platform, another app, a permission, the engine, a service).
Check each on the real environment before building on it. Verdict: holds, false,
or not checkable here — <why>. A false premise is a Stop condition.

## Premise 1
Assumption: the shipped catalog already satisfies the new field rules (plain `id`/`appimage_filename`; reset entries non-empty, relative, no `..`).
Checked: python3 over data/catalog.json — `re.fullmatch(r'[A-Za-z0-9._-]+')` on id and appimage_filename, list every reset_* entry.
Result: all 14 apps pass; reset entries are single names (e.g. `feather`, `Standard Notes`, `com.suicore.qnote`).
Verdict: holds.

## Premise 2
Assumption: every companion seed pack uses a relative `{appimage}…` config_path with only normal components.
Checked: python3 over ~/dev/infra/installers/linux/appimage-control-seed-packs/*.json.
Result: electrum-btc `{appimage}.home/.electrum/config`, electrum-ltc `{appimage}.home/.electrum-ltc/config`, feather `{appimage}.config/feather/settings.json`.
Verdict: holds.

## Premise 3
Assumption: the GUI started from the desktop launcher runs with cwd = $HOME, so a cwd walk would reach ~/data/catalog.json.
Checked: `grep -n 'Exec=\|Path=' data/*.desktop`
Result: `Exec=appimage-control`, no `Path=` key, so the session launcher's default working directory (the home dir) applies.
Verdict: holds.
