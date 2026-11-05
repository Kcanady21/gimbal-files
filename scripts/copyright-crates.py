# SPDX-License-Identifier: GPL-3.0-or-later
"""Keep the crate list in packaging/debian/copyright in step with Cargo.lock.

usage: cargo metadata --format-version 1 --offline | python3 -I scripts/copyright-crates.py [--write]

Without --write it exits 1 when the committed list differs from the crates
cargo resolves. With --write it replaces the list in place. release.sh runs the
check, so a dependency change cannot ship a .deb with stale credits.
"""
import json
import pathlib
import re
import sys

COPYRIGHT = pathlib.Path(__file__).resolve().parent.parent / "packaging/debian/copyright"
LINE = re.compile(r"^ [A-Za-z0-9_-]+ [0-9]\S* - ")

metadata = json.load(sys.stdin)
members = set(metadata["workspace_members"])
expected = sorted(
    f" {p['name']} {p['version']} - {p['license']}"
    for p in metadata["packages"]
    if p["id"] not in members
)

lines = COPYRIGHT.read_text().splitlines()
indexes = [i for i, line in enumerate(lines) if LINE.match(line)]
if not indexes or indexes != list(range(indexes[0], indexes[-1] + 1)):
    sys.exit(f"{COPYRIGHT}: the crate list is missing or not one block")
current = lines[indexes[0] : indexes[-1] + 1]

if "--write" in sys.argv[1:]:
    lines[indexes[0] : indexes[-1] + 1] = expected
    COPYRIGHT.write_text("\n".join(lines) + "\n")
elif current != expected:
    for line in sorted(set(current) - set(expected)):
        print(f"stale: {line.strip()}", file=sys.stderr)
    for line in sorted(set(expected) - set(current)):
        print(f"missing: {line.strip()}", file=sys.stderr)
    sys.exit(1)
