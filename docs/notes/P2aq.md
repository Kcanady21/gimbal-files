# P2aq: a new-tab button next to the tabs

Tags: **[read]** for source, **[observed]** for a command and its output,
**[inferred]** for anything that needs a live session.

Started from the working tree as found, which P2ao's notes record as main at or
after `94e95fb`; the brief's rules forbid git commands, including read-only
ones, so I could not confirm the commit myself. **[read]** Status: code, tests
and the five gates are complete. The live checks at the end are for Claude and
Kyle.

## What this is

A plus-in-a-circle button after the tab strip. It sends the same plain
`tab.new` as Ctrl+T; it never touches the workspace directly. It is always
visible, with one tab or many, and follows the accordion pill when the strip
collapses.

## Where the button sits

`Header::new` now builds the row in this order: menu, back, forward, up, search
box, **strip**, **new tab**, path, Trash, then the window buttons
(`crates/fm-ui/src/header.rs:351-357`). **[read]** The button is appended after
`row.append(&strip)`, so it comes after the last pill when the pills show and
after the "N tabs" pill when they collapse. **[read]**

There are two gaps around it, both from the existing stylesheet:

- `.gl-strip { margin-right: 32px }` (scale 1) between the strip and the button;
- `.gl-icon-button.nav { margin-right: 24px }` between the button and the path.

The button is a sibling of the `GtkFixed` strip, not a child of it, and it gets
the nav look: `gl-icon-button nav`, `valign` Center, so its 32 px glyph and
50 px minimum height line up with the pills' 44 px body plus 6 px of border.
No new CSS rule was needed. **[read]**

## The command

`new_tab_invocation()` (`header.rs:105`) is the one place the command is named:

```rust
Invocation::new("tab.new", Vec::new())
```

The click handler in `Header::wire` (`header.rs:1909`) dispatches it as
`Caller::Ui`. That is the same command and caller path as Ctrl+T
(`fm-core/src/config.rs:640`, `crates/fm-ui/src/app.rs:3805`). The registry
decides the folder, so the button stays correct when P2ap changes `tab.new`.
**[read]**

P2ap is **not** in this tree: `docs/notes/` has no `P2ap.md`, and
`Workspace::tab_new(None)` copies the focused tab's folder
(`fm-core/src/workspace/mod.rs:1003-1019`, used by `app.rs:3805-3815`). So
today the button opens a copy of the current folder, exactly like Ctrl+T; when
P2ap lands, the same button opens home with no change here. **[read]**

The dispatch is tested without a display by asserting the pure function's name
and empty argument list,
`the_new_tab_button_names_the_same_command_as_control_t`
(`header.rs:2930`). The widget wiring itself needs a display and is in the live
checks. **[read]**

## The icon

- `crates/fm-ui/icons/gimbal-new-tab-symbolic.svg` (new) is a square
  `0 0 28.08 28.08` view box with a circle (`cx/cy 14.04`, `r 11`) and a plus
  whose arms run 9.54..18.54. Both use `gpa:stroke="foreground"`,
  `stroke-width="3"` and round caps, matching `close.svg`'s style.
- The plus arms and the ring are separated by 3.5 px after the round caps: the
  ring's inner edge is at radius 9.5, the arm tip plus cap reaches 6.0 from the
  centre. **[read]** (the geometry in the file)
- The SVG's comment says it was drawn for P2aq, not extracted from
  `Gimbal-Files.vectorcraft`, so Kyle can replace it when the mockup has one.
- `MockupIcon::NewTab` (`look/icons.rs:41`) embeds it
  (`look/icons.rs:53`), and `header.rs:351` draws it through
  `icons::button`, i.e. a `gtk::Svg` paintable on a `gtk::Image`. Reusing the
  nav class means `-gtk-icon-size: 32px` is inherited by the image, whose icon
  helper reads that property (`gtk-4.22.4:gtk/gtkiconhelper.c:403-411` via
  `gtk/gtkimage.c:1200`; the property is registered inherited at
  `gtk/gtkcssstylepropertyimpl.c:1280-1285`), so the glyph scales to 16 px at
  scale 0.5 rather than staying at its 28.08 px intrinsic size. **[read]**
