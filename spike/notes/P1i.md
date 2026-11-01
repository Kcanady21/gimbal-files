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

### Live check of P1i-r2 (Claude, Nov 1, 2026): needs fixes (`P1i-r3-fixes.md`)

Floating 2560×1406 on DP-2, `--tabs 3`, pointer through the runner's virtual mouse (left button
only, so right-click menus, Ctrl/Shift selection and keys were not tested this time).

What passed: the layout at 2560 matches the sketch's order and proportions; the path bar takes the
free width with Trash and the window buttons at the right edge; no Minimize under niri; no GTK CSS
warnings. Search opens and pushes the pills and path bar right.

1. **The pill drag moves the window instead.** Dragging Tab 1 to the right logged three
   `drag: update` lines (dx 5, 0, 5), then `drag: cancel`, and the whole window moved about 480 px.
   The header is inside a `gtk::WindowHandle`, whose own drag gesture claims the sequence once the
   pointer passes the drag threshold and starts a window move (`gtkwindowhandle.c:424-458`). The
   r2 fix was right (the cancel no longer comes from unparenting), but nothing stops the handle.
   Fix: in the pill's `drag-update`, once the offset passes the drag threshold
   (`WidgetExt::drag_check_threshold`), claim the sequence with
   `gesture.set_state(gtk::EventSequenceState::Claimed)`. A claim denies the sequence to every
   gesture that handles the event later along the pick stack (`gtkwidget.c:11918-11935`), and the
   pill's bubble-phase gesture runs before its ancestor the handle. Don't claim in `drag-begin`:
   that would also deny the pill's own click gesture, and plain clicks would stop focusing pills.
2. **The window can't be narrowed, and closing search makes it grow without limit.**
   - Asking niri for a 1,500 px width left the window at 2,560: the path bar's `width_request`
     is a minimum, and the scroller's `PolicyType::Never` passes its child's minimum up to the
     window, so the header can never shrink and the overflow collapse can't happen in a floating
     window. (Your r1 run at `--tabs 30` seemed to work because niri was tiling it and forcing
     the size.)
   - Closing search runs `update_layout` before the revealer has finished closing, so the row is
     briefly wider than the window; the window grows to fit, the new page size raises the path
     bar's minimum again, and so on: the log climbs 2,560 → 2,996 → … → 7,908 px, GTK prints
     "layout continuously requested, giving up after 4 tries", and the window ends 7,728 px wide.
   - Fix both the same way: stop feeding the measured width back in as a minimum. Give the path
     bar `width_request = PATH_MIN × scale` and `hexpand`, so it takes the free space by itself,
     and use `header_fits` only to decide the collapse. Make sure the header's minimum width can
     go below the window's (`PolicyType::External` on the scroller, which never propagates the
     child's minimum, or no scroller at all), and say which you chose.
3. **The Forward button shows a broken-image square:** `go-forward-symbolic` is not in this
   Adwaita (`/usr/share/icons/Adwaita/symbolic/actions/` has `go-next-symbolic`). Use
   `go-next-symbolic`, and check every icon name the prototype uses against the installed theme.
4. **Minor:** the placeholder title sits at the top of the body; the brief asked for it centred.

## P1i-r3 changes

Four fixes from the live check. Only `spike/src/bin/look-header/` (`header.rs`, `style.rs`) and this
notes file changed; `Cargo.toml`, the other binaries and the review/live-check sections above are
untouched, and every feature is kept.

### 1. The pill claims the sequence after the drag threshold

`make_pill`'s `drag_update` measures the **surface-coordinate** distance from the press
(`gdk::Event::position`, the same source as `pointer_pos`) and, once
`WidgetExt::drag_check_threshold(0, 0, dx, dy)` is true, calls
`GestureExt::set_state(gtk::EventSequenceState::Claimed)` and logs
`drag: claimed the sequence at dx …`. `set_state` returns true only on the state change, so the line
appears once. `drag_begin` still only raises and moves the pill; it does not claim. The surface
distance is required: the pill follows the pointer, so the gesture's widget-relative `dx` stays near
zero and would never cross the threshold. A plain click never crosses the threshold, so the pill's
click gesture still focuses pills.

Why the claim stops the window move **[read]**:

- `gtk_gesture_set_sequence_state` stores `CLAIMED` and calls
  `gtk_widget_propagate_event_sequence_state` (`gtk-4.22.4:gtk/gtkgesture.c:1056-1059`).
