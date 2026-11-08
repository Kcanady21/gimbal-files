# P2ag: open a split tile in its own tab

Unless tagged otherwise, implementation descriptions are **[inferred]** from
the source and unit tests; gate results are **[observed]**; API citations are
**[read]**.

There is no `phase2/RULES.md`; I read `phase1/RULES.md`, the repository's
`CLAUDE.md` (which `phase2/README.md` names as its replacement) and the whole
P2ag brief. **[read]**

## What I built

- **One command.** `tab.open-in-own-tab` is `Effect::View`, no undo, and takes
  no arguments. It sits directly after `tab.open-in-new-window` in the
  catalogue. Like the other View commands, `Ui`, `Hotkey`, `Cli` and `Agent`
  may all validate it. **[read]**
- **One workspace operation.** `Workspace::tab_open_in_own_tab(tab)` takes the
  target `TabId`, so the model itself refuses a stale or missing id. It refuses
  a missing target with `NoSuchTab` and a target whose group is no longer split
  with the new `GroupNotSplit` error, both before allocating ids or changing
  anything. **[read]**
- **The copy.** The new tab receives the source tile's folder, both history
  stacks and the whole `FileSelection` (names, focused name and Shift anchor)
  by value. It gets a fresh tab id, the usual `None` scroll anchor and normal
  access (`admin = false`). The source group is only read: its tile order,
  layout, ids, folders and selections stay intact, so the tile stays in the
  split. **[read]**
- **Insertion and events.** The new single-tab group is inserted immediately
  after the source group's pill. The code reuses `tab.new`'s rules: a fresh
  group id, `next_group_number`, `initial_columns`, a false pill-selection
  flag, clearing the window's pill selection, moving the anchor to the new
  position, focus on the new tab, then `TabAdded` and `FocusChanged`, plus
  `SelectionChanged` only when the pill selection had actually been set.
  **[read]**
- **Tile menu.** The split tile's pill menu now shows **Open in Own Tab**
  immediately after **Open in New Window**. It keeps the pill's `TabId` and
  uses the existing `focus_tab_and_dispatch`, so it acts on the tile the menu
  came from even when another tile was focused when the menu opened. Before
  dispatch it validates that the clicked id still belongs to a split group in
  the current window; a stale menu therefore leaves the workspace alone.
  **[read]**
- **Handler.** The no-argument handler reads the focused tab's id and calls
  `Workspace::tab_open_in_own_tab` once, then reports the normal
  `Outcome::Done` / `Outcome::Failed` through `finish`. **[read]**

No new dependency, no session-schema change and no new GTK API.

## What was verified, and how

From `$PROJ` I ran `scripts/check.sh`, which runs the five gates in order.
It exited **0**. **[observed]**

- `cargo fmt --all --check` — passed. **[observed]**
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
  **[observed]**
- `cargo test --workspace` — **982 passed, 24 ignored, 0 failed**. Per target:
  fm-agent 23 unit + 16 socket, fm-cli 13, fm-core 484 unit + 6 trash + 11
  trash-browser, fm-ui 429. **[observed]**
- `cargo build --workspace --release` — passed. **[observed]**
- `cargo deny check` — advisories ok, bans ok, licenses ok, sources ok (the
  usual duplicate-crate warnings for `syn`, `system-deps` and `windows-sys`).
  **[observed]**

New fm-core tests:

- `tab_open_in_own_tab_copies_a_split_tile_after_its_group` opens a non-first
  tile of a three-tile split with its own folder, a back stack, a forward
  stack and a selection with distinct names, focused name and anchor. It opens
  the copy while another tile has focus and checks the pill position, fresh
  group and tab ids, focus, all copied values, the empty scroll anchor, normal
  access, the exact events, the unchanged source group and the invariants.
  **[observed]**
- The same test runs Back, Forward and a plain file selection in the new tab
  after the copy, checking the new folder and stacks at each step and asserting
  the source group still compares equal. **[observed]**
- `tab_open_in_own_tab_refuses_a_standalone_or_missing_tile_unchanged` checks
  `GroupNotSplit` for a standalone tab and `NoSuchTab` for `TabId::from_raw(9999)`,
  comparing the whole workspace with its prior clone after each refusal.
  **[observed]**
- `tab_open_in_own_tab_gives_the_copy_normal_access` splits an administrator
  tab with a normal tab, opens the administrator tile in its own tab and
  checks the source still has `admin = true` while the copy has
  `admin = false`, the same folder and focus. **[observed]**
- `open_in_own_tab_is_a_no_argument_view_command_for_every_caller` checks the
  catalogue entry (View, no undo, no arguments) and validation for all four
  callers, and that `index=0` is refused as an unknown argument. **[observed]**
- `the_open_in_own_tab_command_targets_the_clicked_split_tile` in fm-ui builds a
  split, gives one tile a selection, focuses the other tile, then calls the
  real `focus_tab_and_dispatch` path. It checks the copy came from the clicked
  tile and the source group is unchanged. It then dispatches through
  `dispatch_with_outcome` (the real `UiHandler`) against a standalone tab and
  checks `Outcome::Failed` contains `not split` with an unchanged workspace,
  and that an extra argument is refused by the registry before the handler.
  **[observed]**

## API citations

No new GTK/libadwaita/GIO API was needed. The menu reuses the existing
`gtk::Popover`, `gtk::Button` and `menu_item` path in
`crates/fm-ui/src/split.rs`, and the existing
`Process::focus_tab_and_dispatch` in `crates/fm-ui/src/app.rs`. **[read]**

## Needs a live session (Claude/Kyle)

No app run was made by this agent; these checks still need a live session, all
on scratch folders under `/tmp/gimbal-*` that the checker creates.

1. In a split with two different folders, build Back and Forward history and
   select files in the second tile. Open that tile in its own tab while the
   first tile has focus.
2. Check the new pill appears after the split, opens the second tile's folder
   and shows the copied selection. Use Back and Forward.
3. Return to the split. Both tiles are still there and the source tile kept its
   folder, selection, history and layout. Change the selection in each copy and
   check that the other copy keeps its own.
4. **Open in New Window** still moves its target to a separate window.

Worth checking live as well: a right-click on a tile whose group has just
stopped being a split should do nothing (the stale-menu validation), and the
new pill should appear after the split pill without disturbing later pills.

## Things outside these files

None. No manifest, session-schema or socket-protocol change. The new command
is a View command, so it is also reachable from the hotkey, CLI and agent
callers by the existing caller rules; it acts on the focused tab there. If
Kyle wants it restricted to the UI, that is a later rule change rather than a
file outside this brief.

## ChatGPT's review (Nov 8, 2026)

- **[read]** The operation checks the clicked tile and its group before
  allocating IDs. It copies the folder, both history stacks and the selection
  into a new normal tab. The source split stays intact. During review,
  `tab.new` and `tab.open-in-own-tab` were changed to share the standalone
  tab insertion and event code.
- **[observed]** The model and real UI dispatch regressions passed. They
  cover a non-focused source tile, copied history and selection, independent
  changes after copying, and a refusal on a standalone tab.
- **[observed]** The combined P2ag–P2ai tree passed `scripts/check.sh` (999
  passed, 25 ignored, 0 failed), `scripts/test-install.sh` and
  `scripts/test-deb.sh`. Each exited 0.
- **[observed]** Kyle confirmed the desktop re-check passed. The
  **Review split** fixture in `/tmp/gimbal-p2ag-ai-review.Pzcvep/run.sh`
  checks the copied selection and both history directions while keeping the
  original tile in the split.
