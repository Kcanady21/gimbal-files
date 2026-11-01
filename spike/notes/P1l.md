# P1l: header polish from the acceptance run

## What I built

- **Item 1, the path bar reclaims all free width.** The hidden search field's entry had
  `hexpand(true)`. `GtkBoxLayout` gives extra space to children whose
  `gtk_widget_compute_expand` is true (`gtk-4.22.4:gtk/gtkboxlayout.c:171-190,634,793`), and
  `GtkBox` and `GtkRevealer` compute expand by OR-ing their children
  (`gtk-4.22.4:gtk/gtkbox.c:182-199`, `gtk-4.22.4:gtk/gtkrevealer.c:414-423`). That recursion
  does not care that the revealer is collapsed, so the empty search box took **half of every free
  header pixel** and the path only reclaimed the other half. I removed the entry's `hexpand` and
  gave the entry a token-derived CSS `min-width` so it still fills the fixed 496 px field. The path
  is now the header's only expanding child. **Cause: a hidden revealer whose child still computed
  expand, not a stale `GtkFixed` width, slide offset or missing layout pass.**
- **Item 2, search collapse.** `SlideRight` translates the child to `width - child_width`, negative
  while closing (`gtk-4.22.4:gtk/gtkrevealer.c:537-540`), and `GtkRevealer` does not clip its child
  unless its overflow is `Hidden` (`gtk-4.22.4:gtk/gtkwidget.c:12131-12137`). The field's outline
  therefore painted under the back/forward/up buttons. It now uses `Crossfade`; a crossfade does
  no horizontal translation, so nothing can draw outside the revealer's area. A closed crossfade
  still measures its child (`gtk_revealer_measure` only checks the child's `visible` flag,
  `gtk-4.22.4:gtk/gtkrevealer.c:589-631`), so the field is hidden until `open_search` shows it and
  is hidden again 210 ms after a close. The space stays reserved while the 200 ms fade draws, then
  `relayout()` releases it.
- **Item 3, close from the accordion.** Each "N tabs" row is now a label button plus a circular ✕
  (`.gl-pill-close`). A tab's ✕ closes that tab; a group's ✕ uses the same `confirm_group`
  confirmation as the group pill's close. The model close is the pure function `Model::close_entry`
  (a tab closes itself, a group closes every tab it holds, focus moves to a neighbour, and the
  `WindowEmpty` result asks the caller to close the window). `close_list_entry` updates the model,
  the path/body text and the layout; while the header stays collapsed it refills the open popover
  in place and calls `popup()` again, and once the pills fit it rebuilds normal pills. `update_layout`
  never collapses a single remaining tab, so the last-tab case returns to one normal pill.
- **Item 4, spec changes.** Outlines are 3/2, the header window/search icons are 32, the ✕ rings
  are square circles, the focus ring is full accent and selection mixes 0.5 accent. The full
  old/new list is below.
- **Item 5, live theme and keys.** `main.rs` includes the spike's `theme.rs` unchanged with
  `#[path = "../../theme.rs"] mod theme;` and calls `app.connect_startup(theme::init)`, as
  `src/main.rs` does. A copied Noctalia CSS file was recoloured from a temporary timer; the
  monitor logged the change and the resolved named colours changed. Ctrl+= / Ctrl+- are the
  existing handler; a temporary timer exercised `step_scale`.

Only these files changed: `spike/src/bin/look-header/{main.rs,header.rs,strip.rs,style.rs}` and
this notes file. No `Cargo.toml`, `src/theme.rs`, `src/look_shared/` or other binary was edited.

## Item 1: cause, fix and the required test

**Before the fix** (temporary timer, `--tabs 3`, 2,552 px page, closing tabs at a constant width).
The search box was the `GtkBox` at x 288; it grew by exactly half of every 294 px the strip lost:

```text
start:          search box 392 wide, path x 1570, width 734;  strip 890
closed one:     search box 539 wide, path x 1423, width 881;  strip 596
closed to one:  search box 686 wide, path x 1276, width 1028; strip 302
```

**After the fix** (same steps): the search box stays 72 wide and the path moves by the full amount:

```text
start:          search box 72 wide, path x 1250, width 1038;  strip 890
closed one:     search box 72 wide, path x 956,  width 1332;  strip 596
```

The fix is in the search field: `search_entry.set_hexpand(true)` is gone, and
`.gl-search-field entry { min-width: 410px; }` (`search_w - icon-search - gap-m`, scaled) keeps the
entry filling its 496 px field. The header box now has exactly one computed-expand child (the path).

### The required timer test (harness removed before delivery)

3 tabs at the 2,560-wide request; narrow to 1,700 until the pills collapse to the accumulator; then
close tabs down to one. `free` is page minus the header's two insets minus every other child's
allocation; it equals the path width. The window request was 2,560 but the page size is 2,552, the
same CSD/niri value P1i reported.

```text
[gimbal-look-header 1.701] test: three wide: path x 1250, width 1038; strip x 360, width 890; free 1038; page 2552
[gimbal-look-header 2.700] test: after narrow: path x 499, width 937; strip x 360, width 139; free 937; page 1700
[gimbal-look-header 3.701] test: one tab: path x 662, width 774; strip x 360, width 302; free 774; page 1700
```

One-tab reference at the same 1,700 width (a second run, `--tabs 1`, never collapsed):

