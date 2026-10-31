#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Create the drag-and-drop test files, inside and outside the Zen sandbox.
#
# Zen's Flatpak mounts only ~/Downloads from the home folder, so we need the
# same pair of files in two places:
#
#   outside the sandbox (portal path):  ~/gimbal-dnd-test/
#   inside the sandbox  (control):      ~/Downloads/gimbal-dnd-test/
#
# Existing files are never overwritten.  Prints what it created and what was
# already there.

set -eu

DIRS="$HOME/gimbal-dnd-test $HOME/Downloads/gimbal-dnd-test"

python3 - <<'PYEOF'
import os
import struct
import sys
import zlib

def png_chunk(ctype, data):
    return (struct.pack(">I", len(data)) + ctype + data
            + struct.pack(">I", zlib.crc32(ctype + data) & 0xffffffff))

def make_png(width=16, height=16):
    # 16x16 RGB checkerboard, generated with zlib+struct only
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    rows = b""
    for y in range(height):
        row = b"\x00"  # filter: none
        for x in range(width):
            on = (x // 4 + y // 4) % 2 == 0
            row += bytes((230, 90, 40) if on else (245, 245, 245))
        rows += row
    return b"\x89PNG\r\n\x1a\n" + png_chunk(b"IHDR", ihdr) \
        + png_chunk(b"IDAT", zlib.compress(rows)) + png_chunk(b"IEND", b"")

for base in (os.path.expanduser("~/gimbal-dnd-test"),
             os.path.expanduser("~/Downloads/gimbal-dnd-test")):
    os.makedirs(base, exist_ok=True)
    for name, blob in (("sample.png", make_png()),
                       ("sample.txt", b"gimbal drag-and-drop test file\n")):
        path = os.path.join(base, name)
        if os.path.exists(path):
            print("exists, left untouched: %s" % path)
        else:
            with open(path, "wb") as fh:
                fh.write(blob)
            print("created: %s (%d bytes)" % (path, len(blob)))
PYEOF
