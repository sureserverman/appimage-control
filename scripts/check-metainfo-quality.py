#!/usr/bin/env python3
"""Check the metainfo file against the Flathub quality guidelines we can test.

Usage: check-metainfo-quality.py <metainfo.xml>

Exits 0 when every check passes, 1 when one fails (each failure is printed),
2 on a usage or parse error.
"""
import os
import re
import struct
import sys
import xml.etree.ElementTree as ET

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SCREENSHOT_DIR = "docs/screenshots/"
SCREENSHOT_SIZE = (1000, 700)


def text_of(el):
    return " ".join("".join(el.itertext()).split()) if el is not None else ""


def png_size(path):
    with open(path, "rb") as f:
        head = f.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n" or head[12:16] != b"IHDR":
        return None
    return struct.unpack(">II", head[16:24])


def check_screenshots(root):
    errors = []
    shots = root.findall("./screenshots/screenshot")
    if not 3 <= len(shots) <= 6:
        errors.append(f"{len(shots)} screenshots, want 3 to 6")
    for n, shot in enumerate(shots, 1):
        caption = text_of(shot.find("caption"))
        if not caption:
            errors.append(f"screenshot {n} has no caption")
        elif caption.endswith("."):
            errors.append(f"screenshot {n} caption ends with a period: {caption!r}")
        elif caption[0].isdigit():
            errors.append(f"screenshot {n} caption starts with a digit: {caption!r}")
        url = text_of(shot.find("image"))
        m = re.search(r"/main/(" + re.escape(SCREENSHOT_DIR) + r"[^/]+)$", url)
        if not m:
            errors.append(f"screenshot {n} image is not under {SCREENSHOT_DIR} on main: {url!r}")
            continue
        local = os.path.join(REPO, m.group(1))
        if not os.path.isfile(local):
            errors.append(f"screenshot {n} file is missing: {m.group(1)}")
            continue
        size = png_size(local)
        if size != SCREENSHOT_SIZE:
            errors.append(f"screenshot {n} {m.group(1)} is {size}, want "
                          f"{SCREENSHOT_SIZE[0]}x{SCREENSHOT_SIZE[1]} PNG")
    return errors


def check(path):
    errors = []
    root = ET.parse(path).getroot()

    name = text_of(root.find("name"))
    summary = text_of(root.find("summary"))
    if not 10 <= len(summary) <= 35:
        errors.append(f"summary is {len(summary)} characters, want 10 to 35: {summary!r}")
    if summary.endswith("."):
        errors.append(f"summary ends with a period: {summary!r}")
    if name and name.lower() in summary.lower():
        errors.append(f"summary repeats the app name {name!r}: {summary!r}")

    description = root.find("description")
    paragraphs = description.findall("p") if description is not None else []
    if not paragraphs:
        errors.append("description has no <p>")
    else:
        first = text_of(paragraphs[0]).lower()
        if not re.search(r"\binstances?\b|\bcop(y|ies)\b", first):
            errors.append("description's first paragraph does not mention instances or copies")
        if not re.search(r"checksums?|sha-?256|\bhash(es)?\b", first):
            errors.append("description's first paragraph does not mention checksums")
    items = description.findall(".//li") if description is not None else []
    if len(items) > 10:
        errors.append(f"description has {len(items)} list items, want at most 10")

    colors = root.findall("./branding/color[@type='primary']")
    by_scheme = {c.get("scheme_preference"): (c.text or "").strip().lower() for c in colors}
    if len(colors) != 2 or set(by_scheme) != {"light", "dark"}:
        errors.append("branding needs two primary colors, scheme_preference light and dark")
    elif by_scheme["light"] == by_scheme["dark"]:
        errors.append("light and dark brand colors are the same")

    if not root.findall("./keywords/keyword"):
        errors.append("no <keyword> entries")

    errors.extend(check_screenshots(root))
    return errors


def main(argv):
    if len(argv) != 2:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    try:
        errors = check(argv[1])
    except (OSError, ET.ParseError) as e:
        print(f"cannot read {argv[1]}: {e}", file=sys.stderr)
        return 2
    for e in errors:
        print(f"FAIL: {e}")
    if errors:
        return 1
    print("OK: metainfo meets the checked quality guidelines")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
