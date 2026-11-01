# NOTES — P0a2 automated drag runner

Every claim is tagged **[read]** (source/doc on disk or fetched), **[observed]**
(ran it on this machine and watched) or **[inferred]** (reasoning, marked when
load-bearing).  Date of all observations: 2026-10-30.  The agent wrote and
tested the code but never opened `/dev/uinput`, never moved the pointer and
never ran a `niri msg action …` (P0a2 hard limits 1–2); everything real is
Claude's live check (TODO list at the end).

Start condition: **P0a-r3 reviewed and accepted** (NOTES.md, "Review of
P0a-r3: Verdict: accepted. P0a is done."), including its two "Minor, folded
into P0a2" items — both are in the run.sh diff below.

## Architecture: how the launch code is reused (not copied)

P0a's launch code lives in `run.sh` (bash).  The runner reuses it literally:

- `run.sh --auto <source> <location>` runs steps 1–3 exactly as before (D-Bus
  monitor, throwaway Zen, source app, captures), then calls
  `python3 -I autodrag/autodrag.py run --source … --location … --file
  sample.png` **in place of the "press Enter" step**, with the context in
  `GIMBAL_SRC_PID`, `GIMBAL_ZEN_INSTANCE`, `GIMBAL_RUNDIR`,
  `GIMBAL_FOLDER`, `GIMBAL_ZEN_WIN_BEFORE`.  Cleanup, meta.txt, summarize.py
  and the gzip stay in run.sh and run unchanged; the y/n questions are
  skipped because autodrag writes `result.txt` itself.  run.sh exits with
  autodrag's exit code (0 ok, 3 aborted, 2 usage/config).
- `autodrag.py run …` without that context and without `--dry-run` **execs
  `run.sh --auto …`** — so there is exactly one launch implementation and no
  copy of it in Python.  **[inferred]** (design choice; the brief's "using
  P0a's launch code (reuse it; don't copy it)" read as: the launch must not
  be re-implemented.)
- `--dry-run` runs nothing anywhere (no launch, no reads, no actions): it
  prints the planned commands, coordinates and events from apps.toml plus
  the output size (defaults to this machine's 2256×1504, `--width/--height`
  override).
- `--stop-after <placement|select>` is an autodrag flag; run.sh forwards it
  from `GIMBAL_AUTODRAG_STOP_AFTER` so the live check can run
  `GIMBAL_AUTODRAG_STOP_AFTER=placement ./run.sh --auto dolphin outside`.
  Stopping writes `aborted: stop-after <phase>` into result.txt (intentional
  stop, distinct from a safety abort), restores the workspace focus, and
  exits 3; run.sh's own cleanup closes the processes as always.

`result.txt` (P0a format plus the auto: fields the brief asks for):

```
zen_opened_file: y            # y | n | other | aborted
note: auto:true source=… location=… file=… folder=…
auto: true
source: dolphin
location: outside
file: sample.png
selection: selected           # selected | unverified | n/a
click_at: (419, 174)
drop_at: (1682, 823)
title_before: Zen Browser
raw_title: sample.png (PNG Image, 64x64) — Zen
```

summarize.py's `read_result` parses `zen_opened_file:`/`note:` and ignores
the rest (read of summarize.py:212-230 **[read]**), so P0b/P0h consumers are
unaffected; `aborted:` lines are extra.

## Environment as found (this round)

- niri 26.04, one output DP-2 2256×1504@(0,0) scale 1.0; DP-3 present but
  `logical: null`.  The `niri msg --json outputs` schema has **no `enabled`
  key** — an enabled output is one with non-null `logical` **[observed]**.
  niri.py's `enabled_outputs()` implements exactly that rule.
- `niri msg --json windows` fields **[observed]**: `id, pid, app_id, title,
  workspace_id, is_focused, is_floating, is_urgent, layout{
  pos_in_scrolling_layout, tile_size, window_size,
  tile_pos_in_workspace_view, window_offset_in_tile}, focus_timestamp`.
  `is_floating` exists (used for placement readback).  For tiled windows
  `tile_pos_in_workspace_view` is null — confirmed again on 9 windows,
  including Kyle's Zen (app_id `app.zen_browser.zen`, pid 48894).
- `niri msg --json workspaces`: `id, idx, name, output, is_urgent,
  is_active, is_focused, active_window_id`; **id ≠ idx** (Kyle's session:
  ids 1,2,5,6 with idxs 1–4) **[observed]**.
- `niri msg layers`: Noctalia `noctalia-wallpaper` (background),
  `noctalia-bar-default` + `noctalia-dock` (top) are active **[observed]**.
- Kyle's Zen was running the whole time (flatpak instance `2943722691`);
  Thunar, Nautilus and cosmic-files were not (NameHasOwner false,
  re-checked before each probe) **[observed]**.
- Python 3.14.4 (`tomllib` available); Atspi via gi works under
  `python3 -I` **[observed]**.

## niri actions used — exact syntax read from the CLI

All read from `niri msg action --help` and the subcommand `--help`s
(niri 26.04) **[read]**; recorded verbatim:

- `move-window-to-floating` — "Move the focused window to the floating
  layout"; usage `niri msg action move-window-to-floating [OPTIONS]`;
  `--id <ID>` "Id of the window to move. If `None`, uses the focused
  window."  Chosen over `toggle-window-floating` (also exists, same --id)
  because toggling an already-floating window would tile it;
  move-to-floating is idempotent.  **[inferred]** (semantics), name **[read]**.
- `move-floating-window` — usage `… move-floating-window [OPTIONS]`;
  `--id <ID>`, `-x/--x <X>` "How to change the X position [default: +0]",
  `-y/--y <Y>`.  A **plain number is an absolute** position, `+`/`-` is
  relative: confirmed from niri-ipc v26.04 source (raw.githubusercontent.com
  YaLTeR/niri v26.04, niri-ipc/src/lib.rs): the type is `PositionChange`
  with `SetFixed/SetProportion/AdjustFixed/AdjustProportion`, and FromStr
  maps a leading `+`/`-` to Adjust*, otherwise Set* ("10" → SetFixed(10),
  "+10" → AdjustFixed(10)).  **[read]**
- `set-window-width` / `set-window-height` — usage `… set-window-width
  [OPTIONS] <CHANGE>` with `--id <ID>`; the positional `<CHANGE>` parses by
  the same `SizeChange::FromStr` (plain = absolute px, `+`/`-` = adjust,
  `%` = proportion).  **[read]**
- `move-window-to-workspace` — usage `… move-window-to-workspace [OPTIONS]
  <REFERENCE>`; the flag is **`--window-id <WINDOW_ID>`** (not `--id`), plus
  `--focus <FOCUS>` "Whether the focus should follow the moved window …
  [default: true] [possible values: true, false]".  The runner always passes
  `--focus false` and switches workspaces explicitly.  **[read]**
- `focus-workspace` — usage `… focus-workspace <REFERENCE>`: "Focus a
  workspace by reference (index or name)".  **No `--id` flag exists**, and
  `WorkspaceReferenceArg::FromStr` in niri-ipc v26.04 parses a numeral as an
  **index**, not an id (variants Id/Index/Name all exist in the type; the
  CLI FromStr only ever produces Index or Name).  **[read]**  Consequence:
  "return focus to Kyle's original workspace **(by ID)**" is implemented as
  `focus_workspace_by_id` — look the id up in `workspaces()`, pass its idx.
  **[inferred]** (only way to honor "by ID" within the CLI; recorded here
  for the review.)
- `focus-window` — usage `… focus-window --id <ID>`.  **[read]**

The full command sequence the runner issues (all `--id`/`--window-id`-bound
to windows it launched — hard limit 3): float both windows, move both to the
target workspace (`--focus false`), `focus-workspace <idx>`, then per window
`move-floating-window -x … -y …` + `set-window-width` + `set-window-height`,
finally `focus-window <source>`.  Cleanup issues one
`focus-workspace <Kyle's idx>` (id looked up first).  Nothing else.

### Workspace choice

Target = the **empty workspace at the end** of the (single) output:
`max(idx)` when `is_empty` **[read of JSON shape + observed]**.  If no empty
workspace exists, the runner targets `last_idx + 1` and logs a TODO —
whether niri creates a workspace on demand for
`move-window-to-workspace`/`focus-workspace` to a nonexistent index is
**unknown without running an action** (TODO 4 below); if it does not, the
placement readback fails and the run aborts cleanly.

## Window identification (hard limit 3)

Source window: `window_by_pid(windows, GIMBAL_SRC_PID)` — the launched
process owns the window (all four FMs verified: fresh process = fresh
window; probes matched `pid == popen.pid`) **[observed]**.

Throwaway Zen window, two ways (hard limit 3 names both):

1. **Primary — by PID via the flatpak instance's cgroup scope.**  niri
   reports the Wayland client's host-namespace pid (Kyle's Zen window:
   pid 48894 = the inner `/app/zen/zen` main process **[observed]**), and
   `/proc/<pid>/cgroup` contains
   `…/app-flatpak-app.zen_browser.zen-<instance>.scope` (verified for
   instance 2943722691) **[observed]**.  `find_zen_window()` matches the
   scope string; Kyle's own Zen (different instance) can never match.
2. **Fallback — window id new after launch.**  run.sh --auto snapshots
   `niri msg --json windows` ids before the Zen launch into
   `GIMBAL_ZEN_WIN_BEFORE`; autodrag picks the first window not in that set
   whose app_id contains "zen".

## AT-SPI findings per app (probe: launch at ~/gimbal-dnd-test, read tree,
close by PID — hard limit 7; no clicks)

Method: fresh process (`start_new_session`), wait for the niri window by
pid, read the tree via `Atspi.get_desktop(0)`, find the application child
whose `get_process_id()` equals the launched pid — **`Atspi.Accessible.
get_process_id()` exists and works** **[observed]** — then BFS the tree
(budget 5000 nodes), and SIGTERM the pid (SIGKILL after 5 s; all probes
died on TERM; verified `not os.path.exists(/proc/<pid>)`).

Session state that shapes all of this: **`org.a11y.Status.IsEnabled` is
`false`** on the session bus (checked via
`busctl --user get-property org.a11y.Bus /org/a11y/bus org.a11y.Status
IsEnabled` → `b false`) **[observed]**.  The a11y bus itself runs
(org.a11y.Bus = at-spi-bus-launcher; niri exports
org.freedesktop.a11y.Manager) **[observed]**.  I did **not** flip IsEnabled:
that is session-wide state, not something the brief's allowances cover.

