# P2ao: the main window can tell when you're typing

Tags: **[read]** for source, **[observed]** for a command and its output,
**[inferred]** for anything that needs a live session.

Follow-up to P2an's review, item 1 and "Not fixed here". Started from the
working tree as found, which P2an's notes record as main at or after
`94e95fb`; the brief's rules forbid git commands, including read-only ones, so
I could not confirm the commit myself. **[read]** Status: code, tests and the
five gates are complete. The live checks at the end are for Claude and Kyle.

## What this is

A `gtk::Entry` is never its own focus widget: `gtk_entry_grab_focus` hands
focus to the `GtkText` inside it (`gtk/gtkentry.c:426-431`), and
`gtk_window_root_set_focus` calls `gtk_widget_set_has_focus` for the focus
widget alone (`gtk/gtkwindow.c:2251-2257`). So `entry.has_focus()` is always
false. The focus crossing sets `GTK_STATE_FLAG_FOCUS_WITHIN` on the focus widget
and on every ancestor of it (`gtk/gtkwindow.c:5280-5330`), which is the flag
that actually answers "is the caret in here".

P2an's review fixed that in the file chooser. This brief is the same fix in the
main window, plus the sweep it asked for.

## The fix

One shared helper, next to the main window's other entry key rules:

- `crates/fm-ui/src/fileops/keys.rs:21` — `pub fn entry_focused(entry:
  &gtk::Entry) -> bool`, reading `StateFlags::FOCUS_WITHIN`. The doc comment and
  both citations moved here from `portal/picker.rs`, with one added paragraph:
  `FOCUS_WITHIN` is set as part of the focus crossing, before the `is_active`
  gate on `has_focus()`, so it reports the window's *logical* focus whether or
  not the window is the active toplevel. That is what both callers want: a
  global hotkey must stay out of the entry even while another window has the
  compositor's focus. `keys.rs` now imports `gtk::prelude::*` for
  `state_flags()` (it previously imported only `gtk::gdk`).

Used at the three `header.rs` sites:

- `crates/fm-ui/src/header.rs:536` — `text_focused()`, both entries.
- `crates/fm-ui/src/header.rs:540` — `search_entry_focused()`.
- `crates/fm-ui/src/header.rs:716` — `update_text()`: the path bar is left
  alone while the caret is in it.
- `crates/fm-ui/src/header.rs:22` — `use crate::fileops::keys::entry_focused;`.

The file chooser now uses the shared helper and its private copy is gone:

- `crates/fm-ui/src/portal/picker.rs:50` — the import.
- `crates/fm-ui/src/portal/picker.rs:802` and `:873` — the two call sites,
  unchanged.
- The private `fn entry_focused` (was `picker.rs:1484-1490`) is deleted. Its
  body moved verbatim into `keys.rs`, so **the picker's behaviour cannot have
  changed**: same flag, same test, same call sites. **[read]** The portal
  shortcut and picker tests pass, including
  `portal::picker::tests::every_shortcut_is_an_allowlisted_command_and_no_file_key_is_bound`
  and `portal::tests::current_filter_selects_the_entry_whose_whole_value_matches`.
  **[observed]**

No other file changed. No key's own behaviour was edited: `entry_owns_key`,
`search_entry_owns_key`, `search_results_own_key` and `text_view_owns_key` are
untouched, as are the bindings in `commands.rs`.

## The sweep

Every `has_focus()`, `is_focus()` and `has_visible_focus()` call in `fm-ui`.
Before this change, `grep -rn 'has_focus()\|is_focus()\|has_visible_focus()'
crates/fm-ui/src/` returned six calls and one doc comment. All six are in the
table. **[observed]** No `is_focus()` or `has_visible_focus()` call exists at
all.

The module also asks no other compound widget about focus.
`grep -rn 'state_flags()' crates/fm-ui/src/` now returns exactly one line —
`keys.rs:22`, the shared helper — so every `FOCUS_WITHIN` read in the crate goes
through it, and `grep -rn 'connect_focus_\|focus_widget' crates/fm-ui/src/`
returns nothing. The remaining focus code is `grab_focus()` calls, which set
focus rather than test it, and three `gtk::EventControllerFocus` handlers that
work off focus *events* and are unaffected by this bug
(`folder/view.rs:684`, `header.rs:1912`, `header.rs:1935`). **[observed]**
`fm-ui` contains no `SearchEntry`, `SpinButton` or `ComboBox` at all (a third
grep returns nothing), and its two `gtk::DropDown`s — the file chooser's filter
dropdown (`picker.rs:1419-1429`) and its choice dropdowns (`picker.rs:1471`) —
are never asked whether they hold focus, so there is nothing there to correct.
**[observed]**

