#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Run the 8-row gimbal matrix with automated drags (P1g).
#
#   usage: run-matrix-gimbal.sh
#
# Rows, in this order: for `outside`, then `downloads`:
#   sample.png; sample-dir; sample-link.png; sample.png,sample.txt
# (the last row also sets GIMBAL_AUTODRAG_SELECT, so the spike preselects
# both files before the runner presses on the first row).
#
# Each row is a full capture run (run.sh --auto gimbal <location>).  There
# are 2 s between runs.  The whole matrix stops at the first `aborted:` in a
# run's result.txt, when run.sh itself fails (an exit code other than 0 or
# autodrag's abort code 3), or when a run reports `source_intact: no` — a
# vanished or changed source file is a safety failure, not a row to continue
# past.
#
# Each row's capture lands in
#   docs/research/phase0/captures/<timestamp>-gimbal-<location>/
# exactly as in a manual run.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
CAPS="$PROJ/docs/research/phase0/captures"

ROWS=(
  "outside sample.png"
  "outside sample-dir"
  "outside sample-link.png"
  "outside sample.png,sample.txt"
  "downloads sample.png"
  "downloads sample-dir"
  "downloads sample-link.png"
  "downloads sample.png,sample.txt"
)

ABORT_CODE=3

# Parse run.sh's `source_intact:` line through the tested autodrag helper, so
# the matrix and the unit tests cannot disagree about the format.
read_source_intact() {
  python3 -I -c '
import sys
sys.path.insert(0, sys.argv[1])
import autodrag
with open(sys.argv[2], encoding="utf-8", errors="replace") as fh:
    print(autodrag.parse_source_intact(fh.read()) or "missing")
' "$HERE/autodrag" "$1"
}

for row in "${ROWS[@]}"; do
  loc="${row%% *}"
  files="${row#* }"
  sel=""
  case "$files" in
    *,*) sel="$files" ;;
  esac
  echo ""
  echo "=== gimbal matrix row: $loc $files ==="
  # snapshot the matching run directories BEFORE the row (P0a2-r1 minor 9):
  # only a run directory new after this row may be read
  before="$(ls -d "$CAPS/"*-gimbal-"$loc" 2>/dev/null | sort)"
  GIMBAL_AUTODRAG_FILE="$files" GIMBAL_AUTODRAG_SELECT="$sel" \
    "$HERE/run.sh" --auto gimbal "$loc"
  rc=$?
  after="$(ls -d "$CAPS/"*-gimbal-"$loc" 2>/dev/null | sort)"
  rundir="$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -n1)"
  answer="(no result.txt)"
  intact="(no source_intact line)"
  if [ -n "$rundir" ] && [ -f "$rundir/result.txt" ]; then
    answer="$(grep '^zen_opened_file:' "$rundir/result.txt" | head -n1)"
    intact="$(read_source_intact "$rundir/result.txt")"
  fi
  echo "=== row $loc $files done: exit $rc, $answer, source_intact: $intact (${rundir:-no new run dir}) ==="

  if [ -n "$rundir" ] && grep -q '^aborted:' "$rundir/result.txt" 2>/dev/null; then
    echo "MATRIX STOPPED: run $loc $files aborted:"
    grep '^aborted:' "$rundir/result.txt"
    exit 1
  fi
  if [ "$intact" = no ]; then
    echo "MATRIX STOPPED: run $loc $files reported source_intact: no"
    grep '^source_intact:\|^source_diff:\|^  [+-]' "$rundir/result.txt" 2>/dev/null || true
    exit 1
  fi
  if [ "$rc" -ne 0 ] && [ "$rc" -ne "$ABORT_CODE" ]; then
    echo "MATRIX STOPPED: run.sh for $loc $files failed with exit $rc"
    exit 1
  fi
  if [ "$row" != "${ROWS[-1]}" ]; then
    sleep 2
  fi
done

echo ""
echo "matrix complete: all 8 rows ran without an abort and with source_intact: yes"