| app | registers in AT-SPI? | file items exposed? | details |
|---|---|---|---|
| dolphin 25.12.3 | **only with `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1`** set at launch | **yes** | without the env: no application child at all (first probe).  With it: app "dolphin", 506 nodes; `sample.png` role **"list item"**, extents (CoordType.WINDOW) **(312, 88) 135×92**; `sample.txt` (164, 88) 135×92; states selectable/showing/visible/focusable **[observed]** |
| thunar 4.20.7 | yes (app "thunar", 224 nodes) | **no** | full chrome (toolbar buttons, menus, split pane, info bar) but the view contributes no item children; BFS found zero `sample*` names with zero AT-SPI errors **[observed]** |
| nautilus 50.2.2 | yes (app "org.gnome.Nautilus") | **no** | tree is exactly application→frame→panel→panel→panel, 18 nodes; identical with `GTK_A11Y=atspi` and after 10 s settle **[observed]** |
| cosmic-files git 09bdd8251d5f | **no AT-SPI at all** | — | no application child appears (matches the brief's expectation) **[observed]** |

Frame-vs-tile (the decoration question): nautilus's AT-SPI frame extents
(1120×1462) equal niri's `tile_size` exactly; thunar's frame is **48×48 px
larger** than the tile (GTK3 CSD shadow border — window-relative origin
sits 24 px inside the shadow); dolphin's frame is **6×33 px smaller**
(Qt frame excludes niri's SSD titlebar/border) **[observed]**.  These are
per-app `window_offset_correction` values in apps.toml, **deliberately left
[0,0] with TODO(live)**: the placement the runner creates is floating, not
tiled, so the live check must re-measure (TODO 6).  The selection check is
the safety net while they are uncalibrated — a missed click lands on empty
view space and the run aborts.

Consequences wired into the code:

- run.sh --auto exports `QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` for the source
  launch (harmless for GTK/COSMIC; the non-auto path does not set it).
- `locate.py` tries AT-SPI first (so thunar/nautilus start working
  automatically if the a11y gate opens), falls back to apps.toml
  `first_item_offset`, and **aborts with "unconfigured"** rather than
  clicking blind when neither exists (all three non-Qt apps are in that
  state until calibrated).
- Selection check: single click, then re-query the item's
  `STATE_SELECTED`; apps without a locatable item record
  `selection: unverified` and continue (per the brief).

## How libinput sees the uinput device (expected — confirm live)

The brief expected "handled like a VM tablet".  The sources say **VM mouse**
— same family, same practical result (absolute pointer motion), different
name:

- Classification happens in **udev**, not libinput: for a device with
  ABS_X+ABS_Y and BTN_LEFT and nothing else, systemd's
  `src/udev/udev-builtin-input_id.c` (test_pointers) finds `has_mouse_button`
  over the BTN_MOUSE..BTN_JOYSTICK range (BTN_LEFT == BTN_MOUSE == 0x110),
  `is_abs_mouse` = true — its comment reads "This path is taken by VMware's
  USB mouse" — and emits **`ID_INPUT=1` + `ID_INPUT_MOUSE=1`**; no
  TOUCHSCREEN/TABLET/TOUCHPAD/JOYSTICK tag applies (no BTN_TOUCH, no
  BTN_STYLUS/BTN_TOOL_PEN, no REL axes).  **[read]** (fetched from
  github.com/systemd/systemd main, 2026-10-30).
- libinput maps udev tags to capabilities in `src/evdev.c`
  (evdev_configure_device): `EVDEV_UDEV_TAG_MOUSE` →
  `EVDEV_DEVICE_POINTER` ("device is a pointer"); a device matching no tag
  would end in fallback dispatch with no pointer capability.  With
  ID_INPUT_MOUSE we get the **pointer** path.  **[read]** (fetched from
  gitlab.freedesktop.org libinput main, 2026-10-30).
- Absolute pointer motion reaches the compositor as device units the
  compositor scales (`libinput_event_pointer_get_absolute_x_transformed(ev,
  width)` in the libinput pointer-event API).  niri's exact inverse mapping
  (against output width, desktop width, and where pixel centers land) is
  what the calibration resolves.  **[read]** + **[inferred]** for niri.
- `INPUT_PROP_DIRECT` (input-event-codes.h:24) is **never set** — with no
  props and no BTN_TOUCH the udev tree above cannot classify the device as
  a touchscreen.  **[read]**

**Needs confirming live** (TODO 1/2): create the device, watch
`libinput debug-events` / niri's log for "device is a pointer" + the
abs-mouse axes, and verify the cursor actually tracks `--square`.

## Kernel constants — cited, not recalled

All in `vmouse.py` with the defining line in a comment next to each
constant:

- `/usr/include/linux/uinput.h`: `UINPUT_IOCTL_BASE 'U'` (:63),
  `UI_DEV_CREATE _IO('U',1)` (:64), `UI_DEV_DESTROY _IO('U',2)` (:65),
  `UI_DEV_SETUP _IOW('U',3,struct uinput_setup)` (:100),
  `UI_ABS_SETUP _IOW('U',4,struct uinput_abs_setup)` (:135),
  `UI_SET_EVBIT/_KEYBIT/_ABSBIT` (:137/:138/:140), `UI_SET_PROPBIT` (:147,
  never used), `UINPUT_MAX_NAME_SIZE 80` (:47); `struct uinput_setup`
  (:67-71) → format `=HHHH80sI` (92 B), `struct uinput_abs_setup` (:102-106)
  → `=H2x6i` (28 B) over `struct input_absinfo` = 6×__s32
  (/usr/include/linux/input.h:91-98); the `_IOC` bit layout from
  /usr/include/asm-generic/ioctl.h (:23-24, :32, :44-48, :58, :62, :66, :69)
  — the two `_IOW` ioctl numbers are **computed from the struct sizes**, so
  they cannot drift from the cited layouts.
- `/usr/include/linux/input-event-codes.h`: `EV_SYN 0x00` (:39),
  `EV_KEY 0x01` (:40), `EV_ABS 0x03` (:42), `BTN_LEFT 0x110` (:357),
  `ABS_X 0x00` (:871), `ABS_Y 0x01` (:872), `INPUT_PROP_DIRECT 0x01` (:24,
  deliberately never set), `SYN_REPORT 0` (:34).
- `/usr/include/linux/input.h`: `struct input_id` (:57-62), `BUS_VIRTUAL
  0x06` (:257), `struct input_event` (:26-38) → native `@llHHi` = 24 B on
  this x86_64 ABI, guarded by an assert.

Device declaration: `EV_ABS` with ABS_X/ABS_Y (0..65535), `EV_KEY` with
BTN_LEFT only, `EV_SYN`; no REL axes, no other buttons, no props, no keys —
it cannot type (hard limit 4).  uinput's `UI_ABS_SETUP` is used (the header
(:109-122) documents it as superseding the write()-based
`uinput_user_dev` setup) **[read]**.

## Timing and the 20 s limit

The abort clock starts when `autodrag.py run` starts (attached mode: after
run.sh's `sleep 2`), and abort checks run at every step boundary.  Typical
budget: window wait (Zen starts in seconds), ~12 niri actions, AT-SPI tree
walk (<1 s for dolphin's 506 nodes), click+settle, drag ≈ 1.5 s, title wait
≤ 3 s — tight but plausible under 20 s **[inferred]**.  If live checks show
the window wait eating the budget, the natural alternative is anchoring the
clock at the first pointer press — left as-is per the brief, flagged here.
A timeout during the title wait (button already released) classifies the
current title and records it; a timeout between steps records
`aborted: timeout`.

## Dry-run output (saved per the brief)

`python3 -I autodrag/vmouse.py --square --dry-run` (reads the real output
size, sends nothing):

```
output: 2256x1504 logical px, ABS range 0..65535, mapping abs = px*65535/extent
device would declare: EV_ABS ABS_X/ABS_Y 0..65535, EV_KEY BTN_LEFT, EV_SYN (no props, no keys)
move_to (1028, 652) -> ABS (29863, 28410)
move_to (1228, 652) -> ABS (35672, 28410)
move_to (1228, 852) -> ABS (35672, 37125)
move_to (1028, 852) -> ABS (29863, 37125)
move_to (1028, 652) -> ABS (29863, 28410)
no button events: 5 frames, all EV_KEY counts: 0
```

`python3 -I autodrag/autodrag.py run --source dolphin --location outside
--file sample.png --dry-run` (abridged at the travel steps; full text is
reproducible):

```
autodrag --dry-run: nothing is run, launched or moved
plan: source=dolphin location=outside file=sample.png
output: 2256x1504, margin 40
boxes: source=(40, 40, 1068, 1424) zen=(1148, 40, 1068, 1424)

planned niri commands (ids are read at runtime):
  niri msg action move-window-to-floating --id '<source-window-id>'
  niri msg action move-window-to-floating --id '<zen-window-id>'
  niri msg action move-window-to-workspace --window-id '<source-window-id>' --focus false '<empty-end-workspace-idx>'
  niri msg action move-window-to-workspace --window-id '<zen-window-id>' --focus false '<empty-end-workspace-idx>'
  niri msg action focus-workspace '<empty-end-workspace-idx>'
  niri msg action move-floating-window --id '<source-window-id>' -x 40 -y 40
  niri msg action set-window-width --id '<source-window-id>' 1068
  niri msg action set-window-height --id '<source-window-id>' 1424
  niri msg action move-floating-window --id '<zen-window-id>' -x 1148 -y 40
  niri msg action set-window-width --id '<zen-window-id>' 1068
  niri msg action set-window-height --id '<zen-window-id>' 1424
  niri msg action focus-window --id '<source-window-id>'
  # afterwards: verify layout via `niri msg --json windows`
  # on cleanup: niri msg action focus-workspace <Kyle's ws idx, looked up by id>

planned pointer events (logical px -> ABS):
  item point: located at runtime via AT-SPI (apps.toml atspi_items=True); fallback offset: None
  drop point: (1682, 823) (zen box + fraction [0.5, 0.55])
  click:  move_to <item point>; press (EV_KEY BTN_LEFT 1 + SYN); release (EV_KEY BTN_LEFT 0 + SYN)
  verify: STATE_SELECTED via AT-SPI (unverified when no AT-SPI)
  drag:   press (BTN_LEFT 1 + SYN)
    move_to item + (+3, +3)  # past the drag threshold
    move_to item + (+6, +6)  # past the drag threshold
    move_to item + (+9, +9)  # past the drag threshold
    move_to item + (+12, +12)  # past the drag threshold
  lerp(item, drop, t) = item + (drop - item) * t
    move_to lerp(item, drop, 1/30)  # ~20 ms per step
    …
    move_to lerp(item, drop, 30/30)  # ~20 ms per step
  hold 400 ms, then release (EV_KEY BTN_LEFT 0 + SYN)

classify: title unchanged -> n; contains 'sample.png' -> y; else other
abort rules: focused window not ours | window moved/resized | >1 enabled output | run > 20 s
```

## run.sh — the complete diff (with a pristine copy saved before editing)

Non-`--auto` behavior is unchanged except the two review items
(hardening 1 only diverges when **two or more** new Zen sandboxes appear in
the launch window; with one new instance the pick is byte-identical to
r3's):

```diff
--- run.sh (P0a-r3)
+++ run.sh (P0a2)
@@ -2,7 +2,11 @@
 # SPDX-License-Identifier: GPL-3.0-or-later
 # Record one Wayland drag-and-drop run: <source app> -> Zen browser tab.
 #
-#   usage: run.sh <dolphin|thunar|nautilus|cosmic> <outside|downloads>
+#   usage: run.sh [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>
+#
+# With --auto, the drag itself is performed by autodrag/autodrag.py (P0a2):
+# the launch, capture and cleanup below are unchanged, only the "press
+# Enter" step and the y/n question are replaced.
 #
 # Captures three streams into a run directory under $CAPS:
@@ -15,6 +19,9 @@
 #   GIMBAL_MOZ_LOG_MODULE  MOZ_LOG module for GTK drag logging (default
 #                          WidgetDrag; P0g confirms the exact name from
 #                          Firefox source)
+#   GIMBAL_AUTODRAG_FILE  (--auto) file to drag (default sample.png)
+#   GIMBAL_AUTODRAG_STOP_AFTER  (--auto) passed to autodrag.py
+#                          --stop-after <placement|select>
 
 set -eu
@@ -22,15 +29,20 @@
 HERE="$(cd "$(dirname "$0")" && pwd)"
 
+AUTO=""
+if [ "${1:-}" = "--auto" ]; then
+  AUTO=1
+  shift
+fi
 SOURCE_APP="${1:-}"
 LOCATION="${2:-}"
 case "$SOURCE_APP" in
   dolphin|thunar|nautilus|cosmic) ;;
-  *) echo "usage: $0 <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
+  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
 esac
 case "$LOCATION" in
   outside|downloads) ;;
-  *) echo "usage: $0 <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
+  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
 esac
@@ (after zen_instance_killable)
+# Does any process inside this flatpak sandbox still reference the
+# gimbal-dnd profile?  (P0a-r3 review hardening 1 …)  flatpak's
+# child-pid is the bwrap line, which carries no -P flag (observed:
+# "bwrap --args 73 -- launch-script.sh"), and the sandboxed browser
+# re-launches itself with a bare cmdline — but it keeps its profile files
+# open, so the marker is looked for in cmdline, environ and the open-fd
+# links of every process in the instance's cgroup scope.
+zen_instance_has_marker() {
+  local inst="$1" cpid scope p
+  cpid="$(flatpak ps --columns=instance,child-pid 2>/dev/null \
+          | awk -v i="$inst" '$1==i{print $2}')"
+  [ -n "$cpid" ] || return 1
+  scope="$(awk -F: '{print $3}' "/proc/$cpid/cgroup" 2>/dev/null | tail -n1)"
+  [ -n "$scope" ] || return 1
+  for p in /proc/[0-9]*; do
+    [ -r "$p/cgroup" ] || continue
+    grep -qxF "0::$scope" "$p/cgroup" 2>/dev/null || continue
+    if tr '\0' ' ' < "$p/cmdline" 2>/dev/null | grep -q -- "gimbal-dnd" \
+       || tr '\0' '\n' < "$p/environ" 2>/dev/null | grep -q -- "gimbal-dnd" \
+       || ls -l "$p/fd" 2>/dev/null | grep -q -- "gimbal-dnd"; then
+      return 0
+    fi
+  done
+  return 1
+}
@@ (before the Zen launch)
+if [ -n "$AUTO" ]; then
+  # read-only niri snapshot for autodrag: it identifies the throwaway Zen
+  # window as a window id that is new after this launch …
+  GIMBAL_ZEN_WIN_BEFORE="$(niri msg --json windows 2>/dev/null \
+    | python3 -I -c 'import json,sys; print(",".join(str(w["id"]) for w in json.load(sys.stdin)))')" \
+    || GIMBAL_ZEN_WIN_BEFORE=""
+  export GIMBAL_ZEN_WIN_BEFORE
+fi
@@ (instance pick)
-  ZEN_INSTANCE="$(comm -13 "$RUNDIR/.zen-inst-before" "$RUNDIR/.zen-inst-now" | head -n1)"
-  if [ -n "$ZEN_INSTANCE" ]; then break; fi
+  NEW_INSTANCES="$(comm -13 "$RUNDIR/.zen-inst-before" "$RUNDIR/.zen-inst-now")"
+  if [ -n "$NEW_INSTANCES" ]; then break; fi
 done
-[ -n "$ZEN_INSTANCE" ] || die "the gimbal-dnd Zen instance never appeared …"
+[ -n "$NEW_INSTANCES" ] || die "the gimbal-dnd Zen instance never appeared …"
+if [ "$(printf '%s\n' "$NEW_INSTANCES" | grep -c .)" -gt 1 ]; then
+  ZEN_INSTANCE="$(printf '%s\n' "$NEW_INSTANCES" \
+    | while read -r inst; do
+        if zen_instance_has_marker "$inst"; then echo "$inst"; fi
+      done | head -n1)"
+  [ -n "$ZEN_INSTANCE" ] || die "several new app.zen_browser.zen instances appeared (…) and none references the gimbal-dnd profile; refusing to guess"
+  echo "several new Zen sandboxes seen; picked instance $ZEN_INSTANCE by the gimbal-dnd profile marker"
+else
+  ZEN_INSTANCE="$(printf '%s\n' "$NEW_INSTANCES" | head -n1)"
+fi
 if ! zen_instance_killable "$ZEN_INSTANCE" \
    || ! grep -qx "$ZEN_INSTANCE" "$RUNDIR/.zen-inst-now" \
    || grep -qx "$ZEN_INSTANCE" "$RUNDIR/.zen-inst-before"; then
-  die "refusing instance id '$ZEN_INSTANCE': not a new numeric app.zen_browser.zen instance"
+  # hardening 2: clear the variable BEFORE dying so the EXIT trap can never
+  # flatpak-kill a refused id
+  REFUSED_INSTANCE="$ZEN_INSTANCE"
+  ZEN_INSTANCE=""
+  die "refusing instance id '$REFUSED_INSTANCE': not a new numeric app.zen_browser.zen instance"
 fi
@@ (before the source launch)
+if [ -n "$AUTO" ]; then
+  # autodrag locates the file through AT-SPI.  Qt only builds its
+  # accessibility tree when this is set (the session's
+  # org.a11y.Status.IsEnabled is false — see NOTES-autodrag.md); it is
+  # harmless for the GTK and COSMIC apps.
+  export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
+fi
@@ (step 4)
+AUTO_RC=0
+if [ -n "$AUTO" ]; then
+  # --auto (P0a2): autodrag.py performs the drag a person would perform — …
+  GIMBAL_SRC_PID="$SRC_PID" GIMBAL_ZEN_INSTANCE="$ZEN_INSTANCE" \
+  GIMBAL_RUNDIR="$RUNDIR" GIMBAL_FOLDER="$FOLDER" \
+  python3 -I "$HERE/autodrag/autodrag.py" run \
+    --source "$SOURCE_APP" --location "$LOCATION" \
+    --file "${GIMBAL_AUTODRAG_FILE:-sample.png}" \
+    ${GIMBAL_AUTODRAG_STOP_AFTER:+--stop-after "$GIMBAL_AUTODRAG_STOP_AFTER"} \
+    || AUTO_RC=$?
+  if [ "$AUTO_RC" -ne 0 ]; then
+    echo "autodrag exited with code $AUTO_RC (see result.txt in the run directory)"
+  fi
+else
 cat <<EOF
 … (banner, read prompts, result.txt write — byte-identical, now inside else)
+fi
@@ (end of file)
+# propagate autodrag's exit code (--auto only; 0 in manual runs)
+exit "$AUTO_RC"
```

(`bash -n` passes; shebang still line 1, SPDX line 2.  The full
machine-generated diff is 199 lines; the block above elides only comment
wording.)

### Hardening 1 — deviation from the review's wording, with evidence

The review suggested picking the new instance "whose **child-pid command
line** (`/proc/<child-pid>/cmdline`, the bwrap line) contains
`-P gimbal-dnd`".  Observed on this machine today: flatpak's child-pid for
Kyle's Zen is the bwrap process and its cmdline is
**`bwrap --args 73 -- launch-script.sh`** — no profile flag anywhere
**[observed]** — and the sandboxed browser re-launches itself with the bare
cmdline `/app/zen/zen --name app.zen_browser.zen` **[observed]**.  The
marker therefore looks at every process in the candidate's cgroup scope:
cmdline, environ, and the **open-fd links** (the browser holds its profile
DBs open — Kyle's main process shows 71 fds under `.zen/dneqwyin.Default`,
the same mechanism that exposes `…/bt9lefdm.gimbal-dnd/…` for the
throwaway) **[observed]**.  With a single new instance the marker is not
required (r3 behavior preserved, since the marker can legitimately lag the
sandbox by seconds); with several new instances and no marker, run.sh dies
rather than guesses.

## Tests

`python3 -I -B test_vmouse.py && … test_niri.py && … test_locate.py &&
… test_autodrag.py` from `autodrag/` — **11 + 12 + 10 + 34 = 67 tests, all
OK** (pytest is not installed; unittest, matching P0a).  `-B` only avoids
`__pycache__`; `-I` is honored (the test files and autodrag.py insert their
own directory on sys.path, because `-I` deliberately withholds it — that
keeps "run everything with python3 -I" true without env vars or user site).

Coverage per the brief: coordinate math (logical→ABS incl. bounds and
rounding, window-relative + niri position + decoration offset); event
sequence (frame = events + SYN, press/motion/release order, release sent
when an exception is raised mid-drag, close() releases then destroys);
abort rules (focus change, moved window, resized window, two enabled
outputs, timeout — at unit level and integrated into run_real); title
classification (y/n/other); whole-run `--dry-run` output (asserted AND
saved above); plus: place()'s exact argv with `--id`, dry-run printing
instead of running, `--window-id` + `--focus false`,
focus-workspace-by-id→index translation, AT-SPI tree search on fake nodes,
fallback offsets (present/absent), result.txt contents, --stop-after paths,
the no-empty-workspace path, and zen-window discovery (cgroup instance
match + new-id fallback + Kyle's window never matching).

## What was NOT run (and why)

- No `/dev/uinput` open, no pointer motion, no clicks, no keyboard
  injection (hard limit 1; `wtype` unused per the brief).
- No `niri msg action …` of any kind (hard limit 2) — so: whether a
  floating window reports `tile_pos_in_workspace_view`, in which coordinate
  space, whether `focus-workspace`/`move-window-to-workspace` to
  `last_idx + 1` creates the workspace, and whether the actions accept the
  exact argv built here are all **unverified**.
- No real drag, no capture run, no matrix run.

## TODOs for Claude's live check

1. **Device classification.**  Run
   `python3 -I autodrag/vmouse.py --square` (real; it creates and destroys
   the device).  Watch `sudo libinput debug-events` (or niri's log) for the
   device: expect "device is a pointer" with abs axes per the udev
   ID_INPUT_MOUSE path above, and the cursor tracing the square.  If niri
   gives the device to a different tool (e.g. tablet), record what it chose
   and whether absolute motion still moves the cursor.
2. **ABS mapping calibration.**  With the square running, check the cursor
   lands at the printed logical pixels (e.g. corner at 1028,652 on
   2256×1504).  If there is a linear offset (e.g. niri maps ABS to
   desktop-with-struts), adjust `logical_to_abs`/document it.
3. **Floating-window readback + coordinate space.**  Run
   `GIMBAL_AUTODRAG_STOP_AFTER=placement ./run.sh --auto dolphin outside`,
   then `niri msg --json windows`: do the two windows report
   `is_floating: true` with `tile_pos_in_workspace_view` equal to the
   commanded (40,40)/(1148,40)?  Compare against where the windows visually
   are relative to the Noctalia bar: if view coords are offset from output
   coords, set `view_origin_in_output` in apps.toml (this also fixes the
   pointer math, which uses output pixels).
4. **Workspace actions.**  Confirm `focus-workspace <idx>` and
   `move-window-to-workspace --window-id … --focus false <idx>` behave as
   read from the help, and what happens with `<last_idx + 1>` when no empty
   workspace exists (create-on-demand or error).
5. **Selection click + calibration for dolphin.**  Run
   `--stop-after select`: does the item report STATE_SELECTED (runner
   passes) or does the click miss (runner aborts)?  If it misses, measure
   the offset and set `window_offset_correction` for dolphin (expected
   ≈ SSD titlebar/border for the floating geometry).
6. **thunar/nautilus/cosmic fallback calibration.**  For each: with
   `--stop-after select` (or manually), find the first item in the view and
   fill `first_item_offset` in apps.toml.  Optional (Kyle's call, it is
   session-wide): flip `org.a11y.Status.IsEnabled` to true and re-probe —
   if thunar/nautilus then expose their items, set `atspi_items = true`
   and the fallback becomes unnecessary.  cosmic-files exposes nothing
   either way (accesskit, not AT-SPI — not investigated further).
7. **Zen drop point.**  After a successful placement, verify
   `drop_point_fraction = [0.5, 0.55]` lands in the about:blank content
   area below the toolbar; adjust for Zen's actual chrome.
8. **Full run + matrix.**  One `./run.sh --auto dolphin outside` end to
   end (title classification may need the real about:blank title), then
   `./run-matrix.sh`.
9. **20 s budget.**  If window-wait + startup pushes runs past 20 s, decide
   whether the abort clock should start at the first pointer press instead
   of at runner start (one-line change in run_real; left per brief).

## Review of P0a2 (Claude, Oct 31, 2026)

**Verdict: accepted with corrections (P0a2-r1).** The structure is right:
run.sh does the launching and autodrag is called from it, `--dry-run`
runs no actions, the device declares only ABS_X/ABS_Y + BTN_LEFT, and the
button is released in a `finally`. Spot-checked and correct: uinput.h :64
/:100/:135/:137, input-event-codes.h :24/:357/:871, input.h :257, and the
`--help` for all five niri actions (`--id`, `--window-id`, `--focus`, `-x/-y`,
positional `<CHANGE>`/`<REFERENCE>`). `bash -n` passes on both scripts.
The tests pass: 34 + 10 + 12 + 11 = 67 OK, and test_summarize.py is still OK.
No probe processes were left behind, and Kyle's Zen (2943722691) is
untouched. The hardening-1 deviation (look for the marker in every process
of the cgroup scope, because the bwrap cmdline carries no `-P`) is accepted
with its evidence, and so is hardening 2.

### Corrections (must fix before the live check)

1. **`target_workspace` relies on a field niri doesn't have.** niri
   26.04's workspaces JSON has no `is_empty`; your own field list in this
   file says so. Run against the real JSON (Oct 31: ids 1, 5, 2, 6; id 6 is
   idx 4 with `active_window_id: null`), it returns `None` every time, so
   every run takes the "last_idx + 1, creation-on-demand" path. In that path
   the first `move-window-to-workspace 5` most likely lands on idx 4, niri
   then appends a new empty idx 5, and the second move goes there. The two
   windows end up on different workspaces. verify_layout doesn't catch this,
   because `workspace_id` is None on that path.
   Fix: a workspace counts as empty when `active_window_id` is null **and**
   no entry in `windows()` has that `workspace_id`. Target the empty one
   with the highest idx on the enabled output. If there isn't one,
   **abort**; don't rely on creation-on-demand. Rebuild every fake
   workspace/window JSON in the tests from real `niri msg --json` output
   (paste one sample into the test file, with a comment saying where it came
   from) and use no invented keys.

2. **The selection click and the drag press can register as a
   double-click.** From the selection click's release to the drag's press,
   the code waits `select_settle_ms` (300) plus an abort check (about 30 ms)
   and, for Dolphin, an AT-SPI walk. Apps without AT-SPI get about 330 ms
   at the same spot. That's under the GTK and Qt default double-click time
   (400 ms), so the file opens in the default image viewer: a window the
   runner didn't start. Fix: add `click_to_drag_gap_ms = 1000` to apps.toml
   and make the code wait until at least that long has passed since the
   release (measured with the clock, not added on top of other sleeps)
   before the drag press. Test it with the fake clock.

3. **The uinput device gets no time to settle.** `UinputDevice.__init__`
   returns right after `UI_DEV_CREATE`, and the first `move_to` follows
   immediately. libinput/niri add the device asynchronously through udev,
   so those first frames can be lost, and the press then lands wherever
   Kyle's real pointer is. Fix: after create, read the sysfs name with
   `UI_GET_SYSNAME(len)` (uinput.h:161, `_IOC(_IOC_READ, 'U', 44, len)`),
   wait until `/sys/devices/virtual/input/<name>/` has an `event*` child
   (timeout 3 s, then abort), then sleep `device_settle_ms = 500`. Before
   `UI_DEV_DESTROY`, sleep `pre_destroy_ms = 100` after the last release.
   Make both sleeps injectable so the tests don't wait. `--square` also
   needs a pause at each corner (`--dwell-ms`, default 400) so the cursor
   can be seen.

4. **`view_origin_in_output` is declared but never used.** apps.toml
   defines it, but no code reads it. The click point uses
   `tile_pos_in_workspace_view` (view coordinates) as if it were screen
   coordinates, so turning that knob during calibration would do nothing.
   Fix: convert in one function (`screen = view + view_origin_in_output`),
   use it for the click point and the drop point, and add a test with a
   non-zero origin. `travel_ms` is also unused: either derive the step
   interval from it, or delete it.

5. **The "before" title is read too early.** `title_before` is taken when
   Zen's window first appears, which is while Zen is still starting up.
   If its title changes during placement (for example a start page
   finishing loading), a drag that did nothing gets classified `other`
   instead of `n`. Fix: read the title again right before the drag press
   and use that. After the first change, wait 500 ms and read it once more,
   so a passing title isn't the one that gets classified. Add tests for both.

6. **Hard limit 3: the new-window-ID fallback could pick Kyle's Zen.**
   `find_zen_window` falls back to "new ID with a zen-like app_id" in the
   same poll as the cgroup check. If any window of Kyle's Zen (instance
   2943722691) opens before the throwaway's window, it gets picked, and the
   runner then floats and moves it. Fix: skip any fallback candidate whose
   `/proc/<pid>/cgroup` names an `app.zen_browser.zen-<n>.scope` other than
   ours. The fallback is then only used when the cgroup can't be read. Test:
   a new window of Kyle's Zen appears in an earlier poll than ours and is
   never chosen.

### Minor

7. Once the `finally` in `run_real` starts, set SIGINT/SIGTERM to `SIG_IGN`
   so a second Ctrl-C can't raise out of `vm.close()`.
8. `--stop-after` restores the workspace straight away. Add
   `--hold-s <n>` (and pass it through from run.sh as
   `GIMBAL_AUTODRAG_HOLD_S`) to wait n seconds before cleanup, after the
   device is destroyed, so Claude can read `niri msg --json windows` and
   look at the layout.
9. run-matrix.sh finds a row's run directory with `ls -td …-$app-$loc |
   head -1`. If run.sh dies before creating its directory, the matrix
   reads an **older** run's result.txt. Snapshot the matching directories
   before each row and only accept a new one.

### Not corrections (live check, as you listed)

The ABS mapping, floating readback and its coordinate space, whether
`set-window-*` sizes the tile or the window (Kyle has no border, so they
are equal today), Dolphin's `STATE_SELECTED`, the fallback offsets and the
Zen drop point all stay in TODOs 1–9. One edge case to note in the TODOs:
if Kyle starts the run from the empty end workspace, niri removes it once
it's empty and unfocused. `cleanup_workspace` then logs a warning and
leaves Kyle on the test workspace, which is acceptable.

## P0a2-r1 changes

Corrections 1–6 and minor items 7–9 from the review above were applied.
Same limits as P0a2: no `/dev/uinput`, no `niri msg action`, no run.sh /
run-matrix.sh executions, no git, no sudo.  Only read-only `niri msg --json`
calls were run against the live session.  Tags as before.

### Correction 1 — empty-workspace detection without `is_empty`

`target_workspace(workspaces, windows, output_names)` now decides
emptiness exactly as the review required: `active_window_id` is null **and**
no window in `windows()` carries that workspace id; the empty workspace with
the **highest idx on the enabled output(s)** is targeted; when there is
none, `run_real` **aborts** (`aborted: no empty workspace at the end …`) —
the `last_idx + 1`, creation-on-demand path is gone, and with it the
split-workspace failure the review described.  `verify_layout` now always
compares the workspace id (the None path no longer exists).

Every fake workspace/window fixture was rebuilt from real `niri msg --json`
output read **2026-10-31** and pasted verbatim into `test_niri.py`
(`REAL_WORKSPACES`, `REAL_WINDOW`, `REAL_WINDOW_WS_IDS`, with the provenance
comment); no fixture contains a key niri doesn't produce (`is_empty` is gone
everywhere — grep-checked).  The real sample is two-output (DP-2 **and** DP-3
were enabled that day — see below), which makes the output filter testable
with real data: id 13 (DP-3, idx 3) is empty at a higher idx than id 6
(DP-2, idx 2), and `target_workspace(..., {"DP-2"})` must return id 6.

Note on the review's numbers: the review read "ids 1, 5, 2, 6; id 6 is
idx 4" earlier on Oct 31; by the time I read the session (same day), Kyle
had closed workspaces and turned DP-3 on — the sample I pasted shows id 6
at **idx 2**, plus id 13 (idx 3, DP-3) empty.  The "Done means" target —
`target_workspace` returning **id 6** on the real output read today —
holds **[observed]**:

```
enabled outputs: ['DP-2', 'DP-3']          # live, 2026-10-31
target on enabled outputs: (13, 3, 'DP-3') # both outputs enabled (unreachable at run time: the two-output abort fires first)
target DP-2 only: (6, 2)                   # the single-output case the runner is built for
```

Also observed 2026-10-31: DP-2 is now the LG ULTRAGEAR at **2560×1440** (was
2256×1504 on Oct 30), so the dry-run default size and vmouse's offline
fallback were updated to 2560×1440; the run itself always reads the live
size.  With both outputs enabled a run aborts immediately ("expected exactly
one enabled output, found 2") — correct per hard limit 6; the matrix can
only run when Kyle is single-output.

### Correction 2 — click_to_drag_gap_ms

`apps.toml [drag] click_to_drag_gap_ms = 1000`.  `perform_click` returns
the clock time of the release; before the drag press,
`wait_click_gap` sleeps only the **remaining** amount (measured, not
stacked): the AT-SPI recheck and abort checks count toward the gap.  A
double-click would open the file in an external viewer — a window the
runner didn't start — so this is between-steps safety.  Tests: with and
without the AT-SPI recheck, the fake clock shows the gap is exactly
1000 ms (asserted `>= 1.0` and `<= 1.05`).

### Correction 3 — device readiness before the first frame

`UinputDevice` now, after `UI_DEV_CREATE`: reads the sysfs name via
`UI_GET_SYSNAME(64)` — uinput.h:161, computed as
`_ioc(_IOC_READ, 'U', 44, 64)` (`ui_get_sysname()`, never hand-written;
the test asserts the hand-expanded value 0x8040552C) — then polls
`/sys/devices/virtual/input/<name>/` for an `event*` child (timeout 3 s →
RuntimeError, which run_real records as an abort), then sleeps
`device_settle_ms = 500` (apps.toml [vmouse]).  `destroy()` sleeps
`pre_destroy_ms = 100` **before** `UI_DEV_DESTROY`.  Every external effect
is injectable (opener/ioctl_fn/write_fn/closer/sleep/clock/listdir +
sysfs_root), so the tests never touch the kernel and never wait — they
drive the sysfs check through a tempdir and record the sleep/ioctl order:
create → sysname → settle 0.5 s → … → pre-destroy 0.1 s → destroy ioctl →
close.  `--square` gained `--dwell-ms` (default 400): a pause at each of
the 5 corner visits so the cursor is visible; the dry-run never sleeps.

### Correction 4 — view_origin_in_output is now used; travel_ms drives the pace

One function, `view_to_screen(xy, cfg)` (`screen = view +
view_origin_in_output`), is applied to **both** the click point and the
drop point (and `run_dry` prints the origin it would apply).  A test with
`view_origin_in_output = [0, 10]` shows click (419,174)→(419,184) and drop
(1682,823)→(1682,833) in result.txt.  `step_interval_ms` is deleted from
apps.toml; the per-step interval is derived from `travel_ms`
(`travel_ms / travel_steps`), so the travel really takes about travel_ms —
tested with 900 ms (30 steps × 30 ms + 400 ms hold).

### Correction 5 — the "before" title is re-read, and settled

`title_before` is read again **right before the drag press** (after the
click gap), so a title that changed while Zen finished starting up is the
baseline.  `wait_for_title` waits **500 ms after the first change** and
reads once more — the settled read is what gets classified.  Tests: a
title change during placement (modeled: after 5 placement actions) now
classifies `n` with `title_before: Start Page — Zen Browser` (was `other`
before the fix); a transient first change (timeline switch at 2.5 s) with
the settled title arriving during the 500 ms is classified from the
settled read.

### Correction 6 — the new-window-id fallback cannot pick Kyle's Zen

The fallback now skips any candidate whose `/proc/<pid>/cgroup` names an
`app.zen_browser.zen-<n>.scope` other than ours (`zen_scope_instance()`
parses the id), so it is only reachable for windows whose cgroup cannot be
read or names our own scope.  Tests: unit — Kyle's window (scope …
-2943722691…, listed **before** ours, i.e. from an earlier poll) is skipped
both when our cgroup is readable (primary path) and when ours is unreadable
(fallback path, the only case the fallback can now win); run-level — with
Kyle's window present from the first poll, the runner floats exactly
windows [100, 200] and never 300.

### Minor 7 — SIG_IGN around vm.close()

The `finally` sets SIGINT/SIGTERM to `SIG_IGN` before `vm.close()` and
restores the previous handlers right after it, so a second Ctrl-C cannot
raise out of the release/destroy while the `--hold-s` wait and the
workspace restore stay interruptible (tested: the handler is not SIG_IGN
after a completed run).

### Minor 8 — --hold-s

`autodrag.py run --hold-s <n>` (float seconds) sleeps **after the device
is destroyed and before the workspace cleanup**, so the live check can
read `niri msg --json windows` against the placed layout.  run.sh passes
`GIMBAL_AUTODRAG_HOLD_S` through as `--hold-s` (the only run.sh change;
`bash -n` passes), and standalone `autodrag.py run … --hold-s` execs
run.sh --auto with the env set.  README's stop-after block documents the
knob.  Test: the run with `--hold-s 1.5` takes exactly 1.5 s longer on the
fake clock and logs the wait.

### Minor 9 — run-matrix.sh only accepts a NEW run directory

Each row snapshots `ls -d "$CAPS/"*-$app-$loc` before the run and computes
the row's run directory as the `comm -13` difference afterwards; if run.sh
died before creating one, the matrix reports "no new run dir" instead of
reading an older row's result.txt.  `bash -n` passes.

### Tests

`python3 -I -B test_*.py` from `autodrag/` (the `-B` only skips
`__pycache__`), plus P0a's suite for regression:

```
== test_vmouse
Ran 18 tests in 0.002s
OK
== test_niri
Ran 16 tests in 0.001s
OK
== test_locate
Ran 10 tests in 0.001s
OK
== test_autodrag
Ran 46 tests in 0.005s
OK
test_summarize (P0a regression): OK (42 tests)
bash -n run.sh run-matrix.sh: both OK
```

### New dry-run output

`python3 -I -B autodrag/vmouse.py --square --dry-run` (reads today's DP-2 live):

```
output: 2560x1440 logical px, ABS range 0..65535, mapping abs = px*65535/extent
device would declare: EV_ABS ABS_X/ABS_Y 0..65535, EV_KEY BTN_LEFT, EV_SYN (no props, no keys)
move_to (1180, 620) -> ABS (30208, 28216)
move_to (1380, 620) -> ABS (35327, 28216)
move_to (1380, 820) -> ABS (35327, 37319)
move_to (1180, 820) -> ABS (30208, 37319)
move_to (1180, 620) -> ABS (30208, 28216)
no button events: 5 frames, all EV_KEY counts: 0
```

`python3 -I -B autodrag/autodrag.py run --source dolphin --location outside
--file sample.png --dry-run` (travel steps elided — they are the same
lerp series as before):

```
autodrag --dry-run: nothing is run, launched or moved
plan: source=dolphin location=outside file=sample.png
output: 2560x1440, margin 40
boxes: source=(40, 40, 1220, 1360) zen=(1300, 40, 1220, 1360)

planned niri commands (ids are read at runtime):
  niri msg action move-window-to-floating --id '<source-window-id>'
  niri msg action move-window-to-floating --id '<zen-window-id>'
  niri msg action move-window-to-workspace --window-id '<source-window-id>' --focus false '<empty-end-workspace-idx>'
  niri msg action move-window-to-workspace --window-id '<zen-window-id>' --focus false '<empty-end-workspace-idx>'
  niri msg action focus-workspace '<empty-end-workspace-idx>'
  niri msg action move-floating-window --id '<source-window-id>' -x 40 -y 40
  niri msg action set-window-width --id '<source-window-id>' 1220
  niri msg action set-window-height --id '<source-window-id>' 1360
  niri msg action move-floating-window --id '<zen-window-id>' -x 1300 -y 40
  niri msg action set-window-width --id '<zen-window-id>' 1220
  niri msg action set-window-height --id '<zen-window-id>' 1360
  niri msg action focus-window --id '<source-window-id>'
  # afterwards: verify layout via `niri msg --json windows`
  # on cleanup: niri msg action focus-workspace <Kyle's ws idx, looked up by id>

planned pointer events (logical px -> ABS):
  item point: located at runtime via AT-SPI (apps.toml atspi_items=True); fallback offset: None
  view->screen: + [0, 0] (view_origin_in_output, applied to click and drop)
  drop point: (1910, 788) (zen box + fraction [0.5, 0.55])
  click:  move_to <item point>; press (EV_KEY BTN_LEFT 1 + SYN); release (EV_KEY BTN_LEFT 0 + SYN)
  verify: STATE_SELECTED via AT-SPI (unverified when no AT-SPI)
  gap:    >= 1000 ms since the click release before the drag press (click_to_drag_gap_ms; no double-click)
  drag:   press (BTN_LEFT 1 + SYN)
    move_to item + (+3, +3)  # past the drag threshold
    … (4 threshold steps) …
  lerp(item, drop, t) = item + (drop - item) * t
    move_to lerp(item, drop, 1/30)  # ~20 ms per step
    … (30 steps) …
  hold 400 ms, then release (EV_KEY BTN_LEFT 0 + SYN)

classify: title unchanged -> n; contains 'sample.png' -> y; else other
abort rules: focused window not ours | window moved/resized | >1 enabled output | run > 20 s
```

(run_dry's plan deliberately still says the item point is located at
runtime — the dry-run reads nothing.)

### Still open (unchanged live-check TODOs 1–9 from the P0a2 section)

The ABS mapping (now against 2560×1440), floating readback and its
coordinate space, `set-window-*` semantics, Dolphin's `STATE_SELECTED`,
the fallback offsets and the Zen drop point remain the live check, plus
the review's note: if Kyle starts the run **from** the empty end
workspace, niri removes it once empty and unfocused — `cleanup_workspace`
logs a warning and leaves Kyle on the test workspace (acceptable).

## Review of P0a2-r1 (Claude, Oct 31, 2026)

**Verdict: accepted. P0a2 is done** apart from the live check. Re-run here:
test_vmouse 18, test_niri 16, test_locate 10 and test_autodrag 46 are all
OK, and test_summarize is still OK. `bash -n` passes on both scripts.
`is_empty` now appears only in comments. No stray file-manager processes,
and Kyle's Zen (2943722691) is untouched.

Checked against the code and live data:

1. On today's real JSON, `target_workspace` returns id 6 (idx 2) for
   DP-2 and id 13 (idx 3) for DP-3. The creation-on-demand path is gone,
   and the run aborts when there's no empty workspace. The pasted sample
   has only keys niri produces.
2. `wait_click_gap` sleeps only the time remaining since the release, so
   the AT-SPI recheck counts toward the gap. 1000 ms is in apps.toml.
3. `UI_GET_SYSNAME(64)` is computed with `_ioc` (0x8040552C, checked by
   hand). The device waits for `event*` under
   `/sys/devices/virtual/input/<name>/`, settles 500 ms, and sleeps 100 ms
   before destroy. Each step is injectable, and on timeout the fd is closed.
4. `view_to_screen` is the only conversion, and both the click point and
   the drop point go through it. The step interval now comes from
   `travel_ms`.
5. The title is re-read after the click gap, right before the press, and
   the final title is read again 500 ms after the first change.
6. A fallback candidate whose cgroup names another Zen scope is skipped.
7–9. Signals are ignored only around `vm.close()`. `--hold-s` is passed
   through by run.sh. run-matrix.sh only accepts a run directory that's new
   (`comm -13` on sorted lists).

Two small things, neither blocking (fix them if the file is touched again):
the missing-test-file `Abort` is raised before the `try`, so it comes out
as a traceback with no result.txt. And a Ctrl-C during `--hold-s` or the
workspace restore escapes the `finally`, which also means no result.txt.
The button is already released and the device destroyed by then, so
neither one matters for safety.

**Blocker for the live check (not the agent's fault):** DP-3 (Lenovo
L24q-20, 2560×1440 at x=2560) is enabled now as well as DP-2 (LG
ULTRAGEAR, now 2560×1440). Hard limit 6 aborts every run with two enabled
outputs. Either Kyle turns DP-3 off while the runs happen, or a later
round adds multi-output support. That support would map ABS over the
whole layout's bounding box, which is what niri is expected to do for an
unmapped absolute pointer (to be confirmed live), and offset by the
target output's logical position.

## Live check (Claude, Oct 31, 2026)

Setup: DP-3 turned off by Kyle. DP-2 (LG ULTRAGEAR) is 2560×1440 at scale
1.0, and the Noctalia bar is 34 px tall. Kyle's Zen (instance 2943722691)
and Kyle's Dolphin (pid 130898) kept running throughout. After every run,
`flatpak ps` listed only them plus OrcaSlicer, and no test process was left.

### Found and fixed live

1. **The uinput device couldn't be created** (`EINVAL` on `UI_SET_EVBIT`).
   `UI_SET_EVBIT/KEYBIT/ABSBIT` take the int value as the ioctl argument
   itself. The code passed `struct.pack("=i", …)`, which Python sends as a
   pointer. vmouse.py now passes the int. The fakes couldn't catch this.
2. **The throwaway Zen opened "Open Zen in Troubleshoot Mode?"** instead
   of a browser window. Each run's `flatpak kill` counts as an unfinished
   startup. run.sh now rewrites `user.js` in the gimbal-dnd profile only,
   on every run, with: `toolkit.startup.max_resumed_crashes = -1`,
   `browser.sessionstore.resume_from_crash = false` and
   `browser.shell.checkDefaultBrowser = false`. The profile path comes from
   profiles.ini, and run.sh dies if it can't be found.
3. **Working-area offset.** `move-floating-window -x 40 -y 40` puts the
   window at output (40, 74). Commands are relative to the working area
   below the bar, but `tile_pos_in_workspace_view` reads back in output
   coordinates (the screenshot shows the window's top edge at y 74). The
   new `[workspace] working_area_offset = [0, 34]` is added to the boxes
   used by the layout check and the drop point, and taken off the usable
   height. `view_origin_in_output` stays `[0, 0]`, because the click point
   is built from the readback, which is already in output coordinates.
4. **Resize is asynchronous.** Zen still read 1272×1398 right after the
   size commands. `wait_layout` now retries `verify_layout` for up to 3 s.
5. The `[autodrag]` log is printed with `flush=True`, because `-I` ignores
   `PYTHONUNBUFFERED` and the log only appeared once the run ended. locate.py
   now calls `gi.require_version("Atspi", "2.0")`.

Tests after these changes: test_autodrag 49, test_locate 10, test_niri 16
and test_vmouse 18 are all OK. The new tests cover the offset run (click
(419, 208), drop (1682, 838), height 1390), and `wait_layout` retrying and
giving up. The existing coordinate tests pin the offset to [0, 0], and the
dry-run test expects the calibrated values.

### TODOs 1–9

1. **Device type:** udev gives it `ID_INPUT=1` and `ID_INPUT_MOUSE=1`, as
   predicted. niri moves the cursor with it.
2. **ABS mapping:** exact. With the cursor shown in screenshots, the tip
   lands on (1180, 620), (1380, 820), (10, 10) and (2549, 1429).
3. **Floating readback:** reported, in output coordinates (see 3 above).
4. **Workspace actions:** these work as read from `--help`. The target was
   the empty end workspace each time (ids 6, 14, 15 … 19, always idx 4),
   and focus went back to Kyle's workspace id 11.
5. **Dolphin:** `window_offset_correction = [0, 0]` is right for the
   floating window. The click at (419, 208) gave `STATE_SELECTED`, which
   the screenshot confirms.
6. **Fallback offsets,** measured from screenshots (window-relative center
   of sample.png's icon): Thunar (238, 146), which is a 16×16 icon;
   Nautilus (333, 117), a grid thumbnail; COSMIC (335, 141), list view.
   `--stop-after select` screenshots show sample.png highlighted in all
   three, and no viewer opened.
7. **Zen drop point:** `[0.5, 0.55]` → (1910, 803) is in the page area,
   right of Zen's sidebar.
8. **Full run:** `./run.sh --auto dolphin outside` →
   `zen_opened_file: y`, raw title "sample.png (PNG Image, 16 × 16 pixels)
   — Zen Browser". Capture `20261031-065118-dolphin-outside`: Dolphin
   offers text/uri-list, x-kde4-urilist, vnd.portal.filetransfer and
   x-kde-source-id; Zen received through the portal
   (RetrieveFiles → /run/user/1000/doc/7fb4b484/sample.png).
9. **20 s budget:** every run finished well inside it. Zen and the window
   wait take about 3 s, and launching is done in run.sh before the clock
   starts.

Runs `20261031-064710` and `064824` (placement attempts before fixes 2–4),
`065042` and the three `0651xx`/`0652xx` placement/select runs are
calibration runs. Their result.txt says `aborted`, so P0b should ignore them.

**Status: the runner is calibrated. Kyle can run `./run-matrix.sh`**
with DP-3 off, Nautilus not running, and hands off the mouse for about
4 minutes.

## P1g changes

P1g adds a fifth source, `gimbal` (the Phase 1 GTK4 spike), the extra drag
cases for the acceptance run (folder, symlink, several files), and a
source-integrity check for **every** source. Only the P1g Output files were
written: `run.sh`, `make-test-files-gimbal.sh`,
`run-matrix-gimbal.sh`, `autodrag/autodrag.py`, `autodrag/apps.toml`,
`autodrag/test_autodrag.py`, `README.md` and this section. No Phase 0 test
folder was touched; the two new folders were created by the new script and
their contents reported below. Tags as in the earlier notes.

### The spike's AT-SPI tree — **[observed]**

The brief's question was whether the spike's GTK4 list exposes its rows.
It does (GridView, not a ListView in the current P1c/P1e code). Probe:
copy the test folder to `/tmp/gimbal-spike-p1g-probe`, launch the release
binary at it, match the AT-SPI application by pid, walk the tree, then
`SIGTERM` the pid and wait for it to disappear. No click, no pointer.

```
$ "$PROJ/spike/target/release/gimbal-spike" /tmp/gimbal-spike-p1g-probe &
$ python3 -I ... (Atspi.get_desktop(0), app.get_process_id() == pid, BFS)
desktop children: 8
app[7] name='gimbal-spike' pid=1746385
sample* hits (8):
  ('sample-dir',       'table cell', (0,   46, 141, 94), selected=False, selectable=True)
  ('sample-link.png',  'table cell', (141, 46, 141, 94), selected=False, selectable=True)
  ('sample.png',       'table cell', (282, 46, 141, 94), selected=False, selectable=True)
  ('sample.txt',       'table cell', (423, 46, 141, 94), selected=False, selectable=True)
```

Each row also has a child `label` with the display name. The app had 50
nodes. A second probe launched with `--select sample.png,sample.txt`: both
`table cell`s reported `SELECTED=True`, confirming the multi-file hook
before any pointer event. Sort order is folders first, then display name
(case-insensitive): `sample-dir`, `sample-link.png`, `sample.png`,
`sample.txt` — matching `listing.rs`'s sorter.

Consequence for `apps.toml`: `[app.gimbal] atspi_items = true`, so the
runner clicks via AT-SPI and `item_offsets` stays an unset fallback
(`[]`, because TOML has no `null`; a missing/empty value still aborts with
`unconfigured`). `window_offset_correction` is `[0, 0]` as for the other
GTK4 app (Nautilus); the live check confirms it.

One unrelated process was already running before the probe:
`gimbal-spike --select a.txt /tmp/gimbal-spike-p1c` (pid 1728629, P1c's
work). It was left alone; the probes are `pgrep -af gimbal-spike`-checked
afterwards and no probe process remained.

### Test files of its own — **[observed]**

```
$ ./make-test-files-gimbal.sh
folder: /home/kyle/gimbal-dnd-test-gimbal
created: .../sample.png (92 bytes)
created: .../sample.txt (31 bytes)
created: .../sample-dir/
created: .../sample-dir/inside.txt (47 bytes)
created: .../sample-link.png -> sample.png (relative symlink)
folder: /home/kyle/Downloads/gimbal-dnd-test-gimbal
... same five entries ...
```

Running it again prints `exists, left untouched:` for every entry; nothing
is overwritten. `sample.png`/`sample.txt` use the same generator bytes as
`make-test-files.sh` (copied PNG code, same text).

### `run.sh` — the complete diff

No separate copy was saved before editing (the brief reached me after the
file had been read); this diff is against the pre-edit 372-line file
reconstructed verbatim from that read, and `bash -n` passes on both sides.
The only changes for the four Phase 0 sources are the new
`source_intact` block and `sha256sum` in the precheck tool list.

```diff
--- run.sh (before P1g)
+++ run.sh (P1g)
@@ -2,12 +2,19 @@
 # SPDX-License-Identifier: GPL-3.0-or-later
 # Record one Wayland drag-and-drop run: <source app> -> Zen browser tab.
 #
-#   usage: run.sh [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>
+#   usage: run.sh [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>
 #
 # With --auto, the drag itself is performed by autodrag/autodrag.py (P0a2):
 # the launch, capture and cleanup below are unchanged, only the "press
 # Enter" step and the y/n question are replaced.
 #
+# `gimbal` is the Phase 1 GTK4 spike (P1g): the binary at
+# $PROJ/spike/target/release/gimbal-spike, the test folders
+# ~/gimbal-dnd-test-gimbal and ~/Downloads/gimbal-dnd-test-gimbal, and
+# GIMBAL_AUTODRAG_SELECT passed through as --select.  Every source gets a
+# source-integrity check: the dragged files are hashed before the drag and
+# again after cleanup, and result.txt gets `source_intact: yes|no`.
+#
 # Captures three streams into a run directory under $CAPS:
 #   source.log  - WAYLAND_DEBUG=1 stderr of the source file manager
 #   zen.log     - WAYLAND_DEBUG=1 (and MOZ_LOG) stderr of a throwaway-profile Zen
@@ -19,7 +26,10 @@
 #   GIMBAL_MOZ_LOG_MODULE  MOZ_LOG module for GTK drag logging (default
 #                          WidgetDrag; P0g confirms the exact name from
 #                          Firefox source)
-#   GIMBAL_AUTODRAG_FILE  (--auto) file to drag (default sample.png)
+#   GIMBAL_AUTODRAG_FILE  (--auto) file or comma-separated list to drag
+#                          (default sample.png)
+#   GIMBAL_AUTODRAG_SELECT  (gimbal) comma-separated names passed to
+#                          `gimbal-spike --select` (multi-file rows)
 #   GIMBAL_AUTODRAG_STOP_AFTER  (--auto) passed to autodrag.py
 #                          --stop-after <placement|select>
 #   GIMBAL_AUTODRAG_HOLD_S  (--auto) passed to autodrag.py --hold-s: seconds
@@ -40,29 +50,42 @@
 SOURCE_APP="${1:-}"
 LOCATION="${2:-}"
 case "$SOURCE_APP" in
-  dolphin|thunar|nautilus|cosmic) ;;
-  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
+  dolphin|thunar|nautilus|cosmic|gimbal) ;;
+  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>" >&2; exit 2 ;;
 esac
 case "$LOCATION" in
   outside|downloads) ;;
-  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic> <outside|downloads>" >&2; exit 2 ;;
+  *) echo "usage: $0 [--auto] <dolphin|thunar|nautilus|cosmic|gimbal> <outside|downloads>" >&2; exit 2 ;;
 esac
 
 die() { echo "run.sh: $*" >&2; exit 1; }
 
 # ---------------------------------------------------------------- prechecks
-for tool in dbus-monitor busctl flatpak python3 gzip; do
+for tool in dbus-monitor busctl flatpak python3 gzip sha256sum; do
   command -v "$tool" >/dev/null || die "$tool not found"
 done
 [ -n "${WAYLAND_DISPLAY:-}" ] || die "WAYLAND_DISPLAY is not set; run from inside the niri session"
 
-case "$LOCATION" in
-  outside)  FOLDER="$HOME/gimbal-dnd-test" ;;
-  downloads) FOLDER="$HOME/Downloads/gimbal-dnd-test" ;;
-esac
-for f in "$FOLDER/sample.png" "$FOLDER/sample.txt"; do
-  [ -e "$f" ] || die "missing test file $f — run tools/dnd-capture/make-test-files.sh first"
-done
+if [ "$SOURCE_APP" = gimbal ]; then
+  SOURCE_BIN="$PROJ/spike/target/release/gimbal-spike"
+  [ -x "$SOURCE_BIN" ] \
+    || die "gimbal-spike not found at $SOURCE_BIN — build it with cargo build --release in spike/"
+  case "$LOCATION" in
+    outside)  FOLDER="$HOME/gimbal-dnd-test-gimbal" ;;
+    downloads) FOLDER="$HOME/Downloads/gimbal-dnd-test-gimbal" ;;
+  esac
+  for f in "$FOLDER/sample.png" "$FOLDER/sample.txt"; do
+    [ -e "$f" ] || die "missing test file $f — run tools/dnd-capture/make-test-files-gimbal.sh first"
+  done
+else
+  case "$LOCATION" in
+    outside)  FOLDER="$HOME/gimbal-dnd-test" ;;
+    downloads) FOLDER="$HOME/Downloads/gimbal-dnd-test" ;;
+  esac
+  for f in "$FOLDER/sample.png" "$FOLDER/sample.txt"; do
+    [ -e "$f" ] || die "missing test file $f — run tools/dnd-capture/make-test-files.sh first"
+  done
+fi
 
 # Thunar and Nautilus hand drags off to an already-running instance, which
 # would leave source.log empty.  Both are verified to do this; neither has a