| Site (line as the brief gave it) | Widget | Verdict | Reason |
| --- | --- | --- | --- |
| `header.rs:535` `text_focused()` | `gtk::Entry` ×2 | **wrong, fixed** | focus is on the inner `GtkText` |
| `header.rs:539` `search_entry_focused()` | `gtk::Entry` | **wrong, fixed** | same |
| `header.rs:715` `update_text()` | `gtk::Entry` | **wrong, fixed** | same; this is bug 2 |
| `header.rs:1175` `accordion_key()` | `row.choose`, a `gtk::Button` (`header.rs:119`) | correct | see below |
| `header.rs:1296` `fill_accordion()` | same button | correct | see below |
| `preview.rs:1067` `PreviewPanel::text_focused()` | `text_view`, a `gtk::TextView` (`preview.rs:713`) | correct | see below |

The first three rows give the pre-change lines the brief names; they are now
`header.rs:536`, `:540` and `:716`. The other three are current line numbers,
shifted by the import and the comment this brief added.

Claude's reading of the last three is right, and the new widget regression
observes it rather than only arguing it (see *Tests*).

**A `gtk::Button` is its own focus widget.** `gtk_button_class_init` assigns no
`grab_focus` override — the vfunc list is `unrealize`, `state_flags_changed`,
`compute_expand`, `get_request_mode` (`gtk/gtkbutton.c:215-218`) — and
`gtk_button_init` makes the button focusable itself
(`gtk/gtkbutton.c:465`). `gtk_widget_grab_focus` therefore runs
`gtk_widget_real_grab_focus` on the button and the window's focus widget becomes
the button. **[read]** So `has_focus()` on the accordion's choose button is the
right question, and the only ancestor that would carry `FOCUS_WITHIN` instead is
the row or the window — neither of which is asked.

**A `gtk::TextView` is its own focus widget too, and this one is worth stating
loudly because `GtkEntry` is not.** `gtk_text_view_init` calls
`gtk_widget_set_focusable (widget, TRUE)` (`gtk/gtktextview.c:2110`);
`grep -n 'grab_focus' gtk/gtktextview.c` finds no vfunc assignment (its only
hit, line 5842, is a `gtk_widget_grab_focus` *call* in the press gesture that
focuses the view on a click); and `GtkTextView` is a direct child of
`GtkWidget` (`G_DEFINE_TYPE_WITH_CODE (GtkTextView, gtk_text_view,
GTK_TYPE_WIDGET, …)`, `gtk/gtktextview.c:737`) that draws its own text — it
creates no `GtkText` child. Its only parented children are the magnifier
popover, the popup menu, the selection bubble and child anchors
(`gtk/gtktextview.c:2250`, `:9453`, `:9672`, `:10028`), none of which is a
`GtkText` or takes focus in the ordinary editing path.
**[observed]** (the greps) So `preview.rs:1067` is correct as written, and
`commands.rs:85`'s `text_view_owns_key` guard has been working all along.

## What `commands.rs:82-100` now does differently

Read as written, with `text_entry_focused()` (`frame.rs:407-409`) =
`editing.get() || header.text_focused()`. The `editing` flag, which the rename
and new-folder popovers set, always worked; only the header's two entries were
blind. So every change below is "what already applied to a rename box now
applies to the path bar and the search field".

The main window's hotkeys are a `ShortcutScope::Global` `ShortcutController`
(`commands.rs:50`). A global controller registers its shortcuts with the *root*
widget's shortcut manager (`gtk_shortcut_controller_root`,
`gtk/gtkshortcutcontroller.c:649-690`), in the model for its propagation phase.
`GtkShortcutController` sets no phase of its own, so the default applies,
Bubble (`gtk/gtkeventcontroller.c:250`), and the manager's bubble model has one
controller attached to the window itself
(`gtk_shortcut_manager_create_controllers`, `gtk/gtkshortcutmanager.c:45-61`,
added to it by `gtk_shortcut_manager_add_controller`, `:88-99`). **[read]** That
is why most of this bug was invisible: `gtkmain.c` runs capture on the way down
the widget path (`_gtk_widget_captured_event`, `gtk/gtkwidget.c:4971-4989`,
called from `gtk/gtkmain.c:1999`) and then target and bubble at each level going
up (`gtk_widget_event`, `gtk/gtkwidget.c:4992-5018`, called from
`gtk/gtkmain.c:2031` in a loop that stops at the first widget that handles the
event, `gtk/gtkmain.c:2012-2040`), so the global model is consulted at the
window — the last stop, reached only with a key that nothing below it, including
the inner `GtkText` and its own bindings, has consumed.