- Because the stroke is the `foreground` slot and no palette is set, the icon
  follows the text colour in light and dark themes like back, forward and up.
  **[read]**
- The module doc comment now says seven SVGs and explains that six are from the
  mockup and the plus is drawn here. The old
  `all_six_mockup_icons_are_embedded` test is now `all_seven_icons_are_embedded`
  (`look/icons.rs:94`), and `the_new_tab_icon_is_a_foreground_plus_in_a_circle`
  (`look/icons.rs:112`) checks the view box, the three foreground strokes, the
  stroke width and the absence of literal colours.

The button also has the tooltip "New Tab" and
`gtk::accessible::Property::Label("New Tab")` (`header.rs:355-356`). The
property enum and the `update_property` trait method are
`gtk4-0.11.5:src/accessible.rs:105` and `:31`; the same call pattern is already
used by the Settings switches in this file. **[read]**

## The width budget

`header_fits` now subtracts the button. The estimate is one shared helper,
`nav_button_width` (`look/css.rs:391`), next to `window_button_width`:

```
nav_button_width(scale) = scaled(ICON_NAV) + 2 * scaled(GAP_S) + scaled(GAP_L)
```

That is the nav glyph (32), its two 8 px paddings and its 24 px right margin —
exactly the width the CSS gives the button and the gap that follows it. The
four nav buttons and the new-tab button use the same helper, so the stylesheet
and the estimate cannot drift apart. `header_fits` (`look/css.rs:395-413`)
subtracts `4 * nav_button_width` for the existing nav buttons and one more for
the new tab. **[read]**

At scale 1.0:

| Term | Width |
| --- | ---: |
| header insets (2 × 24) | 48 |
| four nav buttons (4 × 72) | 288 |
| search button + its gap | 72 |
| Trash + its gap + two window buttons | 200 |
| strip's right margin | 32 |
| **new-tab button + its 24 px gap** | **72** |
| two pills + their gap | 564 |
| **total before the path** | **1276** |

So with two pills the path is `available - 1276`:

- `header_fits(1600, 2, 1.0)` was 396 px, now **324 px** (exactly 72 less);
- the last width that keeps `PATH_MIN` (320 px) was 1524 px, now **1596 px**;
- 1595 px now collapses to the accordion; 1524 px, which used to fit, now
  collapses.

At scale 0.5 (`nav_button_width` = 36, total fixed = 638):

- `header_fits(800, 2, 0.5)` was 198 px, now **162 px**;
- 798 px still fits with a 160 px path; 797 px collapses (was 762/761).

Those are the only two existing expectation sets that moved; both are listed in
the tests and below. The new test
`the_new_tab_button_costs_its_own_nav_width` (`look/css.rs:777`) pins the
collapse boundary and asserts that at a fixed width the path drops by exactly
`nav_button_width`. `the_small_header_glyphs_and_the_right_edge_agree`
(`look/css.rs:730`) adds the button to its total-chrome sum, and
`header_fits_stretches_then_collapses` (`look/css.rs:829`) carries the new
numbers. **[observed]** (the tests pass).

## Drag

The pills are children of the `GtkFixed`; the button is the next child of the
header box, after the strip and its 32 px right margin. A pill drag is clamped
to `(count - 1) * pitch` (`look/css.rs`, `drag_layout`), so the rightmost pill's
right edge is the strip's own allocated width; the other pills slide with CSS
transforms, which do not change that allocation. The button therefore stays put
and no pill can reach it. **[read]** The brief's live check 4 is the
confirmation; I did not run it.

## Trash

`trash.rs:98` already turns `tab.new` into `PageRoute::LeaveTrash` and then runs
it, so the button gets that behaviour for free; no special case was added.
**[read]** Live check 7 covers it.

