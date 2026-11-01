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
