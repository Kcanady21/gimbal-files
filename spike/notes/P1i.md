# P1i notes: the header look

## What I built

`spike/src/bin/look-header/` — a standalone `look-header` binary. It does not import any of the
spike's modules, and I changed no other source file.

- `main.rs` — `Args` parser, the `log!` macro (`[gimbal-look-header <secs.millis>] <module>: …` when
  `GIMBAL_SPIKE_LOG=1`), `adw::Application` with `NON_UNIQUE`, and the CLI tests.
- `style.rs` — the token table, the colour-role table, `header_fits`, and the one `format!` that
  generates the whole stylesheet. `scale = GIMBAL_LOOK_BASE × setting`; every value is token × scale,
  rounded, with a 1 px minimum for non-zero borders.
- `strip.rs` — the fake tab/group model: entries, focus, selection, close, group/ungroup, rename and
  reorder. No filesystem access.
- `header.rs` — the header row, search revealer, custom pill strip, path bar, window buttons and all
  callbacks.

The header is a `WindowHandle` around a `ScrolledWindow` whose content is the header row. The
scroller has `min-content-width: 0` and `propagate-natural-width: false`, so the window can shrink
below the row's natural width; the row still receives the viewport width when it fits. That is what
lets the path bar shrink and the pills collapse instead of the window's minimum size being pinned
by a 960 px path bar or thirty pills. The body is a `gl-frame` box with one centred `gl-title` label.

## Behaviour

- **Pills** are `gl-pill` `CenterBox`es (no `AdwTabBar`/`AdwTabView`). A tab pill is `270 × scale`
  outer and `control-h` outer: because GTK's `min-width`/`min-height` are content-box, the CSS
  subtracts the 2 × `gap-m` padding and 2 × `line-mid` border to get the inner values. The measured
  outer pill at scale 1 was `270×50` **[observed]**.
- **Search** is a `gtk::Revealer` with `SlideRight`, 200 ms. I kept the Revealer: it grows its
  allocation and therefore pushes the pills and path bar right. I did not need the fallback
  `adw::TimedAnimation`. I have not judged the smoothness live **[inferred]**.
- **Search close**: Escape, or focus leaving the entry while it is empty. Opening hides the
  standalone search button and shows the 496 px field with its own 32 px glyph.
- **Selection**: plain click focuses and clears, Ctrl-click toggles, Shift-click selects the range
  from the focused pill, Escape clears.
- **Reorder**: `GestureDrag`; on release the horizontal offset is divided by the pill pitch and the
  entry moves. The pill does **not** follow the pointer during the drag, and the neighbours re-flow
  on release rather than sliding continuously. This is the one visible requirement I did not fully
  implement; it is the lowest-priority item in the brief.
- **Groups**: right-click → Split View with two or more selected tabs. Selected pills get a
  `fading` class (opacity transition, 200 ms), then the model groups them; the group pill starts
  faded and loses the class after 20 ms, so it fades in. Group pill text is `Group N · K tabs`;
  Rename replaces only the name, Ungroup restores separate pills, and the ✕ opens an
  `adw::AlertDialog` (`Close all K tabs in Group N?`, Cancel + destructive Close).
- **Overflow**: when `header_fits` says no, `rebuild` replaces the strip with one accordion pill
  showing the total tab count (groups count their tabs). Hovering it (also clicking it) opens a
  popover listing every entry; clicking a name focuses it. Widening restores the pills.
- **Path bar**: `width_request` is the `header_fits` result, clamped to `320×scale … 960×scale`.
  A spacer after it takes the remaining space, so the right-hand controls stay at the right edge.
  The refresh button is inside the field, `gap-m` from its end.
- **Live scale**: Ctrl+= / Ctrl+- step the 0.50–1.00 setting by 0.05, regenerate the CSS on the same
  `CssProvider`, and recompute the fit. `header_fits` assumes the niri two-button right group;
  `update_layout` subtracts one extra window-button width on non-niri desktops.
- **Desktop controls**: if `NIRI_SOCKET` is set there is no Minimize; Trash, Float
  (`window-restore-symbolic`) and Close. Otherwise Trash, Minimize, Maximize (toggle) and Close.
  Float runs `gio::Subprocess::newv(["niri","msg","action","toggle-window-floating"])` and logs the
  exit status; I never clicked it. Trash logs on click and is a `DropTarget` for `gdk::FileList`;
  a drop only logs `would trash N files: …`. There is no delete/trash call anywhere.
