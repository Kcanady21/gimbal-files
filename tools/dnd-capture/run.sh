#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Record one Wayland drag-and-drop run: <source app> -> Zen browser tab.
#
#   usage: run.sh [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>
#
# With --auto, the drag itself is performed by autodrag/autodrag.py (P0a2):
# the launch, capture and cleanup below are unchanged, only the "press
# Enter" step and the y/n question are replaced.
#
# `gimbal` is the Phase 1 GTK4 spike (P1g): the binary at
# $PROJ/spike/target/release/gimbal-spike, the test folders
# ~/gimbal-dnd-test-gimbal and ~/Downloads/gimbal-dnd-test-gimbal, and
# GIMBAL_AUTODRAG_SELECT passed through as --select.  Every source gets a
# source-integrity check: the dragged files are hashed before the drag and
# again after cleanup, and result.txt gets `source_intact: yes|no`.
#
# Captures three streams into a run directory under $CAPS:
#   source.log  - WAYLAND_DEBUG=1 stderr of the source file manager
#   zen.log     - WAYLAND_DEBUG=1 (and MOZ_LOG) stderr of a throwaway-profile Zen
#   dbus.log    - dbus-monitor filtered to the xdg document portal
# then asks what happened (result.txt), stops only the processes it started,
# writes meta.txt, reduces everything with summarize.py, and gzips the logs.
#
# Environment knobs:
#   GIMBAL_MOZ_LOG_MODULE  MOZ_LOG module for GTK drag logging (default
#                          WidgetDrag; P0g confirms the exact name from
#                          Firefox source)
#   GIMBAL_AUTODRAG_FILE  (--auto) file or comma-separated list to drag
#                          (default sample.png)
#   GIMBAL_AUTODRAG_SELECT  (gimbal) comma-separated names passed to
#                          `gimbal-spike --select` (multi-file rows)
#   GIMBAL_AUTODRAG_STOP_AFTER  (--auto) passed to autodrag.py
#                          --stop-after <placement|select>
#   GIMBAL_AUTODRAG_HOLD_S  (--auto) passed to autodrag.py --hold-s: seconds
#                          to wait between destroying the virtual device and
#                          the workspace cleanup (inspect the layout then)

set -eu

PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
CAPS="$PROJ/docs/research/phase0/captures"
HERE="$(cd "$(dirname "$0")" && pwd)"

AUTO=""
if [ "${1:-}" = "--auto" ]; then
  AUTO=1
  shift
fi
SOURCE_APP="${1:-}"
LOCATION="${2:-}"
case "$SOURCE_APP" in
  dolphin|thunar|nautilus|cosmic|gimbal) ;;
  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>" >&2; exit 2 ;;
esac
case "$LOCATION" in
  outside|downloads) ;;
  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>" >&2; exit 2 ;;
esac

die() { echo "run.sh: $*" >&2; exit 1; }

# ---------------------------------------------------------------- prechecks
for tool in dbus-monitor busctl flatpak python3 gzip sha256sum; do
  command -v "$tool" >/dev/null || die "$tool not found"
done
[ -n "${WAYLAND_DISPLAY:-}" ] || die "WAYLAND_DISPLAY is not set; run from inside the niri session"

if [ "$SOURCE_APP" = gimbal ]; then
  SOURCE_BIN="$PROJ/spike/target/release/gimbal-spike"
  [ -x "$SOURCE_BIN" ] \
    || die "gimbal-spike not found at $SOURCE_BIN — build it with cargo build --release in spike/"
  case "$LOCATION" in
    outside)  FOLDER="$HOME/gimbal-dnd-test-gimbal" ;;
    downloads) FOLDER="$HOME/Downloads/gimbal-dnd-test-gimbal" ;;
  esac
  for f in "$FOLDER/sample.png" "$FOLDER/sample.txt"; do
    [ -e "$f" ] || die "missing test file $f — run tools/dnd-capture/make-test-files-gimbal.sh first"
  done