- That walk starts at the event widget and climbs the pick stack
  (`gtk-4.22.4:gtk/gtkwidget.c:11903-11945`). On the claiming widget it only touches gestures that
  already claimed; on each ancestor it calls `_gtk_widget_cancel_or_deny_sequence`
  (`gtkwidget.c:11941`, implementation `gtkwidget.c:2382-2410`). That helper denies an ancestor
  gesture whose phase runs before the claiming gesture and **cancels** one that runs after it
  (`gtkwidget.c:2401-2408`). The `GtkWindowHandle`'s drag gesture is bubble-phase — gestures default
  to `GTK_PHASE_BUBBLE` (`gtk-4.22.4:gtk/gtkeventcontroller.c:250`) — so it is cancelled, not merely
  denied. Either way it is stopped.
- The pill is a descendant of the handle and its bubble-phase gesture handles the motion event
  first, so the handle's gesture is cancelled before the event reaches it. The window move starts in
  `drag_gesture_update_cb`, which checks the threshold, claims, and calls
  `gdk_toplevel_begin_move` (`gtk-4.22.4:gtk/gtkwindowhandle.c:424-437,458-462`). That gesture is
  installed on the handle itself (`gtkwindowhandle.c:567-570`). It never reaches that code.
- gtk-rs bindings used: `WidgetExt::drag_check_threshold`
  (`gtk4-0.11.5:src/auto/widget.rs:250`), `GestureExt::set_state`
  (`gtk4-0.11.5:src/auto/gesture.rs:198`), `EventSequenceState::Claimed`
  (`gtk4-0.11.5:src/auto/enums.rs:4386`), `EventControllerExt::widget`
  (`gtk4-0.11.5:src/auto/event_controller.rs:97`).

A live drag should log several `drag: update dx …` lines with `dx` growing, one
`drag: claimed the sequence at dx …`, then on release `tabs: moved pill N to M` if the order
changed. There must be **no** `drag: cancel`; a cancel at the start of a drag means a regression
(the unparent/reset one or a lost claim). The claim and update lines can come in either order for
the same motion event. I could not do this live (no pointer injection), so the claim itself is
**[inferred]** from the GTK source until Claude's drag.

### 2. No measured width is fed back as a minimum

- `update_layout` sets `path.set_width_request(PATH_MIN × scale)` — 320 px at 1.0, 160 px at 0.5 —
  and the path keeps `hexpand`, so it takes the free header width by itself. `header_fits` is used
  only for its `fits` result (the collapse decision). Its `path_width` is still computed because the
  r1 tests assert it, but nothing applies it to a widget.
- I chose **`PolicyType::External`** on the scroller's horizontal policy. With `External`,
  `gtk_scrolled_window_measure` does not take the child-size branch: `need_child_size` is false
  because the policy is not `NEVER` and `propagate_natural_width` is off
  (`gtk-4.22.4:gtk/gtkscrolledwindow.c:1850`); the horizontal minimum falls through to
  `MAX(0, min_content_width)` (`gtkscrolledwindow.c:1931`). The viewport still writes its allocation
  width into the adjustment's `page-size` (`gtk-4.22.4:gtk/gtkviewport.c:178-184`), so the
  `page-size` notify and `available` still drive the layout. Vertical policy stays `Never`, which
  still propagates the header's 96 px height.
- `grep -nE "set_width_request|set_size_request|set_min_content_width" src/bin/look-header/*.rs`:

  ```text
  header.rs:149:    scroller.set_min_content_width(0);
  header.rs:668:        self.path.set_width_request(path_min);
  ```

  Both arguments are constants: zero and `PATH_MIN × scale`. No measured or allocated value becomes
  a widget minimum; the only fixed window size is the required 2560×1440 default.

**Test A: resize from a temporary timer.** `GIMBAL_LOOK_TEMP_TEST=resize`, release binary, started
with `window.set_default_size(2560, 1400)` and later `(1500, 900)` from `timeout_add_local_once`;
each report logs the scroller's page size, `header_fits(page, count, scale)`, the overflow state,
the pill count, and the title/body bounds (the tuple is `(x, y, width, height, centre_y,
centre_x)`). Run in `/tmp/gimbal-spike-p1ir3`, stopped by its own PID with SIGTERM.

`--tabs 3`:

```text
[gimbal-look-header 0.082] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.842] window: header window shown
[gimbal-look-header 0.849] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 2.644] test: set 2560x1400
[gimbal-look-header 2.644] layout: window 2552, path min 320, fits true, overflow false
[gimbal-look-header 3.843] test: before: page 2552, fits true, overflow false, pills 3, window h 1398, title Some((1236.0, 721.0, 80.0, 44.0, 743.0, 1276.0)), body Some((8.0, 96.0, 2536.0, 1294.0, 743.0, 1276.0))
[gimbal-look-header 3.892] test: set 1500x900
[gimbal-look-header 3.893] layout: window 1500, path min 320, fits false, overflow true
[gimbal-look-header 5.093] test: narrow: page 1500, fits false, overflow true, pills 1, window h 900, title Some((710.0, 472.0, 80.0, 44.0, 494.0, 750.0)), body Some((8.0, 96.0, 1484.0, 796.0, 494.0, 750.0))
[gimbal-look-header 6.844] test: narrow: page 1500, fits false, overflow true, pills 1, window h 900, title Some((710.0, 472.0, 80.0, 44.0, 494.0, 750.0)), body Some((8.0, 96.0, 1484.0, 796.0, 494.0, 750.0))
```