```text
[gimbal-look-header 2.727] test: after narrow: path x 662, width 774; strip x 360, width 302; free 774; page 1700
```

The collapsed accumulator is 139 wide, so the path moves left to x 499 (it was x 360 + 890 = 1250
with three pills); closing to one normal pill returns it to **x 662, exactly where the one-tab run
is**. The harness was `GIMBAL_LOOK_TEMP_TEST=p1l`, a `timeout_add_local_once` chain; it is removed
below.

## Item 2: the search field must not draw under the nav buttons

Chosen option: **crossfade**. `SlideRight` is the only transition that translates on close; at
`current_pos` below 1 the child is placed at `width - child_width`, which is negative, and the
revealer paints its child without a clip. Crossfade keeps the child at x 0 and fades it, so the
outline can never reach the buttons. A crossfade revealer still reports the child's full size at
`current_pos == 0` when the child is visible (its size scale is always 1,
`gtk-4.22.4:gtk/gtkrevealer.c:181-185`), so `field.set_visible(false)` is the state that actually
closes it; `open_search` shows it just before the fade in, and the 210 ms close timer hides it and
calls `relayout()`.

Non-input check (temporary timer, removed): open then close, logging the revealer allocation and
the field/icon visibility:

```text
[gimbal-look-header 1.523] test: open: field visible true, revealer 496x50, icon visible false
[gimbal-look-header 2.723] test: closed: field visible false, revealer 0x50, icon visible true
```

No GTK warning appeared. Whether the 200 ms fade reads well, and the width release, need eyes; see
"Not verified live".

## Item 3: the model function and its tests

`strip.rs` adds:

```rust
pub enum Closed {
    Tab { remaining: usize },
    Group { tabs: usize, remaining: usize },
    WindowEmpty,
}

impl Model {
    pub fn close_entry(&mut self, index: usize) -> Option<Closed> { /* ... */ }
}
```

It is pure: no widget, no signal, no global. `close_entry` reuses the existing `close(&[index])`
focus/selection arithmetic. `None` is a stale index. The UI never decides the close itself; it only
chooses between "just close" (tab), "ask first" (group), and "close the window" (`WindowEmpty`).

`cargo test --bin look-header` includes:

```text
test strip::tests::closes_from_the_accordion_list ... ok
test strip::tests::groups_close_and_move ... ok
```

`closes_from_the_accordion_list` covers the three required cases: one tab from a three-tab list
(`Tab { remaining: 2 }`), a two-tab group (`Group { tabs: 2, remaining: 2 }`), and the last tab
(`WindowEmpty`, then a stale index returns `None`).

The UI path was also exercised without input (temporary timer): 4 tabs at 1,000 px, closing entry 0
three times through `close_list_entry`:

```text
[gimbal-look-header 2.605] test: list start: tabs 4, entries 4, overflow true, pills 1, accordion true
[gimbal-look-header 2.605] test: list start: popover visible false, rows 4, count label "4 tabs"
[gimbal-look-header 3.005] test: list closed one: tabs 3, entries 3, overflow true, pills 1, accordion true
[gimbal-look-header 3.005] test: list closed one: popover visible true, rows 3, count label "3 tabs"
[gimbal-look-header 4.005] test: list closed two: tabs 2, entries 2, overflow true, pills 1, accordion true
[gimbal-look-header 4.005] test: list closed two: popover visible true, rows 2, count label "2 tabs"
[gimbal-look-header 5.005] test: list closed to one: tabs 1, entries 1, overflow false, pills 1, accordion false
```

The rows, the "N tabs" label and the body/path text update in place; the popover is re-shown after
each close and is only replaced by normal pills when one tab is left. At the start the popover was
not open (the test did not hover the accumulator); the "stays open" claim for a real hover needs the
live check.

## Item 4: every size and colour changed

Tokens (`style.rs` `tok` / `col`):

| What | Old | New |
|---|---|---|
| `LINE_THIN` (section pills, tiles, trash, ✕ rings, focus-ring width) | 3 | **2** |
| `LINE_MID` (pills, path bar, search field, side panels) | 6 | **3** |
| `ICON_SEARCH` (collapsed search glyph) | 44 | **32** |
| `ICON_WINDOW` (float/close/minimize glyphs) | 24 | **32** |
| `FOCUS_RING` | `alpha(@accent_color, 0.5)` | **`@accent_color`** |
| `SELECTED` accent mix | 0.3 | **0.5** |
| `SELECTED_ACTIVE` accent mix | 0.3 | **0.5** |

`ICON_WINDOW` is also the trash glyph's size in the existing CSS, so the trash glyph follows it to
32; the spec's new row names `icon-window 32`, but item 4 only names float/close/minimize, so Kyle
may want the trash kept at 24 — it is one shared token. Everything else in the header is unchanged
(control/header heights, `PILL_W`, `SEARCH_W`, `PATH_MIN`, `TRASH`, `ICON_NAV` 32,
`ICON_TAB_CLOSE` 28, radius and type tokens).

Derived content sizes (the outline change keeps every **outer** size the same):

| Derived value | Old | New | Why |
|---|---|---|---|
| pill content width | 226 | 232 | outer pill stays 270 |
| field/pill height content | 38 | 44 | outer stays `control-h` 50 |
| search field content width | 452 | 458 | outer stays 496 |
| trash content | 42 | 44 | outer stays 48 |
| ✕ ring content | 22 | 24 | outer stays `icon-tab-close` 28 |
| `.gl-search-field entry` min-width | none | 410 | new; fills the field (item 1) |