else
  case "$LOCATION" in
    outside)  FOLDER="$HOME/gimbal-dnd-test" ;;
    downloads) FOLDER="$HOME/Downloads/gimbal-dnd-test" ;;
  esac
  for f in "$FOLDER/sample.png" "$FOLDER/sample.txt"; do
    [ -e "$f" ] || die "missing test file $f — run tools/dnd-capture/make-test-files.sh first"
  done
fi

# Thunar and Nautilus hand drags off to an already-running instance, which
# would leave source.log empty.  Both are verified to do this; neither has a
# fresh-process flag.  Dolphin and cosmic-files are multi-instance and safe.
name_has_owner() {
  [ "$(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
      org.freedesktop.DBus NameHasOwner s "$1" 2>/dev/null)" = "b true" ]
}
if [ "$SOURCE_APP" = thunar ] && name_has_owner org.xfce.FileManager; then
  die "Thunar is already running and would swallow the drag. Quit it first:  thunar -q"
fi
if [ "$SOURCE_APP" = nautilus ] && name_has_owner org.gnome.Nautilus; then
  die "Nautilus is already running and would swallow the drag. Quit it first:  nautilus -q"
fi

ZEN_DIR="$HOME/.var/app/app.zen_browser.zen/.zen"
if ! grep -q "^Name=gimbal-dnd$" "$ZEN_DIR/profiles.ini" 2>/dev/null; then
  echo "Creating throwaway Zen profile 'gimbal-dnd' (one-time)..."
  flatpak run app.zen_browser.zen -CreateProfile gimbal-dnd >/dev/null
  grep -q "^Name=gimbal-dnd$" "$ZEN_DIR/profiles.ini" || die "could not create the gimbal-dnd Zen profile"
fi
# Every run ends with `flatpak kill`, which Zen counts as an unfinished
# startup; after a few it opens "Open Zen in Troubleshoot Mode?" instead of
# a browser window (seen in the first live --auto run, Oct 31).  Rewritten
# each run, throwaway profile only.
ZEN_PROFILE_DIR="$ZEN_DIR/$(awk -F= '/^\[/{n="";p=""} /^Name=/{n=$2} /^Path=/{p=$2} n=="gimbal-dnd" && p!="" {print p; exit}' "$ZEN_DIR/profiles.ini" 2>/dev/null)"
case "$ZEN_PROFILE_DIR" in
  *gimbal-dnd) cat > "$ZEN_PROFILE_DIR/user.js" <<'EOF'
user_pref("toolkit.startup.max_resumed_crashes", -1);
user_pref("browser.sessionstore.resume_from_crash", false);
user_pref("browser.shell.checkDefaultBrowser", false);
EOF
  ;;
  *) die "could not find the gimbal-dnd profile directory in $ZEN_DIR/profiles.ini" ;;
esac

# ---------------------------------------------------------------- run dir
TS="$(date +%Y%m%d-%H%M%S)"
RUNDIR="$CAPS/$TS-$SOURCE_APP-$LOCATION"
mkdir -p "$RUNDIR"
# only summaries (and meta/result) get committed; raw logs are gzipped and ignored
[ -e "$CAPS/.gitignore" ] || printf '*.log.gz\n' > "$CAPS/.gitignore"

echo "Run directory: $RUNDIR"

# PIDs of everything this script starts; cleanup only ever touches these.
# The throwaway Zen sandbox is stopped by its flatpak INSTANCE id — never by
# the app id, which would also stop Kyle's Zen.
SRC_PID="" ZEN_PID="" ZEN_INSTANCE="" ZEN_INNER_PID="" DBUS_PID=""

# Logging only: the browser process inside the sandbox, if its cmdline still
# carries the profile flag.  Zen may re-launch itself inside the sandbox with
# a bare cmdline (that is how the r3 leak happened), so cleanup must not
# depend on this — it uses flatpak kill on the instance id instead.
find_zen_inner() {
  pgrep -f '^/app/zen/zen .*-P gimbal-dnd' 2>/dev/null | head -n1 || true
}

# flatpak kill is only ever handed an all-numeric instance id that came from
# the before/after instance difference; refuse everything else.
zen_instance_killable() {
  case "$1" in
    ''|*[!0-9]*) return 1 ;;
  esac
  return 0
}