## Gates

`PATH="$T:$PATH" scripts/check.sh` from `$PROJ`: exit **0**, "all checks
passed". **[observed]** `T` is the 1.95.0 toolchain bin dir, per CLAUDE.md; all
five tools, including `cargo fmt` and `cargo clippy`, were present.

| Step | Result |
| --- | --- |
| `cargo fmt --all --check` | no output, exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | `Finished`, no warnings |
| `cargo test --workspace` | **1090 passed, 0 failed, 27 ignored** |
| `cargo build --workspace --release` | `Finished` in 2.03 s |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` |

Counts are the summed `test result:` lines. **[observed]** This brief adds
three tests (the header invocation test, the CSS width test and the icon shape
test); the renamed embed test is the same count as before. `cargo deny` prints
its pre-existing duplicate-crate warnings (`syn`, `system-deps`,
`windows-sys`), unchanged by this brief. **[observed]**

No git command was run. `dbus-run-session` was not used. The app was not
launched: `pgrep -a gimbal-files` found Kyle's instance running (PID 1482720),
so everything that needs the real window is listed below instead.
**[observed]**

## Needs a live session

Claude writes the run script. Work in a scratch folder tree under
`/tmp/gimbal-*` that the script creates.

1. **One tab.** The button sits right after the pill, vertically aligned, with
   the same hover as back/forward/up. Click it: a new tab opens and is focused.
   Today it opens a copy of the current folder; once P2ap is in it opens home.
2. **Collapsed strip.** Open enough tabs to collapse into "N tabs". The button
   follows the accordion pill. Click it: the count goes from N to N + 1.
3. **Resize.** Make the window narrower and wider, floating as well as tiled.
   The strip collapses and expands at the new boundary (1596 px at scale 1 with
   two pills), the path field never drops below 320 px, and the button never
   overlaps the pills or the path.
4. **Drag.** Drag a pill to the end of the strip. The button stays put and the
   dragged pill does not slide over it.
5. **Hover and keyboard.** The tooltip says "New Tab"; Tab from the pills
   reaches the button in row order, before the path entry, and Enter/Space
   activates it.
6. **Light and dark.** The ring and plus follow the text colour like back,
   forward and up, including at 1.0 and 1.5 scale.
7. **Trash.** Enter Trash and click the button: Trash closes and the new tab
   opens (today at the folder that was showing before Trash; home once P2ap is
   in).

## Outside these files

- Nothing outside the five Output files was changed. The button needed no new
  stylesheet rule; `look/css.rs` changed only for the width estimate and its
  tests.
- P2ap is not in this tree, so live check 1 will show a copy of the focused
  folder rather than home. That is `tab.new`'s current behaviour, not the
  button's; the button sends the same command as Ctrl+T. **[read]**
- The icon is a stand-in drawn for this brief. When the mockup gains a
  plus-in-a-circle, replacing the SVG is enough; no Rust changes are needed.
## Claude's review (Nov 17)

Read the notes, the diff and the SVG. The button sits after the strip, dispatches the shared
`new_tab_invocation()` as `Caller::Ui`, and reuses the nav look, so no new CSS was needed.
`nav_button_width` keeps the estimate tied to the stylesheet. GtkSvg handles `<circle>`
(`gtk-4.22.4:gtk/gtksvg.c:11648`). No fixes. P2ap landed in the same tree, so the button now
opens home.

Gates on the combined tree (P2ao–P2ar), run twice in a row after all three agents had finished: `scripts/check.sh` exit 0 both times, 1093 passed, 0 failed, 27 ignored. fmt and clippy are clean, and `cargo deny` shows only the old duplicate warnings. Both agents saw one test failure that didn't reproduce. Each happened while another agent was saving files in the same tree, so the two runs collided; nothing is flaky.

Live run: `/tmp/gimbal-p2ar/run.sh`, checks 8–11 and 15.