`grep -n "padding: 0;"` in `.gl-pill-close` is the only CSS declaration added for the circular
rings; the ring was already `min-width == min-height == 24` content + 2 px border on every side with
`border-radius: 999`, so its border box is 28 × 28 at scale 1. The ✕ is the button's centred child.
The old rule relied on `all: unset` for padding; `padding: 0` now states it.

No other colour expression in the header changed.

## Item 5: live theme and keys

`main.rs`:

```rust
#[path = "../../theme.rs"]
mod theme;
...
app.connect_startup(theme::init);
```

Temporary theme test: a copy of `~/.config/gtk-4.0/noctalia.css` and its import in
`/tmp/gimbal-spike-p1l/theme/`, `GIMBAL_SPIKE_THEME_CSS` pointing at the copy, a timer rewriting
`#180637` → `#2a1145` and `#935fec` → `#22aa66`. The timer also logged the resolved
`window_bg_color` and `accent_color` through the window's style context:

```text
[gimbal-look-header 0.064] theme: reloaded file:///tmp/gimbal-spike-p1l/theme/noctalia.css
[gimbal-look-header 0.064] theme: watching /tmp/gimbal-spike-p1l/theme/noctalia.css
[gimbal-look-header 0.064] theme: watching /tmp/gimbal-spike-p1l/theme/noctalia-extensions.css
[gimbal-look-header 2.027] test: theme before: window_bg_color 0.094,0.024,0.216 accent_color 0.576,0.373,0.925
[gimbal-look-header 2.226] theme: change Changed on file:///tmp/gimbal-spike-p1l/theme/noctalia.css
[gimbal-look-header 2.226] theme: change ChangesDoneHint on file:///tmp/gimbal-spike-p1l/theme/noctalia.css
[gimbal-look-header 2.427] theme: reloaded file:///tmp/gimbal-spike-p1l/theme/noctalia.css
[gimbal-look-header 3.126] test: theme after: window_bg_color 0.165,0.067,0.271 accent_color 0.133,0.667,0.400
[gimbal-look-header 3.627] test: scale after a Ctrl+- step: 0.950
[gimbal-look-header 3.627] test: scale after a Ctrl+= step: 1.000
```

0.094/0.024/0.216 is `#180637`, 0.165/0.067/0.271 is `#2a1145`, 0.576/0.373/0.925 is `#935fec`,
0.133/0.667/0.400 is `#22aa66`. So the running header's named colours followed the file, not only
the provider's own CSS. The timer is removed. The real file was loaded at startup in the delivery
runs (`theme: reloaded .../.config/gtk-4.0/noctalia.css`).

Ctrl+= / Ctrl+- are the unchanged `Ui::key` branch; `step_scale` went 1.000 → 0.950 → 1.000 from the
temporary timer. The physical key events themselves were verified live in P1i; RULES forbids
injecting them here.

## Verification

### RULES.md's four commands (final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.25s
(exit 0)

$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-27f41c3be3242da9)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-0c97cc23f4ccae67)
test result: ok. 41 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-4abd02268319d43d)
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
(exit 0)

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 1.12s
(exit 0)
```

The brief's own checks also pass:

```text
$ cargo build --bin look-header
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.52s
(exit 0)

$ PATH=".../1.95.0/bin:$PATH" cargo clippy --bin look-header -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
(exit 0)
```

`look-header`'s ten tests are its existing CLI, style (fit, opacity, spec, drag) and strip
group/move tests, the three blur tests included from `look_shared/blur.rs`, and the new
`closes_from_the_accordion_list`. The style tests were updated for the new tokens:

```text
test blur::tests::region_rects_at_half_scale ... ok
test blur::tests::region_rects_degenerate_sizes ... ok
test blur::tests::region_rects_stay_inside_and_cut_the_corners ... ok
test strip::tests::closes_from_the_accordion_list ... ok
test strip::tests::groups_close_and_move ... ok
test style::tests::drag_arithmetic ... ok
test style::tests::opacity_parsing_falls_back ... ok
test style::tests::header_fits_stretches_then_collapses ... ok
test style::tests::spec_rules_and_css_shape ... ok
test tests::parses_cli ... ok
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

The style test now also asserts the 3/2 borders, a 32 px icon, the 0.5 selection mix and the plain
accent focus ring, and the exact `header_fits` boundaries were recomputed for the new icon sizes
(1,540 px at scale 1, 770 px at scale 0.5).

### 10-second delivery runs (release, SIGTERM by own PID, no GTK warnings)

