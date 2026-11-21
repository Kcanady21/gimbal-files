# P2aw — the Settings menu fits its window and always closes

2026-11-21. Brief: `gimbal-files-prompts/phase2/P2aw-settings-menu-fits.md`. Start state:
main at 9744c94 (P2av, 0.1.7).

## What I built

1. **The Settings popover is capped at the window** and so are the menus that can
   outgrow a small window, through one shared helper.
2. **The Open/Save row's D-Bus check runs asynchronously**; the menu is built without
   blocking the main thread.
3. **The popup module logs every popover by name** and **closes any Gimbal popover
   still open before showing a new one**, which unwedges the exact stuck state Kyle
   hit (details and source citations below).

## 1. The cap and its numbers

- `popup::menu_height_cap(scale, window_height, anchor_bottom) -> i32` — the pure
  function: the space between the anchor's bottom edge and the window's bottom edge,
  less one `GAP_M`, never negative. A menu that may flip above its anchor passes
  `anchor_bottom = 0`, which limits it by the whole window. Unlike the accordion's
  twelve-row token cap, Settings is limited only by the window (the brief).
- `popup::scrolls(content_height, cap) -> bool` — the scrollbar decision, pure.
- `popup::capped_scroller(menu, cap, window_width) -> ScrolledWindow` — the shared
  helper: natural height and width propagated, `overlay_scrolling(false)` (the
  classic scrollbar, Kyle, Nov 8), the policy decided from a **measurement**, and —
  only when scrolling — `min_content_width` set to the measured rows width clamped
  to the window's width, so the scrollbar's width is counted before the popover
  maps instead of being taken back out of the rows and clipping the switches or the
  Test button. With no scrolling the width is untouched, so a short menu looks
  exactly as before.

**Numbers at Kyle's scale (0.85) for a 700 px window** [computed]:

- `scaled(GAP_M, 0.85)` = round(16 × 0.85) = **14 px**.
- Settings anchors on the ☰ button: `anchor_bottom` is the button's bottom edge in
  window coordinates (`compute_bounds`); with the header at `scaled(HEADER_H, 0.85)`
  ≈ 82 px the cap is 700 − 82 − 14 ≈ **604 px**. The menu's content measures well
  over 1000 px at this scale [inferred from the row count: five headings, seven sort
  rows, four switches with wrapped explanations, status lines, Test and report
  buttons], so the menu now scrolls to Keyboard Shortcuts….
- Whole-window menus (item, empty-space, pill, add-group): cap = 700 − 0 − 14 =
  **686 px**.

**Resize while open**: the cap is computed when the popover is built, which is every
open. Resizing the window while Settings is open leaves the cap as it is until the
menu is reopened — that is the choice I made, for Settings and for every menu that
uses the helper. Say so if you want it live-adjusting.

## Every popover checked

All popover presentation goes through `crate::popup::show` — 18 sites [read]. The
brief's check list plus everything else the inventory turned up:

