# P2ad: select tabs in the accordion, and split them from there

## What this change does

- Accordion rows now take the same left-click modifiers as header pills.
  Ctrl toggles a row and Shift extends from the anchor; both keep the list
  open. A plain click focuses the row and closes the list. The decision is
  `accordion_click(modifiers)` and is unit-tested.
- Rows show `active` and `selected` in the row style, using the same tokens as
  `.gl-pill.active` and `.gl-pill.selected`. The administrator row keeps its
  red text and lock. Every row and its focusable button report
  `gtk::accessible::State::Selected`.
- The header keeps one `Accordion` (`pill`, `popover`, `menu`, `label`,
  `scroller` and row handles) in `Header::accordion`. When a refresh leaves
  the header collapsed, `rebuild` calls `fill_accordion` on that existing
  instance instead of clearing the strip and creating another. The popover,
  its scroll adjustment and the row focus survive. `accordion_is_reused`
  encodes the decision and is tested as a pure function of (collapsed before,
  collapsed after). Leaving the collapsed state drops the header's reference
  before the pill leaves the tree; only the pill's `destroy` unparents the
  popover.
- Right-click on a row opens `open_pill_menu` anchored on that row. An
  unselected row is first made the only selection with a plain `pill_click`;
  a selected row keeps the selection. `row_right_click_selects_only` is the
  pure decision. Split View, Close, Close Selected Tabs, Open in New Window,
  group actions and Add to Group all close the inner menu and then the
  accordion before dispatching. Rename and Add to Group close the accordion
  only when their own action runs.
- The list is in a vertical `gtk::ScrolledWindow` with
  `propagate_natural_height`. Its maximum content height is
  `min(scaled(PIN_ROW_H, scale) * 12, window height - the accordion pill's
  bottom)`, computed when the popover opens. The active row is focused and
  scrolled into view on the popover's `map`, and the scroll position is
  restored after an in-place refill.
- Keyboard: the scroller has a capture `EventControllerKey`. Up/Down move
  between row buttons, Enter is a plain click, Space is a Ctrl-click toggle,
  and Escape pops the accordion down. Enter and Space defer to a focused row's
  ✕ so the existing button activation still closes that row. The row buttons
  keep GTK's normal focus/activation handling when this controller returns
  `Proceed`.

No `fm-core` change was needed: `Workspace::pill_click`, `pill_ctrl_click`,
`pill_shift_click` and `group_split` already provide the operations. The plain
row path uses `WindowUi::focus_pill`, whose `Process::focus_pill` body is the
same `Workspace::pill_click` call as `Process::pill_click` with no modifiers;
Ctrl and Shift call `ui.pill_click(index, modifiers)` directly.

## Nested popover verification

[read] GTK 4.22.4 implements `GtkNative` on `GtkPopover`
(`../gimbal-refs/gtk-4.22.4/gtk/gtkpopover.c:229`, `:234`). A popover's surface
is created from its parent widget's native surface
(`gtkpopover.c:1058-1063`), so a popover parented to a row inside the accordion
gets the outer popover's surface as its popup parent. `gdk_surface_new_popup`
accepts any surface as its parent (`gdk/gdksurface.c:933-950`).

[read] Opening the inner popover only maps it and adds its own grab
(`gtkpopover.c:1219-1222`); closing it removes only that grab
(`gtkpopover.c:1230-1233`). `cascade_popdown` is off by default
(`gtkpopover.c:1021`) and only closes parents when it is on
(`gtkpopover.c:2420-2440`). Nothing in the map path closes the outer popover,
so the accordion stays open under the pill menu.

[inferred] The live compositor path still needs the check below: a refusal of a
nested `xdg_popup` would show a `Gtk-CRITICAL` or close the wrong layer even
though the GTK widget path is allowed.

## Tests

`scripts/check.sh` passed: **945 passed, 0 failed, 23 ignored** across the
workspace (fm-agent 23, fm-cli 12, fm-core 458 + 6 + 11, fm-ui 419; doc-tests
0). The new header tests are:

