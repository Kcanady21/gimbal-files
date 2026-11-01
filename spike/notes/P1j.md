# P1j notes: the body look

## What I built

`spike/src/bin/look-body/` — a standalone `look-body` binary. It imports none of the spike's other
modules and touches no other file (Cargo.toml untouched; `src/bin/look-body/main.rs` is found
automatically). 898 lines total, under the 900 budget.

- `main.rs` — CLI parser (`--tiles/--items/--pins/--drives/--scale/--font`, usage + exit 2 on
  unknown flag), the `log!` macro (`[gimbal-look-body <secs.millis>] <module>: …` when
  `GIMBAL_SPIKE_LOG=1`), `adw::Application`/`adw::ApplicationWindow` (NON_UNIQUE, 2560×1440, no
  header bar), the dev bar (scale slider, Split 2/3/4/6, Single, +/− Pin, drive-fullness slider),
  the `header-h` slot, the `gl-frame` body with one `gtk::Paned` (left sidebar | content), content
  column (Stack: single folder view / split view, then `gl-rule.inset` + status line) and the
  live-scale engine (`apply_scale` regenerates the CSS on the same `CssProvider` and re-applies the
  Rust-side sizes that read the same token table × scale).
- `style.rs` — token table, colour-role table (every colour expression in one place), `bar_class`
  (fullness → fill/label class), `css(scale, font)` generating the whole stylesheet; scale =
  `GIMBAL_LOOK_BASE` × setting, every value token × scale rounded with 1 px minimum for non-zero
  borders.
- `grid.rs` — fake folder view: `GridView` over a `StringList`, first 4 of `--items` are folders
  (`folder` icon), rest files (`text-x-generic`); cell = 80 px icon above a centred, ellipsized,
  2-line label; cell min 120×144 via CSS, `border-spacing: gap-s` (spacing comes from CSS, not a
  property — see citations), `min-columns 1`, `max-columns 1000` (no cap).
- `sidebar.rs` — left `gl-panel`: title row + collapse `gl-icon-button`, rule, one
  `ScrolledWindow` holding the `pinned-bg` list (64-row height, first row `pinned-sel`, no inner
  scrolling), rule, three `gl-section-pill` sections (Recent Directories 5 rows, Devices one block
  per `--drives` entry, Remote "No remotes") each toggling a `SlideDown` 200 ms `Revealer`; below
  the scroller, outside it: rule + main drive block. Drive bars: `Overlay` track (`raised`,
  209×18 / 228×20) with a proportional fill (`width_request = token × scale × fullness`, class
  `bar-lvN`/`bar-full`) and a centred % label with the `bar-label-*` colour class.
- `split.rs` — split view: outer `ScrolledWindow` → `gtk::Fixed`, 2 columns, `tile_rects(n, w, h,
  scale)` pure layout (2 tiles fill the full height, more → half-height rows that scroll, odd tile
  left). Tiles fade in/out over 250 ms with `adw::TimedAnimation` on the container's opacity.
  Click anywhere in a tile focuses it (capture-phase click gesture); focused tile is
  `gl-tile.focused`, its pill `active`, others `recessed`. Ctrl-click toggles, Shift-click selects
  a range (`selected` fill). Pill ✕ and right-click → Close close the selection or the tile;
  closing down to one tile fades back to the single folder view. Closing to one tile and the dev
  bar's "Single" share one fade-to-single closure.