| Popover | Site | Can outgrow? | Change |
|---|---|---|---|
| Settings menu | `header.rs` `open_settings_menu` | yes (≈1000+ px of rows) | capped at the ☰ anchor; `show_named("settings")` |
| Pill menu | `header.rs` `open_pill_menu` | borderline: up to 7 `gl-menu-item` rows ≈ 7×34+14 ≈ 252 px at 0.85; a floating window can be shorter | capped at the whole window (it may flip above its pill) |
| Add-to-Group menu | `header.rs` `open_add_group_menu` | yes: one row per pill, so as many rows as tabs | capped at the whole window |
| Item menu | `dialogs.rs` `show_menu` (via `app.rs` `right_click_item`) | yes: a dozen or more rows | capped at the whole window; gained the `name` parameter, logged as `"item menu"` |
| Empty-space menu | `dialogs.rs` `show_menu` (via `app.rs` `right_click_background`) | yes, same builder | capped; logged as `"empty-space menu"` |
| Main menu | `header.rs` `open_main_menu` | no: 3 rows ≈ 3×34+14 ≈ 116 px at 0.85, smaller than any window Gimbal can make | no cap; the reason is the fixed three-row bound. Named. |
| Rename-group popover | `header.rs` `open_rename_popover` | no: an entry and two buttons ≈ 150 px | no cap; named |
| Rename-item, New Folder / New File forms | `dialogs.rs` `open_rename`, `open_name_form` | no: same small fixed form | no cap; named |
| Accordion | `header.rs` `open_accordion` | no: already capped by `accordion_scroll_cap`, unchanged per the brief | named only |
| Hotkeys | `hotkeys.rs` | not a popover: an `adw::Dialog` with its own Automatic scroller | no change [read] |
| Conflict / confirm / details dialogs, Copy To… folder chooser | `dialogs.rs` | `adw::AlertDialog` / `gtk::FileDialog`, not popovers; they resize | no change [read] |
| Open With submenu | `openwith.rs:104` | **yes: one row per installed application, unbounded** | **needs the cap — `openwith.rs` is outside this brief's Output, so it is flagged for the next brief** |
| Sidebar context menus ×5 | `sidebar.rs:579, 1116, 1197, 1372, 2349` | small fixed menus (3–6 rows) | no cap; outside Output; get the same `show_named` treatment next brief |
| Trash item menu | `trash.rs:1103` | small fixed menu | outside Output; next brief |
| Split tile menu | `split.rs:2141` | small fixed menu | outside Output; next brief |
| Portal picker's create-folder popover | `portal/picker.rs:1996` | no: an entry and a button | outside Output |

## 2. The async Open/Save check

`app.rs` `file_chooser_health()` was one synchronous function: file reads plus
`bus_lists_gimbal()`, a synchronous `ListActivatableNames` with a 1 s timeout, run on
the main thread while the menu was built. Now:

- `file_chooser_parts()` reads the files (locations, desktops, portal dirs, `.portal`
  files, the config chain) — **these stay synchronous**; they are small directory
  reads and config parses, nothing that waits on another process (the brief allows
  this; said so).
- `file_chooser_health_with(parts, installed)` composes the `Health`; pure, so every
  state is testable with an injected bus answer.
- `bus_list_has_gimbal(&[String])` — the injected answer's meaning; pure.
- `Process::refresh_file_chooser_status_all` paints the disk-only state into every
  open Settings row via the new `Header::refresh_file_chooser_checking`, then fires
  `gio::bus_get` + `DBusConnection::call` (both asynchronous — gio-0.21.5
  `functions.rs:13`, `dbus_connection.rs:84`; the sync `call_sync` is gone from this
  path) and fills the row in with `refresh_file_chooser_status` when the bus
  answers. A bus error or a missing name fills in `false`, which is the same text
  the synchronous path showed in those cases.
- When the `.portal` files are missing, the row never says "Checking…": no bus
  answer can install them, so `NotInstalled` paints at once.
- `Process::file_chooser_health` (the synchronous whole-health method) is gone; its
  only caller was the menu build.
- The menu's first paint: a last Test answer wins until the next switch change
  (`initial_file_chooser_line`), a missing `.portal` file says so at once, and only
  a files-installed Gimbal shows `FILE_CHOOSER_CHECKING` ("Checking…").
- The switch flip path (`set_file_chooser` → `refresh_file_chooser_status_all`)
  now shows "Checking…" for the moment the bus call takes, then the real state.

## 3. Why it wouldn't close, and what was fixed

Kyle's log from Nov 21 (`~/.local/state/gimbal-files/logs/gimbal.log`): a fresh
0.1.7 process at 12:01:39; at 23.8 s the first pair of

```
glib: Gdk warning: Tried to map a grabbing popup with a non-top most parent
```

then the watchdog's cleanup 750 ms later, five times in 19 s [read]. Nothing between
launch and the first warning, so the popover that held the grab had opened without a
trace — matches "once, just after opening Settings (nothing clicked)".

### The mechanism [read, with citations]

- A GTK popover with `autohide` relies on the compositor for dismissal: the docs say
  so (gtk-4.22.4/gtk/gtkpopover.c:44-48), the popover is created as a grabbing popup
  (:1063 `gdk_surface_new_popup(parent, priv->autohide)`), and **`gtkpopover.c`
  contains no button-press dismissal path at all** [read: grep over the file]. The
  compositor's only dismissal signal is `xdg_popup.done`, which GDK turns into a
  plain `gdk_surface_hide` with nothing GTK can hook
  (gdk-4.22.4/gdk/wayland/gdkpopup-wayland.c:473-482).