Both ran in `/tmp/gimbal-spike-p1l`, for 10 s each, stopped by their own PID; `pgrep -x look-header`
was empty afterwards. The only warning is the allowed intermittent
`Gdk-WARNING … VK_SUBOPTIMAL_KHR`; there is no `Gtk-WARNING`, `Gtk-CRITICAL`, CSS or
"continuously requested" line.

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.081] theme: reloaded file:///home/kyle/.config/gtk-4.0/noctalia.css
[gimbal-look-header 0.081] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal-look-header 0.081] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal-look-header 0.083] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal-look-header 0.083] blur: capabilities Capability(Blur) (blur true)
[gimbal-look-header 0.083] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.122] window: header window shown
[gimbal-look-header 0.128] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 0.129] blur: surface layout changed to 1322×1448
[gimbal-look-header 0.129] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 12, 9 rects: [Rect { x: 25, y: 37, w: 1272, h: 1374 }, Rect { x: 37, y: 25, w: 1248, h: 3 }, Rect { x: 37, y: 1420, w: 1248, h: 3 }, Rect { x: 30, y: 28, w: 1262, h: 3 }, Rect { x: 30, y: 1417, w: 1262, h: 3 }, Rect { x: 27, y: 31, w: 1268, h: 3 }, Rect { x: 27, y: 1414, w: 1268, h: 3 }, Rect { x: 26, y: 34, w: 1270, h: 3 }, Rect { x: 26, y: 1411, w: 1270, h: 3 }]

