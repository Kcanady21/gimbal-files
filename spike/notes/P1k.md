# P1k: translucency, compositor blur, thinner split-tile padding

## What I built

Both look prototypes now take their window opacity from `GIMBAL_LOOK_OPACITY` (0.50–1.00,
default 0.85), paint every role fill as `alpha(<role>, opacity)`, and leave the window-background
fills out of the header, path bar, open search field, panels and frames. `look-body`'s dev bar
has a live opacity slider. Menus, popovers and dialogs keep the theme's opaque defaults.

`src/look_shared/blur.rs` binds niri's staging `ext_background_effect_manager_v1`, waits for its
`capabilities` event, creates an `ext_background_effect_surface_v1` for the toplevel's
`wl_surface` when the window is realized, and gives it a `wl_region` that approximates the
window's visible rounded rectangle with 9 rectangles (one body plus 4 strips per corner).
The region is recomputed when the surface size or the app scale changes, removed at opacity
1.00 and restored below 1.00. It never includes GTK's client-side shadow. On X11 or when the
global/capability is missing it logs once and leaves the CSS transparency in place.

The split tile's `GridView` is transparent and its margin inside the tile is `gap-m` instead of
`gap-2xl`; `layout::one_column_width` and its tests count `gap-m` now.

## Dependency exception

Only these were added to `spike/Cargo.toml` (all exact, `=`):

```toml
gdk4-wayland = { version = "=0.11.5", features = ["wayland_crate"] }
wayland-client = "=0.31.15"
wayland-protocols = { version = "=0.32.13", features = ["client", "staging"] }
```

- `gdk4-wayland 0.11.5` is the same gtk-rs 0.11 release as `gdk4 0.11.5`; its feature is named
  `wayland_crate` in `gdk4-wayland-0.11.5:Cargo.toml:55-60`, and it exposes
  `WaylandDisplay::wl_display()` / `WaylandSurface::wl_surface()`
  (`gdk4-wayland-0.11.5:src/wayland_display.rs:73`, `src/wayland_surface.rs:30`). **[read]**
- `wayland-client =0.31.15` and `wayland-protocols =0.32.13` were already in
  `~/.cargo/registry`; cargo resolved `wayland-backend 0.3.17` and `wayland-scanner 0.31.11`
  as transitive dependencies. Nothing else was added. **[observed]** (`Cargo.lock`)
- I read the protocol's
  `protocols/staging/ext-background-effect/ext-background-effect-v1.xml`: the global is
  `ext_background_effect_manager_v1` v1, `capabilities` carries the `blur` bit
  (`:62-66`), `get_background_effect` creates the surface object (`:67-90`), and
  `set_blur_region` is "double-buffered state, and will be applied on the next
  wl_surface.commit" (`:104-123`, quote at `:116-117`). **[read]**
- niri 26.04 advertises `ext_background_effect_manager_v1` name 34, v1; the run log below
  shows `capabilities Capability(Blur) (blur true)`. **[observed]**

## How the blur is requested

- A second `wayland_client::Connection` is made from GDK's own backend
  (`WlDisplay::backend().upgrade()`, `wayland-client-0.31.15:src/lib.rs:259`,
  `src/conn.rs:111`), then a separate `EventQueue`/`QueueHandle`
  (`src/conn.rs:106`, `src/event_queue.rs:375`). The registry is `wl_display.get_registry`
  on that queue, and two `roundtrip`s (`src/event_queue.rs:420`) collect the globals and the
  `capabilities` event. **[observed]**
- The manager is bound with `version.min(1)`. The effect object is created from
  `GdkWaylandSurface::wl_surface()` in the window's `realize` handler. **[observed]**
- The surface buffer is larger than the GTK window because GTK draws client-side shadows.
  `gtk_window_native_get_surface_transform` returns `shadow.left - margin_rect.origin.x`
  (`gtk-4.22.4:gtk/gtkwindow.c:2274-2287`), i.e. the window widget's offset inside the surface;
  `get_shadow_width` is `gtk-4.22.4:gtk/gtkwindow.c:4180-4212` and the surface allocation excludes
  those shadows in `gtk_window_native_layout` (`:2295-2341`). I use the window widget rect at that
  offset, so the region is the visible window, not the shadow. The live surface was
  `1322×1448`, the window `1272×1398` at `(25, 25)`; the region starts at `(25, 25)` and is
  `1272×1398`. **[observed]**
