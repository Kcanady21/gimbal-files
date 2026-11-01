#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# make-big-dir.sh [COUNT] [DIR]
# Generates a throwaway folder for the spike's large-folder test: COUNT empty files with
# varied stems/extensions (a few long, spacey names, three with an 80+ char unbreakable run) plus subfolders.
# Idempotent: names are derived from the index only, so re-running adds nothing.
set -euo pipefail

COUNT="${1:-100000}"
DIR="${2:-/tmp/gimbal-spike-100k}"
SUBS=6         # subfolders; the first SUBS*PER_SUB files are spread into them
PER_SUB=100

case "$COUNT" in '' | *[!0-9]*) echo "COUNT must be a positive integer: $COUNT" >&2; exit 2 ;; esac

DIR="$(realpath -m -- "$DIR")" # resolves .. so the guard below cannot be walked around
case "$DIR" in
/tmp/gimbal-spike-??*) ;;
*) echo "refusing: DIR must be under /tmp/gimbal-spike- (got $DIR)" >&2; exit 2 ;;
esac
if [ -e "$DIR" ] && [ ! -d "$DIR" ]; then echo "refusing: $DIR exists and is not a directory" >&2; exit 2; fi
[ "$COUNT" -ge "$((SUBS * PER_SUB))" ] || { echo "COUNT too small for $SUBS subfolders" >&2; exit 2; }

mkdir -p "$DIR"
for i in $(seq 0 $((SUBS - 1))); do mkdir -p "$DIR/$(printf 'sub-%02d' "$i")"; done

seq 1 "$COUNT" |
	awk -v dir="$DIR" -v subs="$SUBS" -v per="$PER_SUB" '
  BEGIN {
    ns = split("note Report data Zeta image file record Item alpha Query", stem, " ")
    ne = split("txt md rs py png jpg pdf json csv gz", ext, " ")
  }
  {
    subdir = ($1 <= subs * per) ? sprintf("/sub-%02d", ($1 - 1) % subs) : ""
    name = sprintf("%s-%06d.%s", stem[($1 - 1) % ns + 1], $1, ext[($1 - 1) % ne + 1])
    if ($1 == 7000) name = "Systemd-Private-0123456789abcdef0123456789abcdef-Crash-Reports-For-Spike-Test.txt"
    if ($1 == 8000) name = "Screenshot-From-2026-11-01-at-09-12-33-abcdef0123456789abcdef0123456789-Cropped-For-The-Spike-Check.png"
    if ($1 == 9000) name = "Backup-Of-The-User-Configuration-fedcba9876543210fedcba9876543210-Archive-From-The-Spike-Folder.tar.gz"
    if ($1 % 5000 == 0) name = "the quick brown fox jumps over " name " and keeps going.txt"
    print dir subdir "/" name
  }
  ' | xargs -d '\n' -n 2000 -P 4 touch

echo "made $COUNT file(s) in $DIR ($(find "$DIR" -type f | wc -l) files, $(find "$DIR" -type d | wc -l) dirs on disk)"