# Does any process inside this flatpak sandbox still reference the
# gimbal-dnd profile?  (P0a-r3 review hardening 1: if a second
# app.zen_browser.zen sandbox starts during the launch window, the
# before/after difference alone cannot tell the two apart.)  flatpak's
# child-pid is the bwrap line, which carries no -P flag (observed:
# "bwrap --args 73 -- launch-script.sh"), and the sandboxed browser
# re-launches itself with a bare cmdline — but it keeps its profile files
# open, so the marker is looked for in cmdline, environ and the open-fd
# links of every process in the instance's cgroup scope.
zen_instance_has_marker() {
  local inst="$1" cpid scope p
  cpid="$(flatpak ps --columns=instance,child-pid 2>/dev/null \
          | awk -v i="$inst" '$1==i{print $2}')"
  [ -n "$cpid" ] || return 1
  scope="$(awk -F: '{print $3}' "/proc/$cpid/cgroup" 2>/dev/null | tail -n1)"
  [ -n "$scope" ] || return 1
  for p in /proc/[0-9]*; do
    [ -r "$p/cgroup" ] || continue
    grep -qxF "0::$scope" "$p/cgroup" 2>/dev/null || continue
    if tr '\0' ' ' < "$p/cmdline" 2>/dev/null | grep -q -- "gimbal-dnd" \
       || tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -q -- "gimbal-dnd" \
       || ls -l "$p/fd" 2>/dev/null | grep -q -- "gimbal-dnd"; then
      return 0
    fi
  done
  return 1
}

# ---------------------------------------------------------------- 1. dbus
dbus-monitor --session \
  "type='method_call',interface='org.freedesktop.portal.FileTransfer'" \
  "type='method_call',interface='org.freedesktop.portal.Documents'" \
  "type='method_return',sender='org.freedesktop.portal.Documents'" \
  "type='error',sender='org.freedesktop.portal.Documents'" \
  "type='signal',interface='org.freedesktop.portal.FileTransfer'" \
  > "$RUNDIR/dbus.log" 2>&1 &
DBUS_PID=$!

cleanup() {
  # logging only — cleanup stops the sandbox via the instance id below
  [ -n "$ZEN_INNER_PID" ] || ZEN_INNER_PID="$(find_zen_inner)"
  local p
  for p in "$SRC_PID" "$ZEN_PID" "$DBUS_PID"; do
    [ -n "$p" ] || continue
    kill -TERM "$p" 2>/dev/null || true
  done
  if zen_instance_killable "$ZEN_INSTANCE"; then
    flatpak kill "$ZEN_INSTANCE" >/dev/null 2>&1 || true
  fi
  sleep 3
  for p in "$SRC_PID" "$ZEN_PID" "$DBUS_PID"; do
    [ -n "$p" ] || continue
    if kill -0 "$p" 2>/dev/null; then
      kill -KILL "$p" 2>/dev/null || true
    fi
  done
  # fail loudly if the throwaway sandbox survived cleanup
  if zen_instance_killable "$ZEN_INSTANCE" \
     && flatpak ps --columns=instance 2>/dev/null | grep -qx "$ZEN_INSTANCE"; then
    echo "ERROR: throwaway Zen instance $ZEN_INSTANCE is still running after cleanup." >&2
    echo "       Stop it manually with:  flatpak kill $ZEN_INSTANCE" >&2
    exit 1
  fi
}
trap cleanup EXIT

# ---------------------------------------------------------------- 2. zen
MOZ_LOG_MODULE="${GIMBAL_MOZ_LOG_MODULE:-WidgetDrag}"

flatpak ps --columns=instance,application 2>/dev/null \
  | awk '$2=="app.zen_browser.zen"{print $1}' | sort > "$RUNDIR/.zen-inst-before"
if [ -n "$AUTO" ]; then
  # read-only niri snapshot for autodrag: it identifies the throwaway Zen
  # window as a window id that is new after this launch (one of the two
  # methods P0a2 hard limit 3 allows; the primary one matches the window's
  # client PID against this flatpak instance's cgroup scope)
  GIMBAL_ZEN_WIN_BEFORE="$(niri msg --json windows 2>/dev/null \
    | python3 -I -c 'import json,sys; print(",".join(str(w["id"]) for w in json.load(sys.stdin)))')" \
    || GIMBAL_ZEN_WIN_BEFORE=""
  export GIMBAL_ZEN_WIN_BEFORE
