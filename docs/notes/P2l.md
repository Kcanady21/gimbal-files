# P2l: drag and drop

Tags: **[observed]** for a command or run, **[read]** for source, **[inferred]**
for anything that needs a real pointer or a live feel.

## What I built

All changes are under `crates/fm-ui/src/` plus the two runner scripts and this
note. `fm-core` was not changed, and no dependency or `Cargo.toml` line changed.

- **Drag source: `dnd.rs`, `folder/view.rs`.** Every grid cell in a single tab
  and in a tile has a `gtk::DragSource` with COPY | MOVE.
  - A drag past the threshold takes the model's selection. An unselected
    pressed item is made the only selection first with the model's
    `file_click_tab(..., ClickRequest::Plain(name))`; a press that never
    reaches the threshold is still the existing click/double-click.
  - The provider is one `gdk::FileList` through
    `gdk::ContentProvider::for_value`; no MIME type is added by hand.
  - `drag-end` only logs `selected_action` and `delete_data` and ignores the
    latter. Nothing is deleted or moved by the source.
  - The drag icon is built from the look: the first item's `gio::Icon` at
    `ICON_GRID`, plus a `.gl-drag-badge` label when more than one item is
    dragged. The badge text is `badge_text(count)` (`None` for 0/1).
- **Drop targets: `dnd.rs`, `folder/view.rs`, `header.rs`, `split.rs`,
  `sidebar.rs`.**
  - A grid's empty space (`GridView`) drops into that grid's folder; a folder
    cell drops into that folder; a file cell accepts the value only to refuse
    it with "only folders are drop targets".
  - Header pills and split tile pills drop into that tab's folder. A 700 ms
    hover over either pill focuses it through the model ("spring-loaded").
  - Pinned rows and device rows drop into their folder/mount point. The empty
    strip under the pinned rows pins folders with one `pins.add` per folder;
    files are ignored.
  - The header's Trash button submits `Request::Trash`.
  - A valid target gets `.gl-drop-target` (`box-shadow: inset 0 0 0
    LINE_THIN accent`, the same shape as the focus ring). A target that would
    refuse shows nothing; the refusal is written to the status line on drop.
  - Only `gdk::FileList` is accepted, so plain text and a URL without a file
    list do not light any target and are ignored.
- **Copy or move (`dnd.rs`).** `action_for(same_filesystem, modifiers)` is the
  pure rule: Ctrl copies, Shift moves, otherwise move on one filesystem and
  copy across filesystems; Ctrl wins when both are down. The target reads the
  modifiers from `gdk::Drop::drag().device().modifier_state()` and reports the
  selected action during hover through `GIMBAL_LOG`.
  - `same_filesystem_async` queries `id::filesystem` on the target and on the
    first source with two `query_info_async` calls. Both on the main context,
    no blocking. A failure, an empty attribute, or a 250 ms timeout falls back
    to copy.
  - `operation_for` resolves every target to a folder, Pins or Trash, applies
    the refusals, and returns the operation. Trash and pin targets ignore the
    modifiers.
- **What a drop runs.** `Process::drop_on_folder` submits `Request::Copy` or
  `Request::Move` to the P2k worker; `drop_on_trash` submits `Request::Trash`;
  `pin_folders` dispatches `pins.add`. The worker therefore owns the conflict
  dialog, progress, Cancel and Ctrl+Z undo.
  - Refused before the worker, with the reason in the status line: the target
    folder containing every source already, a source equal to or above the
    target, and a file cell. The path checks use only `Path` values; the only
    filesystem calls on a drop are the two asynchronous `query_info`s (and the
    asynchronous type queries when an external drag hovers the pin list).
- **Command line (`cli.rs`, `app.rs`, `main.rs`).** `gimbal-files file...`
  groups file arguments by parent folder. A file group opens its parent with
  the names selected through `nav_go` + `selection_all`; several files in one
  folder share one tab; two folders make two tabs. On a fresh session the
  automatically opened home tab is reused, so two files give exactly one tab.
  A folder argument still opens that folder. The spike's `--select` is not
  part of the app; the runner now passes the file paths themselves.
- **Runner (`tools/dnd-capture`).** `run.sh` has an `app` source:
  - launches `target/release/gimbal-files` with the row's file paths, scratch
    `XDG_CONFIG_HOME`/`XDG_STATE_HOME` under
    `/tmp/gimbal-dnd-app-*`, and `GIMBAL_LOG=1`; a folder row (`sample-dir`)
    opens its parent so autodrag can click that folder row, because item 5
    selects files only;
  - does **not** use `dbus-run-session` (the Zen drop needs Kyle's real session
    bus and document portal);
  - refuses the `app` source when `pgrep -x gimbal-files` finds Kyle's app and
    tells him to close it;
  - cleans its scratch folders in `cleanup`.
  `run-matrix-gimbal.sh [gimbal|app]` runs the same 8 rows for either mode.
  The autodrag per-app table has no `app` entry (it is not in this brief's
  Output list), so `run.sh` passes `gimbal` to autodrag for the AT-SPI config
  while the capture, meta and result say `app`. That is a workaround worth a
  review; see "Outside my files".