- Corner strips: `rounded_region_rects(w, h, radius, 4)` puts a full-width body between
  `y = radius` and `y = h − radius`, then 4 rows per corner. Each row's inset is taken at its
  outer edge (`r − isqrt(r² − (r − y0)²)`), so every rectangle stays inside the rounded
  rectangle; the corners are cut slightly more than a true circle. `radius = 12 × app scale`;
  at 0.50 the radius is 6 and the strips are 1–2 px high. **[read]** for the arithmetic,
  **[observed]** for the emitted log.
- Size changes: the `GdkSurface::layout` signal (`gdk4-0.11.5:src/auto/surface.rs:337`) is
  filtered against the last reported surface size, because GTK also emits it for relayouts
  that do not change the size (`gtk-4.22.4:gdk/gdksurface.c:646-660`). App-scale changes call
  `set_scale` directly from `apply_scale`/`step_scale`. **[read]**
- Every change ends with `GdkSurface::queue_render`
  (`gdk4-0.11.5:src/auto/surface.rs:218-223`) so the surface commits and the double-buffered
  region is applied. **[read]** for the API; the logs show the region is sent, the visual
  commit is for the live check.
- Opacity 1.00 sends `set_blur_region(None)`; going back below 1.00 creates a new `wl_region`
  and sets it again. The effect object itself is kept. **[observed]**
- On X11 the `gdk::Display` downcast to `WaylandDisplay` fails; if the global is missing or
  blur is not advertised, `bind` returns an error. `Inner::fallback` logs once and every later
  call returns. **[observed]** for the Wayland path; X11 is **[inferred]** from the downcast.

## Opacity and CSS [observed]

`style::opacity()` parses `GIMBAL_LOOK_OPACITY`, accepts finite values in `[0.50, 1.00]`, and
otherwise logs one line and returns `0.85`. Unit tests cover valid values, `0.49`, `1.01`,
`x`, the empty string, `NaN` and `inf`, plus the CSS clamp (`css(..., 0.2)` writes
`alpha(@window_bg_color, 0.50)`, `css(..., 1.5)` writes `1.00`).

Every role fill uses `alpha(<role>, opacity)`. The window itself is
`alpha(@window_bg_color, opacity)`. These widgets draw **no** fill (their old background-color
is gone): `window.gimbal-look.background` is the window fill; `.gl-header`, `.gl-field` (path bar
and open search field), `.gl-frame`, `.gl-panel` and `.gl-header-slot` draw none. The pinned list
and its selected row, the drive track and fills, the pills, the tiles and the single-view grid
were already or are now explicit fills; the grid is `transparent`. The clear rule inside both
windows is scoped to `window.gimbal-look`:

```css
window.gimbal-look .gl-frame gridview, ... viewport,
window.gimbal-look .gl-frame gridview > child,
window.gimbal-look windowhandle { background-color: transparent; }
```

Outlines (`@accent_color`), rules (`alpha(@window_fg_color, 0.3)`), focus rings
(`alpha(@accent_color, 0.5)`), text and icons are not multiplied by the window opacity.
`.gl-section-pill` still has no fill. Menus/popovers/dialogs are not touched by the clear rule,
so libadwaita's opaque popover/dialog backgrounds remain.

`look-body`'s dev bar now has a second plain `gtk::Scale` (0.50–1.00, step 0.05, initial value
from the environment); `value-changed` reloads the same `CssProvider` and calls
`Blur::set_opacity`. The slider is harness furniture like the existing scale/drive sliders.
`look-header` has no slider; its only scale changes still come from Ctrl+plus/minus.

### No widget opacity

The two pre-existing P1i/P1j fades were moved to `GtkRevealer` crossfades instead of leaving
CSS `opacity`/`Widget::set_opacity` in the translucency code:

- header pills: `pill_revealer` wraps each pill, `GtkRevealerTransitionType::Crossfade`,
  200 ms; the group fade-out calls `set_reveal_child(false)` and the group fade-in starts
  hidden and reveals after 20 ms (`gtk4-0.11.5:src/auto/revealer.rs:71,102,110,118`).
- split view: the same 250 ms crossfade wraps the split `ScrolledWindow`; `fade_out` waits
  250 ms and only switches the stack if the revealer is still hidden.