@@ -275,6 +298,13 @@
             # cosmic-files daemonizes when stdin is a terminal (src/lib.rs),
             # which would orphan the real process and break PID cleanup.
             SOURCE_ARGS=(--no-daemon) ;;
+  gimbal)   SOURCE_BIN="$PROJ/spike/target/release/gimbal-spike"
+            SOURCE_ARGS=()
+            # the multi-file rows preselect their names in the spike; a click
+            # from the runner would reset the selection to one item
+            if [ -n "${GIMBAL_AUTODRAG_SELECT:-}" ]; then
+              SOURCE_ARGS=(--select "$GIMBAL_AUTODRAG_SELECT")
+            fi ;;
 esac
 WAYLAND_DEBUG=1 "$SOURCE_BIN" "${SOURCE_ARGS[@]}" "$FOLDER" > "$RUNDIR/source.log" 2>&1 &
 SRC_PID=$!
@@ -285,6 +315,43 @@
   echo "$SOURCE_BIN running: PID $SRC_PID"
 fi
 
+# ---------------------------------------------------------------- source integrity
+# P1g, every source: hash each dragged item before the drag and again after
+# cleanup.  A symlink is recorded by its link target (readlink), not by the
+# target's bytes; a folder is recorded as every file under it.  This checks
+# the "never delete on drag-end" rule directly: Phase 0 saw every drop into
+# Zen report MOVE, so a vanished source would show up here.
+SOURCE_ITEMS_RAW="${GIMBAL_AUTODRAG_FILE:-sample.png}"
+IFS=',' read -r -a INTEGRITY_ITEMS <<< "$SOURCE_ITEMS_RAW"
+SOURCE_SNAP_BEFORE="$RUNDIR/.source-before"
+source_snapshot() {
+  local out="$1" item path rel f hash
+  : > "$out"
+  for item in "${INTEGRITY_ITEMS[@]}"; do
+    path="$FOLDER/$item"
+    if [ -L "$path" ]; then
+      printf '%s\tlink:%s\n' "$item" "$(readlink "$path")" >> "$out"
+    elif [ -d "$path" ]; then
+      while IFS= read -r -d '' f; do
+        rel="${f#"$FOLDER"/}"
+        if [ -L "$f" ]; then
+          printf '%s\tlink:%s\n' "$rel" "$(readlink "$f")" >> "$out"
+        else
+          hash="$(sha256sum "$f" | awk '{print $1}')"
+          printf '%s\t%s\n' "$rel" "$hash" >> "$out"
+        fi
+      done < <(find "$path" \( -type f -o -type l \) -print0 2>/dev/null)
+    elif [ -f "$path" ]; then
+      hash="$(sha256sum "$path" | awk '{print $1}')"
+      printf '%s\t%s\n' "$item" "$hash" >> "$out"
+    else
+      printf '%s\tmissing\n' "$item" >> "$out"
+    fi
+  done
+  sort -o "$out" "$out"
+}
+source_snapshot "$SOURCE_SNAP_BEFORE"
+
 # ---------------------------------------------------------------- 4. the drag
 AUTO_RC=0
 if [ -n "$AUTO" ]; then