**`commands.rs:82` — `text_entry_focused() && entry_owns_key(...)` → skip the
command.** Newly skipped while the caret is in either header entry:

- `<Alt>Up` → `nav.up`. **This is bug 1, fixed.** `GtkText` binds no Up at all
  on Linux — `GDK_KEY_Up` appears once in `gtk/gtktext.c`, at line 1489, under
  `GDK_META_MASK` inside `#ifdef __APPLE__` — so the key used to fall straight
  through to the hotkeys. Alt+Up no longer goes up a folder mid-path, and in the
  search field it no longer closes the results. **[read]**
- `<Alt>Left` / `<Alt>Right` → `nav.back` / `nav.forward`. Now left to the entry
  too, by the same clause of `entry_owns_key` (`keys.rs:41-50`, "with or without
  Alt"). `GtkText` binds bare and Ctrl+Left/Right (`gtk/gtktext.c:1419-1433`)
  and Alt+Left/Right only under `#ifdef __APPLE__` (`gtk/gtktext.c:1465-1474`),
  so on Linux **these three keys now do nothing at all while you are typing**,
  rather than navigating. **[read]** This is the one change beyond the three
  bugs, and it follows from the rule as written; it is live check A below
  because it is the one a reviewer may want to reconsider.
- `Delete` → `file.trash`, and `Ctrl+A/C/X/V/Z` → `selection.all`,
  `file.copy-to-clipboard`, `file.cut`, `file.paste`, `file.undo`. No visible
  change: `GtkText` already binds Delete (`gtk/gtktext.c:1551`), Ctrl+A
  (`:1510`), Ctrl+X/C/V (`:1631-1640`) and Ctrl+Z (`:1706`), and consumes them
  before the global controller runs. What differs is that the command is now
  refused by the guard instead of never being reached — so a future binding
  change in `GtkText` cannot start trashing files mid-path. Live check 3 keeps
  an eye on it.
- bare `Escape` → nothing to change with the default config. `entry_owns_key`
  claims it (`keys.rs:52-55`), but Escape is not a default hotkey, so the
  window's own bubble `EventControllerKey` (`frame.rs:797-814`) is the only
  handler and it still runs, exactly as the comment at `commands.rs:90-92`
  says. **[read]** Worth recording for later, though: that controller and the
  global hotkey controller are both bubble controllers **on the window**, and
  `gtk_widget_add_controller` prepends (`gtk/gtkwidget.c:11974`) while
  `gtk_widget_run_controllers` walks the list in order
  (`gtk/gtkwidget.c:4780-4800`), so **the later addition runs first** —
  and `install_hotkeys` is called after the Escape controller is added
  (`frame.rs:814` vs `:824`). A custom bare-Escape hotkey therefore beats
  `Header::escape` today, and with this fix it now yields to a focused entry.
  No default binds one, so nothing observable changes here. **[read]**
- **Unchanged, and that matters:** every other default — `Ctrl+T/W/N/Q/L/F/H`,
  `Ctrl+=`, `Ctrl+-`, `<Alt>Return`, `F5` — is not in `entry_owns_key`, so it
  still runs while you type. Ctrl+L still jumps to the path bar from the search
  field, Ctrl+F still opens search, Ctrl+H still toggles hidden files.
  `default_hotkeys` (`fm-core/src/config.rs:638-673`, the `defaults` array at
   `:639-660`) is the list this was checked against. **[read]**

**`commands.rs:85-88` — `preview_text_focused() && text_view_owns_key(...)`.**
No change: the `GtkTextView` check was already correct, so this guard has
behaved all along.

**`commands.rs:93` — `search_entry_focused() && search_entry_owns_key(...)`.**
Now actually fires when the caret is in the search field. `search_entry_owns_key`
(`search/mod.rs:1727-1747`) keeps bare Return, KP_Enter, Tab, ISO_Left_Tab,
Home, End, Up, Down, Left, Right, Page_Up, Page_Down and Escape, and refuses
anything with Ctrl/Alt/Super (`owns_modifier`, `search/mod.rs:1771-1777`).

- **Bug 3, fixed for custom hotkeys:** a hotkey on Page Up/Down or Tab is now
  left to the field. The default config binds neither, so Kyle sees nothing new.
- Alt+Up is deliberately *not* in this list — `owns_modifier` rules it out — and
  `commands.rs:82` catches it instead, because `entry_owns_key` allows Alt on
  Up. So the search field is covered from both sides.
- Bare Return: `GtkText` binds it to `activate` (`gtk/gtktext.c:1539`, KP_Enter
  `:1545`) and runs first, so it already submitted the query; now it is skipped
  by the guard as well. Same result.

**`commands.rs:96-100` — `search_active() && !text_entry_focused() &&
!preview_text_focused() && search_results_own_key(...)`.** Two effects, both
from `!text_entry_focused()` starting to mean something:

- With search open and the caret in the **search field**: outcome unchanged.
  `search_results_own_key` (`search/mod.rs:1751-1769`) and
  `search_entry_owns_key` (`:1727-1747`) accept exactly the same thirteen keys
  under exactly the same modifier rule, and both return `Proceed`. The
  difference is only which `if` gets there first — now line 93, whose comment is
  the one that describes it. **[read]**
- With search open and the caret in the **path bar**: this branch no longer
  fires, so a custom hotkey on bare Down, Home, End, Return, Tab, Page_Up or
  Page_Down is now dispatched while you type in the path bar with results
  showing. (`entry_owns_key` still covers Up, Left, Right, Delete and Escape.)
  No default hotkey uses a bare one of those, so the default config is
  unaffected. **[read]** This is the asymmetry the fix exposes: `entry_owns_key`
  is written for an entry's editing keys, and the path bar is not covered by the
  search-field list at all. Live check B covers it. If Kyle wants the old
  blanket protection there, the smallest change is `!app.search_entry_focused()`
  in place of `!app.text_entry_focused()` at `commands.rs:97`, which is one word
  and outside this brief's "no behaviour beyond the check" rule — so I left it.

**`header.rs:716` (`update_text`) is the other half of the fix and is not a key
rule.** **Bug 2, fixed:** a header redraw while the caret is in the path bar no
longer puts the current folder back over a half-typed path. Note that this was
already safe on the way *out*: the path bar has a
`gtk::EventControllerFocus::connect_leave` handler that calls `sync_path`
(`header.rs:1935-1941`), which re-reads the folder from the model when focus
leaves the entry. `GtkEventControllerFocus` works off the focus crossing and
fires for a widget whose focus sits on a child (`crossing->new_descendent !=
NULL`, `gtk/gtkeventcontrollerfocus.c:88-115`, `:145-152`), so it is unaffected
by the bug — which is why no stale path was ever reported. **[read]**

## Tests

Added one widget regression in `crates/fm-ui/src/fileops/keys.rs`, in the
module's own `mod tests`, so no file outside the brief's Output list is
touched:

- `entry_focused_follows_the_caret` (`keys.rs:134`) spawns
  `entry_focused_follows_the_caret_inner` (`keys.rs:143`) in a fresh process,
  the pattern `app.rs`'s widget regressions use — `gtk::init` takes over the
  process default main context for its lifetime, which would starve the other
  in-process GLib timer tests. `gtk_test_lock` and `run_widget_test`
  (`keys.rs:108`, `:114`) mirror `app.rs:6667-6691`.
- It realises a plain `gtk::Window` with a `gtk::Entry`, a `gtk::Button` and a
  `gtk::TextView` and asserts, in order:
  - after `entry.grab_focus()`: `entry_focused(&entry)` is **true** while
    `entry.has_focus()` is **false**, and the window's `focus-widget` property
    is a strict descendant of the entry — the bug and the fix in one place;
  - after `button.grab_focus()`: `entry_focused(&entry)` is **false**;
  - and, for the sweep, that the focus widget **is** the button, then **is** the
    text view — the two `has_focus()` sites that were left alone.
- Nothing is presented or mapped, and the window is never activated.
  `gtk_widget_grab_focus` asks only for a sensitive, focusable widget under a
  root (`gtk/gtkwidget.c:5154-5164`), and a widget gets its root when it is
  parented (`gtk/gtkwidget.c:2581-2588`). `FOCUS_WITHIN` needs no activation
  either; `has_focus()` does (`gtk/gtkwindow.c:2256-2257`), which is why the
  regression asserts *which widget holds focus* rather than asserting
  `button.has_focus()`. That keeps the test free of compositor state — no
  dependency on whether niri happens to consider the window active. **[read]**
- A display is available on this machine (`WAYLAND_DISPLAY=wayland-0`), and
  `fm-ui`'s existing widget regressions already need one, so this adds no new
  requirement. **[observed]**

Existing tests the brief named: the four `fileops::keys::tests` key-rule tests
(`editing_chords_belong_to_the_entry`,
`delete_and_the_arrows_belong_to_the_entry_with_or_without_alt`,
`escape_belongs_to_the_entry_and_other_hotkeys_do_not`,
`text_view_owns_editing_and_cursor_keys`),
`search::tests::search_owns_its_keys_without_modifiers` and the
`portal::picker::tests` shortcut tests all pass. **[observed]**

### Gates

`PATH="$T:$PATH" scripts/check.sh` from `$PROJ`: exit **0**, "all checks
passed". **[observed]** `T` is the 1.95.0 toolchain bin dir, per CLAUDE.md.

| Step | Result |
| --- | --- |
| `cargo fmt --all --check` | no output, exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | `Finished`, no warnings |
| `cargo test --workspace` | **1080 passed, 0 failed, 27 ignored** |
| `cargo build --workspace --release` | `Finished` in 8.17 s |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` |

Counts are the summed `test result:` lines. **[observed]** P2an's review
reported 1079 passed / 26 ignored; the pair here is +1 test and +1 ignored, the
outer and inner halves of the new regression. `cargo deny` still prints its
pre-existing `warning[duplicate]` for `syn`, unchanged by this brief.
**[observed]**

No git command was run. `dbus-run-session` was not used. The app was not
launched: `pgrep -a gimbal-files` found Kyle's instance running (PID 1367407),
so everything that
needs the real window is listed below instead. **[observed]**

## Needs a live session

Claude writes the run script. 1-5 are the brief's; A, B and 6 are what the fixed
check exposes that I could not settle by reading. Work in a scratch folder tree
under `/tmp/gimbal-*` that the script creates.

1. **Alt+Up in the path bar.** Navigate to `<scratch>/parent`, then click into
   the path bar and type `<scratch>/parent/nested/` without pressing Enter.
   Press Alt+Up. The caret stays where it is and the folder does not change.
2. **Alt+Up in the search field.** Open search (Ctrl+F), type a query, press
   Enter so results show, click back into the field, press Alt+Up. The results
   page stays up and the text and caret are unchanged.
3. **Editing keys stay text keys.** In `<scratch>/parent`, click into the path
   bar, type `xyz`, then Ctrl+A and Delete: the text goes away, no file in the
   folder is trashed and nothing in the grid is selected. Repeat in the search
   field. These were already safe; this checks they stay that way.
4. **A half-typed path survives a header redraw.** Click into the path bar and
   type a string that differs from the current folder and was never Enter'd, so
   the `invalid` class is not involved either way. Then trigger a redraw that
   reaches `update_text` without moving focus out of the entry. `update_text`
   has three call sites in two functions: `Header::relayout` (`header.rs:689-705`,
   call at `:703`), which the pill strip's `page-size` notify drives when the
   window is resized (`header.rs:417-429`), and `Header::rebuild`
   (`header.rs:576`, calls at `:617` and `:650`), which
   `WindowUi::refresh_header` (`frame.rs:327-329`) reaches from a model event
   (`app.rs:2952`). **[read]** So: **drag the window's edge to resize it** —
   that changes the strip's width and does not touch focus. The typed text and
   caret must be untouched. Before this fix the text snapped back to the current
   folder on the first redraw. A tab that changes the pill count is a second
   trigger if a resize is awkward; say which one was used.
5. **The file chooser must not have changed.** Open an Open dialog (attach a
   file to something, or use the portal path P2an used). Ctrl+A in the address
   field selects its text, not files; Ctrl+A in the search field selects its
   text; Ctrl+H in the search field reruns the query and shows or hides
   `.hidden.png`, exactly as after P2an's recheck.
6. **Trash view still behaves.** Enter Trash, then Ctrl+L. `update_text` clears
   `set_can_focus` on the path entry there (`header.rs:723-724`) and
   `focus_path` returns early (`header.rs:743-751`), so `entry_focused` must be
   false and the field must stay showing "Trash" and be uneditable. Then press
   Alt+Up: the point is that it behaves exactly as it did before this brief,
   which is that `text_entry_focused()` is false in Trash and so `nav.up` is
   dispatched. **[read]** Alt+Left, Alt+Right, Ctrl+A and Delete there likewise
   change nothing for the same reason.

**A — Alt+Left / Alt+Right while typing (a deliberate regression?).** Click into
the path bar or the search field and press Alt+Left. Expected after this brief:
nothing at all, because `entry_owns_key` hands the key to an entry that has no
Alt+Left binding on Linux. Expected before it: the tab navigated back. If Kyle
wants navigation back during typing, the change is to drop
`gdk::Key::Left | gdk::Key::Right` from the Alt side of `entry_owns_key`
(`keys.rs:41-50`) — a separate decision, and the same rule the rename box uses,
so it should not be made silently here.

**B — a custom hotkey on a bare nav key, path bar focused, search open.** In a
scratch config under `/tmp/gimbal-p2ao-config`, add a `[hotkeys]` table entry
`Page_Up = "nav.back"` (`read_hotkeys`, `fm-core/src/config.rs:1659-1672`,
accepts a bare command-name string), open search, click into the path bar, press
Page_Up. Before this brief the key was swallowed by `commands.rs:96`; now the
command runs. Whether that is right is Kyle's call — see the `commands.rs:97`
note above.

## Outside these files

- Nothing outside the four files in Output was changed, and nothing needs to be.
- Two things a reviewer may want after this, both deliberately left alone:
  `entry_owns_key`'s Alt allowance for Left/Right suppresses `nav.back` and
  `nav.forward` with no replacement (live check A), and `commands.rs:97`'s
  `!text_entry_focused()` now lets custom bare-nav keys through to the path bar
  during search (live check B). Each is a one-line change in a rule this brief
  said not to edit.
- `fm-ui` has no focus check on any other compound widget, so there is no
  follow-up sweep left. If a later brief adds a `SearchEntry` (the header's
  search field is a plain `gtk::Entry` plus a revealer), it must use
  `fileops::keys::entry_focused` and not `has_focus()`. **[inferred]**

## Claude's review (Nov 17)

Read the notes and the whole diff, reran the gates, and fixed one regression.
The helper, the three `header.rs` sites and the picker switch are as briefed,
and the sweep matches what I found. The account of `commands.rs:82-100` is
right, and the two open questions it raises (A and B) are now live checks for
Kyle to decide.

**Fixed: the path bar kept a stale folder after a tab switch.** Before P2ao,
`update_text` always rewrote the path bar, because `has_focus()` was always
false. With the check working it now leaves the text alone whenever the caret
is in the bar, and that covers more than a half-typed path. A tab pill is
clicked through a `GestureClick` on a `CenterBox` (`header.rs`, the pill build),
which doesn't move focus. So after Ctrl+L, clicking another tab's pill (or
Ctrl+T or Ctrl+W, which don't move focus either) changed the folder but left
the path bar naming the old one. `focus-leave`'s `sync_path` never fires,
because focus never leaves.

The fix: `Header` remembers the folder `update_text` last showed
(`path_folder`). A redraw of the **same** folder keeps what you typed, and a
**different** folder replaces it, even while you're typing. The rule is a pure
function, `keep_path_text(focused, invalid, same_folder)`, which also keeps the
old `invalid` rule unchanged. Its test,
`header::tests::the_path_bar_keeps_typing_only_over_the_same_folder`, covers
all six cases. `path_enter` is unaffected: it navigates, the redraw sees a new
folder and writes it, and then `path_enter` writes the same text.

**Gates after the fix:** `scripts/check.sh` exit 0. 1081 passed, 0 failed,
27 ignored (+1, the new test). fmt and clippy are clean, and `cargo deny` shows
only the old `syn`, `system-deps` and `windows-sys` duplicate warnings.

**Live run:** `/tmp/gimbal-p2ao/run.sh`. It refuses to start while a Gimbal is
running, rebuilds the debug binary, and uses scratch config, state and data,
so Trash is a scratch one. It covers the brief's checks 1–6, A and B (with a
scratch `Page_Up = "nav.back"` hotkey), and check 7 for this fix: Ctrl+L, then
click another tab's pill, and the path bar follows.