**Dropped, per the brief's priority ladder** (lines ran out): tile drag-rearranging and the
preview sidebar (the brief's last two priorities). Consequences for the live check: no
"drag a tile to a new position", no preview pane (so no preview collapse button and the body is
left sidebar | content only). Everything else in the brief is in. The status line shows the same
fake counts in both views because every tile holds the same `--items` count.

## Verified

### RULES.md's four commands (final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(exit 0, no output)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.15s

$ cargo test
     Running unittests src/main.rs …
running 4 tests … test result: ok. 4 passed
     Running unittests src/bin/look-body/main.rs …
running 10 tests … test result: ok. 10 passed
     Running unittests src/bin/look-header/main.rs …
running 3 tests … test result: ok. 3 passed
(look-body's 10: CLI parser ×2, style rules 1+2 + scaled token + drive classes, grid counts,
tile_rects ×4)

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.70s
```

### 10-second runs (stopped by PID with SIGTERM; both exit 143)

```text
$ ./target/release/look-body                 # stderr: 0 bytes
$ ./target/release/look-body --tiles 6 --pins 14 --scale 0.5   # stderr: 0 bytes
$ grep -cE "Theme parser|Gtk-WARNING|Gtk-CRITICAL" …  # 0 in both: no GTK CSS warnings [observed]
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-body --tiles 4 --scale 0.75 --pins 9 --drives 0.1,0.44,0.96
[gimbal-look-body 0.087] layout: scale 0.75, sidebar 261 px        # 348 × 0.75 = 261 ✓
$ ./target/release/look-body --bogus   # prints "unknown flag: --bogus" + usage, exit 2 [observed]
```

Unit tests cover: the CLI parser (defaults, all flags, 8 rejected inputs), style-spec rules 1+2
(no `#`/`rgb(`/keywords; `font-family` only with `--font`), one scaled token at 0.5 (24 → 12, and
1 px line stays 1), the drive-bar class function (0.0, 0.5, 0.94, 0.949 → `lvN`, 0.95, 1.0 →
`bar-full`), `counts`, and `tile_rects` for 2, 3, 4, 6 tiles.

## API citations

- `GtkGridView.max-columns` defaults to 7 (`gtk-4.22.4:gtk/gtkgridview.c:42`); we set 1000
  (`gtk4-0.11.5:src/auto/grid_view.rs:164,172` for the setters). Grid spacing is not a property:
  it comes from the CSS `border-spacing` read by `gtk_list_base_get_border_spacing`
  (`gtk-4.22.4:gtk/gtklistbase.c:2207–2226`; property registered at
  `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1316`).
- `all: unset` shorthand (`gtk-4.22.4:gtk/gtkcssshorthandpropertyimpl.c:1239`); `-gtk-icon-size`
  (`gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1280`, inherits, so the window's value would cascade —
  every icon-bearing class sets its own).
- GTK4 has **no** `size-allocate` signal to watch (checked `gtkwidget.c` signal list): viewport
  size for `tile_rects` comes from the ScrolledWindow's h/v `Adjustment::page-size` notify
  (page size = the visible area). Same for the Paned's first allocation: `notify::max-position`.
- `SingleSelection::new` takes `Option` (`gtk4-0.11.5:src/auto/single_selection.rs:39`);
  `SignalListItemFactory` callbacks give `&glib::Object` (downcast to `ListItem`);
  `Label::set_lines` for the 2-line grid labels (`gtk4-0.11.5:src/auto/label.rs:328`).
- Non-`Send` closures need `connect_notify_local`, not `connect_notify`
  (`glib-0.22.10:src/object.rs:2185`).
- `adw::TimedAnimation::new(widget, from, to, duration, target)` takes the target by value
  (`libadwaita-0.9.2:src/auto/timed_animation.rs:25`); `AnimationExt::connect_done` for the
  fade-out completion. `adw::prelude::*` is needed alongside `gtk::prelude::*`
  (AnimationExt, AdwApplicationWindowExt::set_content).
- `gtk::Fixed::move_/put/remove` (`gtk4-0.11.5:src/auto/fixed.rs:305,317,329`); Paned resize/shrink
  flags (`gtk4-0.11.5:src/auto/paned.rs:124–164`).

## libadwaita styles I had to undo/restore after `all: unset`

Count: 3 of our classes are built on `gtk::Button` (`gl-icon-button`, `gl-pill-close`,
`gl-section-pill`), each needing `all: unset` and then three restorations.

1. **Button geometry**: Adwaita gives every button `min-height: 24px; padding: 5px 10px;` and a
   pill radius (`libadwaita-1.9.1:src/stylesheet/widgets/_buttons.scss:20–22`) — unset, then
   restored with our tokens (padding `gap-s`, `border-radius: radius`/`radius-pill`).