- **Menu**: New Tab (Ctrl+T) and Open in New Window (Ctrl+N, logs only). Back/forward/up log only.
- **Logging**: `[gimbal-look-header 1.567] layout: first pill 270x50 outer, path 960x50 outer, window 2470, overflow false`.

## Verified

### The four RULES commands (final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s

$ cargo test
    Running unittests src/main.rs ...
running 4 tests
test dnd::tests::conflicts_get_copies ... ok
test listing::tests::sorts_folders_first_then_names ... ok
test dnd::tests::extension_edge_cases ... ok
test tests::parses_cli ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

    Running unittests src/bin/look-header/main.rs ...
running 3 tests
test strip::tests::groups_close_and_move ... ok
test tests::parses_cli ... ok
test style::tests::spec_and_fit ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build --release
    Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 0.88s
```

At one point P1c's `src/dnd.rs` was mid-edit and the full suite failed there; per the brief I left it
alone, checked my binary alone (`cargo clippy/test/build --bin look-header` all passed), waited, and
the final suite above passed after P1c's file settled.

### Tests

- `tests::parses_cli`: default 2/1.0/no font; `--tabs 5 --scale=0.75 --font "Fira Sans"`;
  unknown flag; bad/zero tabs; out-of-range scale; `--font` without a value.
- `style::tests::spec_and_fit`: no `#`, `rgb(`, `hsl(`, `red`, `white` or `black` in the CSS; no
  `font-family` without the flag; `font-family: "Fira Sans";` with it; `scaled(24, 0.5) == 12`;
  `header_fits`: wide 2560/2/1.0 → fits with 960 path; 1600/2/1.0 → fits with path in 320..960;
  collapse point 1536 fits / 1535 collapses; 800/2/0.5 → fits with 192 path.
- `strip::tests::groups_close_and_move`: grouping two tabs, label/count, rename, ungroup, close
  focus, close-last, and a reorder.

### 10-second runs (release, SIGTERM by PID, no CSS warnings)

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.101] window: header window shown
[gimbal-look-header 1.567] layout: first pill 270x50 outer, path 960x50 outer, window 2470, overflow false

(look-header:1735487): Gdk-WARNING **: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR)
exit=143

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
[gimbal-look-header 0.115] window: header window shown
[gimbal-look-header 1.576] layout: first pill 64x25 outer, path 480x25 outer, window 1272, overflow true
exit=143