- `an_accordion_is_reused_only_while_the_header_stays_collapsed`
- `ctrl_and_shift_clicks_keep_the_accordion_open_and_a_plain_click_closes_it`
- `right_clicking_an_unselected_row_selects_only_it`
- `the_accordion_cap_is_twelve_rows_or_the_space_below_the_header`

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -D warnings`,
`cargo build --workspace --release` and `cargo deny check` also passed.

## Live checks for Claude and Kyle

Use `/tmp/gimbal-accordion/run.sh` (fresh session, 14 tabs, gdb). No pointer or
keyboard injection was performed here; these need a human. A Gimbal instance
was already running (`pgrep -a gimbal-files`), so no launch was attempted from
this session. `scripts/check.sh` builds release only; build the debug binary
that `run.sh` names before a live pass.

1. The accordion opens on hover and on click, shows all 14 rows, and scrolls
   when the cap is below 14.
2. Ctrl-click three rows: all three show as selected and the menu stays open.
   Shift-click extends the range.
3. Right-click a selected row, then **Split View**: the three tabs become a
   split group and both menus close.
4. Right-click an unselected row: it becomes the only selection and the menu
   opens.
5. Plain-click a row: it focuses and the menu closes. Hover again: it reopens
   with no warning in the log.
6. Ctrl-click the already-focused row, then hover in and out several times: no
   crash and no `Gtk-CRITICAL` in `gdb.log`.
7. Run with 40+ tabs without resetting the session: the menu fits the screen
   and scrolls.
8. Check light and dark: selected rows, the active row and an administrator
   row.

## Outside the Output list

Nothing outside `crates/fm-ui/src/header.rs`,
`crates/fm-ui/src/look/css.rs` and this note was changed. In particular the
new strong `Header::accordion` reference uses weak references in every child
controller so an expanded header can still destroy the pill and let its
`destroy` handler unparent the popover.

## Claude's review (Nov 8, 2026)

**Code.** The return matches the brief. The reuse decision reads
`collapsed_before` from `overflow` before `update_layout` updates it. On the
resize path, which calls `update_layout` first, a stale value is harmless:
reuse also needs a live `Header::accordion`, and that is dropped whenever the
header expands. The pure decisions (`accordion_click`, `accordion_is_reused`,
`row_right_click_selects_only`, `accordion_scroll_cap`) are tested. **[read]**

**Change by Claude (Kyle's request after the live run).** In the accordion,
a close button with keyboard focus showed only a hairline ring, which is easy
to miss on a small icon. It now gets a `{mid}`-px ring in `FOCUS_RING` that
pulses out to twice that width and back (`gl-focus-pulse`,
`motion::ms(900)`, looping while it has `:focus-visible`). Mouse hover is
unchanged. A short launch logged no CSS parser warning. **[observed]**

**Gates.** `scripts/check.sh`: 945 passed, 23 ignored. **[observed]**

### Live checks (Kyle, Nov 8; `/tmp/gimbal-accordion/run.sh`, fresh 14-tab session under gdb)

Kyle reports every check passed:
- Ctrl and Shift selection keeps the menu open.
- Right-click → Split View makes a split group, and both menus close.
- Right-clicking an unselected row selects only it.
- A plain click focuses the row and closes the menu, and hover reopens it.
- Keyboard: Up/Down, Space, Enter and Escape work.

His one note, the faint focus on the close button, is the change above.

**Then two dropdown fixes (Kyle, with 24 tabs so the list scrolls):**
- GTK's overlay scrollbar drew over each row's close button. The scroller now
  uses a classic scrollbar (`set_overlay_scrolling(false)`), and the rows get
  a `{gs}` right gap so the focus pulse isn't clipped.
- With an Automatic policy, GTK then measured the popover before deciding on
  a scrollbar, and took the scrollbar's width out of the rows, which cut the
  close buttons in half. `apply_accordion_cap` now measures the rows' natural
  width on every open and sets it as the minimum content width. It picks
  `Always` when the rows are taller than the cap and `Never` otherwise, so a
  needed scrollbar is counted in the popover's width.

Kyle confirmed the scrollbar and the close buttons look right. Gates: 945
passed, 23 ignored. **[observed]**
