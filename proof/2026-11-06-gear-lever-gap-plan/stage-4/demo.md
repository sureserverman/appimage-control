# Stage 4 demo — 2026-11-06-gear-lever-gap-plan

Environment: this dev host, working tree on branch gear-lever-gap at c05d79ca. The shipped listing files and PNGs were read as a Flathub reviewer would. Tools used: appstreamcli 1.0.2, desktop-file-validate, ImageMagick identify/convert, curl, gh. Gear Lever's Flathub page and the Flathub quality guidelines were fetched for comparison. No VM, no cargo, no source or diff read.

Fill in Verdict and Evidence under each Outcome. Do not edit the quoted lines.

## Outcome 1
> Works: the summary is 35 characters or fewer, starts with a verb and does not repeat the name. The description leads with isolated copies of curated apps whose downloads are checked against a known hash, then settings seeding and backup, in 3 to 6 lines plus at most 6 list items. Light and dark brand colors are set. Three captioned 1000x700 screenshots (catalog, detail page, Preferences) show default light settings.
Verdict: holds
Evidence:
- Summary in metainfo: `<summary>Run apps as isolated copies</summary>`. `printf '%s' "Run apps as isolated copies" | wc -c` gives 27. It starts with the verb "Run". It does not contain "AppImage Control". There is no final period. The desktop file has the same text: `Comment=Run apps as isolated copies`.
- Description paragraph 1: "Install curated apps as separate, isolated copies, each with its own folder, settings and launcher. Every download is checked against a known SHA-256 hash before it is made executable." Paragraph 2: "New copies can start from ready-made settings, and everything the app manages can be backed up and restored." The order is isolated copies + hash check, then seeding + backup. That is about 300 characters, about 4–5 lines at 70 characters. The list has 4 `<li>` items (detail page, settings packs, backup/restore with hash check, login notice).
- Gear Lever's listing for comparison: "An utility to manage AppImages with ease!". It mentions side-by-side versions, has no list, and has one screenshot captioned "Application showcase". This listing's first sentence names the two things Gear Lever's does not: isolated copies with their own settings, and hash-checked downloads.
- Branding: `<color type="primary" scheme_preference="light">#b9e4cf</color>` and `<color type="primary" scheme_preference="dark">#20476e</color>`. Two colors, and they differ.
- Screenshots: three `<screenshot>` entries. Catalog is `type="default"`. Captions: "Catalog of apps with their installed copies", "Detail page with each copy's version, size and folder", "Preferences with the update check at login". None ends with a period. None starts with a digit.
- `identify -format "%w %h"` gives `1000 700` for catalog.png, details.png and preferences.png.
- I viewed each PNG. catalog.png shows the light catalog grid with category headings and installed and available cards. details.png shows the light Mattermost detail window: latest release, pinned update source, "Verified — matches the checksum in the catalog", and two instances with Launch / Open folder. preferences.png shows Preferences → General in light style with Theme = "System" (the default). "Check for updates at login" is on, and the README says it is on by default.
- `appstreamcli validate --no-net --pedantic data/io.github.sureserverman.AppImageControl.metainfo.xml` printed only `P: ...:3: cid-contains-uppercase-letter` and "Validation was successful: pedantic: 1" (rc=0). `desktop-file-validate data/io.github.sureserverman.AppImageControl.desktop` printed nothing (rc=0).

## Outcome 2
> A screenshot URL does not resolve before the push → local validation still passes (`--no-net`). Close-out lists checking the URLs after the push.
Verdict: holds
Evidence:
- `curl -s -o /dev/null -w '%{http_code}' https://raw.githubusercontent.com/sureserverman/appimage-control/main/docs/screenshots/{catalog,details,preferences}.png` returned `404` for all three. So the failure happens today.
- With those URLs unreachable, `appstreamcli validate --no-net data/io.github.sureserverman.AppImageControl.metainfo.xml` still passed: "Validation was successful: pedantic: 1", rc=0.
- For contrast, the networked `appstreamcli validate` failed (rc=3) with `screenshot-image-not-found` for all three URLs. It also gave `url-not-reachable` for the homepage, bugtracker and vcs-browser URLs.
- The plan's close-out notes say: "After `git push origin main` and `git push origin v0.4.0`: confirm the GitHub CI and Flathub workflow runs for the pushed commit succeed, and that the three screenshot URLs return 200." So the URL check after the push is listed.
- The premise is still unchecked, and today's evidence goes against it. `gh repo view sureserverman/appimage-control --json visibility` returns `"visibility":"PRIVATE"`. `curl https://github.com/sureserverman/appimage-control` gives 404, and so does `raw.githubusercontent.com/.../main/README.md`. The push alone will not make the URLs return 200 (see Material finding M1).

## Outcome 3
> Screenshots lack native decorations → close-out lists retaking them on the owner's desktop as ACTION NEEDED (OD-7).
Verdict: holds
Evidence:
- Viewing the three PNGs shows the windows were captured with no window manager. Each fills the frame edge to edge with square corners, a 1–2 px grey border, and no window shadow. catalog.png has only the app's own client-side minimise/maximise/close buttons in its header bar. details.png and preferences.png have only the dialog close button. Flathub's guideline says: "Screenshots must include the native decoration such as any title bar, window shadows, and rounded corners." These screenshots do not meet that rule, so the failure case is present.
- `docs/flathub-publish.md` (step 2) records the Xvfb capture recipe. It notes that a private X server has no window manager.
- The plan's close-out notes include: "ACTION NEEDED (owner): retake the three screenshots on your desktop with native decorations at 1000x700 or smaller (OD-7)." OD-7 in the plan: "Commit plain 1000x700 screenshots now; close-out lists retaking them with window decorations as ACTION NEEDED for the owner."

## Findings

Blocking: none.

Material:
- M1. The GitHub repo is private (`gh repo view` reports `"visibility":"PRIVATE"`). Pushing main will not make the raw.githubusercontent.com screenshot URLs return 200. The homepage and bugtracker URLs will not work either. The close-out step "confirm ... the three screenshot URLs return 200" will fail. Neither the close-out notes nor `docs/flathub-publish.md` say the repo must be made public first. The close-out should add an owner ACTION NEEDED: make the repo public before the URL check and before the Flathub submission. Otherwise it should record the 404s as expected until then.
- M2. The dark brand color #20476e has almost the same brightness as the icon's dark slate body (sampled #2d3647 / #353e54). The contrast ratio is 1.26 / 1.11. Flathub asks that brand colors be "not too similar to the app icon". On the dark banner, the icon outline will barely show. The light color #b9e4cf contrasts well with the icon (about 11:1).

Minor:
- m1. The dark color is a different hue from the light one (navy blue against pale mint). Flathub says the dark variant should "in most cases" be "a darker, more muted version of the light brand color."
- m2. preferences.png shows "Compact rows" switched on. I could not confirm from the README or docs whether that is the default. If it is not, the screenshot breaks "use the platform default configuration".
- m3. The `<screenshot>` entries carry no language tag. Flathub says screenshots "should be tagged with the language they are in". Untagged screenshots are normally taken as the default language, so this is a nit.
- m4. details.png is a 1000x700 dialog with its content in a narrow centre column and about half the frame empty. It is readable but sparse for a store image.