$ grep -nE "Theme parser|Gtk-WARNING|Gtk-CRITICAL" run3.out run30.out
(no output, exit 1)
```

The `Gdk-WARNING … VK_SUBOPTIMAL_KHR` in the first run is intermittent Vulkan/niri swapchain noise,
not a GTK CSS warning and not produced by this code; the required CSS patterns are absent. The
accordion pill measures `64×25` because it is the natural size of the “30 tabs” label, not a 270 px
tab pill. The window was tiled by niri to 2470 / 1272 px in
these runs, so the default 2560×1440 is set but not what the compositor gave us **[observed]**.

## API citations [read]

- `adw::ApplicationWindow` builder and default size: `libadwaita-0.9.2:src/auto/application_window.rs:134,242`;
  `set_content`: `:554`.
- `adw::AlertDialog` responses/extra child/appearance: `libadwaita-0.9.2:src/auto/alert_dialog.rs:441,624,669`;
  `choose`: `libadwaita-0.9.2:src/alert_dialog.rs:32`.
- `gtk::Revealer` child/reveal/duration/transition: `gtk4-0.11.5:src/auto/revealer.rs:102,110,118`.
- `GestureDrag::connect_drag_end`: `gtk4-0.11.5:src/auto/gesture_drag.rs:191`.
- `DropTarget::connect_drop`: `gtk4-0.11.5:src/drop_target.rs:52`; `gdk::FileList` and `files()`:
  `gdk4-0.11.5:src/auto/file_list.rs:10,37`.
- `gio::Subprocess::newv` / `wait_check_async`: `gio-0.22.10:src/auto/subprocess.rs:25,358`.
- `CssProvider::load_from_string`: `gtk4-0.11.5:src/auto/css_provider.rs:101`;
  `style_context_add_provider_for_display`: `gtk4-0.11.5:src/functions.rs:268`.
- `WindowHandleBuilder::child`: `gtk4-0.11.5:src/auto/window_handle.rs:119`.
- `ScrolledWindow::set_min_content_width` / `set_propagate_natural_width`:
  `gtk4-0.11.5:src/auto/scrolled_window.rs:281,333`.
- `CenterBox::new`: `gtk4-0.11.5:src/auto/center_box.rs:42`.
- `EventControllerKey::connect_key_pressed`: `gtk4-0.11.5:src/event_controller_key.rs:11`.
- Modifier state from the gesture's last event: `gtk4-0.11.5:src/auto/gesture.rs:79`,
  `gdk4-0.11.5:src/auto/event.rs:126`.
- `Window::is_maximized/maximize/minimize`: `gtk4-0.11.5:src/auto/window.rs:744,757,764`.
- `Popover::set_parent` (via `WidgetExt`): `gtk4-0.11.5:src/auto/widget.rs:1353`.
- `-gtk-icon-size` and `all: unset`: `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1280` and
  `gtk-4.22.4:gtk/gtkcssshorthandpropertyimpl.c:1239`. Named colours resolve across providers:
  `gtk-4.22.4:gtk/gtkstylecascade.c:142`.

## libadwaita styles undone or restored after `all: unset` [read]

`all: unset` is used on `.gl-icon-button`, `.gl-pill`, `.gl-pill-close`, `.gl-trash`,
`.gl-field entry` and `.gl-menu-item`. The libadwaita rules it removed, and what I put back:

| libadwaita style | Stylesheet line | Restored |
|---|---|---|
| Button `min-height: 24px`, `min-width: 16px`, `padding: 5px 10px`, button radius, `font-weight: bold` | `libadwaita-1.9.1:src/stylesheet/widgets/_buttons.scss:20-24` | Our token sizes: nav 50 high / 32 icon, window 24 icon / 40 button, trash 48 outer, pill close 28 circle, menu item 40 high; label text sizes from `text-*`. No bold weight is re-applied (deliberate). |
| Button focus ring (`outline` 2 px, offset −2) | `_buttons.scss:26`, mixin `_drawing.scss:12-28` | `box-shadow: 0 0 0 line-thin alpha(@accent_color, .5)` on `:focus-visible` for icon buttons, close buttons, trash and menu items. |
| Raised button background and hover / active / checked fills | `_buttons.scss:29-53` | Our `alpha(@window_fg_color, .1)` hover, `.2` active; menu items and icon buttons are otherwise transparent. |
| Disabled button opacity | `_buttons.scss:58-59` | `.gl-menu-item:disabled { opacity: 0.45; }` |
| Entry `min-height: 34px`, 9 px left/right padding, radius, `background-color: $button_color`, `caret-color: currentColor` | `_entries.scss:4-11` | `.gl-field entry`: `padding: 0`, `background-color: transparent`, `color`/`caret-color` from `@window_fg_color`, `font-size` `text-control`. |
| Entry inner focus ring (`:focus-within`) | `_entries.scss:17` | `.gl-field:focus-within { box-shadow: … focus-ring … }` |
| Entry placeholder dimming (`.dimmed`) and text subnode colours | `_entries.scss:19-27`, `.dimmed` at `_labels.scss:13-14` | Placeholder set to `alpha(@window_fg_color, .5)`; selection set to `alpha(@accent_color, .35)` on `@window_bg_color`. |
| Button `image-button` narrower padding (used by icon-only buttons) | `_buttons.scss:66-70` | Our `.gl-icon-button` has explicit horizontal padding `gap-s`, so it is not relying on the removed image-button padding. |

Not undone, because these widgets are not `all: unset`:
- `window` background/CSD shadow/outline (`_window.scss:1-10`): we only override the background with
  `window.gimbal-look`, so the libadwaita window styling still applies around it.
- Popover chrome (`_popovers.scss:7-29`): menus keep the libadwaita popover background, shadow and
  radius; only the buttons inside are ours.
- `label` defaults (`_labels.scss:1-10`): our labels are stock `GtkLabel`s; only font sizes/colours
  are set, no `all: unset`.
- `ScrolledWindow` and `WindowHandle` keep their default styles.

`all: unset` on the pill `CenterBox` also drops inherited `color` and font properties; the pill sets
`color`, and the label sets `font-size`. That is not a libadwaita widget style.

## Not verified without a live session

Claude does these; I did not inject pointer or keyboard events:

- The visual match to `sketch/default.png` and `sketch/search.png` at 2560×1440, and whether colours
  follow Noctalia when the wallpaper changes.
- Search opening/closing smoothly and pushing the pills/path bar.
- Pill focus/close, Ctrl/Shift selection, drag reorder, right-click Split View + fade, rename,
  ungroup and the group-close confirmation.
- Narrowing: path shrink, collapse to “N tabs”, hover list and focus; widening restoring pills.
- Ctrl+= / Ctrl+- live rescale and `GIMBAL_LOOK_BASE=1.25`.
- Trash accepting a Dolphin file (only logs), Float toggling niri floating, and no Minimize under
  niri.
- The exact CSS effect of `all: unset` on the GTK entry's `> text` subnodes in this theme.

## Known deviations and notes

- Reorder is release-time only; no dragged-pill follow or neighbour slide. Lowest-priority item.
- The path for a group shows `/home/kyle/Documents/Group N`; that is fake, like all paths here.
- Group drag-to-add is not implemented (the brief only asks for the right-click menu).
- `#[rustfmt::skip]` is used on the deliberately grouped item bodies to stay under the 700-line
  cap. `rustfmt --check` on the four files passes (exit 0). The rest is normally formatted.