The page size follows the window down (2,552 → 1,500); the same resize flips `header_fits` from true
to false and the strip from three pills to one. `--tabs 6` at 1,500:

```text
[gimbal-look-header 0.082] layout: window 2560, path min 320, fits false, overflow true
[gimbal-look-header 0.122] window: header window shown
[gimbal-look-header 0.128] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 1.923] test: set 2560x1400
[gimbal-look-header 1.924] layout: window 2552, path min 320, fits false, overflow true
[gimbal-look-header 3.122] test: before: page 2552, fits false, overflow true, pills 1, window h 1398, title Some((1236.0, 721.0, 80.0, 44.0, 743.0, 1276.0)), body Some((8.0, 96.0, 2536.0, 1294.0, 743.0, 1276.0))
[gimbal-look-header 3.172] test: set 1500x900
[gimbal-look-header 3.173] layout: window 1500, path min 320, fits false, overflow true
[gimbal-look-header 4.373] test: narrow: page 1500, fits false, overflow true, pills 1, window h 900, title Some((710.0, 472.0, 80.0, 44.0, 494.0, 750.0)), body Some((8.0, 96.0, 1484.0, 796.0, 494.0, 750.0))
[gimbal-look-header 6.124] test: narrow: page 1500, fits false, overflow true, pills 1, window h 900, title Some((710.0, 472.0, 80.0, 44.0, 494.0, 750.0)), body Some((8.0, 96.0, 1484.0, 796.0, 494.0, 750.0))
```

Six pills do not fit at 2,552 either, so this run is collapsed at both widths; at 1,500 the log
shows `fits false, overflow true, pills 1`, i.e. the six pills have collapsed to the single
accordion pill. No GTK CSS or layout warning appeared in either run.