2. **Hover feedback**: Adwaita's hover background is gone after unset; restored as
   `:hover { background-color: alpha(@window_fg_color, 0.1) }` (spec's `gl-icon-button`).
3. **Focus ring (the worst one)**: buttons draw focus with a `focus-ring()` mixin, i.e. an
   *outline* with offset and transition (`_buttons.scss:26` and the mixin it includes). After
   `unset` there is no ring at all, and GTK-CSS outlines can't be spread outside like the spec's
   `gap-s/2` ring; I restored it as `box-shadow: 0 0 0 <line-thin>px alpha(@accent_color, 0.5)`
   on `:focus-visible` for all three classes — visually close, not identical.

Kept on purpose: the Paned separator's 1 px hairline
(`_paned.scss:13–18` draws it with an inset box-shadow) — it doubles as the drag affordance — and
the right-click popover's default menu look. The `gl-pill`, `gl-tile`, `gl-pinned*`, `gl-cell`,
`gl-drive-*` classes are Boxes/Labels/Images, so nothing to unset except what the spec says.
No colour is cached in Rust: fills/labels carry class names only, and every fill width is
recomputed from the token table on each scale/fullness change.

## How `gtk::Paned` handled minimums and the collapse strip

`width_request = 260 × scale` on the panel plus `shrink-start-child(false)` makes the handle stop
exactly at the minimum; `resize-start-child(false)/resize-end-child(true)` gives all window-resize
space to the content, so the sidebar keeps its width until dragged
(`gtk4-0.11.5:src/auto/paned.rs:124–164`). Collapse swaps the start child to a `gl-collapse-strip`
(`gap-2xl` wide, same `sidebar-show-symbolic` button) and calls `set_position` again; expand swaps
the panel back. The initial position is applied on the first `max-position` notify, because GTK4
offers no allocation signal. [inferred] A drag handle still exists next to the collapsed strip; I
did not disable it, so the strip could in principle be dragged wider — Claude should confirm this
is harmless or I can fix it in Phase 2.

## What I could not verify without a live session (Claude/Kyle checks)

- Visual match against `sketch/default.png`/`sketch/split.png` at 2560×1440, accent outlines,
  desktop font, and whether colours follow a wallpaper change (no colour is cached, so it should).
- Grid column count 11 → 1 on narrowing, >11 on widening; hover/selection look of cells.
- Pin list growth pushing sections, panel scrolling with the main drive pinned at the bottom.
- The fade smoothness (250 ms, container opacity), focus ring, Ctrl/Shift selection, close-to-single.
- Live rescale on the slider and `GIMBAL_LOOK_BASE=1.25`.
- The dev bar's drive slider driving the first Devices bar live (wired to
  `LeftPanel::set_drive(0, …)`; contrast rises per `bar_class`, error colour from 95 %).

## Anything I'd change outside my files

Nothing. `Cargo.toml` was not edited; no existing file was touched. If the 900-line budget is ever
relaxed, the first things back are tile drag-rearranging (~35 lines: a `GestureDrag` on the pill
plus a rect hit-test against `tile_rects`) and the preview sidebar (~80 lines including its Paned
column), in that order per the brief.

## Review (Claude, Nov 1, 2026)

**Verdict: needs a round of fixes (`P1j-r1-fixes.md`), then the live check.** What's here is
good: the style engine (the most readable stylesheet of the two prototypes), `tile_rects` and its
four tests, `bar_class`, the Paned minimums, and the honest list of what was dropped.

Checked: only `src/bin/look-body/` and this file changed; `Cargo.toml` unchanged; the four RULES
commands pass from a clean build (10 tests); no colour literal or `font-family` outside the
`--font` path; the colour table and bar levels match `look-style-spec.md`; runs with no flags and
with `--tiles 4 --scale 0.5` print no GTK CSS warnings.

1. **Add the preview sidebar** (P1j's "Preview sidebar" section, and the right-hand part of the
   Paned layout). It's part of Kyle's design, and he added its collapse icon to the sketch
   himself. It was last on the priority list only because the line budget was too tight, which
   was Claude's mistake.