- The header's `ScrolledWindow` is a deliberate layout device: the row can be one frame wider than
  the viewport during a resize, then `update_layout` (80 ms poll) shrinks the path and rebuilds the
  overflow pill. No pixel sizes are set on styled widgets except `path.set_width_request()` from
  `header_fits` (the allowed layout calculation) and the window's required 2560×1440 default.
- Line count after the final edit: **698** (`header.rs` 402, `strip.rs` 107, `main.rs` 93,
  `style.rs` 96). Under 700.
- No files outside `src/bin/look-header/` and this notes file were changed. `Cargo.toml` was not
  touched; no dependencies were added. The only process runs were this binary, stopped by PID with
  SIGTERM; no `niri msg action` was ever run.

## Review (Claude, Nov 1, 2026)

**Verdict: needs a round of fixes (`P1i-r1-fixes.md`), then the live check.** The structure is
good: the token and colour tables, `header_fits`, the fake model and its test, the window-button
choice in one function, and the `all: unset` findings table (the most useful part of these notes).

Checked: only `src/bin/look-header/` and this file changed; `Cargo.toml` unchanged; the four RULES
commands pass from a clean build; no colour literal or `font-family` outside the `--font` path;
the colour table matches `look-style-spec.md`; runs at `--tabs 3` and `--tabs 30 --scale 0.5`
print no GTK CSS warnings (only the Vulkan `VK_SUBOPTIMAL_KHR` notice, which isn't ours); Trash
only logs; Float runs `niri msg action toggle-window-floating` only from its click handler; no
Minimize under niri.

1. **The path bar must stretch into free header space.** Kyle's decision (sketch-values.md,
   round 2, item 7): "the path bar stretches into free header space; as the window narrows it
   shrinks to a minimum first, then the tabs collapse". The code caps it at 960 and fills the rest
   with a spacer. Remove the spacer and the upper cap: the path bar takes all the free space, down
   to its 320 × scale minimum, and only then do the pills collapse. Update `header_fits` and its
   tests to match (the 2560-wide case now returns more than 960). Trash and the window buttons
   stay at the right edge.
2. **Dragging a pill must look like dragging it.** Kyle wants to arrange tabs by dragging
   (round 3, item 2), so release-time reordering isn't enough. While dragging, the pill follows the
   pointer horizontally and the pills it passes slide into their new places (about 150 ms); on
   release it settles into its slot. Still inside the strip, still no Wayland drag and drop.
3. **Keyboard handling swallows keys meant for dialogs.** The window's key controller runs in the
   capture phase and returns `Stop` for Escape, so Escape can't cancel the Rename or Close-group
   dialog (and Ctrl+T in the rename entry adds a tab). Use the bubble phase, or skip the handler
   while an `adw::AlertDialog` is open, and say which.
4. **Replace the 80 ms layout poll** with a notification. P1j uses the `ScrolledWindow`'s
   horizontal adjustment `page-size` notify for the same purpose; the header's scroller has one.
   A timer that runs forever won't survive into Phase 2.
