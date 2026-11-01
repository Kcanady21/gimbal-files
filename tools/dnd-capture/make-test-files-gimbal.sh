#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Create the gimbal spike's drag-and-drop test folders (P1g).
#
# Phase 0's calibrated offsets depend on the exact contents of
# ~/gimbal-dnd-test and ~/Downloads/gimbal-dnd-test, so this script never
# touches them.  It creates a separate pair for the spike:
#
#   outside the sandbox (portal path):  ~/gimbal-dnd-test-gimbal/
#   inside the sandbox  (control):      ~/Downloads/gimbal-dnd-test-gimbal/
#
# Both hold the same five entries:
#
#   sample.png          same 16x16 checkerboard as Phase 0
#   sample.txt          same text as Phase 0
#   sample-dir/         a folder with inside.txt
#   sample-link.png     a relative symlink to sample.png
#
# Existing files are never overwritten.  Prints what it created and what was
# already there.

set -eu

python3 - <<'PYEOF'
import os
import struct
import zlib

def png_chunk(ctype, data):
    return (struct.pack(">I", len(data)) + ctype + data
            + struct.pack(">I", zlib.crc32(ctype + data) & 0xffffffff))

def make_png(width=16, height=16):
    # 16x16 RGB checkerboard, generated with zlib+struct only (same as
    # make-test-files.sh, so Phase 0's screenshots stay comparable)
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

def write_file(path, blob):
    if os.path.exists(path):
        print("exists, left untouched: %s" % path)
        return
    with open(path, "wb") as fh:
        fh.write(blob)
    print("created: %s (%d bytes)" % (path, len(blob)))

def make_dir(path):
    if os.path.isdir(path):
        print("exists, left untouched: %s/" % path)
        return
    os.makedirs(path)
    print("created: %s/" % path)

def make_link(path, target):
    if os.path.lexists(path):
        print("exists, left untouched: %s" % path)
        return
    os.symlink(target, path)
    print("created: %s -> %s (relative symlink)" % (path, target))

for base in (os.path.expanduser("~/gimbal-dnd-test-gimbal"),
             os.path.expanduser("~/Downloads/gimbal-dnd-test-gimbal")):
    os.makedirs(base, exist_ok=True)
    print("folder: %s" % base)
    write_file(os.path.join(base, "sample.png"), make_png())
    write_file(os.path.join(base, "sample.txt"),
               b"gimbal drag-and-drop test file\n")
    make_dir(os.path.join(base, "sample-dir"))
    write_file(os.path.join(base, "sample-dir", "inside.txt"),
               b"gimbal drag-and-drop test file inside a folder\n")
    make_link(os.path.join(base, "sample-link.png"), "sample.png")
PYEOF
