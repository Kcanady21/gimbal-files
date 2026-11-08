# P2ah: rubber-band selection in the folder grid

Tags: **[observed]** for a command or its output, **[read]** for source, and
**[inferred]** for anything that needs a real pointer or a live feel.

## What I built

- **Press ownership and threshold (`fm-ui/src/folder/view.rs`).**
  - A `GtkGestureDrag` is attached to the folder viewport (`body`), not the
    grid. On `drag-begin` (GTK emits it on the press; its own drag source also
    checks the threshold later) the pointer is converted from surface
    coordinates to viewport coordinates and compared with the live cell
    bounds.
  - A press inside a realized cell is left alone: that cell's existing
    `DragSource` keeps the current file drag and all clicks/double-clicks.
    Gaps between cells and the space below the last row own no cell, so they
    start a rubber band.
  - The empty-space sequence is claimed only after the pointer passes
    `gtk-dnd-drag-threshold`; a press that never passes it does nothing.
  - On release the rectangle is cleared before the selection is applied, then
    one `Process::selection_drag_tab` call sends the final hit names and the
    visible order to the model.
  - Moving outside the viewport is clipped to that viewport. The gesture never
    hit-tests or highlights another tile.
  - `set_folder`, `release_root`, a cancelled sequence and an invalidated
    source tab/folder all clear the rectangle and temporary highlights. The
    model selection is untouched on cancellation.
- **Rectangle and highlights (`fm-ui/src/folder/view.rs`, `look/css.rs`).**
  - A `GtkFixed` overlay layer and an empty `gtk::Box` rectangle are parented
    once in the viewport overlay. Their parenting never changes during a
    gesture; only position, size and visibility do.
  - The layer and rectangle have `can-target = false`, so they take no press
    from the cells below. The rectangle is clipped by the overlay.
  - `.gl-rubber-band` uses `alpha(@accent_bg_color, 0.18)` and a
    token-scaled `@accent_color` border. Intersected realized cells get
    `.gl-rubber-hit` (`alpha(@accent_bg_color, 0.30)`). No fixed colour or size
    is in the widget code.
  - Cell highlights are cleared from `cell_positions` on every motion, so a
    recycled cell can never keep another item's highlight.
- **Pure geometry (`fm-ui/src/folder/mod.rs`).** `RubberRect`,
  `normalize_rect` (all four drag directions), `clip_rect` (viewport edges),
  `rects_intersect` (positive area; a touching edge is not a hit), `cell_at`
  (gaps and below-last-row are empty), `rubber_hits`, and
  `drag_passes_threshold` (strict `>`, as GTK does).
- **Workspace update (`fm-core/src/workspace/mod.rs`).**
  `Workspace::selection_drag(hits, order, ctrl, shift)` is one
  `SelectionChanged` event on the focused tab:
  - plain replaces the selection with the hits and moves focus/anchor to the
    first hit in visible order;
  - Ctrl unions the hits into the previous selection and keeps the anchor;
  - Shift takes `order[min(anchor, hits)..=max(anchor, hits)]`, so dragging
    back shrinks the result. With no anchor (or an anchor no longer listed),
    it adds the hits instead of guessing a range;
  - Ctrl wins when both are held, matching `folder::click_request` at
    `folder/mod.rs:414`.
  - Names remain `OsString`, including non-UTF-8 names.
- **App bridge (`fm-ui/src/app.rs`).** `Process::selection_drag_tab` mirrors
  `file_click_tab`: focus the originating tab first, perform one selection
  update, deliver focus and selection events together, then update the user
  preview and the debounced session save. A split tile therefore takes the
  selection and focus while its sibling keeps its own.

There is no `phase2/RULES.md`; I read `phase1/RULES.md`, the repository's
`CLAUDE.md` (which `phase2/README.md` names as its replacement) and the whole
P2ah brief. **[read]**

## The built-in GTK rubber band: rejected, and why

I read GTK 4.22.4 before enabling `GridView::set_enable_rubberband`
(`gtk4-0.11.5:src/auto/grid_view.rs:142`):

- `gtk-4.22.4:gtk/gtklistbase.c:1722` starts a rubber band only when
  `gtk_list_base_get_position_from_allocation` finds an item under the press.
  A gap, the padding, or the space below the last row logs "Could not start
  rubberbanding: No item", which is exactly what item 1 must allow.
- `gtk-4.22.4:gtk/gtklistbase.c:1744` applies the built-in result directly to
  the `GtkSelectionModel`, bypassing the workspace model, its Shift anchor and
  its one-event update. Its Shift branch (`:1794`) unselects items.
- The grid's own drag controller also claims at the threshold
  (`gtk-4.22.4:gtk/gtklistbase.c:1939`), and each cell already has the file
  drag source (`gtk-4.22.4:gtk/gtkdragsource.c:309`). A viewport gesture can
  check ownership first and leave both behaviours intact.

The chosen viewport gesture is therefore ours, and `set_enable_rubberband` is
not enabled. **[read]**

## API citations

- `gtk-4.22.4:gtk/gtkgesturedrag.c:84` (`drag-begin` captures the press point),
  `:96` (`drag-update` carries the current point), `:108` (`drag-end`);
  GTK's own drag source checks the threshold in its update handler.
- `gtk4-0.11.5:src/auto/gesture_drag.rs:158` `connect_drag_begin`, `:191`
  `connect_drag_end`, `:224` `connect_drag_update`;
  `gtk4-0.11.5:src/auto/gesture_single.rs:73` `set_button`.
- `gtk4-0.11.5:src/auto/gesture.rs:198` `set_state` (claiming the sequence),
  `:256` `connect_cancel` (the sequence-cancel hook).