5. **Readable code, new line budget.** Drop `#[rustfmt::skip]` everywhere except the two constant
   tables, one statement per line, and write the stylesheet as one CSS rule per line (a raw string
   like P1j's `style.rs`). Kyle and Claude tune the CSS live and diff it; a single 5,000-character
   line can't be read or diffed. The 700-line budget caused the cramming, and that was Claude's
   mistake: the new budget is **1,200 lines** under `src/bin/look-header/`.

## P1i-r1 changes

Five corrections from the review. Only `src/bin/look-header/` (all four files) and this notes file
changed; `Cargo.toml`, the other binaries and the review section above were not touched.

1. **The path bar stretches.** The spacer and the 960 cap are gone. `header_fits` returns all the free
   width for the path bar, and `fits: false` (with the path at its 320 × scale minimum) only when
   even that minimum will not fit -- which is when the pills collapse. The path is `hexpand` with
   `width_request = path_width`, so Trash and the window buttons stay at the right edge.
   **Tests:** 2560 / 2 pills -> 1344 px (more than 960); 1536 -> fits with exactly 320; 1535 ->
   collapses; every width from 1536 to 2560 fits with the path at 320 or more; at scale 0.5, 800 ->
   192, 768 -> 160, 767 collapses; 1600 at 0.5 -> more than 480. **[read]** the token table and
   `look-style-spec.md`; **[observed]** the `--tabs 3` run reports `path 1050` in a 2560 header
   (three pills, not the two the test uses).

2. **Dragging looks like dragging.** The strip is a `gtk::Fixed`; pills sit on the token pitch
   (`scaled(PILL_W) + scaled(GAP_L)`). Each pill's `GestureDrag` takes the pointer from the event's
   **surface** coordinates (`gdk::Event::position`), not from the gesture's own offset, because the
   dragged pill moves under the pointer and would corrupt widget-relative offsets. The dragged pill
   follows the pointer with `Fixed::move_`; at drag begin it is removed and re-added so `GtkFixed`
   paints it above the pills it passes. A passed pill gets `.slide-left` / `.slide-right`:
   `transform: translateX(-/+pitch)` with a 150 ms transition, so it slides into the hole while the
   drag is still live. On release the model is reordered and the strip rebuilt; the passed pills are
   already at their new slots (no jump) and the dragged pill settles by at most half a pitch.
   **Pure function + test:** `style::drag_layout(from, dx, pitch, count) -> DragLayout { to, offset,
   slides }` -- which slot a pill lands in for a given offset and where each other pill slides.
   Tested for right, left, less than half a pitch, both strip edges, one pill and no pills.
   **[read]** `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1324` (`transform` is an animated GTK CSS
   property), `gtk-4.22.4:gtk/gtkcsstransformvalue.c:1167` (`translateX`), and
   `gtk-4.22.4:gtk/gtkfixed.c:194` (`GtkFixed` clips and paints in child order).

3. **Keyboard.** The window's `EventControllerKey` is in the **bubble** phase, *and* the handler
   returns `Proceed` whenever `dialogs > 0`. `dialogs` is incremented for every `adw::AlertDialog`
   and decremented on that dialog's `closed` signal (**[read]** `libadwaita-0.9.2:src/auto/dialog.rs:596`).
   Both are needed: bubble lets the dialog's entry and its own Escape run first, and the guard covers
   keys the entry does not consume (Ctrl+T was adding a tab behind the Rename dialog). Escape with
   nothing to close now propagates instead of being swallowed.

4. **No layout poll.** The 80 ms `timeout_add_local` is gone. The horizontal adjustment's `page-size`
   notify drives layout; the viewport sets it to the viewport width (**[read]**
   `gtk-4.22.4:gtk/gtkviewport.c:178`), and it is used as `available`. The rebuild is deferred with a
   one-shot idle because the notify runs inside the viewport's allocation.
   `grep -n 'timeout_add_local(' src/bin/look-header/` now finds **nothing**; what remains is
   `timeout_add_local_once` (20/50/200/210 ms) and `idle_add_local_once` (layout, drag end).

5. **Readable code.** rustfmt owns every file, with `#[rustfmt::skip]` only on `mod tok` and
   `mod col` (`grep -n rustfmt::skip` shows exactly those two). The stylesheet is a raw
   one-rule-per-line string (44 rules); a unit test asserts that no line has more than one `{`.
   Rust sets no pixel sizes: `Fixed::put/move_` use the token pitch and the CSS owns the slide
   distance.

### The four RULES commands (P1i-r1 final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.21s

$ cargo test
    Running unittests src/main.rs ...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

    Running unittests src/bin/look-body/main.rs ...
test result: ok. 12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

    Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-…)
