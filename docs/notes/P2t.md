# P2t: open several folders as one new group

Unless tagged otherwise, implementation descriptions are **[inferred]** from
the source and unit tests; gate results are **[observed]**; API citations are
**[read]**.

## What this change does

- **One new command.** `group.open paths=<absolute folder> …` is
  `Effect::View`, has no undo, and takes one required `paths` argument
  (`ArgKind::Paths`, repeated like `file.open`). `Ui`, `Hotkey`, `Cli` and
  `Agent` may all call it, exactly like `group.split`.
- **Pure model operation.** `Workspace::group_open(paths)` collapses exact
  duplicate paths (keeping the first), refuses fewer than two with the existing
  `NotEnoughTabs { selected }`, and otherwise inserts one new pill immediately
  after the focused pill. It builds one split group with a tab per folder,
  `initial_columns` layout, a cleared pill selection, and focus on the first
  folder's tab. With no window it creates a new window holding the group. It
  never touches the filesystem.
- **Handler validation.** `Process::group_open_command` refuses the whole
  command, naming the first path that is not an existing folder, before the
  model is changed. It logs one `group:` line with the folder count and the new
  pill's index, or the refusal reason.
- **Folder view.** The item menu inserts **Open in New Group** directly after
  **Open** when two or more items are selected and every one is a folder. The
  command receives the paths in the view's order.
- **Sidebar multi-select.** Pinned and Recent share one `SidebarSelection`
  (list, selected row indices, anchor). Ctrl-click toggles without navigating;
  Shift-click selects the anchor-to-clicked range; a plain click clears the
  selection and does today's open/navigate action. Selecting in one list clears
  the other list, a rebuild clears the selection, and Escape clears it while a
  sidebar row has focus. Selected rows use the existing `.sel` look; Recent got
  the same `pin_sel` background class as Pinned, with no new colour or size.
- **Sidebar menus.** Right-click inside a selection of two or more shows **Open
  in New Group** with the selected folders in list order. A right-click outside
  it (or with zero/one selected) makes that row the only selection: Pinned then
  shows **Unpin** and **Open in New Tab**, Recent shows **Open in New Tab**.
  Every menu opens through `crate::popup::show` and every action goes through
  `dispatch_for_window`; the sidebar never edits the workspace model directly.
- **Pinned drag.** A Ctrl or Shift press does not begin a reorder. A plain press
  and drag still behaves as before, and a plain click still opens exactly once
  through `last_open`.

Claude's uncommitted last-tab fix in `close_pills` and its tests is untouched.

## What was verified, and how

- `scripts/check.sh`, in order:
  - `cargo fmt --all --check` — passed.
  - `cargo clippy --workspace --all-targets -- -D warnings` — passed.
  - `cargo test --workspace` — 824 passed, 23 ignored, 0 failed.
  - `cargo build --workspace --release` — passed.
  - `cargo deny check` — advisories ok, bans ok, licenses ok, sources ok. **[observed]**
- Test totals by target: fm-agent 23 unit + 16 socket, fm-cli 12, fm-core
  367 unit + 6 trash + 11 trash-browser, fm-ui 389. **[observed]**
- New fm-core tests cover two and four folders, duplicate collapse, one folder
  and empty refusal with no mutation, no-window creation, pill position after
  the focused pill, selection vector length, layout columns, focus on the first
  folder, events, and the catalogue/caller rules. **[observed]**
- New fm-ui tests cover the three folder-menu cases, the sidebar selection
  function (plain, Ctrl, Shift, list switch, out-of-range), the right-click menu
  choice, and the handler refusing a missing folder with the workspace
  unchanged. **[observed]**
- The existing folder-view, sidebar, workspace and session tests still pass.
  **[observed]**

No app run: this agent did not launch the binary. All interactive behavior
below still needs Claude/Kyle's live check.

## API citations

- `gtk4-0.11.5:src/auto/event_controller.rs:49` —
  `EventController::current_event_state() -> gdk::ModifierType`, used by the
  Pinned click and drag gestures to read Ctrl/Shift at the event. **[read]**
- `gtk4-0.11.5:src/event_controller_key.rs:11` —
  `EventControllerKey::connect_key_pressed`, used for Escape. **[read]**
- `glib-0.22.10:src/signal.rs:161` — `glib::Propagation::{Stop, Proceed}`,
  the key handler's return type. **[read]**
- Existing `pin_slot`, `pin_release`, `compute_bounds` and `crate::popup::show`
  patterns were reused without new APIs. **[read]**

## Needs a live session (Claude/Kyle)

1. In a folder, Ctrl-select three folders, right-click → Open in New Group: one
   new pill right of the current one, three tiles, focus on the first.
2. Add a file to that selection: the row is gone. One folder alone: the row is
   gone.
3. Pinned: Ctrl-click two pins (no navigation happens), right-click one of
   them → Open in New Group. Then Shift-click to select a range and do it again.
4. Recent Directories: the same as 3. A single recent row's right-click offers
   Open in New Tab.