- `gtk4-0.11.5:src/auto/settings.rs:186` `gtk_dnd_drag_threshold`; the
  comparison is strict in `gtk-4.22.4:gtk/gtkdragsource.c:832`.
- `gtk4-0.11.5:src/auto/fixed.rs:305` `Fixed::move_`;
  `gtk4-0.11.5:src/auto/overlay.rs:111` `set_clip_overlay`, `:122`
  `set_measure_overlay`. The layer is not measured, so the rectangle cannot
  feed a minimum size back into the viewport (`gtk-4.22.4:
  gtk/gtkoverlaylayout.c:252-266` measures only the main child or a child with
  `measure` set).
- Pointer positions use `gdk::Event::position`, the surface transform and the
  viewport's bounds, the same rule as `split.rs`'s `pointer_in_fixed`
  (`CLAUDE.md`, "GTK lessons from Phase 1"). **[read]**

## Tests

- `folder/mod.rs`:
  - empty space and item ownership, including gaps, below the last row and an
    exact cell edge;
  - rectangle normalization in every direction;
  - intersection at a cell edge and clipping at the viewport edge;
  - the strict threshold test.
- `fm-core/src/workspace/tests.rs`:
  - plain, Ctrl and Shift results, empty hits, a reversed hit list, and an
    absent anchor;
  - a non-UTF-8 (`0xE9`) name through plain and Shift selection;
  - a drag from a non-focused split tile: source focus and selection change,
    sibling selection unchanged, exactly one `SelectionChanged` event.
- `fm-ui/src/app.rs`: the same split-tile case through the real bridge
  (`selection_drag_tab`). The headless process has no widgets, so the test
  checks the model; the preview/status update is the same code path as
  `file_click_tab` and is a live check.
- `fm-ui/src/look/css.rs`: the rectangle and highlight rules use theme
  colours, scale their border and radius, and contain no literal colours.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the 1.95.0 toolchain on `PATH`. Exit status **0**.
**[observed]**

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
running 23 tests
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 16 tests
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 488 tests
test result: ok. 487 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
running 12 tests
test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out
running 22 tests
test result: ok. 11 passed; 0 failed; 11 ignored; 0 measured; 0 filtered out
running 441 tests
test result: ok. 435 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **991 passed, 24 ignored, 0 failed**. The rows are agent 23, agent
socket 16, CLI 13, core 487 passed/1 ignored, trash 6 passed/6 ignored, trash
browser 11 passed/11 ignored, and UI 435 passed/6 ignored. **[observed]**

I did not launch the app: `pgrep -a gimbal-files` showed Kyle's running
instance (PID 519585), and `CLAUDE.md` says not to launch then. **[observed]**

## Live checks (Claude, with scratch folders)

1. Start in a gap and below the last row, drag across a mix of files and
   folders, then release. Check the highlight, count and preview agree.
2. Repeat with Ctrl and Shift from an existing selection. Drag back toward the
   origin and verify the final selection follows the final rectangle.
3. Start on a selected item and then an unselected item. Verify both use the
   existing file drag behaviour. Check an ordinary click and double-click.
4. Start in a non-focused split tile and drag past the tile edge. Confirm the
   origin tile owns the selection and its neighbour keeps its own.
5. Cancel a drag or switch away from its folder. Confirm the rectangle clears
   and the next click or drag works. Check the rectangle in light and dark.

## Not verified live (Claude checks these)

- The actual pick path: whether the viewport gesture receives presses in every
  gap and in the space below a short listing, and whether the cell drag source
  always wins a press on a cell before the viewport gesture claims it.
- The rectangle's on-screen position, border, clipping and contrast in light
  and dark themes, and whether the `GtkFixed` layer ever influences layout.
- The threshold feel and normal click/double-click after a sub-threshold
  press.
- Ctrl and Shift at release on a real keyboard, including releasing a
  modifier before the button.
- A drag that crosses into a split sibling, and cancelling by switching tab or
  folder mid-drag.

## Anything outside my files

No changes are needed outside the brief's Output list. `content.rs` and
`split.rs` needed no edit: both already construct the same `FolderView`, and
the app bridge focuses the originating tab before the selection update.

One point for review: the plain rectangle moves the model's Shift anchor to
the first hit in visible order (like a plain click moves it to the clicked
item). Ctrl keeps the old anchor, and Shift keeps the anchor through the
range. If the intended rule is to keep the previous anchor after a plain
rectangle, only `Workspace::selection_drag` needs changing.

## ChatGPT's review (Nov 8, 2026)

- **[observed]** Kyle clarified the modifiers during review. Plain drag
  selects the items in the rectangle, Shift adds them, and Ctrl subtracts
  them. The workspace operation, test and brief now use those rules in
  place of the earlier range interpretation.
- **[read]** Ctrl takes precedence when both modifiers are held. Modified
  drags with no hits preserve the selection. Release sends the captured
  tab ID and the hit names in grid order. It no longer walks the entire
  folder listing to find a range.
- **[read]** GTK can change a bound item's position without rebinding it.
  The hit test now refreshes the cell position on
  `ListItem::connect_position_notify` (gtk4 0.11.5
  `src/auto/list_item.rs:355`). See GTK 4.22.4
  `gtk/gtklistfactorywidget.c:183` for the bind condition and
  `gtk/gtklistitemwidget.c:167` for position notification.
- **[observed]** The combined gates exited 0 with 999 tests passed and 25
  ignored. The updated regression covers disjoint Shift hits, Ctrl hits
  over selected and unselected items, both modifiers and empty rectangles.
  Native file names and selection in a non-focused split tile also passed.
- **[observed]** Kyle confirmed the scratch desktop re-check passed for
  plain drag, Shift to add, Ctrl to subtract and starting a file drag on
  an item.
