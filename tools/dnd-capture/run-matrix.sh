#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Run the 8-row P0b matrix with automated drags (P0a2).
#
#   usage: run-matrix.sh
#
# Rows in P0b's order: every app from `outside`, then every app from
# `downloads` (the control).  Each row is a full capture run
# (run.sh --auto); there are 2 s between runs.  The whole matrix stops at
# the first `aborted:` in a run's result.txt, or when run.sh itself fails
# (an exit code other than 0 or autodrag's abort code 3) — later rows are
# meaningless once the runner is misbehaving.
#
# Each row's capture lands in
#   docs/research/phase0/captures/<timestamp>-<source>-<location>/
# exactly as in a manual run.

set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
CAPS="$PROJ/docs/research/phase0/captures"

ROWS=(
  "dolphin outside"
  "thunar outside"
  "nautilus outside"
  "cosmic outside"
  "dolphin downloads"
  "thunar downloads"
  "nautilus downloads"
  "cosmic downloads"
)

ABORT_CODE=3

for row in "${ROWS[@]}"; do
  set -- $row
  app="$1"; loc="$2"
  echo ""
  echo "=== matrix row: $app $loc ==="
  # snapshot the matching run directories BEFORE the row (P0a2-r1 minor 9):
  # only a run directory that is new after this row may be read — an older
  # one would be a previous row's result if run.sh died before mkdir
  before="$(ls -d "$CAPS/"*-"$app"-"$loc" 2>/dev/null | sort)"
  "$HERE/run.sh" --auto "$app" "$loc"
  rc=$?
  after="$(ls -d "$CAPS/"*-"$app"-"$loc" 2>/dev/null | sort)"
  rundir="$(comm -13 <(printf '%s\n' "$before") <(printf '%s\n' "$after") | head -n1)"
  answer="(no result.txt)"
  if [ -n "$rundir" ] && [ -f "$rundir/result.txt" ]; then
    answer="$(grep '^zen_opened_file:' "$rundir/result.txt" | head -n1)"
  fi
  echo "=== row $app $loc done: exit $rc, $answer (${rundir:-no new run dir}) ==="

  if [ -n "$rundir" ] && grep -q '^aborted:' "$rundir/result.txt" 2>/dev/null; then
    echo "MATRIX STOPPED: run $app $loc aborted:"
    grep '^aborted:' "$rundir/result.txt"
    exit 1
  fi
  if [ "$rc" -ne 0 ] && [ "$rc" -ne "$ABORT_CODE" ]; then
    echo "MATRIX STOPPED: run.sh for $app $loc failed with exit $rc"
    exit 1
  fi
  if [ "$row" != "${ROWS[-1]}" ]; then
    sleep 2
  fi
done

echo ""
echo "matrix complete: all 8 rows ran without an abort"