The `grep` below still finds the **word** `opacity` in function parameters, `Cell<f64>` fields
and our own `Blur::set_opacity` (which sends a blur region, not a widget fade). There is no
CSS `opacity:` rule left and no `gtk::Widget::set_opacity` call.

## Tile padding, layout and columns

- `look-body/style.rs`: `gridview` / `viewport` inside `.gl-frame` are transparent, `.gl-grid`
  has `background-color: transparent`, and `.gl-grid.tile { margin: gap-m }` (16 at scale 1);
  the single view keeps `margin: gap-2xl grid-margin 0 grid-margin`.
- `layout::one_column_width`'s split term is now
  `2 × gap-2xl (view sides) + 2 × gap-m (tile grid) + 2 × line-thin + cell` = 215 px at scale 1;
  the single view remains `2 × grid-margin + cell` = 241 px, so the content floor is still
  driven by the single view. The window floor therefore drops from 941 to **919 px** (the old
  split term was 263). Tests updated: `defaults_match_the_sketch` (919),
  `one_column_counts_the_thinner_tile_margin` (split 215, overall 241 and 121).
- **Columns at the default design width**: 2560 × 1440 with sidebars 348/648 and scale 1.00,
  the content column is 1546 px, a split tile is 713 px, its grid viewport is
  713 − 2×3 (border) − 2×16 (gap-m) = 675 px, and at the 97 px cell + 32 px gap pitch that is
  **5 columns** — the sketch's split grid. At scale 0.50 the same width gives 15 columns.
  For reference, the same width function on niri's tiled 1272 × 1398 window gives 1 column at
  scale 1.00 and 5 at scale 0.50; the final `--tiles 6 --scale 0.5` log below shows a 332 px
  tile grid viewport (cap 6, 5 cells fit). The 2,560 px figures are **[inferred]** from the
  pure width function; the 332 px live viewport is **[observed]**.

## Verification

### RULES.md's four commands (from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
exit=0

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
exit=0

$ cargo test
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 33 tests
test result: ok. 33 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
running 9 tests
test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
exit=0

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
exit=0
```

`look-body`'s 33 include `blur::tests::region_rects_stay_inside_and_cut_the_corners`,
`region_rects_at_half_scale`, `region_rects_degenerate_sizes`,
`style::tests::opacity_parsing_falls_back`, `layout::tests::one_column_counts_the_thinner_tile_margin`;
`look-header`'s 9 include the same blur/opacity tests and its CSS shape test. **[observed]**

### 10-second delivery runs (release, SIGTERM by PID, no GTK CSS warnings)

Run from `/tmp/gimbal-spike-p1k` (created for this brief), each for 9 s and stopped by its own
PID with SIGTERM. Exit 143 is the SIGTERM. `GIMBAL_SPIKE_LOG=1` was set.

```text
$ ./target/release/look-header --tabs 3
[gimbal-look-header 0.081] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal-look-header 0.081] blur: capabilities Capability(Blur) (blur true)
[gimbal-look-header 0.081] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.120] window: header window shown
[gimbal-look-header 0.126] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 0.127] blur: surface layout changed to 1322×1448
[gimbal-look-header 0.127] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
exit=143

$ ./target/release/look-body
[gimbal-look-body 0.084] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal-look-body 0.084] blur: capabilities Capability(Blur) (blur true)
[gimbal-look-body 0.085] grid: viewport 0 px at scale 1.00 → cap 2
[gimbal-look-body 0.085] layout: scale 1.00: left 348, content 0, preview 648 (inset 8)
[gimbal-look-body 0.142] blur: surface layout changed to 1322×1448
[gimbal-look-body 0.142] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]
[gimbal-look-body 0.147] grid: viewport 114 px at scale 1.00 → cap 2
exit=143