**Test B: search open and close from a timer.** Same harness, `GIMBAL_LOOK_TEMP_TEST=search`, at
2,560 wide (the live check's width): set the size, open search, wait, close search, and report the
page size each time.

```text
[gimbal-look-header 0.082] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.121] window: header window shown
[gimbal-look-header 0.127] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 1.922] test: set 2560x1400
[gimbal-look-header 1.923] layout: window 2552, path min 320, fits true, overflow false
[gimbal-look-header 3.121] test: wide: page 2552, fits true, overflow false, pills 3, window h 1398, title Some((1236.0, 721.0, 80.0, 44.0, 743.0, 1276.0)), body Some((8.0, 96.0, 2536.0, 1294.0, 743.0, 1276.0))
[gimbal-look-header 3.171] layout: window 2552, path min 320, fits true, overflow false
[gimbal-look-header 3.171] search: opened

(look-header:36442): Gdk-WARNING **: 07:57:56.432: vkAcquireNextImageKHR(): A swapchain no longer matches the surface properties exactly, but can still be used to present to the surface successfully. (VK_SUBOPTIMAL_KHR) (1000001003)
[gimbal-look-header 4.371] test: open: page 2552, fits true, overflow false, pills 3, window h 1398, title Some((1236.0, 721.0, 80.0, 44.0, 743.0, 1276.0)), body Some((8.0, 96.0, 2536.0, 1294.0, 743.0, 1276.0))
[gimbal-look-header 4.371] layout: window 2552, path min 320, fits true, overflow false
[gimbal-look-header 4.371] search: closed
[gimbal-look-header 6.122] test: closed: page 2552, fits true, overflow false, pills 3, window h 1398, title Some((1236.0, 721.0, 80.0, 44.0, 743.0, 1276.0)), body Some((8.0, 96.0, 2536.0, 1294.0, 743.0, 1276.0))
```

Every `layout:` line is 2,552; the page size stays at the window width through open and close, and
there is no `layout continuously requested` warning. The `Gdk-WARNING … VK_SUBOPTIMAL_KHR` is the
intermittent Vulkan/niri swapchain notice, not a GTK CSS or layout warning and not produced by this
code; it did not appear in the two delivery runs below.

The harness (`GIMBAL_LOOK_TEMP_TEST`, `temp_test`, `temp_report`, the timers and the
`compute_bounds` probe) is removed before delivery:
`grep -rn "TEMP\|GIMBAL_LOOK_TEMP_TEST\|temp_test\|temp_report\|compute_bounds" src/bin/look-header/`
prints nothing.

### 3. Every icon name resolves in the installed Adwaita

`go-forward-symbolic` does not exist in this theme; the Forward button now uses `go-next-symbolic`.
Every icon name the prototype uses, and the file each resolves to under `/usr/share/icons/Adwaita/`
(checked with `find /usr/share/icons/Adwaita -name '<name>.svg'`; none is a symlink, so `readlink -f`
gives the same path) **[observed]**:

| icon name | file under `/usr/share/icons/Adwaita/` |
|---|---|
| `open-menu-symbolic` | `symbolic/actions/open-menu-symbolic.svg` |
| `go-previous-symbolic` | `symbolic/actions/go-previous-symbolic.svg` |
| `go-next-symbolic` | `symbolic/actions/go-next-symbolic.svg` |
| `go-up-symbolic` | `symbolic/actions/go-up-symbolic.svg` |
| `system-search-symbolic` | `symbolic/actions/system-search-symbolic.svg` |
| `view-refresh-symbolic` | `symbolic/actions/view-refresh-symbolic.svg` |
| `user-trash-symbolic` | `symbolic/places/user-trash-symbolic.svg` |
| `window-close-symbolic` | `symbolic/ui/window-close-symbolic.svg` |
| `window-maximize-symbolic` | `symbolic/ui/window-maximize-symbolic.svg` |
| `window-minimize-symbolic` | `symbolic/ui/window-minimize-symbolic.svg` |
| `window-restore-symbolic` | `symbolic/ui/window-restore-symbolic.svg` |

`go-forward-symbolic` returns no file, confirming the broken icon in the live check.

### 4. The body title is centred

The body label now has `vexpand(true)` as well as `halign/valign(Center)`. A vertical `GtkBox` gives
a non-expanding child only its minimum height — the extra `size_given_to_child` is added only when
`gtk_widget_compute_expand` is true (`gtk-4.22.4:gtk/gtkboxlayout.c:1055-1067`) — so `valign:
center` had nothing to centre in before. With `vexpand`, the label gets the extra height, and
`gtk_widget_size_allocate` applies `adjust_for_align` to centre it within that
(`gtk-4.22.4:gtk/gtkwidget.c:3902-3923,4042-4045`).

**[observed]** in the timer above, using `WidgetExt::compute_bounds` in window coordinates (the
deprecated `allocation()` returned stale, margin-inclusive bounds and disagreed): at 2,552×1,398 the
title is `(1236, 721, 80, 44)` with centre `(1276, 743)`, and the body box is `(8, 96, 2536, 1294)`
with centre `(1276, 743)`. At 1,500×900 the title centre is `(750, 494)` and the body centre is
`(750, 494)`. The title's centre equals the body's centre at both sizes.

### The four RULES commands (P1i-r3 final run, from `spike/`)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.17s
(exit 0)

$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-b2015085d72c48fe)
running 4 tests
test dnd::tests::extension_edge_cases ... ok
test dnd::tests::conflicts_get_copies ... ok
test tests::parses_cli ... ok
test listing::tests::sorts_folders_first_then_names ... ok
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-29a8f160103fcf7b)
running 17 tests
test grid::tests::counts_split ... ok
test grid::tests::column_cap ... ok
test split::tests::drag_moves_backward ... ok
test split::tests::drag_respects_the_grab_offset ... ok
test split::tests::drag_moves_forward ... ok
test split::tests::drag_over_gutter_and_outside_moves_nothing ... ok
test split::tests::four_tiles_fill_two_rows ... ok
test split::tests::hit_test ... ok
test split::tests::pill_hit_test ... ok
test split::tests::six_tiles_scroll ... ok
test split::tests::three_tiles_odd_left ... ok
test split::tests::two_tiles_fill_the_height ... ok
test style::tests::drive_classes ... ok
test style::tests::scaling ... ok
test tests::rejects_bad_input ... ok
test tests::parses_cli ... ok
test style::tests::spec_rules ... ok
test result: ok. 17 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s

     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-f6eea3612da1734e)
running 5 tests
test style::tests::drag_arithmetic ... ok
test strip::tests::groups_close_and_move ... ok
test tests::parses_cli ... ok
test style::tests::header_fits_stretches_then_collapses ... ok
test style::tests::spec_rules_and_css_shape ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
(exit 0)

$ cargo build --release
   Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 0.85s