2. **Add tile rearranging** (P1j's split-view section): drag a tile's pill to move the tile to
   another slot; the other tiles slide to their new rects. Kyle asked for this directly (round 3,
   item 2). Use a pure function over `tile_rects` for the hit test, and unit test it.
3. **`max-columns(1000)` keeps far too many cells.** P1e found (`notes/P1e.md`, "G_MAXUINT is not
   no cap") that GTK 4.22.4 sizes the grid's cell pool from `max-columns`: `32 × max + 1` cells,
   so 1000 means up to 32,001 per grid once a folder has that many items, and a split view has up
   to six grids. The P1j brief said "max-columns high", so this is the brief's fault, not yours.
   Set `max-columns` from the grid's width: the columns that fit at the minimum cell width, plus
   one, recomputed when the width or scale changes. Put that in a pure function and test it:
   the cap is never below the column count the width allows, it shrinks for a narrow tile, and it
   roughly doubles at scale 0.5.
4. **Spacing in Rust:** `sidebar.rs:156` builds the pinned row with `Box::new(_, 8)`. Move it to a
   CSS margin from the token table (style spec rule 3), so it scales.
5. **The collapsed sidebar's handle still drags.** While a sidebar is collapsed, its strip must
   stay `gap-2xl` wide; dragging the handle must not widen it.
6. **Readable code, new line budget.** Drop `#[rustfmt::skip]` everywhere except the constant
   tables, and keep one statement per line. The new budget is **1,600 lines** under
   `src/bin/look-body/`.

## P1j-r1 changes

Fixes round per the review above. 1,598 lines under `src/bin/look-body/` (budget 1,600);
`Cargo.toml` still untouched; P1i's `look-header` was being edited in parallel and all builds
stayed green throughout.

1. **Preview sidebar** — new `preview.rs`: `gl-panel` with title "File Name (25 MB)", collapse
   button (`sidebar-show-right-symbolic`), rule, a scroller with 40 fake text lines (line numbers
   `gl-num`, `text-mono-ish`, right-aligned, `gap-m` from the text via the existing `.gl-pline`
   margin), then a rule and three `gl-icon-button`s at `icon-row`: Discard / Save / Save As — each
   only calls `log!("preview", …)`. The body is now two nested `gtk::Paned`s: left sidebar |
   (content | preview), preview 648 wide, minimum 360 (× scale), `resize_end_child(false)` so the
   pane keeps its width when the window resizes. `PREVIEW_W`/`PREVIEW_MIN` joined the token table.
2. **Tile rearranging** — a `gtk::GestureDrag` on each tile's pill. On drag end, the drop point
   (the dragged tile's rect origin — the pill sits at it — plus the drag offsets) goes through the
   pure hit test `tile_at(&[Rect], x, y)` (`split.rs`); a hit on another slot moves the tile and
   `rebuild()` reflows the rest. Unit tested (`hit_test`): each quadrant, the `gap-2xl` gutters,
   and outside the view. [inferred] The pill-drag coexists with the pill's click gesture; Claude
   should confirm the feel live.
3. **`max-columns` from the viewport** — `grid::max_columns(view_w, scale)`: the columns that fit
   at the minimum cell width (120 × scale + `gap-s` pitch, `grid-margin` side margins subtracted),
   plus one; recomputed on the scroller's `hadjustment` `page-size` notify (viewport width) and on
   scale changes via `grid::set_scale` (a thread-local scale plus a registry of weak scroller
   references, pruned as tiles die). Tested: 12 at the sketch's 1544 px viewport (11 fit), 5 in a
   695 px tile, ≥ 22 at scale 0.5, ≥ 1 always.
4. **Pinned-row spacing** — `sidebar.rs` builds the row with `Box::new(_, 0)`; the icon–label gap
   is now `margin-left: gap-s` on `.gl-pinned-row label`. The only `Box::new(_, N)` with N > 0
   left is the dev bar (`Horizontal, 6`).
5. **Collapsed sidebars can't be dragged wider** — while a sidebar is collapsed, a
   `notify::position` guard on its paned snaps the handle back to the strip width (`gtk_paned_set_position`
   only notifies on a real change, so the guard converges — see citations). This works for both
   paneds: the left strip is the start child (target = strip width); the preview strip is the end
   child (target = `max-position`, the strip keeps its `gap-2xl` minimum). Because the preview is
   the end child, its width needs `position = max-position + min − width`; a swap of end children
   changes `max-position` only at allocation time, so `right_dirty` re-applies the saved width on
   the next `max-position` notify (at startup and after every collapse/expand).
6. **Readability** — `#[rustfmt::skip]` remains only on constant tables (`style.rs` `tok`/`col`,
   `sidebar.rs` `DEFAULT_PINS`/`BAR_FILL_CLASSES`); everything else is rustfmt-clean with one
   statement per line. One P1j behaviour was not restored to keep the budget: dragging a sidebar
   border no longer updates the saved width, so a scale change resets both sidebars and the
   preview pane to their default widths (P1j behaved the same way).

Also changed while in there: the grid's per-view column cap made the old "no cap" note obsolete,
and the paned handlers share an `on_notify` helper.

### Verified

RULES.md's four commands (from `spike/`):

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(exit 0, no output)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.13s

$ cargo test
     Running unittests src/main.rs …            test result: ok. 4 passed
     Running unittests src/bin/look-body/main.rs …  test result: ok. 12 passed
     Running unittests src/bin/look-header/main.rs … test result: ok. 5 passed
(look-body's 12: CLI ×2, style rules 1+2 + scaled token + drive classes, grid counts + column cap,
tile_rects ×4 + tile hit test)

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.73s
```

10-second runs (release binary, stopped by PID with SIGTERM; both exit 143):

```text
$ ./target/release/look-body                                 # stderr: 0 bytes
$ ./target/release/look-body --tiles 6 --pins 14 --scale 0.5 # stderr: 0 bytes
$ grep -cE "Theme parser|Gtk-WARNING|Gtk-CRITICAL" …         # 0 and 0: no GTK CSS warnings [observed]
```

### API citations for the new work

- `gtk_paned_compute_position`: with `position_set`, a position stored before the first
  allocation is honoured (clamped) (`gtk-4.22.4:gtk/gtkpaned.c:1125–1165`); `max-position =
  allocation − end_child_req` when `!shrink_end_child` (no handle/min-start subtraction, hence the
  `max + min − width` arithmetic for the preview) (`gtkpaned.c:1131–1135`); the resize
  flags preserve the fixed child's size on window resize (`gtkpaned.c:1150–1162`);
  `gtk_paned_set_position` notifies `position` only when the value actually changes, which is what
  makes the collapse guard converge (`gtkpaned.c:1917`).
- `GestureDrag::connect_drag_end(g, offset_x, offset_y)` with cumulative offsets
  (`gtk4-0.11.5:src/auto/gesture_drag.rs:191`); `GridView::set_max_columns(u32)`
  (`gtk4-0.11.5:src/auto/grid_view.rs:164`).
- `gtk_widget_translate_coordinates` is deprecated since 4.12 (`gtk4-0.11.5:src/auto/widget.rs:1490`),
  so the drop point is derived from the tile's rect instead of translating the pill's position.

### Not verifiable without a live session (Claude/Kyle check)

- Tile drag feel: drag a pill onto another tile, check the reflow and that a click-without-drag
  still focuses/selects; the drop point uses the tile's rect origin, not the cursor's exact pill
  offset [inferred].
- The preview pane's width arithmetic: default 648, minimum 360, collapse to the `gap-2xl` strip,
  no drag-widening while collapsed, width preserved on window resize, and the brief one-allocation
  delay when swapping the strip in/out.
- The preview's visual match against `sketch/default.png` (line-number column, action row).
- Left sidebar: a drag while collapsed snaps back to the strip; a drag while expanded resizes and
  survives until the next scale change (see item 6).

## Review of P1j-r1 (Claude, Nov 1, 2026)

**Verdict: one more round (`P1j-r2-fixes.md`), for the tile drag.** Corrections 1, 3, 4, 5 and 6
are done: the preview pane with its Paned arithmetic and collapse guard, `max_columns` from the
viewport with its tests, the pinned-row margin in CSS, and rustfmt owning the code. The four
RULES commands pass on the combined tree (21 tests).

**The line budget is dropped.** Don't drop behaviour to fit a number again: restore what item 6
gave up (a dragged sidebar border updates the saved width, so a scale change keeps it).

1. **Bug: moving a tile has no visible effect.** Tiles have no identity: `tile_widget(i, …)`
   labels the tile "Tab {i+1}" from its position, and `move_tile` reorders only `selected` and
   then rebuilds, so every label comes back in order and only the focus moves. Give each tile an
   id (its tab name) that moves with it.
2. **The drag must look like dragging** (Kyle, round 3, item 2; the same as P1i's pills). While
   dragging, the tile follows the pointer and the tiles it displaces slide to their new rects
   (about 150 ms); on release it settles into its slot. Move the existing tile widgets (don't
   rebuild them, which also recreates every grid). Raise the dragged tile with
   `insert_after(&fixed, fixed.last_child())`, **never** remove and re-add it: unparenting resets
   its gestures and cancels the drag (`gtkwidget.c:2702`, `:7963`; `gtkgesture.c:748-751`).
3. **Hit-test the pointer, not the tile's corner.** `drop_pill` uses `rect.x + dx`, which lands
   up to half a pill away from where the pointer is. Use the press point
   (`GestureDrag::start_point`, translated by where the pill sits in its tile) plus the offset.
   Put the slot-and-slide arithmetic in a pure function over `tile_rects`, and test it.

## P1j-r2 changes

Fixes per "Review of P1j-r1" above: tile identity, the live drag, the pointer hit test, and the
saved sidebar width back. No line budget; 1,994 lines under `src/bin/look-body/` (was 1,598).
`Cargo.toml` untouched; only `src/bin/look-body/{split,main}.rs` and this file changed.

1. **Tile identity** — the model is now `Vec<Tile>` (`Tile { id, name, selected }`), indexed by
   slot; `set_count` mints "Tab 1…N" and the name rides with the tile through every move.
   `tile_widget` labels from the model, `move_tile` no longer rebuilds (it reorders the model and
   the widget vectors and re-applies classes), so after moving "Tab 1" to the last slot the last
   tile says "Tab 1". Handlers capture the tile's `id`, not its slot, and resolve the slot through
   `slot_of(id)` at event time — necessary now that widgets outlive reorders.
2. **The drag looks like dragging** — the tile follows the pointer 1:1; tiles it displaces slide
   to their new rects over 150 ms (`adw::TimedAnimation` on the Fixed position via a lerp target);
   on release the dragged tile settles into its slot (150 ms slide back if the drop was invalid).
   Existing tile widgets are never rebuilt in the drag path — `move_tile` only reorders vectors.
   The raise on drag begin is `insert_after(&fixed, last_child())`, which reorders in place.
   **Where the gesture lives:** the one `GestureDrag` is attached to the static `gtk::Fixed`, not
   the pill. Event coordinates are translated into each widget's own space at dispatch
   (`translate_event_coordinates`, gtkwidget.c:4975/:5008, via `gtk_widget_compute_point`,
   gtkmain.c:1180–1196), so a gesture on the moving pill would see its offsets collapse as the
   tile catches up with the pointer. On the Fixed the offsets (`last − start`,
   gtkgesturedrag.c:106–134) stay true. The begin signal's start point is hit-tested against the
   pills (`pill_at`); drags that don't start on a pill are dropped. `GestureDrag` never claims the
   sequence (no `set_state` in gtkgesturedrag.c) and `GestureClick` never claims either (no
   CLAIMED in gtkgestureclick.c), so pill clicks and grid selection still work.
3. **Pointer hit test + pure slide arithmetic** — `drag_layout(rects, from, px, py, grab)` (pure,
   over `tile_rects` output) returns the slot under the pointer and, indexed by slot, every tile's
   rect during the drag; the drop point is the press point (the pill sits inside the tile's border,
   so the press point is the grab offset) plus the drag offset. Unit tested for 2, 3 and 6 tiles:
   forward, backward, gutter, outside the view, plus the grab offset; `pill_at` tested too.
4. **Saved sidebar width restored** — `Ui` keeps `left_width`/`right_width` in design px. The
   `position` notify handlers record `position / scale` (left) and `max-position + min − position`
   over scale (right, the inverse of r1's `right_target`) whenever the pane is expanded, so a drag
   of either border updates the saved width and a scale change re-applies it scaled. Recording is
   suppressed while `right_dirty` is set (startup and collapse/expand swaps notify with positions
   that aren't the saved width's) and while collapsed; collapsed panes keep r1's snap-back guard.
5. **Slides are retarget-safe** — each slot has a shared position cell (it moves with the tile on
   reorder, so running animations stay consistent) and a generation counter; replacing a slide
   bumps the generation and superseded ticks no-op. This matters because a playing animation keeps
   itself alive (`adw_animation_play` refs itself, libadwaita adw-animation.c:248), so dropping
   the Rust handle would not stop it. `relayout` cancels all slides and moves directly.

### Verified

RULES.md's four commands (from `spike/`):

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(exit 0, no output)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s

$ cargo test
     Running unittests src/main.rs …                    test result: ok. 4 passed
     Running unittests src/bin/look-body/main.rs …      test result: ok. 17 passed
     Running unittests src/bin/look-header/main.rs …    test result: ok. 5 passed
(look-body's 17: CLI ×2, style rules 1+2 + scaled token + drive classes, grid counts + column cap,
tile_rects ×4, tile hit test, pill hit test, drag forward/backward/gutter+outside/grab offset ×4)

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.85s
```

10-second runs (release binary, stopped by PID with SIGTERM; both exit 143):

```text
$ ./target/release/look-body                                 # stderr 232 bytes, css_warnings=0
(look-body:20504): Gdk-WARNING **: vkAcquireNextImageKHR(): … (VK_SUBOPTIMAL_KHR) (1000001003)
  — a Vulkan swapchain notice from Gdk, not a GTK CSS warning; not reproducible every run.
$ ./target/release/look-body --tiles 6 --pins 14 --scale 0.5 # stderr 0 bytes, css_warnings=0
$ grep -cE "Theme parser|Gtk-WARNING|Gtk-CRITICAL" …         # 0 and 0 [observed]
$ GIMBAL_SPIKE_LOG=1 … --tiles 3 --scale 0.75
[gimbal-look-body 0.088] layout: scale 0.75: left 261, preview 486   # 348/648 × 0.75 ✓
```

### API citations for the new work

- `gtk_widget_insert_after` reorders an already-parented widget in place — docs at
  gtkwidget.c:12484–12486 ("If @parent is already set as the parent … can also be used to
  reorder"); it early-outs when `widget == previous_sibling` (:12501), so raising the tile with
  `insert_after(&fixed, last_child())` is a no-op when it is already last; `reposition_after`
  only relinks the sibling list, no unparenting (gtkwidget.c:6221–6264) — gestures survive.
- Gesture coordinates are per-widget at dispatch: `translate_event_coordinates` before both
  capture and bubble controllers (gtkwidget.c:4975, :5008), converted from the native via
  `gtk_widget_compute_point` (gtkmain.c:1180–1196); "relative to the widget allocation"
  (gtkgesture.c:1281); `start_point` "in widget-relative coordinates" (gtkgesturedrag.c:233).
- `GestureDrag` claims no sequence (gtkgesturedrag.c has no `set_state` call; claiming is explicit,
  gtkgesture.c:1070+), `GestureClick` neither — the Fixed's drag coexists with the pill's click.
- `adw_animation_play` refs the animation while playing (libadwaita-1.9.1:src/adw-animation.c:248)
  and stops it only on unmap — hence the generation guard instead of relying on drops.
- `GestureDrag::connect_drag_begin` hands out the start point (`gtk4-0.11.5:src/auto/gesture_single.rs`
  signal args), used instead of `start_point()`.
- The distro cargo 1.93.1 reserves `gen` as a keyword (gen blocks, rust issue #117078) — the
  generation counter is named `generation`/`slide_gen` to stay buildable on both toolchains.
- Saved-width arithmetic reuses r1's paned facts: a position set before the first allocation is
  honoured clamped (gtkpaned.c:1125–1165), `max-position = allocation − end_child_req` when
  `!shrink_end_child` (:1131–1135), `set_position` notifies only on real change (:1917).

### Not verifiable without a live session (Claude/Kyle check)

- Drag feel: the tile should track the pointer exactly (grab offset from the press point) and the
  displaced tiles slide ~150 ms; a click-without-drag on a pill must still focus/select (the drag
  gesture claims nothing) [inferred].
- Dragging onto the gutter or outside the view settles the tile back into its slot.
- A drag across a scrolled outer view: the Fixed's offsets are stable while it doesn't scroll;
  scrolling mid-drag would skew them [inferred, edge case].
- Sidebar borders: dragging either border updates the saved width and survives a scale change
  (scaled); collapse/expand keeps the saved width; the collapsed strips still snap back.

## Review of P1j-r2 (Claude, Nov 1, 2026)

**Verdict: accepted on code, with one finding for the live-check round.** Only
`src/bin/look-body/{split,main}.rs` and this file changed; the four RULES commands pass on the
combined tree (26 tests, clippy clean on a real compile). 1,994 lines, no budget.

Good: tiles carry an id and name through moves and handlers resolve the slot at event time;
existing widgets are moved, never rebuilt, in the drag path; the raise is `insert_after`; putting
the one `GestureDrag` on the static `Fixed` sidesteps the moving-widget coordinate problem, and
since the body has no `WindowHandle`, not claiming the sequence is safe here (P1i's header is the
case where it isn't: see its live check). `drag_layout` is pure and tested for 2, 3 and 6 tiles;
the saved sidebar widths are back.

1. **Finding: `move_tile` doesn't reorder `slide_gen` (or `slides`).** It reorders `tiles`,
   `widgets`, `pills` and `pos` to the new slot order, but the generation cells stay in the old
   order. The settle then calls `slide_to(to, …)`, which bumps `slide_gen[to]`: the cell of the
   tile that *was* in slot `to`, usually a displaced tile whose 150 ms slide is still running. Its
   ticks then no-op and it stops wherever it was. Example: 2 tiles, drop Tab 1 onto Tab 2 within
   150 ms of crossing into it; Tab 2 is stranded part-way until the next relayout. Reorder
   `slide_gen` and `slides` with the others (or keep all per-tile state in one struct per slot so
   they can't drift apart). Not sending a round for this alone: it goes with whatever the live
   check finds.

### Live check of P1j-r2 (Claude, Nov 1, 2026): needs fixes (`P1j-r3-fixes.md`)

Floating window on DP-2, 2560×1406 then narrowed, pointer through the runner's virtual mouse
(left button only: right-click menus, Ctrl/Shift selection and the section pills were not tested).
No GTK warnings in the whole run.

What passed:

- **The tile drag.** Dragging Tab 1's pill onto Tab 4: mid-drag the tile follows the pointer and
  Tabs 2-4 have already slid into place; on release it settles in the last slot and every tile
  keeps its name. Logs: `dragging 'Tab 1' from slot 0`, `'Tab 1' moved from slot 0 to 3`.
- **Collapse:** the left sidebar collapses to its strip, and dragging the strip doesn't widen it;
  expanding brings it back at 348.
- **Border drag:** dragging the preview's border widened it by about 250 px, and the tiles reflowed.
- **Live scale** from the dev slider: everything follows, and at 0.75 each tile fits 7 columns.
- **The window can be narrowed** (unlike P1i's header): niri's 1,600 and 1,200 px widths took
  effect.

Corrections:

1. **The fast-drop bug from the review reproduces.** Tab 2 dragged onto Tab 3 and released at
   once: Tab 3 stopped part-way through its slide, overlapping Tab 2's slot, until the next
   relayout. Fix as the review says (reorder `slide_gen` and `slides` with the rest, or one struct
   per slot).
2. **Narrowing squeezes the content to nothing.** At 1,400 px the content column was about 150 px
   with the tiles clipped behind the preview; at 1,200 it was gone, and the app drew a strip past
   the window's right edge (its minimum width is above what niri gave it). Kyle's decision
   (sketch-values.md, round 1, 4c): narrowing drops grid columns one at a time **down to 1**, so
   the content keeps at least one column, and the sidebars give way first, down to their minimums
   (348 and 360 × scale). The window's minimum width is then the two sidebar minimums plus one
   column, and nothing may draw outside the window.
3. **The saved preview width drifts across scale changes.** After dragging the preview to about
   896 px: 0.75 gave 712 (expected 672), and two round trips back to 1.00 gave 917, then 906. The
   `position` notifications that the scale change itself causes are being recorded as user drags.
   Record only while the user drags the handle (or ignore notifications during `apply_scale` and
   the allocation that follows), and test it: the width at 1.00 after any number of scale round
   trips equals the width before.
4. **Look against `sketch/split.png` and `sketch/default.png`:**
   - Split view: each tab pill is short and centred **above** its tile's frame in the sketch; here
     it spans the tile's full width on its top edge.
   - The tiles aren't centred: the `2 × gap-2xl` inset is all taken from the right and bottom, so
     there's an empty strip on the right and none on the left. Split the inset evenly.
   - Inside a tile, the grid is a narrower dark box with 4 columns; the sketch fills the tile with
     5. The single view shows 10 columns where the sketch has 11 at the same width: check the
     cell's real width (padding, margins) against `cell-w` and `gap-s`.
   - The left sidebar is missing the **Remote** section pill (Kyle's decision 4f lists Recent
     Directories, Devices and Remote), and the main drive sits right after the other drives
     instead of at the bottom of the panel.
   - Home uses a folder icon; the sketch has a house (`user-home-symbolic`).