## Choices and notes for review

- **Nested drop targets.** A drop target on a `GridView` and one on each of its
  cells are both entered by GTK; the cell is the pointer-focus widget and
  handles DROP_START first. The grid target also masks itself with
  `GridView::pick`: while the pointer is over a `gl-cell` it bows out, so a
  file cell never lights the whole grid and the grid only owns empty space.
  This is the one interaction I could not check without a pointer.
- **The drag icon is a `WidgetPaintable`.** It has never been through a layout,
  so `dnd::drag_icon` measures and allocates it once before returning it. The
  widget uses the shared `.gl-drag-badge` rule in `look/css.rs`; no fixed
  colours or sizes are in the widget.
- **Spring-loading does not rebuild the header.** `suppress_header` makes the
  model focus update the content only. The header is refreshed in `drag-end` /
  `drag-cancel`, so the pill under the pointer is not unparented mid-drag. The
  700 ms timer only fires while a drop target has a drag over it, so an
  ordinary pointer hover does not switch tabs.
- **Pinned-list empty space.** `PinnedList.root` now has a one-row
  `.gl-pin-empty` strip after the rows: the rows have their own targets and
  this strip (or the root around them) is the pin target.
- **Drops from other apps.** They are read from the same `gdk::FileList`; their
  file types are queried asynchronously only when the pointer is over the pin
  list. No `is_dir`/`query_info` runs on the main thread to decide a drop.

## API citations

Read in the pinned crate sources:

- `gtk4-0.11.5:src/auto/drag_source.rs:83` `set_icon`; `:158`
  `connect_drag_end`; `:191` `connect_prepare`; `:120` `connect_drag_cancel`.
- `gtk4-0.11.5:src/auto/drop_target.rs:55` `current_drop`; `:82` `value`;
  `:110` `connect_accept`; `:138` `connect_enter`; `:193` `connect_motion`;
  `:169` `connect_leave`.
- `gdk4-0.11.5:src/auto/file_list.rs:29` `from_array`; `:37` `files`.
- `gdk4-0.11.5:src/auto/content_provider.rs:40` `for_value`.
- `gdk4-0.11.5:src/auto/device.rs:106` `modifier_state`.
- `gdk4-0.11.5:src/auto/drag.rs:99` `selected_action`; `:102` the getter;
  `gdk4-0.11.5:src/auto/drop.rs:32` `actions`.
- `gio-0.22.10:src/auto/file.rs:1780` `query_info_async`;
  `src/auto/file_info.rs:144` `attribute_string`.
- `gtk4-0.11.5:src/auto/widget_paintable.rs:24` `WidgetPaintable::new`;
  `src/auto/widget.rs:1000` `measure` and `:143` `allocate`.
- `gtk-4.22.4:gtk/gtkmain.c:1504-1521` dispatch of `GDK_DRAG_ENTER` /
  `GDK_DROP_START`; `gtk-4.22.4:gtk/gtkdroptarget.c:365-374` the default
  accept, and `:438-474` enter/motion/drop handling.