fi

flatpak run \
  --env=WAYLAND_DEBUG=1 \
  --env="MOZ_LOG=${MOZ_LOG_MODULE}:5" \
  app.zen_browser.zen --new-instance -P gimbal-dnd about:blank \
  > "$RUNDIR/zen.log" 2>&1 &
ZEN_PID=$!

# Wait for the new sandbox and take its INSTANCE id (not the PID): only an
# instance absent from the before-snapshot can be ours.  With several new
# instances (hardening 1), only the one whose sandbox references the
# gimbal-dnd profile is accepted.
NEW_INSTANCES=""
ZEN_INSTANCE=""
for _ in $(seq 1 40); do
  sleep 0.5
  flatpak ps --columns=instance,application 2>/dev/null \
    | awk '$2=="app.zen_browser.zen"{print $1}' | sort > "$RUNDIR/.zen-inst-now" || true
  NEW_INSTANCES="$(comm -13 "$RUNDIR/.zen-inst-before" "$RUNDIR/.zen-inst-now")"
  if [ -n "$NEW_INSTANCES" ]; then break; fi
done
[ -n "$NEW_INSTANCES" ] || die "the gimbal-dnd Zen instance never appeared (zen.log has details); Kyle's own Zen was not touched"
if [ "$(printf '%s\n' "$NEW_INSTANCES" | grep -c .)" -gt 1 ]; then
  ZEN_INSTANCE="$(printf '%s\n' "$NEW_INSTANCES" \
    | while read -r inst; do
        if zen_instance_has_marker "$inst"; then echo "$inst"; fi
      done | head -n1)"
  [ -n "$ZEN_INSTANCE" ] || die "several new app.zen_browser.zen instances appeared ($(printf '%s ' $NEW_INSTANCES)) and none references the gimbal-dnd profile; refusing to guess"
  echo "several new Zen sandboxes seen; picked instance $ZEN_INSTANCE by the gimbal-dnd profile marker"
else
  ZEN_INSTANCE="$(printf '%s\n' "$NEW_INSTANCES" | head -n1)"
fi
# guard: cleanup may only ever flatpak-kill a numeric instance id that is in
# the before/after difference above (never the app id — that would take down
# Kyle's Zen too)
if ! zen_instance_killable "$ZEN_INSTANCE" \
   || ! grep -qx "$ZEN_INSTANCE" "$RUNDIR/.zen-inst-now" \
   || grep -qx "$ZEN_INSTANCE" "$RUNDIR/.zen-inst-before"; then
  # hardening 2: clear the variable BEFORE dying so the EXIT trap can never
  # flatpak-kill a refused id
  REFUSED_INSTANCE="$ZEN_INSTANCE"
  ZEN_INSTANCE=""
  die "refusing instance id '$REFUSED_INSTANCE': not a new numeric app.zen_browser.zen instance"
fi
rm -f "$RUNDIR/.zen-inst-before" "$RUNDIR/.zen-inst-now"
ZEN_INNER_PID="$(find_zen_inner)"
echo "Zen (throwaway profile) running: flatpak wrapper $ZEN_PID, instance $ZEN_INSTANCE, browser PID ${ZEN_INNER_PID:-unknown}"

# ---------------------------------------------------------------- 3. source
if [ -n "$AUTO" ]; then
  # autodrag locates the file through AT-SPI.  Qt only builds its
  # accessibility tree when this is set (the session's
  # org.a11y.Status.IsEnabled is false — see NOTES-autodrag.md); it is
  # harmless for the GTK and COSMIC apps.
  export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