@@ -333,10 +400,29 @@
 cleanup
 trap - EXIT
 
+# source integrity, after cleanup and before summarize.py (P1g)
+SOURCE_SNAP_AFTER="$RUNDIR/.source-after"
+source_snapshot "$SOURCE_SNAP_AFTER"
+touch "$RUNDIR/result.txt"
+{
+  if cmp -s "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER"; then
+    echo "source_intact: yes"
+  else
+    echo "source_intact: no"
+    echo "source_diff:"
+    diff -u "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER" \
+      | sed -n 's/^\([+-][^+-].*\)$/  \1/p' || true
+  fi
+} >> "$RUNDIR/result.txt"
+rm -f "$SOURCE_SNAP_BEFORE" "$SOURCE_SNAP_AFTER"
+
 # meta.txt before summarize.py so summary.json can embed it
 SOURCE_VERSION=""
 COSMIC_COMMIT=""
-if [ "$SOURCE_APP" = cosmic ]; then
+if [ "$SOURCE_APP" = gimbal ]; then
+  # P1b treats --version as an unknown flag (exit 2): never run it.
+  SOURCE_VERSION="gimbal-spike sha256:$(sha256sum "$SOURCE_BIN" | cut -c1-12)"
+elif [ "$SOURCE_APP" = cosmic ]; then
   # cosmic-files has no --version (it parses every flag as a path); use the
   # git commit of Kyle's build in ~/cosmic-files
   COSMIC_COMMIT="$(git -C "$HOME/cosmic-files" rev-parse --short=12 HEAD 2>/dev/null || echo unknown)"