(exit 0)
```

### 10-second runs (P1i-r3, release, SIGTERM by PID, no warnings)

Both used the delivered binary in `/tmp/gimbal-spike-p1ir3`, run for 9 seconds and stopped by their
own PID with SIGTERM. Neither printed a CSS, theme-parser, `Gtk-WARNING`, `Gtk-CRITICAL`,
`Gdk-WARNING` or `layout continuously requested` line.

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.085] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.845] window: header window shown
[gimbal-look-header 0.852] layout: window 1272, path min 320, fits false, overflow true
exit=143

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
[gimbal-look-header 0.083] layout: window 2560, path min 160, fits false, overflow true
[gimbal-look-header 0.122] window: header window shown
[gimbal-look-header 0.128] layout: window 1272, path min 160, fits false, overflow true
exit=143

$ grep -nE "Theme parser|Gtk-WARNING|Gtk-CRITICAL|Gdk-WARNING|continuously requested" deliver3.out deliver30.out
(no output, exit 1)
```

The 1,272 px `layout:` lines are niri tiling the window to half the output; the page size follows
it. **[observed]**

### Not verified live (Claude does these)

- The pill drag: that the claim cancels the `GtkWindowHandle`'s gesture before it starts the window
  move, the pill following the pointer, the 150 ms slide, the release settle, the dragged pill
  painting on top, and the expected `drag:` lines with no `drag: cancel`.
- The body title's vertical centring on screen (measured centred in the allocation above).
- That the path still visibly fills the free space with the constant 320 px minimum (the resize logs
  show the page size, `fits` and the pill count, not the rendered path pixels).

### Line count and process note

`wc -l src/bin/look-header/*.rs`: `header.rs` 994, `style.rs` 274, `strip.rs` 208, `main.rs` 136
(1,612). No line budget in this brief; all features kept.

The timer runs and the two delivery runs used the binary's own PID (the working directory was set
for the command rather than `cd … && binary &`, whose `$!` is the wrapper subshell). `pgrep -x
look-header` was empty after every run and is empty at delivery. `/tmp/gimbal-spike-p1ir3`, which
held only these logs, is removed. No pointer or keyboard injection and no `niri msg action` was
used; the only process run was this binary.

## Review of P1i-r3 (Claude, Nov 1, 2026)

Read the diff (`header.rs`, `style.rs`) against the r3 brief. All four items are done as asked:
the claim happens in `drag-update` once the surface distance passes `drag_check_threshold`;
the path's minimum is the constant `PATH_MIN × scale` and the scroller's horizontal policy is
`External`; the Forward button uses `go-next-symbolic` and the icon table checks out; the title
has `vexpand`. RULES.md's four commands pass here too (32 tests). Nothing to correct in the code
as delivered.

## Live check of P1i-r3 (Claude, Nov 1, 2026): r3 items pass, one crash found (P1i-r4-fixes.md)

Release build, floating window on DP-2 (DP-3 off). New for this check: the virtual pointer now
also has a right button, Ctrl, Shift and Escape (still no typing keys), so menus and modifier
clicks were tested for the first time.

Passes:

- **Pill drag (item 1).** Tab 1 dragged onto Tab 3's slot: the pill follows the pointer, the
  others slide, it settles in slot 2 (`tabs: moved pill 0 to 2`). The window did not move (its
  position is the same before and after) and there is no `drag: cancel`. The log has one
  `drag: claimed the sequence at dx 9.0` per drag; it prints twice at the same timestamp because
  the update handler runs twice for that motion. Cosmetic.