fi
case "$SOURCE_APP" in
  dolphin)  SOURCE_BIN=dolphin;  SOURCE_ARGS=() ;;
  thunar)   SOURCE_BIN=thunar;   SOURCE_ARGS=() ;;
  nautilus) SOURCE_BIN=nautilus; SOURCE_ARGS=() ;;
  cosmic)   SOURCE_BIN=cosmic-files
            # cosmic-files daemonizes when stdin is a terminal (src/lib.rs),
            # which would orphan the real process and break PID cleanup.
            SOURCE_ARGS=(--no-daemon) ;;
  gimbal)   SOURCE_BIN="$PROJ/spike/target/release/gimbal-spike"
            SOURCE_ARGS=()
            # the multi-file rows preselect their names in the spike; a click
            # from the runner would reset the selection to one item
            if [ -n "${GIMBAL_AUTODRAG_SELECT:-}" ]; then
              SOURCE_ARGS=(--select "$GIMBAL_AUTODRAG_SELECT")
            fi ;;
esac
WAYLAND_DEBUG=1 "$SOURCE_BIN" "${SOURCE_ARGS[@]}" "$FOLDER" > "$RUNDIR/source.log" 2>&1 &
SRC_PID=$!
sleep 2
if ! kill -0 "$SRC_PID" 2>/dev/null; then
  echo "WARNING: $SOURCE_BIN exited immediately; source.log will be empty" >&2
else
  echo "$SOURCE_BIN running: PID $SRC_PID"
fi

# ---------------------------------------------------------------- source integrity
# P1g, every source: hash each dragged item before the drag and again after
# cleanup.  A symlink is recorded by its link target (readlink), not by the
# target's bytes; a folder is recorded as every file under it.  This checks
# the "never delete on drag-end" rule directly: Phase 0 saw every drop into
# Zen report MOVE, so a vanished source would show up here.
SOURCE_ITEMS_RAW="${GIMBAL_AUTODRAG_FILE:-sample.png}"
IFS=',' read -r -a INTEGRITY_ITEMS <<< "$SOURCE_ITEMS_RAW"
SOURCE_SNAP_BEFORE="$RUNDIR/.source-before"
source_snapshot() {
  local out="$1" item path rel f hash
  : > "$out"
  for item in "${INTEGRITY_ITEMS[@]}"; do
    path="$FOLDER/$item"
    if [ -L "$path" ]; then
      printf '%s\tlink:%s\n' "$item" "$(readlink "$path")" >> "$out"
    elif [ -d "$path" ]; then
      while IFS= read -r -d '' f; do
        rel="${f#"$FOLDER"/}"
        if [ -L "$f" ]; then
          printf '%s\tlink:%s\n' "$rel" "$(readlink "$f")" >> "$out"
        else
          hash="$(sha256sum "$f" | awk '{print $1}')"
          printf '%s\t%s\n' "$rel" "$hash" >> "$out"
        fi
      done < <(find "$path" \( -type f -o -type l \) -print0 2>/dev/null)
    elif [ -f "$path" ]; then
      hash="$(sha256sum "$path" | awk '{print $1}')"
      printf '%s\t%s\n' "$item" "$hash" >> "$out"
    else
      printf '%s\tmissing\n' "$item" >> "$out"
    fi
  done
  sort -o "$out" "$out"
}
source_snapshot "$SOURCE_SNAP_BEFORE"

# ---------------------------------------------------------------- 4. the drag
AUTO_RC=0
if [ -n "$AUTO" ]; then
  # --auto (P0a2): autodrag.py performs the drag a person would perform —
  # waits for both windows, places them on the empty end workspace, finds
  # the file via AT-SPI, clicks it, verifies the selection, drags into Zen,
  # classifies Zen's title — and writes result.txt itself.  Everything
  # around it (launch, capture, cleanup, meta, summarize) is unchanged; the
  # context below tells autodrag which windows are ours (P0a2 hard limit 3).
  GIMBAL_SRC_PID="$SRC_PID" GIMBAL_ZEN_INSTANCE="$ZEN_INSTANCE" \
  GIMBAL_RUNDIR="$RUNDIR" GIMBAL_FOLDER="$FOLDER" \
  python3 -I "$HERE/autodrag/autodrag.py" run \
    --source "$SOURCE_APP" --location "$LOCATION" \
    --file "${GIMBAL_AUTODRAG_FILE:-sample.png}" \
    ${GIMBAL_AUTODRAG_STOP_AFTER:+--stop-after "$GIMBAL_AUTODRAG_STOP_AFTER"} \
    ${GIMBAL_AUTODRAG_HOLD_S:+--hold-s "$GIMBAL_AUTODRAG_HOLD_S"} \
    || AUTO_RC=$?
  if [ "$AUTO_RC" -ne 0 ]; then
    echo "autodrag exited with code $AUTO_RC (see result.txt in the run directory)"
  fi