(look-header:274256): Gdk-WARNING **: 13:42:30.686: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
exit=143

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
[gimbal-look-header 0.080] theme: reloaded file:///home/kyle/.config/gtk-4.0/noctalia.css
[gimbal-look-header 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia.css
[gimbal-look-header 0.080] theme: watching /home/kyle/.config/gtk-4.0/noctalia-extensions.css
[gimbal-look-header 0.082] blur: global ext_background_effect_manager_v1 v1 (name 34)
[gimbal-look-header 0.082] blur: capabilities Capability(Blur) (blur true)
[gimbal-look-header 0.082] layout: window 2560, path min 160, fits false, overflow true
[gimbal-look-header 0.121] window: header window shown
[gimbal-look-header 0.127] layout: window 1272, path min 160, fits false, overflow true
[gimbal-look-header 0.127] blur: surface layout changed to 1322×1448
[gimbal-look-header 0.127] blur: opacity 0.85: visible 1272×1398 at (25, 25), radius 6, 9 rects: [Rect { x: 25, y: 31, w: 1272, h: 1386 }, Rect { x: 31, y: 25, w: 1260, h: 1 }, Rect { x: 31, y: 1422, w: 1260, h: 1 }, Rect { x: 28, y: 26, w: 1266, h: 2 }, Rect { x: 28, y: 1420, w: 1266, h: 2 }, Rect { x: 26, y: 28, w: 1270, h: 1 }, Rect { x: 26, y: 1419, w: 1270, h: 1 }, Rect { x: 26, y: 29, w: 1270, h: 2 }, Rect { x: 26, y: 1417, w: 1270, h: 2 }]

(look-header:274372): Gdk-WARNING **: 13:42:38.764: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
exit=143
```

The only warning is the allowed intermittent `Gdk-WARNING … VK_SUBOPTIMAL_KHR`; there is no
`Gtk-WARNING`, `Gtk-CRITICAL`, CSS or "continuously requested" line. The 1,272 px layout lines are
niri tiling the request to half the output, as in P1i.

### Harness removal and process note

```text
$ grep -rn "TEMP\|GIMBAL_LOOK_TEMP_TEST\|temp_" src/bin/look-header/
(no output, exit 1)
```

`wc -l src/bin/look-header/*.rs`: `header.rs` 1163, `main.rs` 141, `strip.rs` 264, `style.rs` 335
(1,903). There is no line budget in this brief. Every test run was stopped by its own PID with
SIGTERM; `pgrep -x look-header` is empty at delivery. `/tmp/gimbal-spike-p1l` (logs and the theme
copy) and the temporary timers are removed.

## API citations [read]

- `RevealerTransitionType::Crossfade`: `gtk4-0.11.5:src/auto/enums.rs:9522`;
  `Revealer::set_transition_type` `gtk4-0.11.5:src/auto/revealer.rs:118`,
  `set_reveal_child` `:102`.
- `WidgetExt::get_visible` `gtk4-0.11.5:src/auto/widget.rs:846`; `set_visible` `:1444`.
- `PopoverExt::popup` `gtk4-0.11.5:src/auto/popover.rs:432`.
- `CenterBox::center_widget` `gtk4-0.11.5:src/auto/center_box.rs:69`, `end_widget` `:76`.
- Revealer `measure` checks only the child's `visible` flag and crossfade's size scale is 1:
  `gtk-4.22.4:gtk/gtkrevealer.c:589-631`, `:181-185`.
- SlideRight's negative translation: `gtk-4.22.4:gtk/gtkrevealer.c:537-540`; no clip in the
  revealer's snapshot: `:634-656`; a widget clips its children only at
  `GTK_OVERFLOW_HIDDEN`: `gtk-4.22.4:gtk/gtkwidget.c:12131-12137` (the same check also limits
  hit-testing at `:10443`).
- Expand propagation: `gtk_widget_compute_expand` `gtk-4.22.4:gtk/gtkwidget.c:8413-8426`;
  `GtkBox` ORs children `gtk-4.22.4:gtk/gtkbox.c:182-199`; `GtkRevealer` ORs its child
  `gtk-4.22.4:gtk/gtkrevealer.c:414-423`; `GtkBoxLayout` allocates extra to
  `gtk_widget_compute_expand` children `gtk-4.22.4:gtk/gtkboxlayout.c:171-190,634,793`.
- The theme module is the spike's unchanged `src/theme.rs` (P1f) via `#[path]`; it watches the file
  and its one-level imports and reloads at `STYLE_PROVIDER_PRIORITY_USER`, as P1f documents.

## Not verified live (Claude's live check)

- **The search animation.** The crossfade and the 496 → 0 revealer release were measured, but the
  200 ms fade, the instant width jump when the field appears (crossfade does not animate width),
  and the focus ring during the fade need eyes. If it reads badly, the alternative is a clipped
  `SlideRight` (overflow `Hidden`) with the search focus ring moved to the revealer.
- **The accordion's ✕ buttons.** Hovering "N tabs", clicking a tab's ✕, clicking a group's ✕ and
  accepting the shared confirmation, the list staying open across a close, the hover list updating,
  and the return to a normal pill when one tab is left. The model and the in-place refill were
  exercised without input; the pointer and dialog interaction need a live run.
- **The circular ✕ rings and the centred ✕** in pills and in the list. The CSS is a 28 × 28 border
  box with radius 999 and a centred child; whether it looks circular is visual.
- **The full-strength focus ring and the 0.5 selection mix.** Only the CSS values are asserted;
  contrast against Noctalia's current accent is visual.
- **The 32 px trash glyph.** It shares `icon-window`; item 4 only names float/close/minimize.
  Kyle should say whether the trash should stay 24 (then it needs its own token).
- **The live recolour.** The resolved named colours changed (above), but the on-screen header
  recolouring and the 200 ms monitor debounce are visual.
- **The path reclaim at scale 0.5 and with grouped tabs.** The test was at scale 1, three separate
  tabs; the code path is the same, but not run at another scale or with a group close.
- **Ctrl+= / Ctrl+- with real key events.** The handler is unchanged and `step_scale` was exercised;
  P1i verified the physical keys live.

## Things I would change outside these items

- `look-header/style.rs` still gives `.gl-frame` a `line-heavy` (10 px) accent border
  (`style.rs:208`), while the Nov 1 spec says `gl-frame` is a plain box with no border and that
  `line-heavy` is unused since the window frame was removed. Item 4 does not list it, so I left it
  exactly as P1k left it; dropping `border: {heavy}px solid {OUTLINE};` and the `heavy` binding is a
  small change if Kyle wants the frame gone in this round.
- Nothing else. No `src/theme.rs`, `src/look_shared/`, `Cargo.toml` or other binary was touched; the
  temporary `/tmp` folder and all timers are cleaned up.

## Review of P1l (Claude, Nov 1, 2026)

All five items done as asked, and item 1's cause is the real one: the hidden search entry's
`hexpand` made the collapsed revealer an expanding child, so it took half of every free header
pixel. The accordion close is a pure model function with the three tests asked for. Search now
crossfades and hides the field after the fade. RULES.md's four commands pass here (55 tests).

## Live check of P1l (Claude, Nov 1, 2026): passes, one icon left (P1l-r1-fixes.md)

- **Path bar:** with three pills at 2,560 the path takes the free width; narrowed to 1,400, the
  pills condense to "3 tabs" and the path bar starts right after it and fills the rest.
- **Accordion ✕:** hovering "3 tabs" shows each tab with a round ✕. Clicking Tab 2's ✕ logs
  `closed from the list: Tab { remaining: 2 }`, and the list stays open, now "2 tabs" with Tab 1
  and Tab 3.
- **Search:** opens and closes; the path bar gets its width back. (The fade itself is shorter than
  a screenshot.)
- **Outlines and sizes:** outlines are visibly thinner; the search icon is smaller.
- **Float button:** set to 32 px, but Adwaita's `window-restore-symbolic` glyph fills only a small
  part of its square, so it still looks tiny next to close. The mockup has its own float icon:
  P1l-r1 draws that one.
- No GTK warnings, no panics.

## P1l-r1 changes

The float button now draws Kyle's mockup icon instead of Adwaita's glyph. [observed]

### What changed

- `src/bin/look-header/icons/float.svg` (new): `phase1/icons/float.svg` converted to a GTK 4.22
  symbolic SVG. The `viewBox`, both `d` strings, `stroke-width="3"`, caps and joins are unchanged
  (geometry copied exactly). `back-window` keeps `fill="none"`; `front-window` takes
  `gpa:fill="error"`; both take `gpa:stroke="foreground"`.
- `header.rs`: `FLOAT_ICON` is `include_bytes!("icons/float.svg")`; `float_button()` builds
  `gtk::Svg::from_bytes(&glib::Bytes::from_static(..))`, wraps it in
  `gtk::Image::from_paintable(Some(&svg))`, and gives the button `gl-icon-button winbtn float`.
  The `float` action in `desktop_buttons()` now goes through `float_button()`; the other desktop
  buttons still use the icon theme. [observed]
- `style.rs`: `FLOAT_PALETTE = "error @window_bg_color"`, emitted as
  `.gl-icon-button.float { -gtk-icon-palette: …; }`; the image gets
  `-gtk-icon-size: icon-window × scale` (32 at 1.0, 16 at 0.5). The image inherits the palette
  and the button's `color: @window_fg_color`, so the strokes are `foreground`.
- No fixed colours in the SVG or the new CSS. [observed]

Why `gtk::Svg` rather than an icon name: GTK's icon-theme loader for `-symbolic.svg` uses the old
symbolic path and ignores `gpa:` slots (P1m's live test: all white). `GtkSvg` implements
`GtkSymbolicPaintable`, and GTK passes the image's `color` and `-gtk-icon-palette` into
`snapshot_symbolic` at draw time, so every slot recolours with the theme. [read]

### Temporary timer log (removed before delivery)

Probe: a `glib::timeout_add_local_once(800 ms)` cloned the float button, read
`image.paintable()`, the image/button allocation, `image.pixel_size()` and the palette expression;
it was gated by `GIMBAL_SPIKE_FLOAT_PROBE=1` and has been deleted from `header.rs`. [observed]

```
$ GIMBAL_SPIKE_FLOAT_PROBE=1 target/debug/look-header --tabs 3 --scale 1.0
temp: float paintable=GtkSvg image=32x32 button=32x32 pixel_size=-1 palette="error @window_bg_color" (error -> window_bg Some(RGBA { red: 0.09411765, green: 0.023529412, blue: 0.21568628, alpha: 1.0 })) color=RGBA { red: 0.9490196, green: 0.9490196, blue: 0.9529412, alpha: 1.0 }

$ GIMBAL_SPIKE_FLOAT_PROBE=1 target/debug/look-header --tabs 3 --scale 0.5
temp: float paintable=GtkSvg image=16x16 button=16x16 pixel_size=-1 palette="error @window_bg_color" (error -> window_bg Some(RGBA { red: 0.09411765, green: 0.023529412, blue: 0.21568628, alpha: 1.0 })) color=RGBA { red: 0.9490196, green: 0.9490196, blue: 0.9529412, alpha: 1.0 }
```

`pixel_size=-1` is why `-gtk-icon-size` drives the size:
`gtk-4.22.4:gtk/gtkiconhelper.c:403-410` returns the CSS `-gtk-icon-size` when no explicit pixel
size is set. The 32 → 16 change proves the CSS size reaches the image at both scales. A live
`Ctrl+=`/`Ctrl+-` regenerates the same string and reloads the same provider (`step_scale`), but only
the CLI values were measured. The icon box is square; the mockup's 28.08:25.12 aspect ratio is
fitted inside it (`gtkiconhelper.c:186-240`), so at 32 it paints about 32 × 28.6, matching the
mockup's proportions.

The style-context dump only printed custom properties (`--window-bg-color`, …), not normal style
properties, so the log records the palette expression and the resolved `@window_bg_color` rather
than a `-gtk-icon-palette: …` line. The mapping itself is asserted from the source: `foreground`
comes from `color`, and `error`/`warning`/`success`/`accent` come from `-gtk-icon-palette`
(`gtk-4.22.4:gtk/gtkcssstyle.c:876-899`). [read]

### Verified

From `$SPIKE`, with rustup 1.95.0 on `PATH` for `fmt`/`clippy`:

```
$ PATH="$T:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.43s

$ cargo test
...
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-…)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-…)
test result: ok. 43 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-…)
test result: ok. 11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 1.32s
```

`cargo build --bin look-header` and
`PATH="$T:$PATH" cargo clippy --bin look-header -- -D warnings` also pass. [observed]

`style::tests::float_icon_is_symbolic_and_maps_the_front_fill` checks the mockup ids and viewBox,
two `gpa:stroke="foreground"` outlines, exactly one `gpa:fill` (`error`), no `#`/`rgb(` colour
literals, the palette rule and `-gtk-icon-size` 32 at scale 1.0 / 16 at 0.5. [observed]

10-second runs (stopped by PID with SIGTERM; `pgrep -x look-header` empty afterwards):
`release/look-header --tabs 3` and `release/look-header --tabs 30 --scale 0.5` both printed an
empty stderr — no Gtk-WARNING/Gtk-CRITICAL/GLib-GObject warnings, no panics. The intermittent
`VK_SUBOPTIMAL_KHR` notice did not appear in these runs. [observed]

### API citations [read]

- `gtk::Svg::from_bytes`: `gtk4-0.11.5:src/auto/svg.rs:35`; `Svg` implements
  `GdkPaintable, SymbolicPaintable`: `:19-21`.
- `gtk::Image::from_paintable`: `gtk4-0.11.5:src/auto/image.rs:86`.
- `glib::Bytes::from_static`: `glib-0.22.*:src/bytes.rs:57`.
- Symbolic SVG slots: `gtk-4.22.4:docs/reference/gtk/icon-format.md:96-118` (supported elements
  and attributes), `:120-150` (`gpa` namespace; `gpa:stroke`/`gpa:fill` names at `:132-133`),
  `:152-154` (`-symbolic.svg` naming).
- Palette remap: `-gtk-icon-palette` registration and flags at
  `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:886-892`; parser at
  `gtk-4.22.4:gtk/gtkcsspalettevalue.c:318-361`; default palette includes the four names at
  `:298-312`; resolution of `foreground` from `color` and the four palette names at
  `gtk-4.22.4:gtk/gtkcssstyle.c:876-899`.
- How an image passes its colours to a symbolic paintable:
  `gtk-4.22.4:gtk/gtkrendericon.c:97-152` (`GTK_IS_SYMBOLIC_PAINTABLE`, five colours from
  `gtk_css_style_lookup_symbolic_colors`, `snapshot_with_weight`), reached from
  `gtk-4.22.4:gtk/gtkiconhelper.c:186-240`.
- `-gtk-icon-size`: `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1280`; used as the icon size when
  `pixel_size == -1`: `gtk-4.22.4:gtk/gtkiconhelper.c:403-410`.
- Theme redraw: `-gtk-icon-palette` carries `GTK_CSS_AFFECTS_ICON_REDRAW_SYMBOLIC`
  (`gtkcssstylepropertyimpl.c:891`), the same mechanism P1m's live accent test exercised.

### Not verified live (Claude's live check)

- How it looks next to close at 32 px: the mockup's two-window icon versus Adwaita's glyph, the
  stroke weight, and whether the front window's `@window_bg_color` fill fully hides the back
  window's outline. The geometry is exact and the allocation is 32 × 32, but the pixels were not
  seen. [inferred]
- The live theme reload's effect on the icon pixels: the palette uses `@window_bg_color`, and GTK's
  symbolic-redraw flag is read above, but no recolour was observed on screen. [inferred]
- The live `Ctrl+=`/`Ctrl+-` rescale of the float icon: the 32 and 16 px sizes came from two
  launches, not from a key-driven reload mid-run. [inferred]
- The 0.5-scale look (16 × 16 allocation): legibility at that size is visual. [inferred]

### Things I would change outside these files

- Nothing. Only `src/bin/look-header/` and this notes section were intentionally edited. The
  package-wide `cargo fmt`/`cargo test`/`cargo clippy`/`cargo build --release` commands that
  RULES.md requires ran while P1m's agent was also editing `src/bin/look-body/`; no look-body file
  was edited by hand.


## Review and live check of P1l-r1 (Claude, Nov 1, 2026): passes

The float icon is the mockup's two overlapping windows, converted with its geometry intact,
drawn as a `gtk::Svg` on a `gtk::Image`, and sized by `-gtk-icon-size` (32 at 1.0, 16 at 0.5).
RULES.md's four commands pass (61 tests). Live: the float button shows both windows at full size,
with the front window's fill hiding the back window's outline, in theme colours. No warnings.

Observation for Kyle, not a defect: next to the new float icon, Adwaita's close ✕ now looks the
smaller of the two. The mockup has its own close icon (VectorCraft group 66) if Kyle wants them
to match.

## P1l-r2 changes

The window's close button now draws Kyle's mockup ✕ instead of Adwaita's
`window-close-symbolic`, using the same drawing path as the float icon. Only the title-bar close
button changed; the ✕ in tab pills, the accordion list and dialogs still uses the icon theme.
[observed]

### What changed

- `src/bin/look-header/icons/close.svg` (new): `phase1/icons/close.svg` converted to a GTK 4.22
  symbolic SVG. The square `viewBox="0 0 28.08 28.08"`, both `d` strings, `stroke-width="3"`,
  caps and joins are unchanged (geometry copied exactly). Both paths are `fill="none"` with
  `gpa:stroke="foreground"`; the original `stroke="#000000"` is gone, so no fixed colour remains.
  [observed]
- `header.rs`: `CLOSE_ICON` is `include_bytes!("icons/close.svg")`. The float and close buttons
  now share `svg_button(bytes, class)`, which builds
  `gtk::Svg::from_bytes(&glib::Bytes::from_static(..))`, wraps it in
  `gtk::Image::from_paintable`, and gives the button `gl-icon-button winbtn` plus the extra class
  (`float` / `close`). `close_button()` is selected for the `close` action in `desktop_buttons()`
  in both branches (niri and non-niri); every other window action still uses `icon_button` and
  the icon theme. [observed]
- `style.rs`: `.gl-icon-button.close image { -gtk-icon-size: {iwin}px; }` mirrors the float rule,
  so the SVG image is `icon-window × scale` (32 at 1.0, 16 at 0.5). The image inherits the
  button's `color: @window_fg_color` from `.gl-icon-button`, so both strokes are the `foreground`
  slot. There is no `-gtk-icon-palette` rule for close because the icon has no palette slots.
  [observed]

Why the same drawing path: GTK's icon-theme loader for `-symbolic.svg` uses the old symbolic
path and ignores `gpa:` slots (P1m's live test: all white), and the mockup's geometry must stay
exact. `GtkSvg` implements `GtkSymbolicPaintable`, so GTK passes the image's `color` into
`snapshot_symbolic` at draw time and the strokes recolour with the theme. [read]

### Size and stroke match with the float icon

Both SVGs keep a 28.08 px wide view box, so when each is fitted into the same 32 × 32 icon box
the scale factor is 32/28.08 ≈ 1.140 and a `stroke-width="3"` draws about 3.42 px in both. The
float icon is 28.08 × 25.12 (it paints about 32 × 28.6 inside the square box, letterboxed); the
close icon is square at 28.08 × 28.08 and centred on the ✕, so it paints 32 × 32. [read]
[inferred]

### Temporary timer log (removed before delivery)

Probe: a `glib::timeout_add_local_once(800 ms)` took the float and close buttons from the
window-control list, read each one's child `gtk::Image`, its `paintable()`, its allocation,
`pixel_size()`, and the button allocation. It was gated by `GIMBAL_SPIKE_ICON_PROBE=1` and has
been deleted from `header.rs`. [observed]

```
$ GIMBAL_SPIKE_ICON_PROBE=1 target/debug/look-header --tabs 3 --scale 1.0
temp: float paintable=GtkSvg image=32x32 button=32x32 pixel_size=-1
temp: close paintable=GtkSvg image=32x32 button=32x32 pixel_size=-1

$ GIMBAL_SPIKE_ICON_PROBE=1 target/debug/look-header --tabs 3 --scale 0.5
temp: float paintable=GtkSvg image=16x16 button=16x16 pixel_size=-1
temp: close paintable=GtkSvg image=16x16 button=16x16 pixel_size=-1
```

Both are `GtkSvg` paintables at both scales, and both image and button allocations are the
square `icon-window` size. `pixel_size=-1` is why `-gtk-icon-size` drives the size
(`gtk-4.22.4:gtk/gtkiconhelper.c:403-410`); the 32 → 16 change proves the CSS size reaches both
images. The probe runs were stopped by PID with SIGTERM; `pgrep -x look-header` was empty
afterwards. [observed]

### Verified

From `$SPIKE`, with rustup 1.95.0 on `PATH` for `fmt`/`clippy`:

```
$ PATH="$T:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$T:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (.../spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.24s

$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-…)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-…)
test result: ok. 46 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-…)
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 1.39s
```

`cargo build --bin look-header` and
`PATH="$T:$PATH" cargo clippy --bin look-header -- -D warnings` also pass. [observed]

`style::tests::close_icon_is_symbolic_with_foreground_strokes` checks the square viewBox, the
two mockup path ids, two `gpa:stroke="foreground"` outlines, no `gpa:fill`, no `#`/`rgb(` colour
literals, `.gl-icon-button.close image` at 32 px for scale 1.0 and 16 px for 0.5, and that close
has no palette rule. [observed]

10-second runs (release, stopped by PID with SIGTERM; `pgrep -x look-header` empty afterwards):

```
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
... theme, blur and layout lines ...
exit=143   (no Gtk-WARNING, Gtk-CRITICAL, CSS or panics)

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
... theme, blur and layout lines ...
exit=143   (no Gtk-WARNING, Gtk-CRITICAL, CSS or panics)
```

The intermittent `VK_SUBOPTIMAL_KHR` Gdk-WARNING did not appear in these runs. [observed]

### Harness removal

```
$ grep -rn "PROBE\|TEMP\|temp_" src/bin/look-header/
(no output, exit 1)
```

No `Cargo.toml`, `src/theme.rs`, `src/look_shared/` or other binary was edited. [observed]

### API citations [read]

- `gtk::Svg::from_bytes`: `gtk4-0.11.5:src/auto/svg.rs:35`; `Svg` implements
  `GdkPaintable, SymbolicPaintable`: `:19-21`.
- `gtk::Image::from_paintable`: `gtk4-0.11.5:src/auto/image.rs:86`; `paintable`: `:157`;
  `pixel_size`: `:164`.
- `glib::Bytes::from_static`: `glib-0.22.*:src/bytes.rs:57`.
- Symbolic SVG slots: `gtk-4.22.4:docs/reference/gtk/icon-format.md:96-118` (supported elements
  and attributes), `:120-150` (`gpa` namespace; `gpa:stroke`/`gpa:fill` names at `:132-133`),
  `:152-154` (`-symbolic.svg` naming).
- Palette remap: `-gtk-icon-palette` registration and flags at
  `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:886-892`; parser at
  `gtk-4.22.4:gtk/gtkcsspalettevalue.c:318-361`; default palette includes the four names at
  `:298-312`; resolution of `foreground` from `color` and the four palette names at
  `gtk-4.22.4:gtk/gtkcssstyle.c:876-899`.
- How an image passes its colours to a symbolic paintable:
  `gtk-4.22.4:gtk/gtkrendericon.c:97-152`, reached from
  `gtk-4.22.4:gtk/gtkiconhelper.c:186-240`.
- `-gtk-icon-size` used as the icon size when `pixel_size == -1`:
  `gtk-4.22.4:gtk/gtkiconhelper.c:403-410`.

### Not verified live (Claude's live check)

- How the mockup's ✕ looks next to the float icon: the geometry and allocation are exact
  (above), but the drawn pixels and stroke weight were not seen. [inferred]
- Whether the square close icon's extents sit where Adwaita's glyph did, and whether the ✕ reads
  too large or small at 32 px. [inferred]
- The live theme reload's effect on the close strokes: both use the `foreground` slot and the
  same symbolic-redraw mechanism as float, but no recolour was observed on screen. [inferred]
- The live `Ctrl+=`/`Ctrl+-` rescale of the close icon: the 32 and 16 px sizes came from two
  launches, not a key-driven reload mid-run. [inferred]
- The 0.5-scale look (16 × 16 allocation): legibility at that size is visual. [inferred]

### Things I would change outside these files

- Nothing. Only `src/bin/look-header/` and this notes section were edited. The package-wide
  commands ran while P1m's agent may also be editing `src/bin/look-body/`; no look-body file was
  edited by hand.

## Review and live check of P1l-r2 (Claude, Nov 1, 2026): passes; P1l is done

The window's close button draws the mockup's ✕ through the same `gtk::Svg` path as float, from a
square view box the float icon's width, so both draw at the same stroke weight. Only the window
button changed. RULES.md's four commands pass (65 tests). Live: the ✕ now matches the float icon
in size and weight; no warnings.