- **Narrowing (item 2).** The window narrows to 1,500 and 1,100 (it couldn't before); the pills
  collapse to "3 tabs" and the path ellipsizes. Widening restores the three pills. Search opened
  and closed (Escape) three times at 2,560: the window stays 2,560 and every `layout:` line reads
  2560.
- **Icons and title (items 3, 4).** The Forward chevron is drawn; the title is centred.
- **Menus.** Right-click on a pill opens its menu (Split View greyed with one tab selected,
  Close, Open in New Window). Ctrl+click on two pills selects both and enables Split View.
  The group pill's menu adds Rename… and Ungroup; Rename opens its dialog (Escape cancels) and
  Ungroup restores the tabs. The main menu opens with its two items.
- No GTK or Gdk warnings in the run.

Needs fixing:

1. **Split View crashes the app.** Choosing Split View with two tabs selected aborts with
   `RefCell already mutably borrowed` at `header.rs:663` (`update_layout`'s
   `self.model.borrow()`). Cause: `split_selected`'s timer runs
   `if let Some(first) = ui.model.borrow_mut().group_selected() { …; ui.refresh(); }`, and the
   `borrow_mut()` temporary in an `if let` scrutinee lives to the end of the block, so it is
   still held when `refresh()` borrows the model. The panic is inside a GLib callback, which
   cannot unwind, so the process aborts. This has been in the code since r1; no earlier live check
   could reach it because Split View needs Ctrl+click. With the borrow taken into a `let` first
   (tested locally, then reverted), grouping works: "Group 1 · 2 tabs" appears and its menu works.
2. **Minor: clicking a pill doesn't end search.** With search open and empty, clicking a pill
   makes that tab active, but the entry keeps focus and search stays open; only Escape closes it.
   The pill doesn't take keyboard focus, so the entry's focus-leave never fires.

## P1i-r4 changes

Three fixes from the r3 live check. Only `spike/src/bin/look-header/header.rs` and this notes file
changed; `Cargo.toml`, the other binaries, and the review/live-check sections above are untouched,
and every feature is kept. No line budget applies.

### 1. Split View no longer crashes

`split_selected`'s fade timer now takes the grouping result into a `let` first:

```rust
let first = ui.model.borrow_mut().group_selected();
if let Some(first) = first {
    ui.animate_group.set(Some(first));
    ui.refresh();
}
```

The temporary `RefMut` now ends at the `let` statement, before `refresh()` borrows the model in
`update_layout`. The old form kept the scrutinee's temporary alive through the whole `if let`
block, which is exactly the `RefCell already mutably borrowed` abort at `update_layout`'s
`self.model.borrow()`.

**Test (temporary timer, no input).** `GIMBAL_LOOK_TEMP_TEST=split` started a one-second timer that
put tabs 0 and 1 in the selection, called `split_selected()`, and reported again 500 ms later
(`FADE_MS + 300`). Release binary, `/tmp/gimbal-spike-p1ir4`, stopped by its own PID with SIGTERM:

```text
$ GIMBAL_SPIKE_LOG=1 GIMBAL_LOOK_TEMP_TEST=split ./target/release/look-header --tabs 3
[gimbal-look-header 0.081] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.848] window: header window shown
[gimbal-look-header 0.855] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 1.081] test: split: selected tabs 0 and 1 of 3 entries
(look-header:91530): Gdk-WARNING **: … vkAcquireNextImageKHR(): … (VK_SUBOPTIMAL_KHR) (1000001003)
[gimbal-look-header 1.281] layout: window 1272, path min 320, fits false, overflow true
[gimbal-look-header 1.581] test: split: after fade 2 entries ["Group 1 · 2 tabs", "Tab 3"], first is group true
exit=143
```

The process reached the end of the fade and logged the group; it did not abort. The `Gdk-WARNING`
is the intermittent Vulkan/niri swapchain notice seen in earlier rounds, not a GTK CSS or layout
warning. The whole harness (`GIMBAL_LOOK_TEMP_TEST`, the timer, all `test:` logs) is removed before
delivery; `grep -rn "TEMP\|GIMBAL_LOOK_TEMP_TEST\|temp_test" src/bin/look-header/` prints nothing.

### 2. Every `RefCell` borrow, audited

The file has two `RefCell`s: `model: RefCell<Model>` and `pills: RefCell<Vec<gtk::Widget>>`.
`drag`, `available`, `overflow`, `search_open`, `animate_group`, `dialogs` and `scale` are `Cell`s
whose reads are `Copy` and hold no borrow. Line numbers are the delivered `header.rs`.

Fixed sites — a borrow was alive while code could borrow the same cell again:

| Site | What was wrong | Fix |
|---|---|---|
| `split_selected`, timer, 844 | `if let Some(first) = ui.model.borrow_mut().group_selected()` held the `borrow_mut` across `ui.refresh()` (item 1's abort). | Take the result into a `let`; the `RefMut` ends at that statement. |
| `split_selected`, 833 | `for … in self.pills.borrow().iter()` held the `pills` borrow across `pill.add_css_class` for every pill. | Clone the `Vec<gtk::Widget>` first; the borrow ends at the statement. |
| `drag_begin`, 918 | `if let Some(pill) = self.pills.borrow().get(index).cloned()` held the borrow across `insert_after` and `strip.move_`. | Look the clone up into a `let` first. |
| `drag_move`, 934 | `if let Some(pill) = self.pills.borrow().get(drag.index)` held the borrow across `strip.move_`. | Same: clone into a `let` first. |
| `drag_move`, 939 | The `if … && let Some(pill) = self.pills.borrow().get(index)` let-chain held the borrow across `set_slide`'s CSS-class calls. | Clone the pill vector once, then index it. |

Checked and safe as delivered:

| Site | Borrow | Why it cannot double-borrow |
|---|---|---|
| `open_pill_menu`, 393 | `model.borrow()` in a block | The block ends (borrow dropped) before the popover is built, `set_child` and `popup`. |
| `open_pill_menu`, 417 | `borrow_mut().ungroup(index);` | A statement temporary drops at the `;`; `refresh()` is the next statement. |
| `make_accordion`, 559 | `model.borrow()` | Statement temporary; it collects owned `String`s and ends before the popover and menu items. |
| `make_accordion`, 562 | `borrow_mut().focus(…)` | Statement temporary; `rebuild()` is the next statement. |
| `path_text`, 613 | `let model = borrow()` | Held across `entry.label()` and `format!` only; neither touches the model. |
| `rebuild`, 624 | `pills.borrow_mut().clear()` | Statement temporary. |
| `rebuild`, 626 | `model.borrow()` in a block | Ends before `make_accordion`/`make_pill` run; this is the borrow the r4 crash collided with. |
| `rebuild`, 645 | `pills.borrow_mut().push(pill)` | Statement temporary; the `strip.put` before it is on a different statement. |
| `rebuild`, 648 | `model.borrow()` in a block | Ends before `body.set_label`/`path_label.set_label`. |
| `update_layout`, 679 | `model.borrow().entries.len()` | Statement temporary; ends before `header_fits` and `set_width_request`. |
| `add_tab`, 701 | `let mut model = borrow_mut()` in a block | Ends before `log!` and `refresh`; `label()` inside does not re-borrow. |
| `pill_clicked`, 713 | `let mut model = borrow_mut()` | Explicit `drop(model)` before `rebuild()`. The new `dismiss_empty_search()` runs before the borrow. |
| `close_menu_click`, 724 | `let model = borrow()` | Explicit `drop(model)` before `close_indices`. |
| `close_indices`, 735 | `if self.model.borrow_mut().close(indices)` | An `if` condition is its own temporary scope, so the borrow ends before either branch (`window.close` / `refresh`). No fix needed. |
| `close_or_confirm`, 747 | `let … = group_of(&self.model.borrow(), …) else` | A `let` initializer's temporary ends at the end of the `let` statement; the `else` diverges and `ask` is after it. No fix needed. |
| `rename_dialog`, 761 | same let-else form | Same; the borrow ends before `ask`, and the response closure borrows later. |
| rename response, 774 | `borrow_mut().rename(…)` | Statement temporary; `refresh()` is the next statement. |
| `split_selected`, 820 | `model.borrow()` in a block | Ends before the `pills` borrow at 833 and before the timer. |
| `drag_move`, 930 | `pills.borrow().len()` | Statement temporary. |
| `drag_end`, 961 | `borrow_mut().move_entry(…)` | Statement temporary; `log!` and `rebuild` are later statements. |
| `key`, 984 | `if !self.model.borrow().selection.is_empty()` | `if` condition temporary scope; ends before the block. |
| `key`, 985 | `borrow_mut().selection.clear()` | Statement temporary; `rebuild()` is the next statement. |

Every other `if let` scrutinee in the file upgrades a `Weak` (`weak.upgrade()`) or matches a
`gdk` value; none holds a `RefCell` borrow. The three temporary-scope rules behind the "safe"
entries were checked with a scratch program under the same toolchain and edition **([observed]**,
`rustc 1.95.0`, `--edition 2024`):

```text
$ rustc --edition 2024 borrow_scope.rs && ./borrow_scope
thread 'main' panicked at borrow_scope.rs:26:16: RefCell already borrowed
if condition then block ok, value 3; let-else then mutation ok;
if-let double borrow panics: true; x 2
```

That is: an `if` condition releases before its block, a `let … else` releases at the end of the
`let` statement, and an `if let` scrutinee stays borrowed through its then-block.

### 3. Clicking a pill, the path bar or the body ends an empty search

I close it from the click handlers rather than making those widgets focusable. A new method applies
the same rule as the entry's focus-leave handler:

```rust
fn dismiss_empty_search(self: &Rc<Self>) {
    if self.search_open.get() && self.search_entry.text().is_empty() {
        self.close_search();
    }
}
```

- A pill calls it first thing in `pill_clicked`, before the model borrow and `rebuild`.
- The path `Box` and the `gl-frame` body `Box` (kept in the new `Ui::frame` field) each get a
  button-1 `GestureClick`; its `released` calls `dismiss_empty_search`. A click on their
  non-focusable children (the path label, the title label) bubbles to the box, so it is covered
  too.
- Clicks on the search field are not on those widgets, so an open, empty search stays open there.
  Clicks on the focusable nav/trash/window buttons and the menu already closed it through
  focus-leave.

Focus was the other option in the brief, but a pill is a `GtkCenterBox` with `all: unset`, and
making pills, the path bar and the body focusable would change the keyboard focus order, add a
focus ring path to widgets that are not controls, and still leave the body a label. The click
handlers leave focus handling exactly as it was. **[inferred]** The gesture delivery itself needs
Claude's live check; the rule was exercised without input in the temporary `search` harness
(`open` → `dismiss_empty_search` → `open flag false`; `open` → text `"x"` →
`dismiss_empty_search` → `open flag true`). **[observed]**

### The four RULES commands (P1i-r4 final run, from `spike/`, harness removed)

```text
$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo fmt --check
(no output, exit 0)

$ PATH="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin:$PATH" cargo clippy --all-targets -- -D warnings
    Checking gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.22s
(clippy exit 0)

$ cargo test
     Running unittests src/main.rs (target/debug/deps/gimbal_spike-b2015085d72c48fe)
running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-body/main.rs (target/debug/deps/look_body-29a8f160103fcf7b)
running 23 tests
test result: ok. 23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/bin/look-header/main.rs (target/debug/deps/look_header-f6eea3612da1734e)
running 5 tests
test style::tests::drag_arithmetic ... ok
test strip::tests::groups_close_and_move ... ok
test tests::parses_cli ... ok
test style::tests::header_fits_stretches_then_collapses ... ok
test style::tests::spec_rules_and_css_shape ... ok
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
(exit 0)

$ cargo build --release
    Compiling gimbal-spike v0.0.0 (/home/kyle/Documents/03.) Code-Projects/gimbal-files/spike)
    Finished `release` profile [optimized] target(s) in 0.92s
(exit 0)
```

P1j's `look-body` happened to be settled during this run (23 tests), so the full suite and the
release build covered the whole tree. `look-header`'s own `cargo build --bin look-header` and
`cargo clippy --bin look-header -- -D warnings` also pass.

### 10-second delivery runs (release, SIGTERM by PID, no warnings)

Both ran the delivered binary (harness removed) in `/tmp/gimbal-spike-p1ir4`, for 9 seconds each,
and were stopped by their own PID with SIGTERM. No pointer/keyboard injection and no
`niri msg action` was used.

```text
$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 3
[gimbal-look-header 0.081] layout: window 2560, path min 320, fits true, overflow false
[gimbal-look-header 0.850] window: header window shown
[gimbal-look-header 0.857] layout: window 1272, path min 320, fits false, overflow true
exit=143

$ GIMBAL_SPIKE_LOG=1 ./target/release/look-header --tabs 30 --scale 0.5
[gimbal-look-header 0.082] layout: window 2560, path min 160, fits false, overflow true
[gimbal-look-header 0.116] window: header window shown
[gimbal-look-header 0.122] layout: window 1272, path min 160, fits false, overflow true
exit=143

$ grep -nEi "Theme parser|Gtk-WARNING|Gtk-CRITICAL|Gdk-WARNING|continuously requested|CSS" deliver3.out deliver30.out
(no output, exit 1)
```

The 1,272 px `layout:` lines are niri tiling the window to half the output; the page size follows
it. **[observed]**

### API citations for the new code [read]

- `WidgetExt::add_controller`: `gtk4-0.11.5:src/auto/widget.rs:113`.
- `GestureClick::connect_released`: `gtk4-0.11.5:src/auto/gesture_click.rs:66`.
- `GestureSingleExt::set_button`: `gtk4-0.11.5:src/auto/gesture_single.rs:73`.
- `Revealer::reveals_child`, used only by the removed temporary harness:
  `gtk4-0.11.5:src/auto/revealer.rs:71`.

### Line count and process note

`wc -l src/bin/look-header/*.rs`: `header.rs` 1032, `style.rs` 274, `strip.rs` 208, `main.rs` 136
(1,650). No line budget in this brief; all features kept.

The timer test and the two delivery runs used the binary's own PID (the working directory was set
for the command rather than `cd … && binary &`, whose `$!` is the wrapper subshell).
`pgrep -x look-header` was empty after every run and is empty at delivery.
`/tmp/gimbal-spike-p1ir4` (logs only) and the `/tmp/opencode` scratch program are removed. No files
outside `src/bin/look-header/` and this notes file were changed; I have nothing to ask for outside
my files.

## Review of P1i-r4 (Claude, Nov 1, 2026)

All three items done as asked. The Split View fix takes the result into a `let`; the borrow audit
covers both `RefCell`s, fixes four more sites that held a borrow across GTK calls, and backs the
"safe" verdicts with a scratch program showing Rust 2024's temporary scopes. Search now closes
from the pill, path and body click handlers through one `dismiss_empty_search` rule. RULES.md's
four commands pass here (36 tests).

## Live check of P1i-r4 (Claude, Nov 1, 2026): passes; P1i is done

Release build, floating 2,560 × 1,406 window, DP-3 off.

- **Split View:** Ctrl+click on two pills, right-click, Split View: "Group 1 · 2 tabs" appears
  and the app keeps running (no panic).
- **Search:** with search open and empty, a click on a pill (Tab 4 becomes active), on the path
  bar, or on the body each closes search (`search: closed`).
- **Pill drag (regression check):** still claims past the threshold, reorders
  (`tabs: moved pill 0 to 2`), and the window stays put.
- No GTK warnings, no panics.

Together with the r3 check (drag, narrowing, icons, title, menus, groups), every P1i requirement
has now been seen working live. P1i is ready for the P1h acceptance pass.