- wlroots (Mango's library) ends an `xdg_popup` grab on a button press **only when
  the seat has no focused client**: `xdg_pointer_grab_button` calls
  `wlr_seat_pointer_send_button` and ends the grab when it returns 0
  (`~/wlroots/types/xdg_shell/wlr_xdg_popup.c:71-81`); `send_button` returns 0 only
  when `pointer_state.focused_client` is NULL
  (`~/wlroots/types/seat/wlr_seat_pointer.c:261-278`). While the grab is up, enter
  events may focus **any surface of the grabbing client, including its toplevel**
  (wlr_xdg_popup.c:52-60). So **a click anywhere on Gimbal's own window is delivered
  to Gimbal and keeps the grab**: no `popup_done`, no dismissal.
- Mango additionally consumes presses before they reach the seat at all:
  `handle_cursor_button` forwards only `if (!pointer_process_button_press(event))`
  (`~/mango/src/input/pointer.c:475-478`), and that function consumes bar and tab
  clicks, window menus, mouse bindings, overview clicks, and **border/resize grabs**
  (:1734-1900, resize branch :1891-1898). The overflowing popover hangs over the
  neighbouring tile, so many of Kyle's clicks landed on the neighbour's resize
  strip — swallowed by Mango, never forwarded, grab untouched [inferred from the
  geometry].
- Meanwhile GDK's client-side popup stack still lists the mapped Settings popover
  (registered at create, gdkpopup-wayland.c:1025-1026; removed only when its surface
  hides, :293-294). Every later grabbing popup — each ☰ click's main menu — has the
  window as parent while Settings is the top-most entry, so
  `can_map_grabbing_popup` (:906-918) refuses it with the warning at :981. The
  watchdog closed each refused popover after its 750 ms `MAP_DEADLINE` [read,
  popup.rs]. Two warnings per attempt [inferred]: two presentation attempts, each
  refused.

That reproduces all three of Kyle's observations: the popover ran past the window,
the neighbour's border drew over the overflow, and once Settings was up nothing
would close it — clicks on Gimbal's window keep the grab, clicks on the overhang
are consumed by Mango.

### What Gimbal can fix, and did [read + built]

`show` now closes any Gimbal popover still open before presenting the new one.
Hiding the popover's surface destroys its `xdg_popup`, and the compositor ends the
seat grab when the grabbing popup goes (`~/wlroots/types/xdg_shell/wlr_xdg_popup.c`,
`reset_xdg_popup` :443-468). The stack entry clears (gdkpopup-wayland.c:293-294), so
the new popup's parent is again the top-most — the warning can no longer fire from
Gimbal's own stacking, and ☰ always recovers the window. The self-heal skips the
popover being shown and **its anchor chain**: a pill menu on an accordion row is a
child popup of the still-open accordion (`popup.rs` `keepers`), which is the parent
grab the Wayland stack allows against — closing the accordion there would unmap the
anchor under the menu.

### What is left, and is Mango's

Outside-click dismissal itself. On Mango, an `xdg_popup` grab survives: clicks on
the grabbing client's own window (wlroots delivers them and keeps the grab,
wlr_xdg_popup.c:52-60, 71-81), and clicks Mango consumes for its own handling never
reach the seat grab at all (pointer.c:475-478). Compliant compositors dismiss the
grab on any click outside the popup's surface tree. A Mango fix would forward
pointer presses during a popup grab, or end the grab when a press lands on another
client. Written up here with the source lines, per the brief; **not** worked around
in a way that hides it — the self-heal only restores Gimbal's own stacking.

## 4. The new log lines

`popup.rs` connects `map` and `closed` per popover and arms the watchdog per show
(one line per event, no per-frame noise). Expected shape for the stuck session,
had this existed [inferred from the code]:

```
popup: shown settings
popup: mapped settings
popup: open before showing main menu: settings
popup: closed settings (closed to show main menu)
popup: shown main menu
```