5. Selecting in Recent clears Pinned's selection; Escape clears; dragging a pin
   to reorder still works, and a plain click still opens a pin.
6. Right-click a pin outside the selection: the selection becomes that pin and
   its menu shows Unpin and Open in New Tab.
7. Close every tab of the new group, then the last remaining tab: the window
   stays open at home.

Also worth checking live: Ctrl/Shift plus a drag on a pin starts no reorder, and
the Recent highlight sits on the whole row at every scale.

## Things outside these files

None. No dependency, manifest, session-schema or socket-protocol change. The
`commands` documentation's illustrative total was updated from 54 to 57 because
two P2s commands and this one were missing from it; that is documentation only.

## Review (Claude, Nov 4, 2026)

**Passes, with two fixes Claude made in `sidebar.rs`.** Gates after the fixes: `scripts/check.sh`
exit 0, 824 passed / 23 ignored. [observed]

Checked and fine [read]:
- `group.open` catalogue entry, `REQUIRED_PATHS`, and `Workspace::group_open` (dedupe, refusal,
  pill after the focused one, selection vector, `initial_columns`, focus, events, no-window path).
  The UI rebuilds the window on `TabAdded`/`GroupFormed` (`events.rs:56,59`).
- The handler validates every path before touching the model and is reached the same way as
  `tab.new`. The folder-view row and its three tests match the brief.
- Recent rows lost their per-row `attach_click`, but the list-level click does the same thing
  (`open_recent_folder` on release), so plain-click behavior is unchanged.
- The last-tab fix in `close_pills` is untouched.

Fixed by Claude:
1. **Plain clicks took keyboard focus.** `clicked` (Pinned) and `recent_clicked` called
   `grab_focus()` on every click, and right-clicks did too. Navigating doesn't refocus the grid
   (only a cell click does, `folder/view.rs:311`), so after clicking a pin the grid's arrow keys,
   Delete and type-ahead went to the sidebar. Before P2t the sidebar could not take focus. Now only
   a Ctrl or Shift click takes focus, so Escape can still clear that selection. [read]
2. **Escape was always swallowed.** The sidebar's key handler returned `Stop` for every Escape. The
   window's Escape handler is in the bubble phase (`frame.rs:834`), so while the sidebar had focus
   it could not close an empty search, clear a pill selection or cancel a split drag.
   `clear_selection` now reports whether it cleared anything, and Escape stops only then. [read]

Left as is (minor):
- While Recent owns the selection, Pinned still paints its older single-row `selected` highlight
  when it has one. Cosmetic; worth a look during live check 5.
- A Recent rebuild clears the shared selection, and Recent rebuilds when any window navigates. A
  half-made Ctrl selection can vanish if another window navigates meanwhile. The brief allows it.

## Live checks (Claude, Nov 4, 2026)

All 7 pass. Run on scratch folders under `/tmp/gimbal-p2t` with scratch `XDG_CONFIG_HOME`,
`XDG_STATE_HOME` and `XDG_CACHE_HOME`, at niri's half width (1120×1462), with a uinput pointer
(left/right buttons, Ctrl, Shift, Esc). Widgets were found through AT-SPI, and results were read
from the `GIMBAL_LOG=1` log and AT-SPI dumps. [observed]

1. Grid: click alpha, Ctrl beta, Ctrl gamma, right-click → Open in New Group:
   `group: opened 3 folders as one group at pill 1`, three tiles, focus on alpha's tab.
2. Folder plus file: the menu has no group row. One folder: Open in New Tab, no group row.
3. Pinned: Ctrl alpha + Ctrl gamma did not navigate, then group "Group 1 · 2 tabs". Ctrl beta +
   Shift delta, right-click gamma → "Group 1 · 3 tabs".
4. Recent: Ctrl delta, gamma, alpha → "Group 1 · 3 tabs". A single row's menu offers Open in New
   Tab.
5. Pinned Ctrl ×2, then Ctrl in Recent, then right-click a pin: the single menu, so Pinned's
   selection was cleared. Pinned Ctrl ×2, Esc, right-click: the single menu again. A plain pin click
   navigates. Dragging alpha onto delta: `pins.move` 0 → 3, written to the config.
6. Ctrl alpha + beta, right-click delta: Unpin + Open in New Tab. Open in New Tab → pill "delta".
7. After a group of two, `tab.close` ×3 (sent through the agent socket, because the virtual
   device's letter keys don't reach niri): "Group 1 · 2 tabs" → "gamma" → "root" → "kyle" (home).
   No WindowClosed, and the app stayed up.

Notes:
- The driver first clicked about 15 px above the row centres. On 30 px Recent rows, that landed on
  the boundary and picked the row above. A temporary log line (since removed) showed the gesture y.
  With the offset fixed, every click lands mid-row. Not an app bug.
- One plain pin click, in one of four runs, didn't navigate. It didn't recur in three reruns with a
  log on the click handler. Kyle: say if a pin click ever needs a second try.
- Not checked: the cosmetic Pinned highlight while Recent owns the selection (AT-SPI doesn't
  expose CSS classes).