```

`summarize.py`'s `read_result` only looks at `zen_opened_file:` and `note:`
(read of `summarize.py:211-229`), so the appended `source_intact:` /
`source_diff:` lines do not change `summary.json` **[read]**.

### `autodrag.py` — multi-file and classification

- `split_files()` turns `--file` into names; `--file a,b` is a multi-file
  drag. Each name must exist under the folder.
- Multi-file path: locate the first name's row (AT-SPI, or `item_offsets`
  for that exact name), **do not click**, and press there. If AT-SPI exposes
  the row, each name is re-read first and a missing `STATE_SELECTED` aborts
  as `preselection check failed`; the run records `selection: preselected`.
  There is no click -> no `click_to_drag_gap_ms` wait.
- `title_candidates()` adds a symlink's target basename (via `realpath`,
  falling back to `readlink`) to the names that count. `classify_title_any()`
  returns `(verdict, matched)`; `result.txt` now carries
  `matched_name: <name or None>`. A folder's listing title contains the
  folder name, itself a candidate, so a folder drop classifies `y`.
- `fallback_item_point()` reads `item_offsets[name]`; `None`, `[]`, a short
  list or a missing key means unconfigured -> the existing `unconfigured`
  abort, never a blind click. Apps without `item_offsets` keep the old
  `first_item_offset` path.
- `parse_source_intact()` reads the line `run.sh` appends (last one wins,
  only `yes`/`no` accepted); `run-matrix-gimbal.sh` uses it for its stop
  rule, so the matrix and the unit tests share one parser.
- Standalone `autodrag.py run … --file a,b` also sets
  `GIMBAL_AUTODRAG_SELECT=a,b` before exec'ing `run.sh`, so a multi-file run
  works without exporting the variable by hand.
- `--dry-run` now prints the files, whether the row is preselected, the
  `item_offsets` table and the any-name classification. It still reads
  nothing.

### `apps.toml`

New `[app.gimbal]` with the probe evidence, `atspi_items = true`,
`window_offset_correction = [0, 0]`, and
`item_offsets = { "sample.png" = [], "sample.txt" = [], "sample-dir" = [], "sample-link.png" = [] }`.
TOML has no `null`; `[]` is the unset marker and `fallback_item_point`
treats it exactly like a null. The sort order (folders first, then display
name) is recorded in the section comment. No value for the four existing
apps changed.

### `run-matrix-gimbal.sh`

Eight rows in the brief's order: `outside` then `downloads`, each with
`sample.png`, `sample-dir`, `sample-link.png`, `sample.png,sample.txt`.
The last row exports both `GIMBAL_AUTODRAG_FILE` and
`GIMBAL_AUTODRAG_SELECT`. Like `run-matrix.sh`, it snapshots matching run
directories before a row and only accepts a new one (`comm -13`), sleeps
2 s between rows, and stops at the first `aborted:` or a run.sh exit code
other than 0/3. It also stops on `source_intact: no` and prints the
`source_diff:` lines.

### Tests and checks — **[observed]**

```
$ python3 -I -B test_autodrag.py
Ran 60 tests in 0.006s
OK
$ python3 -I -B test_vmouse.py   -> Ran 18 tests, OK
$ python3 -I -B test_niri.py     -> Ran 16 tests, OK
$ python3 -I -B test_locate.py   -> Ran 10 tests, OK
$ python3 -I -B ../test_summarize.py -> Ran 42 tests, OK
$ bash -n run.sh make-test-files-gimbal.sh run-matrix-gimbal.sh
bash -n: all OK
```

New tests: the multi-file path never clicks and its first move starts on
the first row (one press, 4+30 moves, no click move); classification for a
regular file, folder, symlink target and multi-file (matched name
recorded); `source_intact` parsing (yes/no/missing/malformed/last line);
a null/empty gimbal `item_offsets` entry aborts `unconfigured` without a
pointer device; a configured one is window-relative. All 49 previous
autodrag tests still pass.

### TODOs for Claude's live calibration

1. **Placement and AT-SPI.** Build the reviewed spike, run
   `GIMBAL_AUTODRAG_STOP_AFTER=placement ./run.sh --auto gimbal outside`
   with DP-3 off, then `GIMBAL_AUTODRAG_STOP_AFTER=select …` and take
   screenshots. Confirm `window_offset_correction = [0, 0]` (the AT-SPI
   `table cell` center should land on `sample.png`). If it misses, only
   that correction changes; no offsets are needed while AT-SPI works
   **[inferred]**.
2. **Confirm the probe's rects at the runner's 1220x1326 floating size.**
   The probe measured at the default 900x600; the selection check and the
   screenshot are the guard.
3. **One full row** (`sample.png`, outside): expect `zen_opened_file: y`,
   `selection: selected`, `source_intact: yes`, and a raw title naming
   `sample.png`.
4. **`run-matrix-gimbal.sh`**: 8 rows. Watch the folder row's title
   (directory listing contains `sample-dir`), the symlink row (title may
   name `sample.png`, the target; `matched_name` says which), and the
   two-file row (`selection: preselected`, no click, both sources still
   present).
5. **`source_intact` on the Phase 0 sources.** Rerun `run-matrix.sh` once
   and check every row reports `source_intact: yes`; the four old sources
   should otherwise behave exactly as in the Phase 0 live check.
6. **If AT-SPI ever stops exposing the rows,** fill the four
   `item_offsets` values live and re-run the select-stop screenshots;
   until then they stay `[]` and the runner aborts with `unconfigured`
   rather than clicking blind.

Not run here, per the hard limits: no `run.sh`, no `run-matrix*.sh`, no
`/dev/uinput`, no pointer/keyboard injection, no niri action. The only
spike launches were the two `/tmp/gimbal-spike-p1g-probe*` probes above,
both closed by PID.

## Review of P1g (Claude, Nov 1, 2026)

**Verdict: accepted on the code. Live calibration still to run** (the six TODOs above).

Checked: only the Output files changed; no new `niri msg action` call; nothing deletes, moves or
renames a file; `bash -n` passes; all 104 Python tests pass (`python3 -I -m unittest`). The
AT-SPI probe was taken on P1e's grid, so it already reflects the `GridView`.

1. Before TODO 5: the Phase 0 folder `~/gimbal-dnd-test` has no `sample.txt` (only `sample.png`;
   the folder hasn't changed since Oct 30 21:09, so this predates P1g). `run.sh` dies on that
   folder until `make-test-files.sh` is run again. `~/Downloads/gimbal-dnd-test` is complete.

## Live calibration of P1g (Claude, Nov 1, 2026)

Setup: DP-3 turned off by Kyle; DP-2 2560×1440, Noctalia bar 34 px. The spike was the
reviewed release build. After every run only Kyle's Zen and Mission Center were left in
`flatpak ps`, and no `gimbal-spike` process.

1. **Placement and AT-SPI.** `--stop-after placement`, then `select` with `--hold-s 8`: the
   AT-SPI `table cell` click at (377, 167) selected `sample.png` (screenshot), and no viewer
   opened. `window_offset_correction = [0, 0]` is right; `item_offsets` stay `[]`.
2. **The probe's rects at 1220×1326** hold: every click in the matrix landed on its file.
3. **One full row** (`sample.png`, outside), capture `20261101-051557-gimbal-outside`:
   `zen_opened_file: y`, `source_intact: yes`. The spike offered `x-gtk-local-dnd`,
   `text/plain;charset=utf-8`, `text/uri-list`, `vnd.portal.filetransfer` and
   `vnd.portal.files` (nothing added by hand); Zen read `vnd.portal.files` and called
   `RetrieveFiles` → `/run/user/1000/doc/9e1f8e71/sample.png`. Zen reported MOVE and the
   source survived.
4. **`run-matrix-gimbal.sh`**: 6 of 8 rows passed on the first run. Both two-file rows
   (`051650`, `051736`) gave `zen_opened_file: n`: the spike never called `start_drag` and saw
   no button event. **Found and fixed live:** `perform_drag` presses wherever the pointer is.
   The single-file path's selection click had already moved it onto the file; the multi-file
   path had no click, so the press missed. `autodrag.py` now moves onto the first row and
   waits `select_settle_ms` before the drag; `test_multi_file_path_never_clicks_and_presses_first_row`
   now expects that move first (104 tests OK). Rerun: `051846` (outside) and `051857`
   (downloads) both `zen_opened_file: y`, `selection: preselected`, `source_intact: yes`;
   `RetrieveFiles` returned both files. The folder and symlink rows passed (`matched_name`
   recorded). Runs `051501` and `051530` are calibration runs (`aborted`/stopped early);
   `051650` and `051736` are the pre-fix multi-file runs. P1h should ignore all four.
5. **`source_intact` on the Phase 0 sources: not run.** `~/gimbal-dnd-test` has no
   `sample.txt`, so `run.sh` stops for those sources until `make-test-files.sh` is rerun. Not
   needed for Phase 1's verdict, which depends on the gimbal source.
6. AT-SPI exposes every row, so the `item_offsets` fallback wasn't needed.

**Status: P1g accepted. The gimbal source is calibrated** and `run-matrix-gimbal.sh` passes
all 8 rows.