— the "open before showing" line names the culprit by itself, as the brief asked.
Close reasons, where GTK lets me tell:

- `(closed to show <name>)` — the self-heal closed it;
- `(closed by Gimbal)` — a popdown through the new `popup::popdown`, which
  `take_down` in `header.rs` and `dialogs.rs` now calls, so every menu-item click
  attributes its close;
- `(the compositor never showed it; closing it to release its grab)` — the watchdog
  (the old watchdog line's words, folded into the closed line);
- `(popdown or compositor dismissal)` — everything else, because GTK does not
  expose `xdg_popup.done` (gdkpopup-wayland.c:473-482), so a direct
  `popover.popdown()` and a compositor dismissal look identical from the widget.
  The accordion's direct `popdown()` calls close this way.

`show` keeps its signature for the popovers whose files this brief may not edit and
logs them as `(unnamed)`; `show_named(name, …)` is the named form the brief asked
for. A claim that nothing consumes is dropped on the next idle, so a reason can
never name a later popover at the same address.

## 5. What I verified and how

- `scripts/check.sh` from `$PROJ` with the 1.95.0 toolchain: **all five gates pass,
  exit 0** [observed]:
  - `cargo fmt --all --check` clean;
  - `cargo clippy --workspace --all-targets -- -D warnings` clean;
  - `cargo test --workspace` **1188 passed, 0 failed** across the suites (fm-core
    23, fm-agent 16, fm-cli 13, fm-ui 561 + 558, plus 6 and 11 in the small bins);
  - `cargo build --workspace --release` finished (21.5 s);
  - `cargo deny check` exit 0 (the pre-existing duplicate-crate advisories only).
- New and changed tests [observed]:
  - `popup::tests`: the cap (tall window, short window, anchor near the bottom with
    the zero clamp, the margin, scale 0.85 and 2.0), the margin never loses its
    pixel, shorter content never scrolls / taller content always does.
  - `app::tests::the_bus_answer_fills_in_the_same_health_text_for_every_state`: for
    each `Health` state — Gimbal, NotInstalled by bus answer, NotInstalled by
    missing files, Backend(GTK), None, PendingRestart enabled and not — the async
    composition (`disk && bus_list_has_gimbal(injected)`) produces the same text the
    synchronous path did.
  - `app::tests::real_settings_menu_in_a_short_window_fits_inside_it` (+ `_inner`):
    the P2at-style real-GTK child process presents a window, then checks that a
    Settings-shaped menu (40 switch rows) in the capped scroller measures within
    the cap, that the cap leaves one `GAP_M` of the window, that tall content
    scrolls and short content does not, and that a short menu gains no height from
    the scroller.
  - `new_folder_menu…` / `new_file_menu…` descend the capped scroller now
    (`menu_rows` helper; the auto-added viewport is the scroller's child,
    gtkscrolledwindow.c:4394).
- Kyle's log was read (the Nov 21 session above); nothing in
  `~/.local/state/gimbal-files/` was written or deleted. Kyle's instance was
  running throughout (pid 7502); nothing was launched against it, and the widget
  tests' applications register `NON_UNIQUE` like the existing suite.

### API citations

- gio-0.21.5 `src/auto/functions.rs:13` (`bus_get`, async), `src/auto/dbus_connection.rs:84`
  (`call`, async; `call_sync` no longer on this path).
- glib-0.21.5 `src/object.rs:2214` (`downgrade()` → `WeakRef`), used for the
  registry.
- gtk-4.22.4 `gtk/gtkscrolledwindow.c:4394` (a non-scrollable child is wrapped in an
  auto viewport, which `gtk_scrolled_window_get_child` then returns).
- gtk-4.22.4 `gtk/gtkpopover.c` and `gdk/wayland/gdkpopup-wayland.c` as cited in
  section 3.

## 6. What needs a live session (Claude)

1. In a short tiled window, open Settings: it stays inside Gimbal's window and
   scrolls to Keyboard Shortcuts…; no neighbour's border over it.
2. Open and close Settings ten times by clicking outside: it always closes; the log
   shows `shown`, `mapped` and `closed` for each; no "non-top most parent" warning.
3. Click ☰ while Settings is open: the log shows
   `open before showing main menu: settings`, `closed settings (closed to show main
   menu)`, `shown main menu` — and nothing gets stuck.
4. The Open/Save row says "Checking…" for a moment at most, then the real status.
5. In a floating window as well (CLAUDE.md).
6. A pill menu on an accordion row opens while the accordion stays open (the
   nested-popup `keepers` rule); no `closed accordion` line appears for it.
7. The item menu in a short window scrolls; the scroller paints no background of
   its own (it carries no CSS class — libadwaita paints none for a bare
   ScrolledWindow; if the popover looks different, a css.rs rule is the next
   brief's fix).
8. The rename popover's entry still takes focus; its close reads
   `(closed by Gimbal)`.

## 7. Things outside my files

- `openwith.rs` — the Open With submenu needs the shared cap (unbounded app list)
  and its `show` call site should become `show_named("open-with")`. One brief.
- `sidebar.rs`, `trash.rs`, `split.rs`, `portal/picker.rs` — their `show` sites
  should become `show_named` (log names only; no caps needed except arguably the
  drive menus). One brief.
- `look/css.rs` — only if the live check shows the capped scroller painting a
  background; nothing is needed as far as the code can tell.
- Mango — section 3's write-up; a fix there belongs to Mango.

## Claude's review (Nov 21)

Read the notes and the diff. The cap (a pure function plus one shared scroller helper, with the
scrollbar decided from a measurement), the async bus check, and the popup logging are as
briefed. The self-heal in `show` is a sound Gimbal-side fix: closing Gimbal's own open popover
before showing another means a second grabbing popup can never stack on a stale one. The
"keepers" rule for the accordion's nested pill menu is right too.

**One claim to confirm live:** the notes conclude that on Mango a click on Gimbal's own window
never dismisses a popover. Before 0.1.7 Kyle closed Settings by clicking the window without
trouble, so something did dismiss it then. The new `popup: closed <name> (<reason>)` lines will
settle it, and check 2 asks for clicks on the grid, the header and another window separately.

**Fixed in review: the Settings Test button (P2av) never worked.** Kyle's 0.1.7 log, 13:27:30:
`InvalidArgs: Type of message, "(ssv)", does not match expected type "(ssa{sv})"`.
- `options.end()` is a `glib::Variant`, and a Variant inside a Rust tuple is boxed as `v`.
- Now `test_open_parameters` builds `(ssa{sv})` with `Variant::tuple_from_iter`. New test:
  `header::tests::the_test_button_sends_the_portal_its_exact_signature`, which checks the type
  string and the token.
- A second bug behind it: the `(o)` reply was read as `(String,)`, which always fails on an
  object path. The handle came back empty, and the code would then re-subscribe on an empty
  path. It now reads `glib::variant::ObjectPath` and only re-subscribes for a non-empty,
  different handle.

**Gates:** `scripts/check.sh` exit 0, 1189 passed, 0 failed, 31 ignored (the agent's 1188 plus
the new test).

**Live run:** `/tmp/gimbal-p2aw/run.sh` (Kyle closes his Gimbal first). It covers the notes'
items 1–8, plus Test now answering "Test: Cancelled".

**Follow-up brief (agent's §7):** the Open With submenu needs the cap (its app list is
unbounded), and the remaining `show` sites need names: `sidebar.rs`, `trash.rs`, `split.rs`,
`portal/picker.rs`.

**Kyle's live run (Nov 21):** everything worked except that the Settings menu still extended past
the window. Kyle is fine with that; he only wants it not drawn under the neighbours' borders.
- Every Settings open has a matching close, and there's no "non-top most parent" warning. The
  notes' claim that a click on Gimbal's own window can't dismiss a popover on Mango was wrong:
  the closes logged `(popdown or compositor dismissal)` from those clicks.
- The border overlap is Mango's stacking. A popup's scene node is created inside its own
  window's tree (`~/mango/src/manage/layer.c:474`) and raised only within that tree (`:471`), so
  later tiles in the same layer, borders included, draw over any part outside the window. That
  goes to Mango: brief `mango-prompts/P1-popups-above-borders.md`. Gimbal needs no further
  change for it.