else
cat <<EOF

=====================================================================
Capture running.  Logs: source.log zen.log dbus.log in
  $RUNDIR

1. Tile Zen and $SOURCE_APP side by side (niri: Mod+H / Mod+V, or
   drag a window to a screen edge).
2. Both should show $FOLDER (Zen shows about:blank).
3. Drag sample.png from $SOURCE_APP into the empty Zen tab and drop it.
4. Come back to this terminal and press Enter.
=====================================================================
EOF
read -r -p "Press Enter when the drag is done... " _

read -r -p "Did Zen open the file? [y/n/other] " ANSWER || ANSWER=""
read -r -p "Optional note (Enter to skip): " NOTE || NOTE=""
{
  echo "zen_opened_file: $ANSWER"
  echo "note: $NOTE"
} > "$RUNDIR/result.txt"
fi

# ---------------------------------------------------------------- 5. stop + reduce
cleanup
trap - EXIT

# source integrity, after cleanup and before summarize.py (P1g)
SOURCE_SNAP_AFTER="$RUNDIR/.source-after"
source_snapshot "$SOURCE_SNAP_AFTER"
touch "$RUNDIR/result.txt"
{
  if cmp -s "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER"; then
    echo "source_intact: yes"
  else
    echo "source_intact: no"
    echo "source_diff:"
    diff -u "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER" \
      | sed -n 's/^\([+-][^+-].*\)$/  \1/p' || true
  fi
} >> "$RUNDIR/result.txt"
rm -f "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER"

# meta.txt before summarize.py so summary.json can embed it
SOURCE_VERSION=""
COSMIC_COMMIT=""
if [ "$SOURCE_APP" = gimbal ]; then
  # P1b treats --version as an unknown flag (exit 2): never run it.
  SOURCE_VERSION="gimbal-spike sha256:$(sha256sum "$SOURCE_BIN" | cut -c1-12)"
elif [ "$SOURCE_APP" = cosmic ]; then
  # cosmic-files has no --version (it parses every flag as a path); use the
  # git commit of Kyle's build in ~/cosmic-files
  COSMIC_COMMIT="$(git -C "$HOME/cosmic-files" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
  SOURCE_VERSION="cosmic-files git $COSMIC_COMMIT"
else
  SOURCE_VERSION="$("$SOURCE_BIN" --version 2>/dev/null | head -n1)"
fi
{
  echo "date: $(date -Is)"
  echo "source: $SOURCE_APP"
  echo "location: $LOCATION"
  echo "source_version: $SOURCE_VERSION"
  if [ "$SOURCE_APP" = cosmic ]; then
    echo "cosmic_commit: $COSMIC_COMMIT (~/cosmic-files)"
  fi
  echo "zen_version: $(flatpak info app.zen_browser.zen 2>/dev/null | sed -n 's/^ *Version: *//p' | head -n1)"
  echo "zen_profile: gimbal-dnd (throwaway)"
  echo "moz_log_module: $MOZ_LOG_MODULE"
  echo "niri_version: $(niri msg version 2>/dev/null | head -n1)"
  echo "folder: $FOLDER"
} > "$RUNDIR/meta.txt"

python3 -I "$HERE/summarize.py" "$RUNDIR"

gzip -f "$RUNDIR/zen.log" "$RUNDIR/source.log" "$RUNDIR/dbus.log"

echo
echo "Done.  Summary:"
echo "  $RUNDIR/summary.md"
echo "  $RUNDIR/summary.json"
echo "Raw logs gzipped next to them (*.log.gz, git-ignored)."
# propagate autodrag's exit code (--auto only; 0 in manual runs)
exit "$AUTO_RC"
