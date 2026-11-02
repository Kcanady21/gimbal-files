# P1m: body polish from the P1h acceptance run

Built on P1j-r5 in `spike/src/bin/look-body/`. Only files under that folder and this note were
changed. Every item in the brief is covered; the body's existing features (split, tile drag,
selection, sidebars, collapse, sections, drive bars, scale, transparency, blur) are kept.

## What changed, item by item

1. **Split spacing and position.** `tile_rects` now uses `gap-l` (24) between slots in both axes,
   keeps the `gap-2xl` side/bottom view inset, and starts the first row at y 0 so the pills' top
   edges are level with the side panels. The pill-to-frame gap is `gap-s` (8), set by
   `.gl-pill.tab { margin-bottom: 8px }`. `pill_at` and `press_on_pill` still use the slot top as
   the pill top. `one_column_width` still counts the split view's `2 × gap-2xl + 2 × gap-m +
   2 × line-thin + cell`, now 213 px (line-thin 2); the single view's 241 px still sets the floor.
   The tile's frame height is `slot − control-h − gap-s`, so slots (pill + gap + frame) share the
   visible height, not the frames alone. [read/observed: timer, below]
2. **Taller columns.** `column_counts(n) = (n.div_ceil(2), n/2)`, row-major slot order: slot `i` is
   column `i % 2`, row `i / 2`. With 3 tiles: 2 left + 1 full-height right; 4: 2 + 2; 5: 3 + 2;
   6: 3 + 3. A column's slots are `(inner_h − (count−1)·gap-l) / count` tall, so the shorter
   column's tiles are taller. Beyond 4 tiles a slot is floored at `(inner_h − gap-l)/2` (the
   two-slot height, i.e. half the visible height less the half-gap) and the view scrolls. The
   row-major order keeps a drag simple: the slot right of a tile is +1 and the slot below is +2,
   even where one column is shorter. Pure `tile_rects` + `column_counts`, tests for 2–6 tiles.
   [read/inferred: rule is from the brief; the on-screen judgement is Kyle's]
3. **Sidebar type hierarchy.** New `text-item` token 22; section pills 22 text / 32 tall; pinned
   rows 22 text / 24 icons (`icon-pin`); directory and drive names 22; grid labels stay
   `text-row` 20. `text-section` (28) is no longer used and was removed from the token table.
4. **Mockup icons.** New `src/bin/look-body/icons/`:
   `gimbal-folder-symbolic.svg`, `gimbal-save-symbolic.svg`, `gimbal-save-as-symbolic.svg`,
   `gimbal-discard-symbolic.svg`. Geometry and path ids are copied from
   `gimbal-files-prompts/phase1/icons/*.svg`; every part is a GTK 4.22 `gpa:fill`/`gpa:stroke`
   symbolic slot. The folder uses `foreground` (tab, fill-opacity 0.85), `accent` (front),
   `success` (back), `warning` (side), `error` (front-shadow). `.gl-cell image` sets
   `-gtk-icon-palette: accent @accent_color, success shade(@accent_color, 0.7), warning
   shade(@accent_color, 0.6), error shade(@accent_color, 0.45)`. Save/Save As/Discard strokes use
   `foreground`; Save As' pencil fill is `foreground` and its outline is `error` remapped by
   `SAVE_AS_PALETTE` to `@window_bg_color`. Folders in the grid load the new name; files keep
   `text-x-generic`. The four names are found through an icon search path added at startup
   (`gtk::IconTheme::for_display(...).add_search_path(...)`), never a fixed path or colour. The
   accent slot is remappable in GTK 4.22, so no success/warning/error fallback was needed for it.
   [read: GTK source; observed: `has_icon` true for all four]
5. **Contrast.** `FOCUS_RING` is now `@accent_color` (was `alpha(@accent_color, 0.5)`); `selected`
   and `selected-active` mix 0.5 accent (was 0.3). Unit-tested in `style::tests`.
6. **Circular close buttons.** The ring was 28×44: the button's content width was 28 but the pill
   Box stretched the child vertically to the 44 px content height (a Box child fills its cross
   axis by default). `close.set_valign(gtk::Align::Center)` (and halign Centre) keeps the 28×28
   square; the ✕ image is centred by the button. Timer measured four rings at 28×28. [observed]
7. **Collapse button stays put.** The collapsed strip is now the button's own minimum (48) plus
   the panel's inner padding (`gap-m` 16 + `line-mid` 3) on the inner/top edge: 67 px at scale 1,
   34 at 0.5 (`layout::strip_width`). The strip's left/right padding is 19 and the button is
   start/end aligned; the left panel's collapse button moved to the row's start (window edge) so
   both sides share the strip's edge. Timer: left 27,161,48,48 → 27,161,48,48; preview
   1197,161,48,48 → 1197,161,48,48 in a 1272-wide tiled window. [observed]
8. **Preview and pinned rows.** The preview scroller is `External` horizontally with
   `propagate_natural_width(false)`, so the labels get the viewport width; text labels wrap
   (`WordChar`) and line numbers are start-aligned, so a wrapped line keeps its number at its top.
   Pinned rows are a vertical Box inside `gl-pinned`; a `GestureDrag` on the Box claims after the
   threshold and moves rows with top margins, so the dragged row follows the pointer and the
   others slide 150 ms to their post-move slots. It only reorders children (`insert_after` /
   `reorder_child_after`), never unparents during the drag; clicks still select. Pure
   `pin_order` / `pin_drag_layout`, tests below. [read/inferred: feel is for Claude's live check]
9. **Live theme and keys.** `#[path = "../../theme.rs"] mod theme;` + `app.connect_startup(
   theme::init)`, exactly the spike's reload. The copy under `/tmp/gimbal-spike-p1m/` was changed
   from a temporary timer; the log below shows the reload and the resolved `accent_color` changing
   to the new value. Ctrl+= / Ctrl+plus / KP_Add and Ctrl+- / KP_Subtract step the dev-bar scale
   slider by ±0.05, clamped 0.50–1.00 (`stepped_scale`, unit-tested). [observed: reload; inferred:
   icon pixels, key events]
10. **Outlines.** `line-thin` 3 → 2 and `line-mid` 6 → 3 in the token table, so panels/pills/path
    bar/search field are 3 and section pills/tiles/trash/✕ rings/focus ring are 2. [read]

## Unit tests added

- `split::tests::column_counts_are_row_major`
- `split::tests::uneven_columns_make_their_tiles_taller` (2–6 tiles, frame heights, scroll)
- `split::tests::five_tiles_never_drop_below_half_the_visible_height`
- `split::tests::two_tiles_fill_the_height`, `three_tiles_odd_left`, `six_tiles_scroll`,
  `hit_test`, `pill_hit_test` updated to the new geometry
- `sidebar::tests::pin_order_moves_one_row`
- `sidebar::tests::pinned_drag_follows_the_pointer_and_slides_the_others`
- `layout::tests::the_strip_keeps_the_collapse_button_in_place` (scale 1.0 and 0.5)
- `style::tests::nov_1_outlines_and_contrast`
- `style::tests::icon_palettes_map_the_spec_table` (palette entries + SVG slots, no grey/hex left)
- `tests::ctrl_scale_steps_and_clamps`

## Commands (RULES.md's four), from `$SPIKE`

`cargo fmt --check`:

```text
fmt: OK
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
```

(While P1l's agent was editing `look-header`, its deprecated `allocation`/`style_context` calls
failed this command; I waited and re-ran until its file compiled. I made no substantive edit to it
(only the required package-wide `cargo fmt` ran while P1l was writing). [observed])

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-52e7a6fb793e52cc)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-3e6a112f6dd1233d)
test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-c53a97a1ce9c8b42)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
    Finished `release` profile [optimized] target(s) in 0.05s
```

## 10-second runs (release binary, from `/tmp/gimbal-spike-p1m`, stopped by PID with SIGTERM)

Default:

```text
(no output)
```

`--tiles 3`:

```text
(no output)
```

`--tiles 6 --pins 14 --scale 0.5`:

```text
(look-body:272903): Gdk-WARNING **: 13:41:08.415: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
```

The `VK_SUBOPTIMAL_KHR` notice is the intermittent one the brief allows. No GTK CSS/theme/icon
warnings, no panics.

## Temporary timer logs (harness removed before delivery)

Run: debug binary, `GIMBAL_SPIKE_LOG=1 GIMBAL_SPIKE_THEME_CSS=/tmp/gimbal-spike-p1m/noctalia.css
look-body --tiles 4`, 8 s, niri tiled to 1272 × 1398. The timer logged after allocation, then
collapsed each sidebar, then rewrote the watched copy's `accent_color` from `#935fec` to `#22aa88`
and read it back.

```text
[gimbal-look-body 0.066] theme: reloaded file:///tmp/gimbal-spike-p1m/noctalia.css
[gimbal-look-body 2.348] temp: item1: panel top y Some(142), first pill top y Some(142), pill frame top y Some(200)
[gimbal-look-body 2.349] temp: item6: 4 close rings, width×height [(28, 28), (28, 28), (28, 28), (28, 28)]
[gimbal-look-body 2.349] temp: icon gimbal-folder-symbolic: has_icon true
[gimbal-look-body 2.349] temp: icon gimbal-save-symbolic: has_icon true
[gimbal-look-body 2.349] temp: icon gimbal-save-as-symbolic: has_icon true
[gimbal-look-body 2.349] temp: icon gimbal-discard-symbolic: has_icon true
[gimbal-look-body 2.749] temp: item7 left: collapse button Some((27, 161, 48, 48)) -> expand button Some((27, 161, 48, 48)); window 1272 wide
[gimbal-look-body 3.150] temp: item7 preview: collapse button Some((1197, 161, 48, 48)) -> expand button Some((1197, 161, 48, 48)); window 1272 wide
[gimbal-look-body 4.549] temp: item9: rewrote accent_color in /tmp/gimbal-spike-p1m/noctalia.css
[gimbal-look-body 4.549] theme: change Changed on file:///tmp/gimbal-spike-p1m/noctalia.css
[gimbal-look-body 4.549] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-p1m/noctalia.css
[gimbal-look-body 4.750] theme: reloaded file:///tmp/gimbal-spike-p1m/noctalia.css
[gimbal-look-body 5.449] temp: item9: accent after reload Some(RGBA { red: 0.13333334, green: 0.6666667, blue: 0.53333336, alpha: 1.0 })
```

`(34, 170, 136)` is `#22aa88`, the colour written by the timer, so the running window's resolved
accent changed without a restart. The symbolic icons read the palette's `@accent_color` /
`shade(@accent_color, …)` at draw time; GTK's `-gtk-icon-palette` property carries
`GTK_CSS_AFFECTS_ICON_REDRAW_SYMBOLIC` (`gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:887-894`), which
is what redraws them. The pixel change is for Claude's eye.

## Sizes and colours changed (old → new)

### Sizes

| Where | Old | New |
|---|---|---|
| `line-thin` | 3 | 2 |
| `line-mid` | 6 | 3 |
| Split tile gap (both axes) | 40 (`gap-2xl`) | 24 (`gap-l`) |
| Pill-to-frame gap | 24 (`gap-l`) | 8 (`gap-s`) |
| Split view top inset | 40 | 0 (pill tops level with panels) |
| Split view side/bottom inset | 40 | 40 (unchanged) |
| Column rule | rows of 2, fixed half height; odd tile bottom-left | per-column shares: 3 = 2+1, 4 = 2+2, 5 = 3+2, 6 = 3+3; beyond 4, slot ≥ half visible height and scrolls |
| `one_column_width` split part | 215 | 213 |
| `section-h` | 40 | 32 |
| Section pill text | 28 (`text-section`) | 22 (`text-item`) |
| Section pill border | 3 | 2 |
| Pinned row text | 28 | 22 |
| Pinned row icon | 32 (`icon-row`) | 24 (`icon-pin`) |
| Directory row text | 20 | 22 |
| Drive name text | 20 | 22 |
| Grid label text | 20 | 20 (unchanged) |
| Pill border | 6 | 3 |
| Tile border | 3 | 2 |
| ✕ ring border | 3 | 2 |
| ✕ ring content / total | 22 / 28 (but 28 × 44) | 24 / 28 (28 × 28; valign Centre) |
| Collapsed strip width scale 1 | 40 requested (48 actual button) | 67 (48 button + 19 padding) |
| Collapsed strip width scale 0.5 | 20 requested (24 actual button) | 34 |
| `text-section` token | 28 | removed; `text-item` 22 added |
| `icon-pin` token | — | 24 added |

### Positions

| Where | Old | New |
|---|---|---|
| First pill top | view top + 40 (`gap-2xl`) | view top + 0 (level with the panels) |
| Left collapse button | row end, x 289 in the 1272-wide timer window | row start, x 27 (same place as the strip's expand button) |
| Left panel title label start | x 27 | x 75 (button now first) |
| Collapsed strip expand button height | 1229 (stretched to the strip) | 48 (`valign: Start`; both buttons 48 × 48) |

### Colours

| Role | Old | New |
|---|---|---|
| `focus-ring` | `alpha(@accent_color, 0.5)` | `@accent_color` (full) |
| `selected` | `mix(shade(@window_bg_color, 0.8), @accent_color, 0.3)` | `... 0.5` |
| `selected-active` | `mix(@card_bg_color, @accent_color, 0.3)` | `... 0.5` |
| Folder front | mockup `#333333` | `@accent_color` (`accent` slot) |
| Folder back | `#656565` | `shade(@accent_color, 0.7)` (`success` slot) |
| Folder side | `#565656` | `shade(@accent_color, 0.6)` (`warning` slot) |
| Folder tab | `#8e8e8e` | `@window_fg_color` at 0.85 (`foreground` + fill-opacity) |
| Folder front-shadow | `#000000` | `shade(@accent_color, 0.45)` (`error` slot) |
| Save / Save As / Discard strokes | `#000000` | `@window_fg_color` (`foreground`) |
| Save As pencil fill / outline | `#000000` / `#d4d4d4` | `@window_fg_color` / `@window_bg_color` (`foreground` / `error` palette) |

No other colour expression changed. The CSS still contains no `#rrggbb`, `rgb()`, `hsl()` or colour
keyword other than `transparent`; the new SVGs contain no fixed colour either (`fill="none"` only).

## API citations

- Symbolic icon format: `gtk-4.22.4:docs/reference/gtk/icon-format.md:96-118` (paths/circles/rects,
  the supported attributes), `:120-150` (`gpa` namespace; `gpa:stroke`/`gpa:fill` accept
  `foreground`, `success`, `warning`, `error`, `accent` at `:132-133`), `:152-154` (`-symbolic.svg`
  file naming).
- Palette property and remapping: `gtk-4.22.4:docs/reference/gtk/css-properties.md:148-162` and
  `:223`. The docs' list of recognized names is error/warning/success; the 4.22 source also has
  `accent`: `gtk/gtkcssstyle.c:876-900` (the `names` array at `:883` includes `accent`, and
  `:892` looks each name up in the style's palette), `gtk/gtkcsspalettevalue.c:298-312` (default
  palette includes `accent`), `:318-362` (the parser accepts arbitrary name/colour pairs), `:364`
  (`get_color`).
- Symbolic colours are the five-color `GtkSymbolicPaintable` contract, with accent added in 4.22:
  `gtk-4.22.4:gtk/gtksymbolicpaintable.c:24-39`, `gtk/gtksvg.c:6472` (`accent` name), `:6525`
  (`gpa:accent` symbol), `:24235-24240` (accent participates in the rendernode cache check).
- `-gtk-icon-size` (inherited, affects icon texture): `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1280-1285`;
  the spec's first citation (`:1280`) is this property.
- `-gtk-icon-palette` property flags: `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:887-894`
  (`GTK_STYLE_PROPERTY_INHERIT | ANIMATED`, `GTK_CSS_AFFECTS_ICON_REDRAW_SYMBOLIC`), so a theme
  reload redraws symbolic icons.
- Icon search path: `gtk4-0.11.5:src/auto/icon_theme.rs:45-48` (`add_search_path`), `:197`
  (`for_display`).
- gtk-rs API used: `WidgetExt::set_valign/set_halign/set_margin_top/margin_top/
  drag_check_threshold/compute_bounds/reorder_child_after` in `gtk4-0.11.5:src/auto/widget.rs`
  and `box_.rs` (`reorder_child_after` at `:392-393`); `GestureExt::set_state` in
  `gtk4-0.11.5:src/auto/gesture.rs`; `ButtonExt::emit_clicked` in `:536` (used only by the
  removed timer, not by the delivered code).
- `gtk::TimedAnimation` / `CallbackAnimationTarget` (the 150 ms row slides) are the same
  `libadwaita-0.9.2` bindings P1j's tile slides used (no new dependency).

## Not verified live (Claude's live check)

- How the four mockup icons actually render: the folder's five tones and its 0.85 paper tab, the
  Save As pencil outline against the button's translucent background, aspect ratios at 80 px and
  32 px, and whether GTK honours the `accent` slot as this code expects. `has_icon` is true for all
  four, and no SVG parse warning was printed, but the pixels were not seen. [inferred]
- The pinned-row drag feel: follow-and-slide, the 150 ms margins, the raised row's draw order, the
  threshold claim inside the window handle, and that a plain click still selects. Unit tests cover
  the arithmetic and the child-order/field movement only. [inferred]
- The preview's wrapped lines and the line numbers riding at the top of each wrapped line at
  panel widths from 648 down to 360 and at scale 0.50. [inferred]
- Ctrl+= / Ctrl+- through the real window and focus states (the key logic is unit-tested; no
  pointer/keyboard injection was used, per RULES). [inferred]
- The left panel's title after moving its collapse button to the row's start (window edge); the
  timer measured the button position, not the look. [inferred]
- The theme reload's effect on the icon pixels: the log proves the provider reloaded and the
  resolved accent changed; GTK's symbolic redraw flag is `[read]`, the visual is not.
- The 10-second runs were niri-tiled at 1272 × 1398; floating 2560 × 1440 was not available to me.

## Anything I'd change outside my files

No substantive edit was made outside the brief's Output list; the required package-wide `cargo fmt`
ran while P1l was editing `look-header`. Two notes for Phase 2:

- The icon search path points at `env!("CARGO_MANIFEST_DIR")/src/bin/look-body/icons`; a shipped
  Phase 2 app should embed the four SVGs in a GResource so the binary is self-contained.
- `look-style-spec.md` still lists `text-section` 28 and `icon-row` 32 for pinned rows; P1m's
  item 3 supersedes those rows (22/22 and 24). The spec's "Changes after the P1h acceptance run"
  section already states the new values; its type-token table could drop the stale row. I did not
  edit the spec.
- The collapsed strip is wider than the old `gap-2xl` (67 at scale 1) because it must hold the
  48 px icon button plus the 19 px inner/top padding that reproduce the collapse button's
  position. If Phase 2 wants a 40 px strip, the expand button has to be a floating overlay or the
  collapse button has to move in from the panel's inner edge; both are design choices for Kyle.

## Review and live check of P1m (Claude, with Kyle's findings, Nov 1, 2026): needs fixes (P1m-r1-fixes.md)

RULES.md's four commands pass (55 tests). Items 3, 5, 6 (rings now 28 × 28), 7, 8 (text wrap), 9
and 10 are done as asked. Kyle tried the build and Claude confirmed:

1. **Icons render entirely white** (folders in the grid; Save, Save As and Discard as white blobs).
   Cause, tested by Claude in a GTK 4.22.4 window with the same files and a red/green/blue/yellow
   palette:
   - loaded by name through the icon theme, the `gpa:`-only SVG renders all white, because GTK
     loads theme `-symbolic.svg` files through its older symbolic path, which ignores `gpa:fill`;
   - the same paths with legacy classes (`success-fill` and so on) get three palette colours, but
     legacy classes have no accent;
   - the unchanged SVG drawn as a `gtk::Svg` paintable on a `gtk::Image` gets all five colours,
     accent included.

   So the SVGs are right; they must be drawn through `gtk::Svg`.
2. **Split columns (Kyle):**
   - When the layout changes (4 → 3 tiles), tiles snap instead of growing smoothly.
   - The full-height tile keeps its height when dragged to the other column and covers the tile
     there.
   - Only the right column can be the full-height one: closing a left tile out of 4 should make the
     left column's tile full height. The row-major slot order (`column_counts`) ties the tall
     column to the tile count, not to where the tiles are.
3. **Pinned folders (Kyle):**
   - The list has a shaded box behind it that Kyle doesn't want.
   - The drag reorder doesn't follow the mouse and is generally buggy.
4. **Collapsed section pills (Kyle):** should sit about 6 px apart.
5. **Collapsed sidebars (Kyle):** the expand button's vertical position is right; it should be
   centred horizontally in the collapsed strip.

## P1m-r1 changes

Built on P1m in `spike/src/bin/look-body/`. The review and live-check sections above are
unchanged. Only files under `look-body/` and this note were touched; the temporary timer
harness was removed before delivery.

### Item by item

1. **Icons through `gtk::Svg`.** New `icons.rs` holds one shared, thread-local
   `gtk::Svg` per mockup icon (`MockupIcon::{Folder,Save,SaveAs,Discard}`) and builds
   every image with `gtk::Image::from_paintable`. Grid folders call
   `image.set_paintable(Some(&icons::svg(..)))`; files still call
   `set_icon_name("text-x-generic")`. The preview's Save, Save As and Discard buttons
   use `icons::button`, whose child is the same kind of image. The icon-theme search
   path is gone from `main.rs`. `color` and `-gtk-icon-palette` stay in CSS on
   `.gl-cell image`; the action row now sets `-gtk-icon-palette` on
   `.gl-action-row image` instead of on its button. The cause of the white icons is
   GTK's older theme-symbolic path: an icon loaded by name ignores `gpa:` slots. A
   `GtkSvg` is a `GtkSymbolicPaintable`, so the image's CSS style passes its five
   colours to `gtk_symbolic_paintable_snapshot_with_weight`
   (`gtk-4.22.4:gtk/gtkrendericon.c:97-134`, especially the
   `GTK_IS_SYMBOLIC_PAINTABLE` test at :121 and the colour lookup/call at :131-134;
   the style is fetched from the image's icon helper at
   `gtk-4.22.4:gtk/gtkiconhelper.c:195`, used at :227 and :235). The palette names
   (`accent`, `error`, …) are resolved at
   `gtk-4.22.4:gtk/gtkcssstyle.c:876-899`; `accent` is in the array at :883.
   gtk-rs: `gtk4-0.11.5:src/auto/svg.rs:19` (`@implements SymbolicPaintable`), :35
   (`from_bytes`); `gtk4-0.11.5:src/auto/image.rs:86` (`from_paintable`).
2. **Columns own their tiles.** `State.columns` is now `[Vec<usize>; 2]`, with the
   pure model in `split.rs`: `initial_columns` / `add_to_shorter` (new tiles join the
   shorter column, left on a tie), `columns_without` (a close grows that column's
   survivors and renumbers the ids), `columns_after_drop` (same column reorders,
   other column swaps), `column_rects` and `rects_by_tile` (each column shares its
   height, floor at half the visible height beyond four tiles), and `drag_layout`.
   `set_count` now adds/removes one tile at a time; a close no longer rebuilds the
   view. Tile handlers capture a stable `Tile.uid`, so a close's renumbering cannot
   stale them.
3. **Every layout change animates.** `Slot` now holds position *and* size.
   `slide_to` interpolates both over 200 ms with the existing per-slot generation;
   close/add/drop call `relayout(true)`, viewport and scale changes still snap with
   `relayout(false)`. The dragged tile is moved directly each update at its old size
   and resizes into the target slot on release. Raising still uses `insert_after`;
   no widget is unparented in the drag path.
4. **Pinned list has no fill.** `.gl-pinned` lost its `background-color`; the
   `PINNED_BG` constant went with it. `.gl-pinned-row.sel` still paints the selected
   row with `alpha(mix(@sidebar_bg_color,@sidebar_fg_color,0.12), opacity)`.
5. **Pinned drag rebuilt.** The old rows were a `GtkBox` moved with top margins
   while the gesture's widget-relative `dx/dy` and `reorder_child_after` changed the
   child order mid-drag. Changing one row's margin reflowed every row below it, so
   the pointer-to-slot arithmetic drifted and the dragged row did not track the
   mouse. The rows now all live in one `gtk::Fixed` at absolute positions; the
   local pointer is `gdk::Event::position()` (surface) minus the Fixed's window
   bounds (`WidgetExt::compute_bounds`, gtk4-0.11.5:src/auto/widget.rs:166;
   `gdk4-0.11.5:src/auto/event.rs:146`), never the gesture offset. The drag claims
   with `drag_check_threshold` + `set_state(Claimed)` and denies the click gesture,
   raises with `insert_after`, slides the other rows 150 ms, and settles in slot
   order on release. Plain clicks still select.
6. **Collapsed pills sit 6 px apart.** `.gl-section` is `margin-top: 6 × scale`;
   the section pill lost its 8 px bottom margin; the 32 px `gap-xl` moved to
   `.gl-section-content`, inside the revealer, so it exists only above an expanded
   section's content.
7. **Collapsed strip button centred.** The strip's asymmetric horizontal padding is
   gone; two expanding spacers centre the 48 px button in the 67 px strip (24 in 34
   at scale 0.5). `valign: Start` and the 19 px top padding (10 at 0.5) keep the
   vertical position. A `GtkBox` ignores `halign` on its main axis, which is why the
   spacers are used.

### Unit tests (all in the look-body binary)

- `split::tests`: `initial_columns_fill_the_shorter_column`,
  `closing_from_either_column_leaves_the_other_taller`,
  `dropping_swaps_across_columns_and_reorders_within_one`,
  `a_dragged_tile_ends_at_the_slot_sizes`, `rects_never_overlap` (2–6 tiles, every
  drop pair and every close), `uneven_columns_make_their_tiles_taller`,
  `five_tiles_never_drop_below_half_the_visible_height`, `selection_indices_follow_a_close`,
  and the drag/hit/selection tests updated to the column model.
- `sidebar::tests`: `every_pin_reorder_moves_exactly_one_row`,
  `pinned_drag_layout_uses_original_row_indices`,
  `pinned_drag_follows_the_pointer_and_slides_the_others`, `pin_order_moves_one_row`.
- `style::tests::pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart`.
- `layout::tests::the_strip_centres_the_expand_button` (scale 1.0 and 0.5).
- `style::tests::icon_palettes_map_the_spec_table` still checks the SVG slots and the
  palette entries.

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check`:

```text
(no output; rustfmt --check passes package-wide)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.40s
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
    Finished `release` profile [optimized] target(s) in 1.20s
```

### 10-second runs (release binary, from `/tmp/gimbal-spike-p1m-r1`, stopped with SIGTERM)

Default:

```text
(no output)
```

`--tiles 3`:

```text
(no output)
```

`--tiles 6 --pins 14 --scale 0.5`:

```text
(no output)
```

No GTK warnings, no CSS/theme/icon warnings, no panics; the intermittent
`VK_SUBOPTIMAL_KHR` notice did not appear in these three runs.

### Temporary timer logs (harness removed before delivery)

Debug binary, `GIMBAL_SPIKE_LOG=1 look-body --tiles 4`, niri-tiled 1272 × 1398.
Item 1 (paintable type and palette), items 2–3 (4 tiles → close the left-bottom
tile → add one; every tile's visual rect at start / 100 ms / end), items 6–7.

```text
[gimbal-look-body 1.326] temp: item1 folder image: paintable GtkSvg, palette accent @accent_color, success shade(@accent_color, 0.7), warning shade(@accent_color, 0.6), error shade(@accent_color, 0.45)
[gimbal-look-body 1.326] temp: item1 discard image: paintable GtkSvg, palette error @window_bg_color
[gimbal-look-body 1.327] temp: item1 save image: paintable GtkSvg, palette error @window_bg_color
[gimbal-look-body 1.327] temp: item1 save-as image: paintable GtkSvg, palette error @window_bg_color
[gimbal-look-body 1.327] temp: item2 start: 0:(40,0,77,563) 1:(141,0,77,563) 2:(40,587,77,563) 3:(141,587,77,563)
[gimbal-look-body 1.327] temp: item2 start targets: 0:(40,0,77,563) 1:(141,0,77,563) 2:(40,587,77,563) 3:(141,587,77,563)
[gimbal-look-body 1.425] temp: item2 mid close: 0:(40,0,77,1094) 1:(141,0,77,563) 3:(141,587,77,563)
[gimbal-look-body 1.625] temp: item2 end close: 0:(40,0,77,1151) 1:(141,0,77,563) 3:(141,587,77,563)
[gimbal-look-body 1.625] temp: item2 end close targets: 0:(40,0,77,1151) 1:(141,0,77,563) 3:(141,587,77,563)
[gimbal-look-body 1.725] temp: item2 mid add: 0:(40,0,77,658) 1:(141,0,77,563) 3:(141,587,77,563) 4:(40,587,77,563)
[gimbal-look-body 1.925] temp: item2 end add: 0:(40,0,77,563) 1:(141,0,77,563) 3:(141,587,77,563) 4:(40,587,77,563)
[gimbal-look-body 1.925] temp: item2 end add targets: 0:(40,0,77,563) 1:(141,0,77,563) 3:(141,587,77,563) 4:(40,587,77,563)
[gimbal-look-body 2.525] temp: item7 left before: button (27.0,161.0,48.0,48.0) centre x 51.0
[gimbal-look-body 2.525] temp: item7 right before: button (1197.0,161.0,48.0,48.0) centre x 1221.0
[gimbal-look-body 2.525] temp: item6 pill 0: top 673.0, bottom 707.0, h 34.0
[gimbal-look-body 2.525] temp: item6 pill 1: top 713.0, bottom 747.0, h 34.0
[gimbal-look-body 2.525] temp: item6 pill 2: top 753.0, bottom 787.0, h 34.0
[gimbal-look-body 2.525] temp: item6 gap 1: 6.0
[gimbal-look-body 2.525] temp: item6 gap 2: 6.0
[gimbal-look-body 3.125] temp: item7 left: strip (8.0,142.0,67.0,1248.0) centre x 41.5; button (18.0,161.0,48.0,48.0) centre x 42.0 halign Fill
[gimbal-look-body 3.125] temp: item7 right: strip (1197.0,142.0,67.0,1248.0) centre x 1230.5; button (1207.0,161.0,48.0,48.0) centre x 1231.0 halign Fill
```

Same timer at `--scale 0.5`:

```text
[gimbal-look-body 2.538] temp: item7 left before: button (14.0,92.0,24.0,24.0) centre x 26.0
[gimbal-look-body 2.538] temp: item7 right before: button (1234.0,92.0,24.0,24.0) centre x 1246.0
[gimbal-look-body 2.538] temp: item6 pill 0: top 349.0, bottom 366.0, h 17.0
[gimbal-look-body 2.538] temp: item6 pill 1: top 369.0, bottom 386.0, h 17.0
[gimbal-look-body 2.538] temp: item6 pill 2: top 389.0, bottom 406.0, h 17.0
[gimbal-look-body 2.538] temp: item6 gap 1: 3.0
[gimbal-look-body 2.538] temp: item6 gap 2: 3.0
[gimbal-look-body 3.138] temp: item7 left: strip (4.0,82.0,34.0,1312.0) centre x 21.0; button (9.0,92.0,24.0,24.0) centre x 21.0 halign Fill
[gimbal-look-body 3.138] temp: item7 right: strip (1234.0,82.0,34.0,1312.0) centre x 1251.0; button (1239.0,92.0,24.0,24.0) centre x 1251.0 halign Fill
```

The close animation's mid point (1094/1151) and the add's mid point (658/563) show
the 200 ms interpolation; the target rects equal the end rects. Vertical positions
for item 7 are identical before and after (161 at scale 1, 92 at 0.5), while the
horizontal centre moves to the strip's centre.

### Sizes and colours changed (r1)

| Where | Old | New |
|---|---|---|
| Tile slide duration | 150 ms | 200 ms (position and size) |
| `.gl-section` top margin | 32 (`gap-xl`) | 6 (`SECTION_GAP`) |
| Section pill bottom margin | 8 (`gap-s`) | 0 |
| `.gl-section-content` top margin | — | 32 (`gap-xl`) |
| Collapsed strip horizontal padding | 19 left (left) / 19 right (right) | 0 both |
| Collapsed strip button alignment | start (left) / end (right) | centred by two expanding spacers |
| `.gl-pinned` background | `alpha(@sidebar_bg_color, opacity)` | none |
| Pinned row slide duration | 150 ms | unchanged 150 ms |
| Icon loading | icon theme by name (`gimbal-*-symbolic`) | shared `gtk::Svg` paintable |
| Action-row palette target | `.gl-icon-button` | `.gl-action-row image` |

No colour expression changed; `PINNED_BG` was removed with the fill.

### Not verified live (Claude's check)

- How the four SVG icons actually look through `gtk::Svg`: the folder's five tones,
  the 0.85 paper tab, the Save As pencil outline over the translucent button, and
  whether the shared `Svg` redraws per-image palettes correctly. The timers prove the
  paintables are `GtkSvg` and the CSS palettes are the ones configured; the pixels
  were not seen. [inferred]
- The tile drag feel after the column model: swapping across columns, reordering
  within one, the dragged tile's 200 ms resize on release, and the raise. The model
  and rects are unit-tested; the pointer interaction is not. [inferred]
- The pinned-row drag feel: follow from surface coordinates, the threshold claim,
  the followers' 150 ms slides, the raised row's draw order, and that a plain click
  still selects (the drag denies the click only after it claims). [inferred]
- The section-pill gap and the strip centring by eye at other scales/widths; the
  timer values above are from a 1272 × 1398 tiled window.
- The icons' live recolour on a Noctalia theme reload (the r1 change keeps the
  `-gtk-icon-palette` on the image; the reload path itself is unchanged from P1m).

### Anything I'd change outside my files

None. The `text-section`/`icon-row` stale rows in `look-style-spec.md` and the Phase 2
GResource suggestion from P1m still stand; no file outside the brief's Output list
was edited.

## Review and live check of P1m-r1 (Claude, Nov 1, 2026): mostly passes, three fixes (P1m-r2-fixes.md)

RULES.md's four commands pass (61 tests). Live, release build, DP-3 off:

- **Icons:** the folders are multi-tone purple from the theme (accent front, darker back and side,
  light paper); Save, Save As and Discard draw as their mockup shapes. `gtk::Svg` fixed it.
- **Columns:**
  - Closing Tab 3 (bottom left) out of 4 makes Tab 1, the left column's lone tile, full height.
  - Dragging Tab 1 onto Tab 4 in the other column swaps them during the drag: Tab 4 moves left at
    full height and Tab 1 takes the half-height slot. Sizes are right afterwards, with no overlap.
- **Pinned drag:** Home follows the pointer down; the rows below slide up; it settles in slot 4
  (`pins: moved 'Home' from slot 0 to 4`). The cause given (margins reflowing the rows) fits.
- **Collapsed sections:** all three collapsed sit about 6 px apart.
- **Expand buttons:** centred in their strips (left: button centre 42, strip centre 41.5; right:
  2,519 against 2,518), at the same height as before.

Needs fixing:

1. **Pinned names show only "…"** at the default panel width. In the new `gtk::Fixed` each row
   gets only its minimum width.
2. **Section spacing is upside down.** A large gap sits between a pill and its own content, and
   only about 6 px between that content and the next pill. The spec's earlier wording ("the
   `gap-xl` spacing applies only above an expanded section's content") was ambiguous, Claude's
   fault; it is corrected now.
3. **An empty column.** With Tab 4 alone in the left column, closing it leaves the left half empty
   and Tab 2 and Tab 1 stacked on the right. A column that empties should take a tile from the
   other.

Observation for Kyle: the collapsed strips draw no panel outline; earlier rounds drew one.

## P1m-r2 changes

Built on P1m-r1 in `spike/src/bin/look-body/`. The review and live-check sections above are
unchanged. Only files under `look-body/` and this note were touched; the temporary timer harness was
removed before delivery. Below, command output and timer logs are **[observed]**, GTK/gtk-rs source
citations are **[read]**, and the live look/feel items are **[inferred]**.

### Item by item

1. **Pinned rows span the list.** The `GtkFixed` is replaced by `PinnedRows`, a small `gtk::Widget`
   subclass that installs `PinnedRowLayout`, a `gtk::LayoutManager` subclass. Its `measure` returns
   the widest child's minimum/natural width for both orientations — never the list's allocation — and
   its `allocate` gives every row x = 0, the list's full width, the row's own natural height and the
   y stored for it by the drag arithmetic. A `gtk::Widget::measure` call supplies the child sizes; a
   `gsk::Transform` translation places the row at its slot y. A row's label therefore has the full
   row width and only ellipsizes at the row's own minimum. The list's `width_request` stays unset
   (`-1` in the log below).

   I chose the layout manager over setting each row's `width_request` from the list allocation
   because that is exactly the P1i feedback loop: a `GtkFixed`'s minimum is its children's extents,
   so a width request fed from the allocation makes the panel's minimum grow with its allocation.
   The manager reports one row's minimum instead. It also keeps the existing drag unchanged: the
   rows stay in one container with absolute y values, so `insert_after`, `last_child`, the surface
   pointer arithmetic, the 150 ms follower slides and the settle path are untouched; only `put`,
   `move_`, `child_position` and `remove` became `append_row`, `set_y`, `y_of` and `forget`.

   API: `LayoutManagerImpl::{measure, allocate}` at
   `gtk4-0.11.5:src/subclass/layout_manager.rs:14-49`; `Widget::set_layout_manager` at
   `gtk-4.22.4:gtk/gtkwidget.c:12968-12993`; `Widget::allocate(width, height, baseline, transform)`
   at `gtk-4.22.4:gtk/gtkwidget.c:4148-4160` with the transform translation used by GTK itself at
   :3869-3878; `gtk::gsk::Transform::translate` at `gsk4-0.11.5:src/transform.rs:119`. The cause of
   the old "…" is the fixed layout allocating each child at its preferred size
   (`gtk-4.22.4:gtk/gtkfixedlayout.c:297-322`, child measured at :251-255). `gtk_widget_get_width`
   returns the content width (padding excluded), `gtk-4.22.4:gtk/gtkwidget.c:12821-12845`, which is
   why the logged row content widths are 16 px less than the list's 310 px (the row's 8 px padding
   each side).

2. **Section spacing corrected.** `.gl-section-content` is now `margin: gap-s 0 (gap-xl − 6) 0`:
   8 px below its own pill, and 26 px after the content so the next section's existing 6 px top
   margin makes the required `gap-xl` (32). A collapsed revealer measures zero, so the next pill is
   still 6 px below. The last section has no next pill, so
   `.gl-section:last-child .gl-section-content` drops its bottom margin and the rule below the
   scroller keeps its old spacing. The 6 px between collapsed pills is unchanged. Old: content
   `margin-top: 32` (the large gap sat between the pill and its content); new:
   `margin: 8px 0 26px 0`. The style test
   `pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart` checks the new rule.

3. **No column stays empty.** `split::columns_without` now removes the tile, then, if that emptied a
   column while the other still has two or more tiles, moves the other column's last tile across,
   then renumbers the surviving ids. Two tiles therefore always end one per column at full height.
   The move is a model change during a close, so the existing `relayout(true)` slides the survivor
   into its new column over 200 ms; no new animation path was needed. The drop rule is unchanged.

### Unit tests added

- `split::closing_the_lone_tile_moves_one_across` — 3 → 2, lone tile on the right and on the left.
- `split::closing_both_tiles_of_one_column_ends_two_full_height` — 4 → 3 → 2 from either column.
- `split::the_drop_rule_still_holds_after_a_close` — swapping across and reordering within after
  a 3 → 2 close.
- Existing `rects_never_overlap` now also covers the rebalanced close results.

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check`:

```text
(no output)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.32s
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 49 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 1.32s
```

### 10-second runs (release binary, from `/tmp/gimbal-spike-p1m-r2`, stopped with SIGTERM)

Default, `--tiles 3`, and `--tiles 6 --pins 14 --scale 0.5` each produced an empty log:

```text
(no output)
```

No GTK warnings, CSS/theme/icon warnings or panics; the intermittent `VK_SUBOPTIMAL_KHR` notice did
not appear in these three runs.

### Temporary timer logs (harness removed before delivery)

Debug binary, `GIMBAL_SPIKE_LOG=1 look-body --tiles 4 --pins 14`, niri-tiled 1322 × 1448. The default
panel width is 348 design px (list content 310 at scale 1) and the minimum is 300 (list content 262;
130 at 0.5).

```text
[gimbal-look-body 1.343] temp: item1 default width: list width 310 min 66, row widths [294, 294, 294, 294, 294, 294, 294, 294, 294, 294, 294, 294, 294, 294], list min-request -1
[gimbal-look-body 1.343] temp: item2 expanded: pill 0 top 960.0 bottom 994.0; content top 1002.0 bottom 1162.0
[gimbal-look-body 1.343] temp: item2 expanded: gap content -> pill 1: 32.0
[gimbal-look-body 1.343] temp: item2 expanded: pill 1 top 1194.0 bottom 1228.0; content top 1236.0 bottom 1404.0
[gimbal-look-body 1.343] temp: item2 expanded: gap content -> pill 2: 32.0
[gimbal-look-body 1.343] temp: item2 expanded: pill 2 top 1436.0 bottom 1470.0; content top 1478.0 bottom 1510.0
[gimbal-look-body 1.844] temp: item1 minimum width: list width 262 min 66, row widths [246, 246, 246, 246, 246, 246, 246, 246, 246, 246, 246, 246, 246, 246], list min-request -1
[gimbal-look-body 2.143] temp: item3 start: columns [[0, 2], [1, 3]] rects (40,0,77,563) (141,0,77,563) (40,587,77,563) (141,587,77,563)
[gimbal-look-body 2.544] temp: item3 after close 2: columns [[0], [1, 2]] rects (40,0,77,1151) (141,0,77,563) (141,587,77,563)
[gimbal-look-body 2.744] temp: item2 collapsed: gap pill -> pill 1: 6.0
[gimbal-look-body 2.744] temp: item2 collapsed: gap pill -> pill 2: 6.0
[gimbal-look-body 2.945] temp: item3 after close 0: columns [[1], [0]] rects (141,0,77,1151) (40,0,77,1151)
```

Same timer at `--scale 0.5` (no `--tiles`, so no item 3):

```text
[gimbal-look-body 1.308] temp: item1 default width: list width 154 min 33, row widths [146, 146, 146, 146, 146, 146, 146, 146, 146, 146, 146, 146, 146, 146], list min-request -1
[gimbal-look-body 1.308] temp: item2 expanded: pill 0 top 481.0 bottom 498.0; content top 502.0 bottom 582.0
[gimbal-look-body 1.308] temp: item2 expanded: gap content -> pill 1: 16.0
[gimbal-look-body 1.308] temp: item2 expanded: pill 1 top 598.0 bottom 615.0; content top 619.0 bottom 703.0
[gimbal-look-body 1.308] temp: item2 expanded: gap content -> pill 2: 16.0
[gimbal-look-body 1.308] temp: item2 expanded: pill 2 top 719.0 bottom 736.0; content top 740.0 bottom 756.0
[gimbal-look-body 1.809] temp: item1 minimum width: list width 130 min 33, row widths [122, 122, 122, 122, 122, 122, 122, 122, 122, 122, 122, 122, 122, 122], list min-request -1
[gimbal-look-body 2.710] temp: item2 collapsed: gap pill -> pill 1: 3.0
[gimbal-look-body 2.710] temp: item2 collapsed: gap pill -> pill 2: 3.0
```

At both scales the rows' content widths equal the list content width minus the row's own 8 px
padding each side, and the minimum equals the widest row's minimum (66 / 33), independent of the
list's allocation. Item 2's 8 / 32 / 6 become 4 / 16 / 3 at 0.5. Item 3's second close moves tile 1
from the right column into the empty left one, leaving one full-height tile in each column.

### Sizes changed (r2)

| Where | Old | New |
|---|---|---|
| Pinned rows' container | `gtk::Fixed` (preferred-size children) | `PinnedRows` + `PinnedRowLayout` (full-width rows) |
| Row width request | set to the list's allocated width | none (`-1`) |
| `.gl-section-content` top margin | 32 (`gap-xl`, between pill and content) | 8 (`gap-s`, below the pill) |
| `.gl-section-content` bottom margin | — | 26 (`gap-xl` − `SECTION_GAP`; next section adds 6) |

No colour expression changed.

### Not verified live (Claude's check)

- That the pinned labels now draw their full names at the default panel width and ellipsize only
  when the panel is genuinely too narrow. The widths and the list minimum are in the timer log
  above, but the pixels were not seen. [inferred]
- The pinned drag's feel through the new container: pointer follow, the followers' slides, the
  raise, and a plain click still selecting. The arithmetic and y storage are unchanged; the
  pointer interaction was not replayed. [inferred]
- The animated cross-column move after a close (the log proves the model and end rects, not the
  200 ms frames). [inferred]
- The section gaps by eye at other window sizes and scales; the timer values are from the tiled
  window above. [inferred]

### Anything I'd change outside my files

None. No file outside the brief's Output list was edited.


## Review and live check of P1m-r2 (Claude, Nov 1, 2026): passes

RULES.md's four commands pass (65 tests). The new `PinnedRowLayout` reports one row's width (never
its own allocation) and the rows' bottom extent as its height, which keeps the panel's minimum
independent of its allocation, as asked. Live, release build, DP-3 off:

- **Pinned names:** shown in full across the panel. Dragging Home down still follows the pointer and
  settles below Music (`pins: moved 'Home' from slot 0 to 4`).
- **Section spacing:** content sits just under its pill, with the larger gap before the next pill.
  (The 6 px between collapsed pills passed in r1 and the rule for it is unchanged.)
- **Empty column:**
  - 3 tiles, closing the lone right tile: Tab 3 moves across, and Tab 1 and Tab 3 sit side by side
    at full height.
  - 4 tiles, closing both left tiles: the two survivors end one per column at full height.
- No GTK warnings, no panics.

Every item from Kyle's P1m notes and the three reviews has now been seen working live.

## Follow-up (Claude, Nov 1, 2026): 16 px after an open section

Kyle found the space between the last directory name and the Devices pill too large and chose
16 px. Claude changed it directly (one value): `section_tail` in `style.rs` is now
`gap-m − section-gap` (10, plus the next section's 6 px top margin = 16), was `gap-xl − 6` (26),
and the style test follows. RULES.md's four commands pass (65 tests). Live: the gaps after Recent
Directories and Devices are 16 px; collapsed pills are unchanged.

## P1m-r3 changes

Built on P1m-r2 in `spike/src/bin/look-body/`. Only `split.rs` (rewritten around a shares
model), `sidebar.rs` (the pinned click), `style.rs` (the drop line's class) and this note were
touched; the temporary timer harness was removed before delivery. Below, command output and timer
logs are **[observed]**, GTK/gtk-rs source citations are **[read]**, and the live look/feel items
are **[inferred]**.

### Item by item

1. **Rebalance on close.** `split::columns_without` now runs three rules in order after removing
   the tile: (a) the existing empty-column rule (a column that empties takes the other column's
   last tile while the other keeps one); (b) then, while the counts differ by two or more, the
   longer column's **bottom** tile moves across; (c) the surviving ids are renumbered. So 3 + 3
   closed twice from one column goes 3 + 2 → 3 + 1 → (one move) 2 + 2. A hand-made layout is left
   alone otherwise: 3 + 1 closed from the long column is 2 + 1 and stays; 3 + 1 with the lone tile
   closed is 2 + 1 by rule (a). Every close still goes through the per-slot generation slides
   (`relayout(true)`, 200 ms), so the moved tile animates; no new animation path was added.
   Timer, below: `columns [[0,2,4],[1,3]]` → `[[0,2],[1,3]]` with a clean 2 + 2 rect set.

2. **Move a tile between tiles.** New pure `Drop::{Swap, Insert}`: a press over a gap band inserts
   (`insert` makes that column one longer and the other one shorter), a press on a tile swaps
   across columns or reorders within one. The band is the `gap-l` gutter plus a fifth of the
   adjacent slot's height on each side, and it wins over the tile under it; the column x range
   selects the column (`drop_at`). `columns_after_insert` refuses (`None`) a move that would empty a
   column while two or more tiles exist, and also refuses a no-op; the release path then slides the
   tile back to its resting rect. The preview (`drag_layout`) recomputes the whole slot layout for
   an insert, so the other tiles slide into the new (shorter/taller) slots while the dragged tile
   follows the pointer at its old size. A `gtk::Box` with the new `gl-drop-line` class — `line-thin`
   tall, `@accent_color`, `radius-pill` — sits in the Fixed and is moved to the gap and shown only
   for an accepted insert.

3. **Resize tiles by their borders.** `State` now stores **shares**: `heights[column]` is the
   share of the column's available height (`inner_h − (n−1)·gap`) for each slot, and
   `width_share` is the left column's share of the width after the gutter. `split::layout` turns
   them into rects, so a window resize or a scale change keeps the proportions (test). One
   `GestureDrag` on the Fixed classifies the press: a pill starts a tile drag, the gutter between
   the columns (`Border::Columns`) or between two stacked tiles (`Border::Heights`) starts a
   resize. A `EventControllerMotion` sets `ew-resize` / `ns-resize` from the same hit test, so the
   cursor and the drag can never disagree. The border drag writes the share from the press value
   plus the pointer delta on every update (`relayout(false)`, direct follow); every reset
   (double-click) or close/add goes through `relayout(true)`. Minimums: `min_column_width` is one
   grid column (`CELL_W`) or, when larger, the tab pill's own minimum (`PILL_W`, see the note
   below); `min_slot_height` is `control-h + gap-s + CELL_H` (the pill plus one grid row), and
   beyond four tiles `slot_minimum` also applies the existing half-visible-height floor.
   `heights_after_drag` clamps the pair either side of the dragged border (a pair already below two
   minimums cannot move). A double-click on a border sets equal shares (heights) or 0.5 (widths).
   `heights_after_change` resets the shares of any column that gained or lost a tile (add and
   close), keeps them through a swap or a same-column reorder, and `remove_tile` resets the width
   share only when a column empties.
   Timer, below: an equal 2 + 2 column's shares 0.5/0.5 dragged 120 px become 0.6069/0.3940 on a
   1127 px available height, min 202, and the rects follow (684 / 444).

4. **Pinned rows: click selects, double-click opens, only a drag moves.**

   **The cause of "a plain click moves the row down one."** The pinned list's drag handler read the
   pointer from `gdk::Event::position()` — **surface** coordinates — and subtracted the rows'
   `compute_bounds` (window coordinates), then compared that with `GestureDrag`'s press point, which
   GTK reports in **widget** coordinates. Those differ by the window's surface transform: GTK 4.22
   draws its client-side shadow inside the surface and offsets the widget by it
   (`gtk_window_native_get_surface_transform`, `gtk-4.22.4:gtk/gtkwindow.c:2274-2287`), and its own
   event translation subtracts exactly that offset (`translate_event_coordinates`,
   `gtk-4.22.4:gtk/gtkwidget.c:4954-4956`). On this machine the transform is 25 px: r2's blur log
   prints `surface layout changed to 1322×1448` with the visible window `1272×1398 at (25, 25)`, and
   that 25 px is the same value (`blur.rs::visible_geometry` reads `window.surface_transform()`).
   So on every press `local − start` was **25 px**, which is past the 8 px `gtk-dnd-drag-threshold`:
   the first `drag-update` (GTK emits one more on release — `gtkgesture.c:684-690`) claimed the
   sequence, `drag-end` computed the drop slot from `start.1 + 25`, and a press in the lower ~39 px
   of a 64 px row landed in the row below → `pin_order(n, from, from+1)`. The same 25 px also made
   a real drag ride 25 px below the pointer (small enough to pass a live check).

   The fix: `PinDrag` stores the rows' bounds **and** the window's transform; `pin_local` does
   `surface − transform − bounds` for both the threshold and the drop, so the two are in one space
   (`NativeExt::surface_transform`, `gtk4-0.11.5:src/auto/native.rs:60-73`). The threshold is now
   measured as a delta (`drag_check_threshold(0, 0, local − start)`), and nothing is raised or moved
   on the press: the raise, the claim, the click denial and the log all happen at the threshold.
   `pin_release(n, row_h, from, pointer_y, n_press, moved)` is the pure decision: `!moved` is
   `Select` (or `Open` at `n_press ≥ 2`), and only `moved` can produce `Reorder`. The click gesture
   now passes `n_press`, so a double-click logs `pins: open <name>`; a single click selects the row
   under the cursor; the same function drives the drag's commit. Tests:
   `sidebar::tests::a_click_never_reorders` (every row, every offset, with a 25 px transform: the
   local delta is 0 and the decision is `Select`), `only_a_past_threshold_press_reorders` (a real
   drag reorders; a drag back to its own row does not; a double-click opens).

### Unit tests added / changed (all in the look-body binary)

- `split::tests::a_close_rebalances_two_or_more_apart` — 6 → 5 → 4 from one column ends 2 + 2, and
  the tile that moved is the long column's bottom one; the mirror case too.
- `split::tests::a_hand_made_layout_survives_a_close_that_leaves_one_gap` — hand-made 3 + 1 → 2 + 1.
- `split::tests::closing_the_lone_tile_rebalances` — 3 + 1 with the lone tile closed → 2 + 1; 3 → 2
  is one per column, full height.
- `split::tests::dropping_on_a_tile_swaps_across_and_reorders_within`,
  `dropping_in_a_gap_inserts_and_the_column_gets_longer` (between two, below the last, and a
  same-column move), `a_drop_never_empties_a_column` (refused; a no-op drop is refused too),
  `the_drop_zone_is_the_gap_plus_a_fifth_on_each_side`, `the_drop_line_spans_the_gap`.
- `split::tests::border_drags_clamp_at_the_minimums` (height pair both ways, width both ways),
  `a_double_click_resets_a_border_to_equal_shares`, `shares_survive_a_window_resize` (ratios at two
  window sizes and at scale 0.5), `adding_or_closing_resets_that_column`.
- Existing geometry tests updated to shares: `tiles_fill_the_height_and_split_the_width`,
  `uneven_columns_make_their_tiles_taller`, `rects_never_overlap` (now also every insert),
  `border_hit_test`, `drag_follows_the_pointer_and_previews_the_drop`, `hit_test`, `pill_hit_test`.
- `sidebar::tests::a_click_never_reorders`, `only_a_past_threshold_press_reorders`.
- `style::tests::nov_1_outlines_and_contrast` now also checks the `gl-drop-line` rule.

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check`:

```text
(no output; exit 0)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
    Finished `release` profile [optimized] target(s) in 0.02s
```

### 10-second runs (release binary, from `/tmp/gimbal-spike-p1m-r3`, stopped with SIGTERM)

Default, `--tiles 3`, and `--tiles 6 --pins 14 --scale 0.5` each produced an empty log:

```text
(no output)
```

No GTK warnings, CSS/theme/icon warnings or panics; the intermittent `VK_SUBOPTIMAL_KHR` notice did
not appear in these runs.

### Temporary timer logs (harness removed before delivery)

Debug binary, `GIMBAL_SPIKE_LOG=1 look-body --tiles 4 --pins 14`, niri-tiled 1272 × 1398 (so the
content column is narrow and the tiles are 77 px wide; the *model* numbers are what matter). One
timer at 1.6 s: build six tiles, close two from the right column, then simulate a 120 px drag of
the left column's first border.

```text
[gimbal-look-body 1.691] temp: item1 start: columns [[0, 2, 4], [1, 3, 5]] rects [Rect { x: 40, y: 0, w: 77, h: 563 }, Rect { x: 141, y: 0, w: 77, h: 563 }, Rect { x: 40, y: 587, w: 77, h: 563 }, Rect { x: 141, y: 587, w: 77, h: 563 }, Rect { x: 40, y: 1174, w: 77, h: 563 }, Rect { x: 141, y: 1174, w: 77, h: 563 }]
[gimbal-look-body 1.691] split: closed 'Tab 6' from slot 5
[gimbal-look-body 1.692] temp: item1 after close 5: columns [[0, 2, 4], [1, 3]] rects [Rect { x: 40, y: 0, w: 77, h: 563 }, Rect { x: 141, y: 0, w: 77, h: 564 }, Rect { x: 40, y: 587, w: 77, h: 563 }, Rect { x: 141, y: 588, w: 77, h: 564 }, Rect { x: 40, y: 1174, w: 77, h: 563 }]
[gimbal-look-body 1.692] split: closed 'Tab 4' from slot 3
[gimbal-look-body 1.692] temp: item1 after close 3: columns [[0, 2], [1, 3]] rects [Rect { x: 40, y: 0, w: 77, h: 564 }, Rect { x: 141, y: 0, w: 77, h: 564 }, Rect { x: 40, y: 588, w: 77, h: 564 }, Rect { x: 141, y: 588, w: 77, h: 564 }]
[gimbal-look-body 1.692] temp: item3 avail 1127 min 202: shares [0.5, 0.5] -> [0.6069210292812778, 0.3939662821650399]; rects [Rect { x: 40, y: 0, w: 77, h: 684 }, Rect { x: 141, y: 0, w: 77, h: 564 }, Rect { x: 40, y: 708, w: 77, h: 444 }, Rect { x: 141, y: 588, w: 77, h: 564 }]
```

The first close leaves 3 + 2 (one apart, kept); the second leaves 3 + 1, the left column's bottom
tile (id 4) moves across and the ids renumber, so the model is 2 + 2 and the rects are two equal
stacks. Item 3: the dragged border's two slots go 563/563 → 684/444 on a 1127 px available
height, and the untouched right column stays at 564.

### Sizes and model changes (r3)

| Where | Old | New |
|---|---|---|
| Column height | equal shares of the visible height, recomputed | stored shares per slot (reset on add/close, clamped to `min_slot_height` / the half floor) |
| Column width | always 50 / 50 | stored left share, clamped to `min_column_width` |
| Tile minimum height | `pill + gap + 1` | `control-h + gap-s + CELL_H` (202 at scale 1) during a border drag; the half-visible-height floor stays beyond four tiles |
| Column minimum width | — | `max(CELL_W, PILL_W)` = 270 at scale 1 (see below) |
| Drop line | — | new `.gl-drop-line`: `@accent_color`, `line-thin`, `radius-pill` |
| Pinned pointer | raw surface − bounds | surface − surface transform − bounds |
| Pinned press | raised + started a drag on press | nothing until the drag threshold; the raise, claim and click denial happen at the threshold |

No colour expression changed (`gl-drop-line` reuses `OUTLINE`). The CSS still contains no
`#rrggbb`, `rgb()`, `hsl()` or colour keyword other than `transparent`.

### API citations (r3)

- The window's surface transform is its client-side shadow offset:
  `gtk-4.22.4:gtk/gtkwindow.c:2274-2287` (`get_shadow_width` minus the margin rect), installed as
  the `GtkNative::get_surface_transform` vfunc at `:2362`; the default returns 0 at
  `gtk-4.22.4:gtk/gtknative.c:74-92`.
- Event coordinates are translated by that transform before controllers see them:
  `gtk-4.22.4:gtk/gtkwidget.c:4936-4966` (the `event_x -= nx; event_y -= ny` at :4954-4956), then
  `gtk_widget_compute_point` into the controller's widget (:4958-4961).
- Gesture press points are relative to the widget allocation:
  `gtk-4.22.4:gtk/gtkgesture.c:1271-1307` (`data->widget_x` from `_gtk_gesture_update_point`,
  :500-503).
- `GtkGestureDrag` emits `drag-begin` when the gesture is recognised on the press and one more
  `drag-update` before `drag-end` on release: `gtk-4.22.4:gtk/gtkgesturedrag.c:83-136` (the
  `begin`/`update`/`end` vfuncs) and `gtk-4.22.4:gtk/gtkgesture.c:655-700` (recognition and the
  release path). It claims nothing on its own.
- `gtk::Fixed` allocates each child its preferred size and honours the child transform in picking:
  `gtk-4.22.4:gtk/gtkfixedlayout.c:297-322`; child transforms in hit testing
  `gtk-4.22.4:gtk/gtkwidget.c:10440-10500`, and `gtk_widget_compute_transform` walking them at
  `:10576-10635`.
- gtk-rs: `NativeExt::surface_transform` at `gtk4-0.11.5:src/auto/native.rs:60-73`;
  `WidgetExt::set_cursor_from_name` at `gtk4-0.11.5:src/auto/widget.rs:1168-1176`;
  `EventControllerMotion::{connect_motion, connect_leave}` at
  `gtk4-0.11.5:src/auto/event_controller_motion.rs:111` and `:87`;
  `GestureExt::set_state` / `WidgetExt::drag_check_threshold` as cited in P1m.

### Not verified live (Claude's check)

- **The drop line**: its accent colour, position across the gap, thickness at scale 1.0 and 0.5, and
  that it appears only for an accepted gap drop (and that the band feels right — the fifth-of-a-tile
  band overlaps the neighbouring tiles by design, so dropping "on" a tile near a gap inserts
  instead of swapping). [inferred]
- **Border hover and drag feel**: the `ew-resize` / `ns-resize` cursor showing only over the
  gutters, the border following the pointer without lag or jitter, the stop at the minimums, and
  that a press on a pill still drags the tile (never the border). [inferred]
- **Double-click on a border** resetting and animating (heights to equal shares, the column border
  to the middle); the 200 ms reset and the 150/200 ms slides by eye. [inferred]
- **The pinned click fix**: a single click now selects without moving the row, a double-click logs
  `pins: open <name>`, and a drag still follows the pointer — now exactly under it instead of 25 px
  below. The arithmetic and the decision are unit-tested, but no pointer was injected (RULES). The
  cause's 25 px is read from the r2 blur log on this machine; a window without a client-side shadow
  (transform 0) would not have shown the bug at all. [read + inferred]
- The split view at the default 2560-wide window: my timer window was 1272 wide, where two pill-wide
  columns do not fit and the model falls back to equal halves (77 px tiles, as in r2). [inferred]

### Anything I'd change outside my files

- **The column minimum is 270, not 97.** `look-style-spec.md` says a column never shrinks below one
  grid column (`CELL_W`, 97). A tile's tab pill is `PILL_W` (270) wide and GTK will not shrink it
  (`gtk_widget_measure` takes the child's minimum), and `GtkFixed` allocates the tile its preferred
  size, so a 97 px column would draw a 270 px pill over its neighbour. `min_column_width` therefore
  uses `max(CELL_W, PILL_W)`; it is *stricter* than the spec's floor. If Kyle wants the border to
  reach 97, the pill has to be allowed to shrink (or the tile clipped) — one line in
  `min_column_width`, but the pill's CSS `min-width` is the real limit. This is the only place where
  I deliberately did not reach the stated number.
- "One grid row" in the height minimum: I read it as `CELL_H` (144), i.e. `control-h + gap-s +
  CELL_H` = 202 at scale 1. The tile's grid has a `gap-m` margin on each side that this does not
  count; if Kyle means "one cell visible including the tile's inner margin", it is 234. One token
  in `min_slot_height`.
- `look-style-spec.md`'s shared-components table could gain `gl-drop-line` (accent, `line-thin`,
  radius-pill) so Phase 2 has the rule in one place; I did not edit the spec.
- The Phase 2 GResource note for the mockup SVGs from P1m still stands.

## Review and live check of P1m-r3 (Claude, with Kyle's findings, Nov 1, 2026): needs fixes (P1m-r4-fixes.md)

RULES.md's four commands pass (72 tests). The pinned-click cause fits: the handler compared a
surface-coordinate pointer with a widget-coordinate press point, 25 px apart (GTK's client-side
shadow), so every click looked like a 25 px drag. Live, release build, DP-3 off:

- **Works:**
  - 5 tiles start as 3 + 2.
  - Dragging Tab 2 into the gap between Tab 1 and Tab 3 shows the accent drop line, then moves it
    (`dropped at Insert { column: 0, row: 1 }`), giving 4 + 1 with Tab 4 alone at full visible
    height.
  - In 2 + 2, dragging the border between Tab 1 and Tab 2 down 200 px moves Tab 1's frame bottom
    from y 742 to 941.
- **Scrolling (Kyle; reproduced):** with 4 + 1, scrolling the split view down carries the lone tile
  up and away, leaving the right half empty. Kyle chose: a column that fits in the view stays
  pinned while the taller one scrolls.
- **Vertical resize (Kyle: "doesn't work vertically"; explained):**
  - Only the 24 px gap between a frame and the next slot grabs: a resize cursor at y 750–765, a
    plain arrow from y 782 (beside the next pill).
  - In a column of four, every tile is at the half-height floor, so nothing moves.
  - The fix is to grab the whole strip and let hand drags go below the floor.
- **Pinned rows (Kyle):** 64 px apart is too much; Kyle chose 44.

### Correction to the r3 check (Claude, Nov 1, 2026)

Kyle reports that with his real mouse the vertical resize cursor appears but dragging does
nothing. The note above was too sure. More testing with the virtual pointer, a fresh 2 + 2 window
each time, dragging 150 px down from x 600:

| Press y | Floating, full width | Tiled, full width |
|---|---|---|
| 728, 740 (Tab 1's frame) | no change | no change |
| 750 (top of the strip) | **+233** (overshoot) | **+233** |
| 757, 765 | +150 (follows) | +150 |
| 775, 790 (next slot, beside its pill) | no change | no change |

At niri's half-width tile (1,272 px), a press at 757 also follows (+150).

So the virtual pointer moves the border, but the overshoot at the top of the strip shows that the
press point and the updates disagree, and Kyle's real-mouse failure is not reproduced yet. Also new:
at 1,272 px the two tile columns overlap each other and run under the preview panel (r3 made a
tile's minimum the pill's 270 px; the content minimum still assumes one grid column). Both are in
P1m-r4, with permanent border-press log lines so Kyle's run can be diagnosed.

## P1m-r4 changes

### Item 1: a fitting column is pinned while the taller one scrolls

`split.rs` keeps one outer `ScrolledWindow`. `State` now carries the scroller's `scroll` value.
`column_fits` says whether a column's last frame bottom is inside the visible `inner_h`, and
`column_offsets` returns `scroll` for a fitting column and `0` for a taller one; `offset_slots`
moves the fitted column's slots (and so its widgets) to `scroll`. `Board` carries `scroll`, so the
layout, hit test, drop zones and drop line all use the same offset. `watch_scroll` follows
`vadjustment`'s `value`; `schedule_scroll` re-applies the layout in one `glib::idle_add_local_once`,
never inside the scroller's allocation. A tile drag owns the visuals until it ends.

Live at 1,272 × 1,398 with 4 + 1 (the temporary timer put Tab 4 alone and the other four left):

```text
[gimbal-look-body 1.619] temp: item1 lone tile at scroll 0: Some(Rect { x: 161, y: 0, w: 97, h: 1151 })
[gimbal-look-body 1.619] temp: item1 lone tile at scroll 566: Some(Rect { x: 161, y: 566, w: 97, h: 1151 })
[gimbal-look-body 1.619] temp: item1 lone tile at scroll 1133: Some(Rect { x: 161, y: 1133, w: 97, h: 1151 })
[gimbal-look-body 2.321] temp: item1 actual lone tile at half: Some(Rect { x: 478, y: 143, w: 97, h: 1151 })
[gimbal-look-body 2.621] temp: item1 actual lone tile at max: Some(Rect { x: 478, y: 142, w: 97, h: 1151 })
```

The model y moves by exactly the scroll value, and the actual window-space y stays at 142/143: the
lone tile is pinned at full visible height while the left column's content height is 2,324 px for a
1,191 px page (range 1,133). [observed]

### Item 2: the whole strip grabs; hand drags ignore the half-height floor

`border_at(slots, x, y, scale)` now takes the whole strip from `slot[row-1]`'s frame bottom to
`slot[row]`'s frame top (`y + control-h + gap-s`), and only rejects the lower tile's own pill
(`pill_zone`, which uses the shrinking pill width). The cursor and the press still call the same
function (hover uses `border_at`, `begin_drag` calls it once and reuses the result for the log and
the drag).

The default half-height floor is now selected per column by `State.floored`: true after build, add,
close and a double-click reset, false as soon as a hand drag moves a border. `layout_floored` applies
`half_floor` only to a floored column. Border drags work from the actual pixel sizes the pointer saw
(`Drag::Height.sizes`) through `sizes_after_drag`, so they are no longer expressed as shares of an
`avail` captured at the press.

The temporary hit-test scan down x = 45 in the 4-tile column (transitions only):

```text
[gimbal-look-body 1.620] temp: item2 border at x 45, y 563: Some(Heights { column: 0, row: 1 })
[gimbal-look-body 1.620] temp: item2 border at x 45, y 587: None
[gimbal-look-body 1.620] temp: item2 border at x 45, y 637: Some(Heights { column: 0, row: 1 })
[gimbal-look-body 1.620] temp: item2 border at x 45, y 645: None
[gimbal-look-body 1.620] temp: item2 border at x 45, y 1150: Some(Heights { column: 0, row: 2 })
...
```

The 24 px gutter and the 8 px gap below the lower pill grab; the pill's own 50 px band does not.
[observed, model scan]

### Item 3: pinned rows 44 px apart

`PIN_ROW_H` is 44 (was 64). `PinnedList::row_h` is the only pitch the row layout, drag arithmetic,
slides, `sel` hit test and container height use, so the change follows everywhere; the stylesheet's
`.gl-pinned-row { min-height: 44px }` comes from the same token. [read + inferred]

### Item 4: the split view at half width

I chose the first option: tiles may be narrower than a full pill, the pill shrinks and the label
ellipsizes, and the content minimum accounts for two true columns (one `CELL_W` each) so the
sidebars give way first.

- `min_column_width` is one cell again (97 at scale 1), not `max(CELL_W, PILL_W)`.
- `one_column_width`'s split branch is `2 × gap-2xl + gap-l + 2 × CELL_W` = 298 at scale 1 (150 at
  0.5), up from 213; the window floor is 976 at scale 1 (was 919). At 1,272 px the content keeps
  exactly 298 px: the left sidebar gives 40 px (348 → 308), the preview keeps 648.
- A tile's content now sits in a `ScrolledWindow` with EXTERNAL policies. `GtkFixed` allocates a
  child its *preferred* size (`gtk-4.22.4:gtk/gtkfixedlayout.c:297-322`), while a ScrolledWindow
  with an EXTERNAL/never policy measures zero (`gtk-4.22.4:gtk/gtkscrolledwindow.c:1847-1851`); so
  the tile is allocated exactly its slot and clips instead of drawing over its neighbour.
- The tile grid's own scroller is EXTERNAL horizontally for the same reason.
- `.gl-pill.tab` no longer has a hard 270 px minimum; `.gl-pill.tab.wide` gets it back when the
  column is at least `PILL_W` wide. Below that, `relayout` caps the label with
  `set_max_width_chars` (from `pill_label_chars`) and the label ellipsizes. The hit tests use
  `pill_width`, the same estimate.

At 1,272 × 1,398 (actual allocations in the window, after the temporary 4 + 1 setup):

```text
[gimbal-look-body 2.020] temp: item4 tile 0: Rect { x: 357, y: 142, w: 97, h: 563 }
[gimbal-look-body 2.020] temp: item4 tile 1: Rect { x: 357, y: 729, w: 97, h: 563 }
[gimbal-look-body 2.021] temp: item4 tile 2: Rect { x: 357, y: 1316, w: 97, h: 563 }
[gimbal-look-body 2.021] temp: item4 tile 3: Rect { x: 357, y: 1903, w: 97, h: 563 }
[gimbal-look-body 2.021] temp: item4 tile 4: Rect { x: 478, y: 142, w: 97, h: 1151 }
[gimbal-look-body 2.021] temp: item4 window 1272: preview x 616 w 648
```

Left column 357–454, right 478–575 (24 px gutter), preview starts at 616 and ends at 1,264 with the
8 px window inset: no tile is under the preview and the two columns do not touch. [observed]

### Item 5: one coordinate space for border drags, and permanent logs

The r3 code used two spaces: `begin_drag` hit-tested with the gesture's press point (widget-relative,
`gtk-4.22.4:gtk/gtkgesture.c:1271-1307`; `drag-begin` passes the start point relative to the widget
allocation, `gtk-4.22.4:gtk/gtkgesturedrag.c:118-124`), then `drag_update` applied GTK's cumulative
`dy` to a *share* vector through `heights_after_drag`, which converted shares to pixels with the
`avail` captured at the press and divided by that same old `avail`. `layout` then multiplied those
shares by the viewport's *current* `avail`. Any change of `avail` (the page size or the Fixed's
allocation moving with the scroller) rescales the pointer delta — a 150 px move becomes 233 px when
the captured avail was the smaller one. The top of the strip is where the first update re-allocates,
so it showed the worst case. [inferred from the r3 code + Claude's numbers; no pointer was injected]

Now `pointer_in_fixed` is the single conversion for the press, hover-independent hit test and every
update: surface position (`gdk::Event::position`) minus the window's `surface_transform` minus the
Fixed's `compute_bounds` in the window — the same surface-space correction the pinned rows got in
r3. `Drag::Tile` stores the press point, `Drag::Width` the press x, `Drag::Height` the press y and
the actual pixel sizes; updates compute `dy = pointer_y − press_y` and apply it once in pixels.
`pointer_in_fixed` falls back to `press + (dx, dy)` if the event is unavailable.

Permanent log lines (all through `crate::log!`, so they appear only with `GIMBAL_SPIKE_LOG=1`):

```text
split: border press at (<x>, <y>) hit Some(Heights { column: c, row: r })   or hit None
split: border drag dy <dy> shares [<f64>, ...]
split: tile press at (<x>, <y>) on '<name>' slot <n>
split: resizing the column border from x <x>
```

### Tests added or changed

- `a_short_column_is_pinned_and_a_tall_one_scrolls`: 4 + 1 → `[0, scroll]`, 3 + 3 → `[0, 0]`,
  2 + 1 → `[scroll, scroll]`, and a 2 + 2 column that fits at 1,400 but not at 100.
- `a_hand_drag_reaches_the_minimum_not_the_half_floor`: a 4-tile floored column's pair goes from
  half/half to `min/2·half−min`, and `layout_floored` with that column unfloored renders the
  minimum while the default still renders half.
- `border_hit_test_covers_the_whole_strip_except_the_pill`: every integer y in three strips,
  both columns, plus the pill band and the frame top.
- `border_drags_clamp_at_the_minimums`, `a_double_click_resets_a_border_to_equal_shares` and
  `shares_survive_a_window_resize` now use `sizes_after_drag` and `layout_floored`.
- The layout tests expect the new split content minimum (298 / 976 / 450 collapsed).

### RULES.md's four commands

```text
$ PATH="$T:$PATH" cargo fmt --check
(no output; exit 0)

$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s

$ cargo test
     Running unittests src/main.rs
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-body/main.rs
running 58 tests
test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-header/main.rs
running 12 tests
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
```

### 10-second release runs (stopped with SIGTERM)

`look-body` and `look-body --tiles 5` each printed only the allowed intermittent
`Gdk-WARNING ... VK_SUBOPTIMAL_KHR ... (1000001003)` notice (232 bytes); `look-body --tiles 6
--pins 14 --scale 0.5` was empty. No other GTK warnings and no panics. An earlier run of the same
three produced fully empty logs. [observed]

### Not verified live (Claude's check)

- The real-mouse border drag: the press/update path and logs are in place, but no pointer was
  injected (RULES). The 150 px drag should now be exactly 150 px at every press y.
- The feel of the pinned column while wheeling (the idle applies one layout per frame, and a tile
  drag defers it until release). [inferred]
- The shrinking pill's ellipsis at 1,272 px and while dragging the column border through 270 px;
  the max-width-chars estimate is 3/5 of `TEXT_CONTROL` per character. [inferred]
- The 44 px pinned rows' drag and double-click feel. [inferred]

### Anything I'd change outside my files

- None required. `look-style-spec.md` could note `.gl-pill.tab.wide` (the 270 px minimum only while
  the column has room) and the tile's clipping scroller; I did not edit it.


## Review and live check of P1m-r4 (Claude, with Kyle's findings, Nov 1, 2026): passes; one change (P1m-r5)

RULES.md's four commands pass (74 tests). Kyle: scrolling works, the pinned rows have good spacing,
and the border drag is close. Live, release build, tiled window, DP-3 off:

- **Half width (1,272 px):** no overlap. The tiles narrow, their pills shorten to "…", and the
  preview starts after the content column.
- **Border drag:** presses at y 750 and 757 in a full-width 2 + 2 both log
  `split: border drag dy 150` with the same shares, and the screenshots are identical (an earlier
  edge reading of mine picked the wrong outline). The r3 overshoot is gone.
- **Change (Kyle):** dragging a vertical border should push the tiles below instead of shrinking
  the one below. Dragging up mirrors that, and the last tile fills a column shorter than the view.
  Rule in the spec; brief P1m-r5.

## P1m-r5 changes

Built on P1m-r4 in `spike/src/bin/look-body/`. Only `split.rs` under that folder and this note
changed; the temporary timer harness was removed before delivery. Below, command output and timer
logs are **[observed]**, GTK/gtk-rs source citations are **[read]**, and the live look/feel items are
**[inferred]**.

### Item 1: vertical borders push, not trade

The height model is no longer a vector of shares. `State.heights` is now `[Heights; 2]`:

```rust
pub enum Heights {
    Default,          // equal shares, half-height floor beyond four tiles
    Hand(Vec<i32>),   // hand-set heights, unscaled design px, top to bottom
}
```

and the pure model is:

```rust
heights_after_push(heights, border, dy, min_h, visible, gap) -> Pushed { stored, shown }
fill_last(heights, visible, gap) -> Vec<i32>
```

- **Down** changes only `heights[border - 1]`: it grows by `dy`. Every tile below keeps its stored
  height and moves down by `dy`, so the column's total grows by `dy` and scrolls. `shown` equals
  `stored` once the column is taller than the view.
- **Up** changes only the same tile: it shrinks by `dy`, clamped at `min_slot_height` (202 design px
  at scale 1). Every tile below keeps its stored height and moves up. If the column now ends short,
  `fill_last` stretches its last tile to the visible height; that fill is *not* stored, so a later
  resize/scale change only changes the fill.
- **Layout** (`layout`) computes a `Default` column as `avail / n` device px with the old half-height
  floor, and a `Hand` column as `scaled(design, scale)` with no floor. Both then fill the last tile
  in device pixels if the column ends short, so there is never empty space under it. Doing the fill
  after scaling means the bottom edge is exact at 0.5 and other scales.
- **Drag start.** For a `Hand` column the press heights are the stored design heights (not the
  displayed last fill), so a large fill is never baked into the model. For a `Default` column they
  are the slot heights the pointer sees, converted to design px (its last fill is at most a rounding
  pixel). Every update applies the whole pointer delta to those press heights, converted with
  `dy / scale`, so the pointer delta is in one space (r4's fix kept).
- **Log.** `split: border drag dy <device dy> heights [<device heights>]` (was `… shares […]`).

A hand-sized tile now keeps its exact design-px height through a window resize and a scale change;
only the last tile's fill changes. A reset (double-click), an add, a close, or a drop that changes a
column's length writes `Heights::Default` for that column (`heights_after_change`), which is the old
equal-share/floor layout. The column border still trades width (`width_share_after_drag` unchanged).

The old `floored: [bool; 2]` is gone: a `Default` column is floored, a `Hand` column is not. Pinned
columns, scroll offsets, drop zones, the drop line and the border hit test all keep using the same
slot rects, which now include the pushed heights.

### Unit tests added / changed (all in the look-body binary)

Added:

- `split::tests::dragging_down_grows_the_tile_above_and_moves_the_rest_down`
- `split::tests::dragging_up_shrinks_to_the_minimum_and_moves_the_rest_up`
- `split::tests::the_last_tile_fills_a_column_shorter_than_the_view`
- `split::tests::hand_heights_keep_their_design_px_through_resizes_and_scale_changes`
- `split::tests::a_reset_returns_to_the_equal_default`
- `split::tests::a_column_that_grows_past_the_view_stops_being_pinned`
- `split::tests::the_column_border_still_trades_width`

Changed to the new model: `tiles_fill_the_height_and_split_the_width`, `rects_for`/`slots` helpers,
`a_short_column_is_pinned_and_a_tall_one_scrolls`, `a_hand_drag_reaches_the_minimum_below_the_half_floor`,
`drag_follows_the_pointer_and_previews_the_drop`, `a_drag_respects_the_grab_offset`. The old
`sizes_after_drag` trade tests and the share-reset tests were replaced by the push tests above.
`rects_never_overlap` still covers 2–6 tiles, every swap, insert and close.

### Temporary timer log (harness removed before delivery)

Debug binary, `GIMBAL_SPIKE_LOG=1 look-body --tiles 4`, niri-tiled 1272 × 1398, so the split view is
298 × 1191 and each left column is 97 wide. The timer at 1.5 s computed a 2 + 2 left column's model
rects with the same `layout` + `offset_slots` that `relayout` uses, before and after a +200 and a
−150 drag of its first border. [observed, model]

```text
[gimbal-look-body 1.577] temp: r5 view [298, 1191] scale 1: before [Rect { x: 40, y: 0, w: 97, h: 563 }, Rect { x: 40, y: 587, w: 97, h: 564 }]
[gimbal-look-body 1.577] temp: r5 +200: left rects [Rect { x: 40, y: 0, w: 97, h: 763 }, Rect { x: 40, y: 787, w: 97, h: 564 }]
[gimbal-look-body 1.577] temp: r5 -150: left rects [Rect { x: 40, y: 0, w: 97, h: 413 }, Rect { x: 40, y: 437, w: 97, h: 714 }]
```

The +200 drag grows the top tile 563 → 763, leaves the bottom at 564 and moves it 587 → 787 (the
column is 200 px taller). The −150 drag shrinks the top tile to 413, moves the bottom 587 → 437,
and the last tile's fill grows 564 → 714, so the column still ends at the view (y 437 + 714 =
1151 = `inner_h`). The 1 px difference between the two default slots (563/564) is the default
layout's last-tile fill before the drag.

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check`:

```text
(fmt exit 0; no output)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
(clippy exit 0)
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 61 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 1.49s
```

### 10-second release runs (stopped by PID with SIGTERM, from `/tmp/gimbal-spike-p1m-r5`)

Default, `--tiles 5`, and `--tiles 6 --pins 14 --scale 0.5` each printed an empty log:

```text
(no output)
```

No GTK warnings, CSS/theme/icon warnings or panics; the intermittent `VK_SUBOPTIMAL_KHR` notice did
not appear. [observed]

### API citations

No new gtk-rs or GTK API was added; the change is the pure height model. The drag callbacks are the
same as r4: `GestureDrag::connect_drag_begin` / `connect_drag_update` / `connect_drag_end` at
`gtk4-0.11.5:src/auto/gesture_drag.rs:158`, `:224`, `:191`; the gesture's current event is read with
`Gesture::current_event` (same file). `scaled` is `style.rs:86-89`. The timer used
`glib::timeout_add_local_once` only while it existed and was removed. [read]

### Not verified live (Claude's check)

- The real-mouse push feel: that dragging down moves the border exactly with the pointer and grows
  the column, dragging up stops at 202 design px, and the tiles below move without changing height.
  No pointer was injected (RULES); the pure function and the timer's model rects are tested, the
  interaction is not. [inferred]
- The last tile's fill at window sizes and scales other than the 298 × 1191, scale 1 timer window;
  the fill is applied after scaling in device pixels, but the pixels were not seen. [inferred]
- That the pinned column switches correctly while wheeling after a hand push grows one column past
  the view (the test checks `column_offsets`, not a live scroll). [inferred]
- That the drop zones and accent drop line follow the pushed tiles in a real tile drag (they read
  the same committed rects, but the pointer interaction was not replayed). [inferred]
- The new `split: border drag dy … heights …` line through a real drag. [inferred]

### Anything I'd change outside my files

None. No file outside the brief's Output list was edited. The five `look-body` processes from the
P1m-r4 session (PIDs 83235, 83650, 85626, 85704, 85773) were already running when this round began; I
left them alone. [observed]

## Review and live check of P1m-r5 (Claude, with Kyle's findings, Nov 1, 2026): passes; one change (P1m-r6)

RULES.md's four commands pass (77 tests). Kyle: scrolling behaves as he wants. Live, release build,
tiled at full width: in 2 + 2, dragging the left border down 493 px logs
`border drag dy 493 heights [1056, 564]`. Tab 1 grows, and Tab 3 keeps its height and moves down.
The right column, which fits, stays pinned as the left one scrolls.

Change (Kyle, reproduced): scrolled halfway, Tab 1's contents are still on screen but its pill has
scrolled off the top. New rule "Sticky pills": a pill stays visible while any of its frame is, and
its own tile's bottom pushes it up. Brief P1m-r6.

## P1m-r6 changes

Built on P1m-r5 in `spike/src/bin/look-body/`. Only `split.rs` under that folder and this note
changed. The temporary timer harness was removed before delivery. Below, command output and the
timer log are **[observed]**, GTK/gtk-rs source citations are **[read]**, and everything that needs a
real pointer is **[inferred]**.

### Item 1: sticky pills

The tile's widget is now the framed pane only: its clipping scroller holds a vertical box with the
`gl-tile` frame, whose top margin is the pill band (`control-h` + `gap-s`). The pill is no longer
inside that box. It lives in `pill_wrap`, a `ScrolledWindow` (EXTERNAL both axes, measure zero)
containing a horizontal row with the centred pill, and `pill_wrap` is a **sibling of the tile in the
split's `GtkFixed`**, inserted directly after it so it paints over the frame and is cut to the
tile's width [read: gtkfixedlayout.c:297-321, gtkscrolledwindow.c:3184].

`relayout` writes the wrap's size and position from the model, outside the slot allocation and from
the same `scroll` value the pinned column uses:

```text
x = rect.x
width = rect.w
height = band
y = rect.y + pill_local_top(rect, scroll, view_h, scale)
```

`pill_local_top` is the pure rule's content-space form. The rule itself is:

```rust
pub fn sticky_pill_y(slot: Rect, frame: Rect, scroll: i32, visible_h: i32, pill_h: i32) -> i32 {
    let natural = slot.y - scroll;
    let frame_top = frame.y - scroll;
    let frame_bottom = frame.y + frame.h - scroll;
    if frame_top >= visible_h {
        return natural;                    // a frame below the view: no push
    }
    natural.max(0).min(frame_bottom - pill_h)
}
```

- the slot fully visible: `natural` (no change),
- its top scrolled above the view: `0` (sticks at the top),
- near its bottom: `frame_bottom - pill_h`, so the pill's bottom rides the frame's bottom and the
  pill leaves with the frame's last visible pixel,
- the frame entirely below the view: unchanged.

`frame_rect(slot, scale)` is the slot minus the pill band; `pill_content_rect` gives the pill's rect
in content coordinates for the hit tests. All hit tests take `(scroll, visible_h)` now:
`pill_at` (tile drags), `border_at`/`pill_zone` (the strip is never a border where any pill is
drawn), and the tile click no longer needs a geometric pill test because the pill is its own widget.
`press_on_pill` stays as a pure test helper only. A tile drag that starts on a stuck pill raises the
wrap with the tile and carries the pill at the tile's top; a press that is not a drag leaves
click/Ctrl/Shift, the ✕ and the right-click menu working on the pill where it is drawn.

A pinned column's rects already carry `+scroll`, so `pill_local_top` is 0 and its pills do not move.

### Startup assertion (unrelated to the rule, needed for the new tree)

The pill being a separate Fixed child changes the tile's content box to hold only the frame. With
the first (0-width) allocation, GTK's box gives that sole expanding child `0 - 0 = 0` height
[read: gtkboxlayout.c:985-1014], the `GridView` is allocated at 0×0, and GTK aborts in
`gtk_list_base_update_adjustments` (`bounds.y == 0`, gtklistbase.c:2121-2131). This existed in
principle before; the r5 tree's overflowing pill happened to give the frame a different path. The
fix is in the same file: `State.view` starts at `[1200, 1280]` (a provisional layout of a real
size), and `relayout` skips an all-zero layout until the scroller reports a viewport. The first real
page size then applies normally.

### Unit tests added (all in the look-body binary)

- `split::tests::a_visible_slot_leaves_its_pill_at_the_slot_top`
- `split::tests::a_slot_top_above_the_view_sticks_the_pill_at_the_top`
- `split::tests::the_tiles_own_bottom_pushes_its_pill_up`
- `split::tests::the_frame_fully_off_leaves_the_pill_with_it`
- `split::tests::a_pinned_column_never_moves_its_pills`

The existing hit tests were updated for the extra `scroll`/`visible_h` arguments.

### Temporary timer log (harness removed before delivery)

Debug binary, `GIMBAL_R6_TIMER=1 GIMBAL_SPIKE_LOG=1 look-body --tiles 4`, niri-tiled 1272×1398.
The timer set the left column to the reproduction's hand heights `[1056, 564]`, then drove the outer
adjustment through five values from 0 to its maximum and logged Tab 1's pill and frame bounds
relative to the split's Fixed (content y; "view" is content y − scroll). The left column is 97 wide;
its total is 1056 + 564 + 24 = 1644, the view 1191, so the maximum is 453 — the live check's
arithmetic. [observed]

```text
[gimbal-look-body 2.171] temp: r6 scroll 0: pill y 0 (view 0) frame [40, 58] 97x998 (view y 58)
[gimbal-look-body 2.371] temp: r6 scroll 113: pill y 113 (view 0) frame [40, 58] 97x998 (view y -55)
[gimbal-look-body 2.571] temp: r6 scroll 227: pill y 227 (view 0) frame [40, 58] 97x998 (view y -169)
[gimbal-look-body 2.771] temp: r6 scroll 340: pill y 340 (view 0) frame [40, 58] 97x998 (view y -282)
[gimbal-look-body 2.971] temp: r6 scroll 453: pill y 453 (view 0) frame [40, 58] 97x998 (view y -395)
```

The pill's view y is 0 at every nonzero scroll while the frame's view y goes negative: it sticks at
the top of the visible area. (`pill y` in the log is content y, so it equals `scroll` while stuck.)
The frame's content rect is fixed at `[40, 58] 97×998`; only the view moves. The bottom push is not
reachable in this layout before the maximum (the frame's last 50 px need scroll > 1006); the unit
test `the_tiles_own_bottom_pushes_its_pill_up` covers it.

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check` (through the rustup toolchain):

```text
(fmt exit 0; no output)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
(clippy exit 0)
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 66 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
    Finished `release` profile [optimized] target(s) in 0.02s
```

### 10-second release runs (stopped by PID with SIGTERM, from `/tmp/gimbal-spike-p1m-r6`)

Default, `--tiles 5`, and `--tiles 6 --pins 14 --scale 0.5` each ran for 10 s and printed an empty
log (exit 143 from the SIGTERM). No GTK warnings, no `Gtk-CRITICAL`, no CSS/theme/icon warnings and
no panics; the intermittent `VK_SUBOPTIMAL_KHR` notice did not appear in these three runs.
[observed]

### API citations

- `WidgetExt::insert_after`: `gtk4-0.11.5:src/auto/widget.rs:922`.
- `WidgetExt::set_margin_top` (the frame's pill band): `gtk4-0.11.5:src/auto/widget.rs:1321`.
- A `GtkFixed` allocates a child its preferred size: `gtk-4.22.4:gtk/gtkfixedlayout.c:297-321`.
- A scroller's child gets the scroller's full allocation:
  `gtk-4.22.4:gtk/gtkscrolledwindow.c:3184`; EXTERNAL/measure-zero is the r4 citation,
  `gtk-4.22.4:gtk/gtkscrolledwindow.c:1847-1851`.
- The sole-inconstant-child box path that gave the frame 0 height:
  `gtk-4.22.4:gtk/gtkboxlayout.c:985-1014`.
- The list-base assertion avoided: `gtk-4.22.4:gtk/gtklistbase.c:2121-2131`.
- CSS margins applied inside an allocation: `gtk-4.22.4:gtk/gtkwidget.c:4247-4252`.

### Not verified live (Claude's check)

- The visual stick at the top while wheeling: the timer set the adjustment programmatically; no
  pointer was injected (RULES). [inferred]
- A drag that starts on a stuck pill, click and Ctrl/Shift selection, the ✕ and the right-click
  menu at the stuck position. They use the pill widget itself (drag via `pill_at`), but the pointer
  path was not replayed. [inferred]
- The pill being pushed out by its tile's bottom in a real scroll to that point. [inferred]
- The pinned column's pills staying still while the other column wheels. [inferred]
- The paint order over the frame (the wrap is a later Fixed child, then cut to the tile width); no
  screenshot was taken. [inferred]
- The startup guard at window sizes other than this session's 1272×1398 tile. [inferred]
- Stuck-pill interpretation: the pill sits at view y 0 and overlaps the frame's content, as the
  "draws over the frame's content while stuck" sentence says. If "the same `gap-s` above the frame's
  visible top" means a real 8 px gap while stuck, that is not what this code does; the live check
  should decide. [read/inferred]

### Anything I'd change outside my files

None. `grid.rs`, `style.rs` and `main.rs` are byte-identical to their P1m-r5 state (the extra timer
harness and its temporary experiments were removed). `look-style-spec.md` already describes the
rule; it could note that while stuck the pill covers the top `control-h` of the frame content.

## Review of P1m-r6 (Claude, with Kyle's finding, Nov 1, 2026): change of rule (P1m-r7)

RULES.md's four commands pass (82 tests). Kyle: the pill now stays visible, but it draws over the
tile's contents, and the tile must never go behind its pill. Kyle chose the rule "Stuck tiles":
- the pill and the frame's top border stay fixed while the tile is stuck;
- the frame's bottom keeps moving;
- the contents keep scrolling and disappear under the frame's own top border.

Brief P1m-r7. r6's sticky-pill positioning is committed as the base it builds on.

## P1m-r7 changes

Built on P1m-r6. Only `spike/src/bin/look-body/split.rs` and `main.rs` (temporary timer call only,
removed) changed under that folder, plus this note. Below, command output and the timer log are
**[observed]**, GTK/gtk-rs source citations are **[read]**, and everything that needs a real pointer
or a screenshot is **[inferred]**.

### Item 1: the whole tile top sticks

`sticky_pill_y` (r6) is replaced by one pure function, `stuck_geometry`, in
`spike/src/bin/look-body/split.rs`. Its inputs are the slot rect, the frame rect, the scroll value,
the visible height, the pill height and the frame minimum; it returns a `Stuck` in **content
coordinates** (the space the split's `GtkFixed` children live in; subtract `scroll` for view
coordinates):

```text
Stuck { pill_y, frame (visible rect), content_offset }
```

- slot top still in view: the pill stays at the slot top, the frame is its natural rect, offset 0;
- slot top above the view and the frame's visible height still at least one grid row:
  `pill_y = scroll` (view 0), the frame's top is `scroll + pill_h + gap_s` (view `pill_h + gap_s`,
  i.e. the border stays `gap-s` under the pill), its bottom stays the slot's bottom, and
  `content_offset = scroll - slot.y`;
- the frame's visible height would drop below `min_frame_height` (one grid row, `CELL_H`): the whole
  tile, pill included, keeps its content-space geometry and the page scrolls it off; the frame's
  visible height stays one row (`frame.y + frame.h` is the slot's bottom).

The minimum is `scaled(CELL_H, scale)` (144 design px); it is the frame part of the existing
`min_slot_height` (`pill + gap-s + one grid row`). While the tile is stuck the frame only ever gets
shorter, so `bottom - band < minimum` is the spec's "frame's bottom reaches the pill's height plus
`gap-s` plus the frame's minimum".

### Which offset belongs to which scroller

- The **outer page scroller** still owns the page scroll: a tile's outer box stays at its slot rect
  in the Fixed's content coordinates, so the page moves it exactly as before.
- The **content offset** is not put on the grid's own vertical adjustment. That adjustment belongs
  to the grid's own scrolling: it has no range at all when the grid is shorter than the tile
  (`upper - page_size = 0`, so the offset would clamp), and for a longer grid it would fight the
  grid's own position. Instead the frame's child is a `GtkFixed` (`grid_plane`) holding the grid at
  its natural size, and `relayout` moves the grid up by `content_offset` inside it. The grid keeps
  its natural height while stuck, so its cells never re-lay out, they only slide. The tile's own
  clipping scroller stays at range 0 (logged below), so the page offset cannot be applied a second
  time through the tile's clip; the grid's own scroller keeps whatever range it had in r6.
- The **frame clips**: `gtk::Overflow::Hidden` on the frame clips its children to its padding box,
  inside the 2 px border. The grid therefore disappears under the frame's own top border, and the
  `gap-s` strip between the pill and that border stays empty. The pill is still a separate Fixed
  child (r6) so it never overlaps the frame's contents.

The frame's top margin is `stuck.frame.y - slot.y` (the pill band when not stuck, `band + offset`
while stuck); the pill wrap moves to `stuck.pill_y`. `place_pill` became `place_stuck`, and a tile
drag resets its frame margin and grid offset (`drag_pill`) so the dragged tile shows its natural
geometry; a no-drop slide-back restores the stuck one.

### Hit tests while stuck

`pill_content_rect`, `pill_at`, `pill_zone` and `press_on_pill` all follow the new pill position.
`border_at`'s height strip now ends at the lower tile's *visible* frame top from `stuck_geometry`
(its upper end is the upper tile's frame bottom, which stays the slot's bottom and therefore follows
a stuck tile's moving frame bottom). The strip still excludes any drawn pill. Drop zones and the
drop line are in content coordinates and are unchanged.

### Unit tests

Added in the look-body binary:

- `split::tests::a_visible_slot_leaves_its_pill_at_the_slot_top` (not stuck)
- `split::tests::a_slot_top_above_the_view_sticks_the_pill_and_the_frame_top` (stuck mid-scroll)
- `split::tests::the_frame_shrinks_to_one_grid_row_at_the_minimum` (at the minimum)
- `split::tests::below_the_minimum_the_whole_tile_scrolls_off` (scrolled off)
- `split::tests::a_stuck_tiles_strip_follows_its_frame_top` (border strip)
- `split::tests::a_pinned_column_never_moves_its_pills` (adapted from r6)

r6's `the_tiles_own_bottom_pushes_its_pill_up` and `the_frame_fully_off_leaves_the_pill_with_it`
encoded the old "pill rides the frame bottom" rule; they no longer hold and were replaced by the two
minimum/scrolled-off tests. The other r6 sticky tests were kept and adapted to `stuck_geometry`.

### Temporary timer log (harness removed before delivery)

Debug binary, `GIMBAL_R7_TIMER=1 GIMBAL_SPIKE_LOG=1 look-body --tiles 4`, niri-tiled 1272×1398, same
reproduction as r6: 2 + 2, left column hand heights `[1056, 564]` (the +493 px border drag), so the
column is 1,644 content px against a 1,191 px view and the maximum scroll is 453. The timer set those
heights, drove the outer adjustment through five values and logged Tab 1's actual widget bounds
(relative to the split's Fixed), its content offset and, for a cross-check, the pure function's
values. `clip range` is the tile's own clipping scroller's `upper - page_size`. [observed]

```text
[gimbal-look-body 2.038] temp: r7 1 of 5 scroll 0: pill y 0 (view 0) frame [40, 58] 97x998 (view y 58) content 0 clip range 0 (model pill 0 frame y 58 offset 0)
[gimbal-look-body 2.198] temp: r7 2 of 5 scroll 113: pill y 113 (view 0) frame [40, 171] 97x885 (view y 58) content 113 clip range 0 (model pill 113 frame y 171 offset 113)
[gimbal-look-body 2.358] temp: r7 3 of 5 scroll 227: pill y 227 (view 0) frame [40, 285] 97x771 (view y 58) content 227 clip range 0 (model pill 227 frame y 285 offset 227)
[gimbal-look-body 2.518] temp: r7 4 of 5 scroll 340: pill y 340 (view 0) frame [40, 398] 97x658 (view y 58) content 340 clip range 0 (model pill 340 frame y 398 offset 340)
[gimbal-look-body 2.678] temp: r7 5 of 5 scroll 453: pill y 453 (view 0) frame [40, 511] 97x545 (view y 58) content 453 clip range 0 (model pill 453 frame y 511 offset 453)
```

The pill's view y stays 0 and the frame's view top stays 58 (`pill_h + gap-s`) at every nonzero
scroll, while its content y moves down by exactly the scroll and its height shrinks by the same
amount (`998 → 885 → … → 545`; the content bottom stays 1,056 = the slot bottom). `content` equals
the model offset at every step, the frame's top border is therefore at view 58 (8 px under the
50 px pill), and the tile's own scroller never gains a range. A second temporary run at
`--scale 0.5` with hand heights `[2400, 1200]` gave the same pattern with a 29 px band and offsets
132/265/397/529. [observed]

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check` (rustup 1.95.0 toolchain):

```text
(fmt exit 0; no output)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.02s
(clippy exit 0)
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 67 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 1.58s
```

### 10-second release runs (stopped by PID with SIGTERM, from `/tmp/gimbal-spike-p1m-r7`)

Default, `--tiles 5`, and `--tiles 6 --pins 14 --scale 0.5` each ran for 10 s and wrote nothing:
exit 143, empty stderr/stdout. No GTK warnings, no `Gtk-CRITICAL`, no CSS/theme/icon warnings, no
panics, and no `VK_SUBOPTIMAL_KHR` in these runs. [observed]

### API citations

- `gtk::Overflow::Hidden`: `gtk4-0.11.5:src/auto/enums.rs:7486-7490`;
  `WidgetExt::set_overflow` `gtk4-0.11.5:src/auto/widget.rs:1344-1348`.
- The overflow clip is a rounded clip around the **padding box** applied around the children's
  snapshot, after background and border: `gtk-4.22.4:gtk/gtkwidget.c:12129-12138`; picking is
  likewise limited to the padding box, `gtk-4.22.4:gtk/gtkwidget.c:10443-10453`.
- A `GtkFixed` measures a child at its preferred size and applies the child transform to the
  measured bounds, so a child at `-offset` reduces the fixed's own measured height:
  `gtk-4.22.4:gtk/gtkfixedlayout.c:242-289`; allocate at preferred size
  `gtk-4.22.4:gtk/gtkfixedlayout.c:297-321`.
- Widget margins are applied inside the allocation and CSS margins are a separate box-model inset
  (`gtk-4.22.4:gtk/gtkwidget.c:4246-4288`); that is why the grid's `.gl-grid.tile { margin: 16px }`
  still insets the `GridView` inside the scroller, and why the earlier negative-margin experiment
  was rejected (a GtkBox measured it as a negative minimum and emitted
  `GtkBox … adjusted size vertical min -55 …`).
- `GestureDrag` callbacks and the Fixed hit-test conversion are unchanged from r4/r6.

### Not verified live (Claude's check)

- The paint order and clipping themselves: the log proves the frame's allocation, the pill's
  position and the applied offset, and the source [read] proves `overflow: hidden` clips at the
  padding box, but no screenshot was taken. That contents visibly slide under the frame's top
  border and never under the pill is [inferred].
- The stuck drag (pressing a stuck pill and moving it), click/Ctrl/Shift selection, the ✕ and the
  right-click menu at the stuck position: they all use the pill widget at its drawn position, but
  no pointer was injected (RULES). [inferred]
- The border strip while the upper stuck tile's frame bottom is moving: `border_at` is pure-tested
  in content coordinates; the hover cursor and the real grab were not replayed. [inferred]
- The minimum/scrolled-off transition at a real scroll position: driven and unit-tested, not wheeled
  by hand. [inferred]
- Wheel-over-tile behaviour while stuck: the tile's clip range logs 0, but the grid inside it keeps
  its own scroller (and the r6 range it had), so which of the two takes a wheel event was not
  replayed. [inferred]

### Anything I'd change outside my files

None. `grid.rs`, `style.rs`, `main.rs` (apart from the temporary timer call, removed) are otherwise
unchanged. The spec's "Stuck tiles" rule already says what this implements; it could add that the
frame's minimum visible height is one grid row (`144` design px), which is what this code uses.

## Review and live check of P1m-r7 (Claude, with Kyle's look, Nov 2, 2026): passes

Kyle: "looks good so far". Reviewed the diff (`split.rs` only). `stuck_geometry` is continuous at
both thresholds: at the minimum, `scroll - slot.y = frame.h - minimum`, so the scrolled-off branch
starts from exactly the stuck geometry, and past it the pill, the frame and the content offset are
constant in content coordinates, so the page carries the whole tile off. The grid sits at its
natural size in a `GtkFixed` inside a frame with `overflow: hidden`, so it slides rather than
re-laying out, and is clipped at the frame's padding box.

Gates, from `spike/` (rustup 1.95.0): `cargo fmt --check` clean; clippy `-D warnings` clean;
`cargo test` 4 + 67 + 12 pass; release build ok. 10-second runs of `look-body`,
`look-body --tiles 3` and `look-body --tiles 6 --pins 14 --scale 0.5` printed nothing. No temporary
logs left in `src/`.

Live, release build, niri-tiled on DP-2 (DP-3 off), wheel injected over the gap between the columns
(a new wheel axis on the test device):

- **4 tiles, Tab 1's lower border dragged down 500 px:** after 3 and 7 wheel notches Tab 1's pill
  stays at the top of the view, its frame top border stays 8 px under the pill, the frame bottom
  moves up with the page, and the file labels are cut cleanly by the frame's own top border. The
  gap between the pill and the border stays empty; nothing goes behind the pill. The pinned right
  column holds still.
- **6 tiles (3 + 3), one notch at a time:** both top tiles stick together; Tab 1 shrinks to one row
  of files, then the whole tile, pill included, scrolls off, and Tab 3 reaches the top.
- **Half width (1280):** the same stick and clip with one-column tiles.
- A wheel over a tile scrolls that tile's own file list first, as in r6. Unchanged; Kyle hasn't
  asked for anything different.

Not replayed: dragging a stuck tile by its pill, the ✕ and the right-click menu at the stuck
position, and the border strip under a stuck tile. They use the same hit tests the unit tests
cover.

## Kyle's findings after P1m-r7 (Nov 2, 2026): needs fixes (P1m-r8-drag-cancel-pills.md)

- **Right click mid-drag:** drag a tile over a tile in the other column, press the right button,
  then release the left. The dragged tile goes back, but the target stays where it slid, so the two
  overlap. Read from the source: the second button resets the single-button `GestureDrag`
  (`gtkgesturesingle.c:206-214`), and `cancel_drag` slides back only the dragged tile, leaving the
  others at their preview rects.
- **The target's pill doesn't follow its tile:** `slide_to` animates only the tile's outer box.
  Since r6 the pill is a separate `pill_wrap`, which `relayout` moves straight to its final place,
  and the drop preview doesn't move it at all.
- Claude's live reproduction stopped early: with DP-3 enabled, the virtual pointer's absolute range
  covers both outputs, so its coordinates no longer land on DP-2. Live checks need DP-3 off, or the
  helper has to map coordinates across both outputs.

## P1m-r8 changes

Built on P1m-r7. Only `spike/src/bin/look-body/split.rs` and `main.rs` changed under that folder,
plus this note. Below, command output and the timer log are **[observed]**, GTK/gtk-rs source
citations are **[read]**, and everything that needs a real pointer or a screenshot is **[inferred]**.

### Item 1: a cancelled tile drag restores every tile

Kyle's cause is confirmed **[read]**:

- a second button pressed during a drag resets the single-button `GestureDrag`
  (`gtk-4.22.4:gtk/gtkgesturesingle.c:206-214`, through `gtk_gesture_reset` at
  `gtk-4.22.4:gtk/gtkgesture.c:748-750`), so the split's gesture emits `cancel`;
- r7's `cancel_drag` slid back only `from`; every other tile stayed at its preview rect from
  `Drag::Tile.targets`, because a preview never changes `state.columns`.

The timer log shows exactly that preview state at the cancel point, before the restore starts (the
cancel log line is emitted immediately before `restore_tiles`):

```text
[1.834] temp: r8 at cancel: Tab 1 rect [161,256] ...; Tab 2 rect [40,0] ...; Tab 3 rect
        [40,587] ...; Tab 4 rect [161,587] ...
[1.834] temp: r8 at cancel model: Tab 1 model [40,0] ...; Tab 2 model [161,0] ...
```

Tab 2 was at its preview slot (40,0), not its model slot (161,0). The fix is one restore path,
`restore_tiles`: every tile is slid to `state.rects()` through the same `slide_to`/`place_parts`
pair as any other animation, with its pill, and the drop line is hidden. It serves:

- a gesture `cancel` (the right-button case and any lost grab);
- Esc (a new `EventControllerKey` on the window);
- the window losing focus mid-drag (`notify::is-active` → `cancel_drag`);
- a release with no drop, including a release outside the window (`end_drag` now restores all,
  not only the dragged tile).

A border drag that is cancelled is still taken and dropped without touching the model, so its
already-written heights stay. `begin_drag` now ignores non-left buttons
(`GestureSingleExt::current_button`), so the right press cannot start a new drag after the reset.
The pill's menu is suppressed while a drag is active, and for the one event in which the cancel
may arrive before the pill's click handler, a `menu_blocked` flag set by `cancel_drag` and cleared
by an idle blocks it as well.

### Item 2: the pill is part of every slide

r7's cause is confirmed **[read]**: `slide_to` moved only the tile's outer box, while `relayout`
moved each `pill_wrap` straight to the final position, and `drag_layout` moved only tiles, never
wraps. The frame margin and the grid's content offset had the same split.

The fix is a pure `pill_layout(slot, scroll, visible_h, scale) -> PillLayout` plus
`dragged_pill_layout` in `split.rs`. `PillLayout` carries the wrap rect (tile x and width, the
`stuck_geometry` pill y, the pill band tall), the frame's top margin and the grid's content
offset. `Inner::place_parts` applies it; `set_rect` (static layout), every `slide_to` frame and
`restore_tiles` all go through `place_parts` with the same rect. The dragged tile uses
`place_dragged`, which sets its box and its natural-geometry pill in one call. `pill_content_rect`
(hit tests) now takes the pill y from the same `pill_layout`, so hit tests and drawing agree.
The generation check in `slide_to` runs before `place_parts`, so a newer slide or `set_rect` stops
an older animation from moving either the tile or its pill.

### Unit tests

Added to the look-body binary:

- `split::tests::a_pills_wrap_sits_with_its_tile_when_not_stuck` — not stuck: wrap = slot x/y/w,
  band height, frame margin band, offset 0; the wrap travels with the slot after a scroll.
- `split::tests::a_stuck_pills_wrap_holds_the_view_top_with_its_frame` — stuck at scroll 200:
  wrap y = 200 (view 0), frame margin 158, offset 100, and `pill_content_rect` uses the same y.
- `split::tests::a_dragged_tile_keeps_its_pill_at_its_own_top` — the dragged layout ignores the
  stuck position and puts the pill at the tile's own top.

### Temporary timer log (harness removed before delivery)

Debug binary, `GIMBAL_R8_TIMER=1 GIMBAL_SPIKE_LOG=1 look-body --tiles 4`, niri-tiled (the split
content was narrow in this session, so the tile rects are 97 px wide; the positions are what
matter). The timer simulated the same states the gesture drives: `start_tile_drag` at Tab 1's pill,
`update_tile_drag` over Tab 2's slot, then `cancel_drag`; afterwards a second drag, a preview over
Tab 3 and a drop over Tab 2 through the drop path (the temporary `end_drag_at`, now
`finish_tile_drag`). It read each tile's Fixed position/size and each `pill_wrap`'s Fixed
position. [observed]

```text
[1.445] temp: r8 before: Tab 1 rect [40,0] 97x563 pill y 0; Tab 2 rect [161,0] 97x563 pill y 0;
        Tab 3 rect [40,587] 97x564 pill y 587; Tab 4 rect [161,587] 97x564 pill y 587
[1.534] temp: r8 preview frame 1: Tab 2 tile [93.11609649658203,0] pill [93,0] pill y == tile y true
[1.584] temp: r8 preview frame 2: Tab 2 tile [56.527496337890625,0] pill [57,0] pill y == tile y true
[1.634] temp: r8 preview frame 3: Tab 2 tile [42.516422271728516,0] pill [43,0] pill y == tile y true
[1.834] temp: r8 at cancel: Tab 1 rect [161,256] 97x563 pill y 256; Tab 2 rect [40,0] 97x563 pill y 0;
        Tab 3 rect [40,587] 97x564 pill y 587; Tab 4 rect [161,587] 97x564 pill y 587
[1.834] temp: r8 at cancel model: Tab 1 model [40,0] 97x563; Tab 2 model [161,0] 97x563;
        Tab 3 model [40,587] 97x564; Tab 4 model [161,587] 97x564
[1.845] split: tile drag cancelled; sliding everything back
[2.134] temp: r8 after the 200 ms slide-back: Tab 1 rect [40,0] 97x563 pill y 0;
        Tab 2 rect [161,0] 97x563 pill y 0; Tab 3 rect [40,587] 97x564 pill y 587;
        Tab 4 rect [161,587] 97x564 pill y 587
[2.134] temp: r8 cancel check: every tile rect equals before true; every pill y equals its tile y true
[2.634] split: 'Tab 1' dropped at Swap(1)
[2.664] temp: r8 settle frame 1: Tab 2 tile [107.94338989257813,0] pill [108,0] pill y == tile y true
[2.714] temp: r8 settle frame 2: Tab 2 tile [63.608551025390625,0] pill [64,0] pill y == tile y true
[2.764] temp: r8 settle frame 3: Tab 2 tile [44.75578308105469,0] pill [45,0] pill y == tile y true
[2.984] temp: r8 after the settle: Tab 1 rect [161,0] 97x563 pill y 0;
        Tab 2 rect [40,0] 97x563 pill y 0; Tab 3 rect [40,587] 97x564 pill y 587;
        Tab 4 rect [161,587] 97x564 pill y 587
```

Every tile's rect at the cancel is its preview rect; 200 ms later all four equal the "before" rows,
and the check line says so. During both the preview slide and the settle, Tab 2's pill wrap x is
the tile x rounded to a whole pixel (the wrap moves in integers; the tile's position is kept as a
float), and its pill y equals its tile y at every frame. The harness methods and the timer call
were then removed; a grep for `r8_`, `end_drag_at` and `TEMPORARY` in `src/bin/look-body/` finds
nothing. [observed]

### RULES.md's four commands, from `$SPIKE`

`cargo fmt --check` (rustup 1.95.0 toolchain):

```text
(fmt exit 0; no output)
```

`cargo clippy --all-targets -- -D warnings`:

```text
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.45s
```

`cargo test`:

```text
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

`cargo build --release`:

```text
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 1.63s
```

### 10-second release runs (stopped by PID with SIGTERM, from `/tmp/gimbal-spike-p1m-r8`)

Default, `--tiles 5`, and `--tiles 6 --pins 14 --scale 0.5` each ran for 10 s: exit 143, 0 lines
on stderr and 0 on stdout. No GTK warnings, no `Gtk-CRITICAL`, no CSS/theme/icon warnings and no
panics. [observed]

### API citations

- A different button resets an active single-button gesture:
  `gtk-4.22.4:gtk/gtkgesturesingle.c:206-214`; `gtk_gesture_reset` cancels every sequence:
  `gtk-4.22.4:gtk/gtkgesture.c:748-750`.
- Touch is mapped to button 1, so `current_button() != 0 && != 1` only rejects extra mouse
  buttons: `gtk-4.22.4:gtk/gtkgesturesingle.c:151-155`.
- `GestureSingleExt::current_button`: `gtk4-0.11.5:src/auto/gesture_single.rs:35`.
- `Window::is_active` (the `is-active` notify): `gtk4-0.11.5:src/auto/window.rs:728`.
- `EventControllerKey::connect_key_pressed`: `gtk4-0.11.5:src/event_controller_key.rs:11`.
- `gdk::Key::Escape`: `gdk4-0.11.5:src/keys.rs:1068`.
- `glib::idle_add_local_once`: `glib-0.22.10:src/source.rs:353`.
- `GtkFixed::child_position` (used only by the removed timer harness):
  `gtk4-0.11.5:src/auto/fixed.rs:278`.
- The `GtkFixed` allocation behaviour behind the separate pill wrap is unchanged from r6/r7.

### Not verified live (Claude's check)

- The right button mid-drag with a real pointer: that the fixed's `GestureDrag` cancels, that no
  context menu opens, and that the second button does not start a new drag. The timer log calls
  `cancel_drag` directly. [inferred]
- Esc with a real keyboard, and the window losing focus mid-drag: the key and `is-active` wiring
  is read from the pinned crate sources, but no key or focus event was injected (RULES). [inferred]
- A release outside the window: `end_drag`'s no-drop path now restores every tile, but the real
  grab/lost-grab sequence was not replayed. [inferred]
- The pill following its tile visually: the timer reads the Fixed positions of the tile and its
  wrap, which are what is moved, but no screenshot was taken. [inferred]
- The right press's event order (pill click handler before or after the fixed's gesture reset)
  under a real pointer; both orders are handled by the drag check and the one-idle block. [inferred]

### Anything I'd change outside my files

None. `grid.rs`, `style.rs` and the rest of `look-body` are unchanged apart from the Esc/focus
wiring in `main.rs` and the new code in `split.rs`. The spec's two new rules already say what this
implements; it could add that a cancelled border drag keeps its written sizes (the current code
already does that).

## Review and live check of P1m-r8 (Claude, with Kyle's look, Nov 2, 2026): passes

Kyle: "aesthetically it's pretty much there"; he'd like the animations a little slower overall
(separate change, below). Reviewed the diff (`split.rs`, `main.rs`). The model is only written on
a drop, so `restore_tiles` sliding every tile to `state.rects` is the right single restore.
`place_parts` / `apply_parts` is now the one place a pill, frame margin and grid offset are set,
for the static layout and every slide frame, and the generation guard runs before it.
`begin_drag` ignores non-left buttons, and the menu is blocked while a drag is active and for one
idle after a cancel.

Gates, from `spike/` (rustup 1.95.0): fmt clean; clippy `-D warnings` clean; tests 4 + 70 + 12
pass; release build ok. The three 10-second runs printed nothing. No temporary logs left.

Live, release build, niri-tiled on DP-2 (DP-3 off), 4 tiles. Tab 1 was dragged by its pill over
Tab 2 and held:

- **Mid-drag:** Tab 2 had slid into Tab 1's place **with its pill**, and the dragged Tab 1 carried
  its own pill.
- **Right button pressed, then left released:** log `tile drag cancelled; sliding everything back`.
  All four tiles and pills were back in their starting places, nothing overlapped, and no context
  menu opened.
- **Esc instead:** the same result; the log also has `Escape cancelled the drag`.

Not replayed: a release outside the window and a focus loss mid-drag (both go through the same
restore), and a frame-by-frame look at the pill during a slide (the agent's timer log covers it).

### Animation durations today (for Kyle's "a little bit slower")

| Where | What | ms |
|---|---|---|
| look-header `style.rs` | pill hover background | 120 |
| look-header `style.rs` | tab pill reorder slide (`.gl-pill-wrap`) | 150 |
| look-header `header.rs` | search open/close crossfade | 200 |
| look-header `header.rs` | accordion / pill fade (`FADE_MS`, plus timers at `FADE_MS` and 210) | 200 |
| look-body `sidebar.rs` | pinned row slide | 150 |
| look-body `sidebar.rs` | section expand/collapse | 200 |
| look-body `split.rs` | tile move/resize, drop settle, slide back | 200 |
| look-body `split.rs` | split view fade in/out (plus a 250 ms timer) | 250 |