$ ./target/release/look-body --tiles 6 --scale 0.5
[gimbal-look-body 0.084] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal-look-body 0.084] blur: capabilities Capability(Blur) (blur true)
[gimbal-look-body 0.091] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.091] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.091] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.092] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.092] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.092] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.092] grid: viewport 0 px at scale 0.50 → cap 2
[gimbal-look-body 0.092] layout: scale 0.50: left 174, content 0, preview 324 (inset 4)
[gimbal-look-body 0.161] blur: surface layout changed to 1322×1448
[gimbal-look-body 0.162] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 6, 9 rects: [Rect { x: 25, y: 31, w: 1272, h: 1386 }, Rect { x: 31, y: 25, w: 1260, h: 1 }, Rect { x: 31, y: 1422, w: 1260, h: 1 }, Rect { x: 28, y: 26, w: 1266, h: 2 }, Rect { x: 28, y: 1420, w: 1266, h: 2 }, Rect { x: 26, y: 28, w: 1270, h: 1 }, Rect { x: 26, y: 1419, w: 1270, h: 1 }, Rect { x: 26, y: 29, w: 1270, h: 2 }, Rect { x: 26, y: 1417, w: 1270, h: 2 }]
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
[gimbal-look-body 0.168] grid: viewport 332 px at scale 0.50 → cap 6
exit=143
```

`grep -cE "Theme parser|Gtk-WARNING|Gtk-CRITICAL|Gdk-WARNING|CSS" final-*.err` returned
`0` for all three files. The 1,272 px window is niri's half-output tiling of the default
2,560 px window; the 25 px surface inset is GTK's client-side shadow. **[observed]**

### Temporary timer tests (removed before delivery)

A `GIMBAL_SPIKE_P1K_TEST=1` harness in `look-body`'s `show()` (then removed; source now
contains no `P1K_TEST` or `temp:` string) ran its first tick 1.2 s after `present` and the
next two 600 ms apart (the log clock read about 2.0 s, 2.6 s and 3.2 s). It read the
opacity in effect, dumped every generated CSS line containing `background-color`, called the
pure region builder at 2,560 and 1,600 wide, then drove the real opacity slider
(`set_value(1.00)`, then `0.85`) and finally the real scale path (`setting = 0.50`,
`apply_scale`).

```text
[gimbal-look-body 2.036] temp: opacity in effect 0.85, scale 1.00
[gimbal-look-body 2.036] temp: css window.gimbal-look, window.gimbal-look.background { background-color: alpha(@window_bg_color, 0.85); ... }
[gimbal-look-body 2.036] temp: css .gl-title { ... } .gl-rule { ... background-color: alpha(@window_fg_color, 0.3); }
[gimbal-look-body 2.036] temp: css .gl-icon-button:hover { background-color: alpha(@window_fg_color, 0.1); } ...
[gimbal-look-body 2.036] temp: css .gl-pill { ... background-color: alpha(shade(@window_bg_color, 0.8), 0.85); ... }
[gimbal-look-body 2.036] temp: css .gl-pill.active { background-color: alpha(@card_bg_color, 0.85); ... }
[gimbal-look-body 2.036] temp: css .gl-pill.selected { background-color: alpha(mix(shade(@window_bg_color, 0.8), @accent_color, 0.3), 0.85); } .gl-pill.active.selected { background-color: alpha(mix(@card_bg_color, @accent_color, 0.3), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-pill-close:hover { background-color: alpha(@window_fg_color, 0.15); } ...
[gimbal-look-body 2.036] temp: css .gl-section-pill:hover { background-color: alpha(@window_fg_color, 0.1); } ...
[gimbal-look-body 2.036] temp: css .gl-pinned { background-color: alpha(@sidebar_bg_color, 0.85); ... }
[gimbal-look-body 2.036] temp: css .gl-pinned-row.sel { background-color: alpha(mix(@sidebar_bg_color, @sidebar_fg_color, 0.12), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-track { ... background-color: alpha(@card_bg_color, 0.85); } ...
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv0 { background-color: alpha(mix(@card_bg_color, @window_fg_color, 0.2), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv1 { ... 0.28), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv2 { ... 0.36), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv3 { ... 0.44), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv4 { ... 0.52), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv5 { ... 0.6000000000000001), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv6 { ... 0.6799999999999999), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv7 { ... 0.76), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv8 { ... 0.8400000000000001), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-lv9 { ... 0.9199999999999999), 0.85); }
[gimbal-look-body 2.036] temp: css .gl-drive-fill.bar-full { background-color: alpha(@error_bg_color, 0.85); }
[gimbal-look-body 2.036] temp: css .gl-grid { ... background-color: transparent; } .gl-grid.tile { margin: 16px; }
[gimbal-look-body 2.036] temp: css .gl-tile { background-color: alpha(@card_bg_color, 0.85); ... }
[gimbal-look-body 2.036] temp: css window.gimbal-look .gl-frame gridview, ... viewport, ... { background-color: transparent; }
[gimbal-look-body 2.036] temp: rects 2560×1400 at scale 1.00, radius 12: [Rect { x: 0, y: 12, w: 2560, h: 1376 }, Rect { x: 12, y: 0, w: 2536, h: 3 }, Rect { x: 12, y: 1397, w: 2536, h: 3 }, Rect { x: 5, y: 3, w: 2550, h: 3 }, Rect { x: 5, y: 1394, w: 2550, h: 3 }, Rect { x: 2, y: 6, w: 2556, h: 3 }, Rect { x: 2, y: 1391, w: 2556, h: 3 }, Rect { x: 1, y: 9, w: 2558, h: 3 }, Rect { x: 1, y: 1388, w: 2558, h: 3 }]
[gimbal-look-body 2.036] temp: rects 1600×1000 at scale 1.00, radius 12: [Rect { x: 0, y: 12, w: 1600, h: 976 }, Rect { x: 12, y: 0, w: 1576, h: 3 }, Rect { x: 12, y: 997, w: 1576, h: 3 }, Rect { x: 5, y: 3, w: 1590, h: 3 }, Rect { x: 5, y: 994, w: 1590, h: 3 }, Rect { x: 2, y: 6, w: 1596, h: 3 }, Rect { x: 2, y: 991, w: 1596, h: 3 }, Rect { x: 1, y: 9, w: 1598, h: 3 }, Rect { x: 1, y: 988, w: 1598, h: 3 }]
[gimbal-look-body 2.036] temp: rects 2560×1400 at scale 0.50, radius 6: [Rect { x: 0, y: 6, w: 2560, h: 1388 }, Rect { x: 6, y: 0, w: 2548, h: 1 }, Rect { x: 6, y: 1399, w: 2548, h: 1 }, Rect { x: 3, y: 1, w: 2554, h: 2 }, Rect { x: 3, y: 1397, w: 2554, h: 2 }, Rect { x: 1, y: 3, w: 2558, h: 1 }, Rect { x: 1, y: 1396, w: 2558, h: 1 }, Rect { x: 1, y: 4, w: 2558, h: 2 }, Rect { x: 1, y: 1394, w: 2558, h: 2 }]
[gimbal-look-body 2.036] temp: rects 1600×1000 at scale 0.50, radius 6: [Rect { x: 0, y: 6, w: 1600, h: 988 }, Rect { x: 6, y: 0, w: 1588, h: 1 }, Rect { x: 6, y: 999, w: 1588, h: 1 }, Rect { x: 3, y: 1, w: 1594, h: 2 }, Rect { x: 3, y: 997, w: 1594, h: 2 }, Rect { x: 1, y: 3, w: 1598, h: 1 }, Rect { x: 1, y: 996, w: 1598, h: 1 }, Rect { x: 1, y: 4, w: 1598, h: 2 }, Rect { x: 1, y: 994, w: 1598, h: 2 }]
[gimbal-look-body 2.037] blur: opacity 1.00: blur region removed
[gimbal-look-body 2.037] temp: slider 1.00; opacity 1.00
[gimbal-look-body 2.639] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [...]
[gimbal-look-body 2.639] temp: slider 0.85; opacity 0.85
[gimbal-look-body 3.240] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 6, 9 rects: [...]
[gimbal-look-body 3.240] temp: scale 0.50; blur radius should be 6
```

The log had 25 background-setting lines; the excerpt above keeps every rule class and
shortens some declarations. Transparent rules are the clear list; hover fills are interaction
alphas, not window-role fills. **[observed]**

An invalid environment value was also run:

```text
$ GIMBAL_SPIKE_LOG=1 GIMBAL_LOOK_OPACITY=1.5 ./target/debug/look-body
[gimbal-look-body 0.083] opacity: GIMBAL_LOOK_OPACITY="1.5" is not 0.50–1.00; using 0.85
[gimbal-look-body 0.878] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [...]
```

### `grep` check

```text
$ grep -rn "set_opacity\|opacity:" src/bin/look-* src/look_shared
src/bin/look-body/style.rs:100:pub fn bar_css(opacity: f64) -> String {
src/bin/look-body/style.rs:115:pub fn css(scale: f64, font: Option<&str>, opacity: f64) -> String {
src/bin/look-body/main.rs:141:    opacity: Cell<f64>,
src/bin/look-body/main.rs:182:        self.blur.set_opacity(self.opacity.get());
src/bin/look-body/main.rs:448:        opacity: Cell::new(opacity),
src/bin/look-header/style.rs:139:pub fn css(scale: f64, font: Option<&str>, opacity: f64) -> String {
src/bin/look-header/header.rs:39:    opacity: Cell<f64>,
src/bin/look-header/header.rs:189:        opacity: Cell::new(opacity),
src/look_shared/blur.rs:188:    opacity: Cell<f64>,
src/look_shared/blur.rs:208:    pub fn attach(window: &gtk::Window, scale: f64, opacity: f64) -> Self {
src/look_shared/blur.rs:211:            opacity: Cell::new(opacity),
src/look_shared/blur.rs:242:    pub fn set_opacity(&self, opacity: f64) {
src/look_shared/blur.rs:395:    "opacity {opacity:.2}: blur region removed"
src/look_shared/blur.rs:426:    "opacity {opacity:.2}: visible ..."
```

Every line is a parameter, a `Cell<f64>` field, or the shared blur API/log. `Blur::set_opacity`
only sets/removes a Wayland blur region; it is not `gtk::Widget::set_opacity`. There is no CSS
`opacity:` rule and no widget-opacity call left. The P1i/P1j fades are `GtkRevealer` crossfades
as described above. **[read]/[observed]**

## API citations [read]

- `gdk4-wayland-0.11.5:src/wayland_display.rs:73` `WaylandDisplay::wl_display`;
  `src/wayland_surface.rs:30` `WaylandSurface::wl_surface`; `Cargo.toml:55-60` the
  `wayland_crate` feature.
- `wayland-client-0.31.15:src/conn.rs:106` `new_event_queue`, `:111` `from_backend`,
  `:116` `backend`; `src/lib.rs:259` `Proxy::backend`; `src/event_queue.rs:375` `handle`,
  `:420` `roundtrip`.
- `gdk4-0.11.5:src/auto/surface.rs:218-223` `queue_render`, `:337` `connect_layout`,
  `:192` `scale_factor`.
- `gtk-4.22.4:gtk/gtkwindow.c:2274-2287` surface transform (shadow minus margin origin),
  `:4180-4212` `get_shadow_width`, `:2295-2341` surface layout/allocation excluding the shadow.
- `gtk4-0.11.5:src/auto/revealer.rs:71,102,110,118` `reveals_child`, `set_reveal_child`,
  `set_transition_duration`, `set_transition_type`.
- `wayland-protocols-0.32.13:protocols/staging/ext-background-effect/ext-background-effect-v1.xml:34`
  capabilities are sent on bind and on change; `:62-66` the `capabilities` event and `blur`
  bit; `:67-90` `get_background_effect`; `:104-123` `set_blur_region` and the double-buffered
  commit sentence at `:116-117`.
- `gtk-4.22.4:gdk/gdksurface.c:646-660` the `layout` signal's "changed or relayout" wording.

## Not verified live (Claude's live check)

- Whether niri actually blurs what is behind the window, and how the 0.85 alpha and the
  blur strength look against the Ghostty reference; the region is sent, the compositor's
  blur algorithm is not observable from the program. **[inferred]**
- The rounded-corner cut visually: the region is 9 rectangles per the builder, but whether
  the 4 strip rows per corner read as a smooth radius at scale 1.00 and 0.50 is visual.
- That the region lands exactly on the visible window on a floating 2,560 px window; the
  live runs were niri-tiled to 1,272 px, where surface 1,322 × 1,448 and window
  1,272 × 1,398 at (25, 25) showed the shadow exclusion, but the 2,560 px geometry is from
  the pure builder.
- The header and body fades after the `GtkRevealer` swap: the startup split fade-in ran under
  `--tiles 6` and the process stayed up, but the pill group fade and the Split 2 → ✕ → single
  fade-out are gesture-driven and were not exercised without input.
- The opacity slider's feel and the CSS/text contrast at 0.50, and that all menus/popovers
  and dialogs still look opaque over the translucent window.
- The X11 fallback line and a missing-global run (I could not remove niri's global).
- A real monitor scale-factor change (both outputs are at scale 1) and capability changes
  after startup (the event queue is only driven during the initial roundtrips).

## Files changed / outside my files

Changed only files under the brief's Output list:

- `spike/Cargo.toml` (the three pinned dependencies)
- `spike/src/look_shared/blur.rs` (new)
- `spike/src/bin/look-header/{main.rs,header.rs,style.rs}`
- `spike/src/bin/look-body/{main.rs,layout.rs,split.rs,style.rs}`
- `spike/notes/P1k.md` (this file)

One adjacent inconsistency I did **not** change: `look-header/style.rs` still gives `.gl-frame`
the old `line-heavy` border, while the Nov 1 spec now says `gl-frame` is a plain box with no
border and P1j removed it from `look-body`. P1k's items do not list that border, so I left it
alone rather than making an unrequested visual change; Claude/Kyle can drop it in the live pass.

`spike/Cargo.lock` was updated automatically by cargo when the three crates were resolved
(it now records `gdk4-wayland 0.11.5`, `wayland-client 0.31.15`,
`wayland-protocols 0.32.13`, plus transitive `wayland-backend 0.3.17` and
`wayland-scanner 0.31.11`). I did not hand-edit it. No file outside these was changed; no
`src/*.rs` module owned by P1b–P1g was touched, and no earlier notes file was edited. The
temporary test harness and `/tmp` scratch runs are removed; no binary is left running.

## Review of P1k (Claude, Nov 1, 2026)

Read the whole diff, `look_shared/blur.rs` in full, and `Cargo.toml`/`Cargo.lock`. The three
crates are pinned exactly as the brief allowed; the lock adds only them and their Wayland
dependencies (`wayland-backend`, `wayland-scanner`, `wayland-sys` and friends). The blur module
binds the global on GDK's own connection, checks the blur capability, places the region at the
window's offset inside the surface (so GTK's shadow stays out), keeps the corner strips inside the
curve, and falls back with one log line. The stylesheets follow the spec rule for rule. The two
fades that used CSS `opacity` are now `GtkRevealer` crossfades: not asked for, but it is what
"no CSS opacity on containers" implies, and both still work (below). RULES.md's four commands pass
here (46 tests).

Minor, not worth a round: the opacity slider does not snap to 0.05 steps on a drag (the live check
landed on 0.84). It is harness furniture, like the other dev-bar sliders.

## Live check of P1k (Claude, Nov 1, 2026): passes; P1k is done

Release builds, DP-2 only for the mouse part.

- **Found first: niri filled the background in.** At 0.85 the body's background read
  (42, 19, 82) everywhere, which is exactly 85 % of the window colour over niri's focus-ring colour
  (147, 95, 236). niri draws the focus ring as a solid rectangle behind windows it treats as having
  no decorations of their own, and GTK 4 never tells it otherwise (the same reason Kyle's Ghostty
  rule sets `draw-border-with-background false`). With Kyle's yes, his niri config now has a rule
  for app IDs starting `io.github.kcanady21.Gimbal` with `draw-border-with-background false`.
  For the real app: GimbalOS can ship that rule in `/etc/niri/config.kdl`; an app-side fix
  (telling niri through `xdg-decoration` that the window draws its own decorations) is for
  Phase 2 to look at.
- **Transparency and blur (items 1, 2).** With the rule, the body's background follows the
  wallpaper behind it (pink near the sun, dark lower down). Reconstructing what sits behind a plain
  strip of the window gives a smooth glow with none of the wallpaper's stars or rock edges, so it is
  blurred, not just faded. The header shows the same. Corners at scale 0.5 (blur radius 6, window
  radius 12) are clean: no blurred square outside the rounded edge.
- **Opacity slider.** Dragged to 1.00: the background is solid (24, 6, 55) at every sample point
  and the log says `blur region removed`. Back below 1.00: the blurred wallpaper returns.
- **Tiles (item 3).** No inner box; icons sit on the tile fill; 5 columns per tile at full width.
- **Fades.** Split 4 shows the tiles and Single returns to the grid; in the header, Split View makes
  "Group 1 · 2 tabs" and Ungroup restores the tabs. Screenshots take about 0.6 s, longer than the
  fades, so the fade frames themselves are for Kyle's eye in P1h.
- **Pill drag (regression check, pills now inside revealers):** follows the pointer, reorders
  (`tabs: moved pill 0 to 2`), the window stays put.
- No GTK warnings, no panics.
