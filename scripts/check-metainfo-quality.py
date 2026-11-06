#!/usr/bin/env python3
"""Check the metainfo file against the Flathub quality guidelines we can test.

Usage: check-metainfo-quality.py <metainfo.xml>

Exits 0 when every check passes, 1 when one fails (each failure is printed),
2 on a usage or parse error.
"""
import re
import sys
import xml.etree.ElementTree as ET


def text_of(el):
    return " ".join("".join(el.itertext()).split()) if el is not None else ""


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