- The phase 0 report and spike: `docs/research/drag-and-drop.md` ("Minimal set
  for Gimbal Files", "Effect on Phase 1") and `spike/src/dnd.rs`.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the 1.95.0 toolchain on `PATH`; exit 0. The command
headers and result lines (the full test list is the same shape as P2k's):

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 175 tests
test result: ok. 175 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 12 tests
test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out
running 141 tests
test result: ok. 141 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

`fm-ui` has 141 tests, 14 more than P2k's 127: 10 in `dnd` and 4 in `cli`.
The total that ran is 330 (1 agent + 7 CLI + 175 core + 6 trash parents + 141
UI). The 6 ignored children and 0 doc-tests are the same as P2k. **[observed]**

### Runner syntax

```text
$ bash -n tools/dnd-capture/run.sh
$ bash -n tools/dnd-capture/run-matrix-gimbal.sh
```

Both exit 0. I did not run the runner: it injects pointer input and drives Zen,
which is Claude's live check. **[observed]**

### Live run: two files on the command line

Scratch `/tmp/gimbal-p2l-live5`, release binary, `dbus-run-session`, scratch
`XDG_CONFIG_HOME`/`XDG_STATE_HOME`, `GIMBAL_LOG=1`, stopped by the app's PID
with SIGTERM after 6 s. `pgrep -a gimbal-files` was empty afterwards. **[observed]**

```text
[gimbal 1.069] launch: argument files in /tmp/gimbal-p2l-live5/src: ["one.txt", "two.txt"]
[gimbal 1.069] header: window 1 pills ["src"] (active 0, overflow false)
[gimbal 1.069] model: SelectionChanged { window: WindowId(1) }
[gimbal 1.069] launch: command line selected 2 file(s) in /tmp/gimbal-p2l-live5/src: ["one.txt", "two.txt"]
[gimbal 1.069] launch: command line done: 1 tab(s), 2 selected in the focused tab
```

The first attempt (before the fresh-session fix) showed two tabs: the automatic
home tab plus the file tab. The fix reuses the automatic tab only when the
session file was missing. **[observed]**

All live scratch folders were removed afterwards. **[observed]**

## Not verified live (Claude checks these)

- Every drag in item 1: a selected item, an unselected item becoming the only
  selection, single tab, tile, the count badge, and the drag icon's look.
- Every target in item 2: empty grid, folder cell, header pill, tile pill,
  pinned row, device row, pinned-list empty strip and the Trash button; the
  hover outline; the file-cell refusal and its status line.
- Ctrl and Shift during a real drag, and the source's reported selected action
  in the `GIMBAL_LOG` hover line.
- The 700 ms spring-loaded header pill and tile pill.
- The Zen 8-row matrix through `run-matrix-gimbal.sh app`.
- Dolphin and Nautilus both ways: into Gimbal and from Gimbal into Zen.
- The Trash button drop with a real pointer.

## Anything outside my files

- `autodrag` has no `app` entry in its `APPS` tuple or `apps.toml`; both files
  are outside this brief's Output. `run.sh` therefore passes `gimbal` to
  autodrag for the app source. If Claude wants the autodrag log to say `app`,
  add an `[app.app]` block and `app` to `APPS`, and pass the real source name.
  I did not edit them.
- No `fm-core` change was needed. The model already had `nav_go(folder,
  select)`, `selection_all` and `focus_tab`; `Process` gained a small
  `tab_folder` accessor for the tile-pill target. The file-operation
  `Request`s and the worker's `submit` already existed.
- One possible follow-up: the grid mask uses `GridView::pick`, which is a
  pointer-only path; Claude's live check should confirm that empty space still
  lights and a file cell stays dark. If pick behaves differently under niri,
  the fallback is to drop the per-cell targets and resolve the cell under the
  pointer in the grid's own drop callback.

## Review and live check (Claude, Nov 2, 2026): passes in part, with one fix by Claude

`scripts/check.sh` from `/tmp`: exit 0, with 141 `fm-ui` tests, and deny is clean. The drag source
never deletes. The runner's cleanup only removes `/tmp/gimbal-dnd-app-*`, and it refuses the `app`
source while Kyle's app runs. Kyle's real Trash was identical before and after.

**Live** (release build, files under `~/.cache/gimbal-tests/p2l` with a scratch `XDG_DATA_HOME`):
- **Drag `a.txt` onto the `sub` folder:** `sub` lit with the accent outline and the drag showed
  the move cursor. The log shows `same filesystem true … action Move`; the file moved, and
  `drag-end … delete_data=true (ignored)`.
- **Drop on its own folder:** refused, with "… is already in this folder".
- **Trash button drop:** `b.txt` went to the scratch trash ("Moved 1 item to the trash").
- **Spring-loaded pill (after the fix):** a drag held over the `dst` pill logged "spring-loaded
  header pill 1" after 700 ms, and the drop went into `dst`.

**Fix (Claude): spring-loading never fired.**
- **The cause:** `spring_load` used an `EventControllerMotion`, and GTK delivers crossing events
  during a drag only to drop controllers.
- **The fix:** it now uses `gtk::DropControllerMotion` (same `enter` and `leave` signals).

**Still open:**
- **Ctrl during a drag wasn't seen.** Claude held Ctrl on a separate virtual keyboard; the hover
  logged `modifiers none` and the drop moved instead of copying. That may be the test device, or a
  real problem: reading `Drop::drag().device().modifier_state()` from the pointer device under
  Wayland. Kyle should try Ctrl and Shift with his real keyboard. If they fail, the brief should
  derive the action from the negotiated drag action (`Drop::actions()`/`selected_action`), which
  the compositor sets from the modifiers, instead of reading modifier state.
- **Not run yet:**
  - the 8-row Zen matrix (`run-matrix-gimbal.sh app`, which needs Kyle's app closed and a
    calibration of where the first grid cell sits);
  - Dolphin both ways;
  - the tile pill and pinned-row targets;
  - pinning by dropping a folder on the pinned list;
  - the count badge.

**Environment incident:** this check's early isolated launches (`dbus-run-session`, which the
agent also used) left about 15 session services running per launch. That used up the user's
inotify limit (128), so GIO reported "Unable to find default local file monitor type", and live
folder updates stopped in every app. With Kyle's approval Claude stopped the 135 leftover
processes on `/tmp/dbus-*` buses, and watching works again. `CLAUDE.md` now forbids
`dbus-run-session`: when Kyle's app is open, don't launch.