running 5 tests
test strip::tests::groups_close_and_move ... ok
test style::tests::drag_arithmetic ... ok
test tests::parses_cli ... ok
test style::tests::header_fits_stretches_then_collapses ... ok
test style::tests::spec_rules_and_css_shape ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.02s
```

For a while P1j's `src/bin/look-body/` was mid-edit: `cargo fmt --check` listed diffs only in its
`grid.rs`, `main.rs`, `preview.rs`, `sidebar.rs`, `split.rs` and `style.rs`, and clippy failed in its
`split.rs`. Per the brief I left those files alone, checked my binary on its own (below), waited, and
the final run above is clean.

My binary's own checks, from `spike/`, while look-body was still moving:

```text
$ cargo build --bin look-header
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ PATH="…/1.95.0…/bin:$PATH" cargo clippy --bin look-header -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ rustfmt --check --edition 2024 src/bin/look-header/*.rs
exit 0
$ grep -n 'timeout_add_local(' src/bin/look-header/*.rs
(no output, exit 1)
$ grep -n 'rustfmt::skip' src/bin/look-header/*.rs
src/bin/look-header/style.rs:4:#[rustfmt::skip]   (mod tok)
src/bin/look-header/style.rs:17:#[rustfmt::skip]  (mod col)
```

### 10-second runs (P1i-r1, release, SIGTERM by PID, no GTK CSS warnings)

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.067] layout: window 2560, path 1050, overflow false
[gimbal-look-header 0.102] window: header window shown
exit=143

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
[gimbal-look-header 0.078] layout: window 2560, path 160, overflow true
[gimbal-look-header 0.113] window: header window shown
[gimbal-look-header 0.119] layout: window 1272, path 160, overflow true
exit=143

$ grep -nE "Theme parser|Gtk-WARNING|Gtk-CRITICAL" final3.out final30.out
(no output, exit 1)
```

The runs were in `/tmp/gimbal-spike-p1ir1`, which I created and removed afterwards. The second
`layout` line in the 30-tab run shows the page-size notify following niri's tiling to 1272 px; the
overflow decision was already true at 2560 for 30 pills. **[observed]**

### API citations for the new code [read]

- `gtk::Fixed` `put`/`move_`/`child_position`: `gtk4-0.11.5:src/auto/fixed.rs:278,305,317`.
- `ScrolledWindow::hadjustment`: `gtk4-0.11.5:src/auto/scrolled_window.rs:64`;
  `Adjustment::page_size`: `gtk4-0.11.5:src/auto/adjustment.rs:177`.
- `EventController::set_propagation_phase`: `gtk4-0.11.5:src/auto/event_controller.rs:136`.
- `gdk::Event::position`: `gdk4-0.11.5:src/auto/event.rs:146`.
- `adw::Dialog::connect_closed`: `libadwaita-0.9.2:src/auto/dialog.rs:596`; `AlertDialog::choose`
  consumes the dialog: `libadwaita-0.9.2:src/alert_dialog.rs:32`.
- CSS `transform` / `translateX`: `gtk-4.22.4:gtk/gtkcssstylepropertyimpl.c:1324`,
  `gtk-4.22.4:gtk/gtkcsstransformvalue.c:1167`; viewport page size:
  `gtk-4.22.4:gtk/gtkviewport.c:178`.

### Line count (not met)

`wc -l` under `src/bin/look-header/`: `header.rs` 964, `style.rs` 274, `strip.rs` 208, `main.rs`
136 -- **1,582**, over the 1,200 budget. I did not cram to hide it. **[inferred]** 1,200 is not
reachable when rustfmt owns the files, every statement is on its own line and all of P1i's
behaviour is kept: the header alone (nav, search, pills, groups, overflow, menus, drag, scale,
window buttons) is 964 lines in that form, and the five fixes add to it rather than replace it.
To get under 1,200 the remaining candidates are P1i features, not formatting: the group fade and
its two dialogs (~60 lines) and the accordion hover list plus the pill context menu (~75). I kept
them because the review says everything P1i asked for still holds. Say which to drop and I will.

### Not verified live (Claude does these)

- The drag: the pill following the pointer, the 150 ms slide of the pills it passes, the release
  settle, and the dragged pill being drawn on top.
- Escape cancelling the Rename and Close-group dialogs, and Ctrl+T doing nothing while one is open.
- Window resize (narrow/widen) driving the path stretch and the collapse through the page-size
  notify; I cannot resize without `niri msg action`.
- The path bar visibly filling the free header space at 2560, with Trash/buttons still at the edge.
- Ctrl+=/Ctrl+- after the reflow changes, and `GIMBAL_LOOK_BASE=1.25`.

## Review of P1i-r1 (Claude, Nov 1, 2026)

**Verdict: one more round (`P1i-r2-fixes.md`), for the pill drag.** Corrections 1, 3, 4 and 5 are
done and right: the path bar stretches with no cap, `header_fits` and `drag_layout` are pure and
tested, the key handler is in the bubble phase with the dialog guard, the poll is gone, and the
CSS is one rule per line. The four RULES commands pass on the combined tree (21 tests).

**The line budget is dropped.** The 1,582 lines are readable rustfmt code doing what P1i asked
for, not padding; thanks for saying so instead of cramming. Keep every feature.

1. **Bug: the pill drag cancels itself as it starts.** `drag_begin` raises the dragged pill with
   `strip.remove(&pill)` then `strip.put(...)`. Removing unparents the pill, unparenting unmaps it
   (`gtkwidget.c:2702`), unmap resets its controllers (`gtk_widget_real_unmap`,
   `gtkwidget.c:7963`), and resetting a gesture cancels it (`gtk_gesture_reset`,
   `gtkgesture.c:748-751`). So the `GestureDrag` fires `cancel` inside its own `drag-begin`,
   `drag_cancel` clears the state, and no `drag-update` ever arrives: the pill never moves. Raise
   it without unparenting: `pill.insert_after(&strip, strip.last_child().as_ref())`. With the same
   parent, `gtk_widget_reposition_after` only relinks the siblings (`gtkwidget.c:6233`) and the
   pill stays mapped.
2. **Minor: the main menu's popover is never unparented when dismissed by clicking outside.**
   `open_pill_menu` calls `close_on_closed`; `open_main_menu` doesn't, so each dismissed main menu
   leaves a popover parented to the menu button. Call `close_on_closed` there too.

## P1i-r2 changes

Two fixes in `src/bin/look-header/header.rs`, plus the debug logging the brief asks for. No other
file changed; the review sections above are untouched and every feature is kept.

1. **The pill drag no longer cancels itself.** In `drag_begin` the raise is now
   `pill.insert_after(&strip, strip.last_child().as_ref())` followed by `strip.move_(&pill, base,
   0.0)`. Nothing in the drag path removes, unparents or re-adds the dragged pill.
   The chain, checked line by line in the pinned GTK source `[read]`:
   - Old code called `strip.remove(&pill)`, whose unparent path calls
     `gtk_widget_unrealize` (`gtk-4.22.4:gtk/gtkwidget.c:2702`).
   - The resulting unmap runs `gtk_widget_real_unmap` (`gtkwidget.c:7942`) and, inside it,
     `gtk_widget_reset_controllers` (`gtkwidget.c:7963`).
   - A controller reset calls `gtk_gesture_reset` (`gtk-4.22.4:gtk/gtkgesture.c:748`), which cancels
     every sequence — including the `GestureDrag` that is currently emitting its own `drag-begin`.
     That is why `cancel` arrived immediately and no `drag-update` followed.
   - `gtk_widget_insert_after` (`gtkwidget.c:12492`) with the same parent takes the documented
     reorder path; it returns immediately if the widget is already the previous sibling
     (`gtkwidget.c:12501-12503`), otherwise `gtk_widget_reposition_after`
     (`gtkwidget.c:6221`, relink block from 6233) only relinks siblings and reorders the CSS node.
     It does not unrealize, unmap or reset controllers, so the pill stays mapped and the gesture
     stays live. The widget is now last in the `GtkFixed` child order, so it paints above the others.
   - **[read]** gtk-rs binding used: `WidgetExt::insert_after(parent, previous_sibling)` and
     `WidgetExt::last_child` — `gtk4-0.11.5:src/auto/widget.rs:922,544`.

   Debug logs added exactly where the brief asks:
   - `drag_update`: `[gimbal-look-header …] drag: update dx <offset>` on every pointer move while
     dragging. **This is the line Claude should expect during a live drag** (many of them, with
     `dx` growing), followed on release by `tabs: moved pill N to M` when the order changed.
   - `cancel` handler: `drag: cancel`. With the fix, this must **not** appear at drag begin. Any
     `drag: cancel` at the start of a drag means the unparent/reset regression is back.

2. **Main menu popover is unparented on close.** `open_main_menu` now calls
   `close_on_closed(&popover)` before `popup()`, exactly as `open_pill_menu` does. The popover is
   unparented on its `closed` signal (outside click included) and `take_down` still unparents on a
   menu-item click.

### The four RULES commands (P1i-r2 final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
warning: corrupt incremental compilation artifact found at `…/target/debug/incremental/gimbal_spike-…/dep-graph.bin`. This file will automatically be ignored and deleted. …
warning: corrupt incremental compilation artifact found at `…/target/debug/incremental/gimbal_spike-…/dep-graph.bin`. This file will automatically be ignored and deleted. …
warning: `gimbal-spike` (bin "gimbal-spike" test) generated 1 warning
warning: `gimbal-spike` (bin "gimbal-spike") generated 1 warning
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
(clippy exit 0)
```

Those two "corrupt incremental compilation artifact" lines are cargo noticing stale files under
`target/debug/incremental` (P1j was building concurrently), not source diagnostics; cargo says it
ignores and deletes them. There is no clippy diagnostic on any target.

```text
$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-…)
running 4 tests … test result: ok. 4 passed; 0 failed; 0 ignored

     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-…)
running 12 tests … test result: ok. 12 passed; 0 failed; 0 ignored

     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-…)
running 5 tests
test strip::tests::groups_close_and_move ... ok
test style::tests::drag_arithmetic ... ok
test tests::parses_cli ... ok
test style::tests::header_fits_stretches_then_collapses ... ok
test style::tests::spec_rules_and_css_shape ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
(exit 0)

$ cargo build --release
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 0.87s
(exit 0)
```

My binary's own checks (same toolchain), while the shared tree was in use:

```text
$ cargo build --bin look-header
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.24s
$ PATH="…/1.95.0…/bin:$PATH" cargo clippy --bin look-header -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s)
$ rustfmt --check --edition 2024 src/bin/look-header/*.rs
(no output, exit 0)
```

### 10-second run at `--tabs 3` (release, SIGTERM by PID)

Run in `/tmp/gimbal-spike-p1ir2`, which I created and removed afterwards. The process was stopped by
its PID with SIGTERM; no pointer/keyboard injection and no `niri msg action` was used.

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.083] layout: window 2560, path 1050, overflow false
[gimbal-look-header 0.829] window: header window shown
exit=143

$ grep -nE "Theme parser|Gtk-WARNING|Gtk-CRITICAL|CSS" run3.out
(no output, exit 1)
```

No GTK CSS warnings. **[observed]**

### Not verified live (Claude does these)

- The drag itself: the pill following the pointer, the 150 ms slide, the release settle and the
  dragged pill painting on top. The fix removes the `cancel`-inside-`drag-begin`, but only a live
  drag can show `drag: update …` lines and the final `tabs: moved pill …`.
- Clicking outside the main menu twice in a row and checking that no popover stays parented (no
  visible symptom; this is a leak fix).

Line count under `src/bin/look-header/`: `header.rs` 971, `style.rs` 274, `strip.rs` 208,
`main.rs` 136 (1,589). Budget dropped, all features kept. **[read]**

## Review of P1i-r2 (Claude, Nov 1, 2026)

**Verdict: accepted on code; the live check follows.** Only `header.rs` and this file changed.
`drag_begin` raises the pill with `insert_after` and moves it; nothing in the drag path removes or
re-adds a widget. The `drag: update` / `drag: cancel` logs give the live check a clear signal, and
the main menu now unparents its popover on close. The four RULES commands pass on the combined
tree (21 tests); the 10-second `--tabs 3` run printed no GTK CSS warnings. 1,589 lines, no budget.
