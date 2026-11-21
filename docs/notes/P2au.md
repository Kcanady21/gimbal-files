# P2au (portal F4): the dialog sits on its app and remembers each app's folder

Tags: **[read]** for source, **[observed]** for a command and its output,
**[inferred]** for anything that needs a live session.

Starts from main at `033ae28` (P2at). Status: code, tests, notes and all five
gates are complete. The in-process real-GTK regressions passed here, including
a real `xdg-foreign` attach on Mango; the visual and workspace checks at the end
are for Claude and Kyle.

## What changed

- `crates/fm-core/src/picker_state.rs` (new): the whole state file, GTK-free.
- `service.rs`: `parent_handle`, the remembered-folder argument of
  `starting_folder_for`, `modal` in `CallOptions`, `parent_window` in `Call`;
  the old "known but ignored" list is gone.
- `picker.rs`: attach on present, `set_modal`, the state read in `PickerHost::open`,
  the folder write on success and the size write on close, plus `dialog_size`.
- `tests.rs`: new pure tests and one real-GTK parent test; P2at's save regression
  gained the folder-memory checks.
- `portal/mod.rs`: the module doc only.

## 1. Parsing `parent_window`

`service::parent_handle` (`service.rs:476`) is pure:

| Input | Result |
| --- | --- |
| `wayland:<handle>` | `Parent::Wayland(<handle>)` |
| `x11:<hex XID>` | `Parent::X11(<value>)` |
| `x11:0`, `x11:not-hex`, `x11:`, `wayland:` | `Parent::None` |
| empty, a missing/other prefix, leading spaces | `Parent::None` |

The Wayland handle is kept verbatim after the prefix, including any colons. The
XID is parsed with `u64::from_str_radix(_, 16)`, so upper- and lower-case hex
both work. Every form is pinned in `parent_handles_read_wayland_x11_and_nothing`.

`Call` now carries `parent_window` alongside `app_id`; the service already
logged the argument, so only the field is new.

## 2. Where the dialog sits

After `present()` the picker takes the window's `gdk::Surface`, downcasts it to
`gdk4_wayland::WaylandToplevel`, and calls
`set_transient_for_exported(<handle>)` (`picker.rs`, `attach_parent`). The
binding is `gdk4-wayland-0.11.5:src/auto/wayland_toplevel.rs:42-49`; the type
extends `gdk::Surface`, which is what makes the downcast valid
(`.../src/auto/wayland_toplevel.rs:9-16`). GTK imports the handle through
`zxdg_importer_v1`, falling back to v2
(`gtk-4.22.4:gdk/wayland/gdktoplevel-wayland.c:2810-2851`; v1 at `:2827-2834`,
v2 at `:2837-2845`). Mango creates both importers and the registry
(`~/mango/src/main.c:906-910`; includes at `:115-117`). **[read]**

`set_transient_for_exported` returns `false` for a missing importer or a handle
the compositor rejects. That is one log line ("the app's parent handle was
refused; not attached") and the dialog stays exactly as it was. An X11 handle
logs `parent is an X11 window; not attached`. An empty handle does nothing.

**What Mango does with it.** `client_is_float_type` returns true when the
`xdg_toplevel` has a parent (`~/mango/src/manage/client.c:289-332`, the xdg
branch's `return toplevel->parent || …` at `:321-323`). `client_set_rules` uses
the parent for the monitor (`:1632-1635`), ORs `parent` into `c->isfloating`
(`:1637`), and copies the parent's tag set (`:1668-1670`). **[read]**

So the dialog floats instead of tiling. It appears **centred on the parent's
monitor**, not offset from the parent window: `client_center_geometry`
(`client.c:1001-1052`) centres on `m->w` (`:1013-1015`) and is called for a
floating window at `:1739-1746`. Its tags are a copy of the parent's tags at
map time; `client_has_children` exists (`:270-276`) but has no callers, so
moving the parent to another workspace afterwards leaves the dialog on the
workspace it opened on. **[read]** That last visible result needs Kyle's live
check.

Mango has no `xdg_dialog` / `xdg_wm_dialog` anywhere in `~/mango/src` (grep,
empty). **[read]**

## 3. Modal

`modal` is parsed for every method with a default of true
(`service.rs:parse_call_options`) and applied with `window.set_modal(...)`.
A missing, true or wrong-typed value is modal; only an explicit `false` is not.
The old `ignored` vector, the per-method known-key lists and the log line that
named them are removed.

On a compositor with `xdg_wm_dialog_v1`, GTK marks the dialog modal through
`maybe_set_xdg_dialog_modal` (`gtk-4.22.4:gdk/wayland/gdktoplevel-wayland.c:1241-1252`,
called on commit at `:1018-1021`). Mango has no such protocol, and the fallback
`gtk_surface1_set_modal` (`:1234-1238`) is a GTK-shell hint Mango does not
implement either. On Mango `modal` therefore changes nothing beyond the float
the parent already gives. Nothing was built to fake it. **[read]**

## 4. The state file

Chosen name: **`picker-state.toml`**. The brief allowed renaming
`picker-folders.toml`; the file now holds a size too, so `picker-state` reads
better.

- **Path:** `$XDG_STATE_HOME/gimbal-files/picker-state.toml`, from
  `fm_core::logfile::default_state_dir()` (`logfile.rs:64-75`), the same
  resolution the logs use. With a relative or unset `XDG_STATE_HOME` it falls
  back through `HOME`; with neither, nothing is remembered and one log line
  says so.
- **Format:**

  ```toml
  # SPDX-License-Identifier: GPL-3.0-or-later
  # Gimbal Files picker state: the last folder per app id and the dialog size.
  # The main app never reads or writes this file.

  [[folders]]
  app_id = "org.mozilla.firefox"
  folder = "/home/kyle/Downloads"
  last_used = 1763660000

  [window]
  width = 1100
  height = 700
  ```

  `last_used` is Unix seconds; the folder table is written in `app_id` order
  (a `BTreeMap`), the window table after it.
- **The empty `app_id`:** stored under `"(no app id)"`
  (`picker_state::NO_APP_ID`). A D-Bus name cannot contain spaces or
  parentheses, so no real id can collide.
- **Trimming:** at most `MAX_APPS = 200`. A new id at the limit evicts the
  entry with the smallest `last_used` (ties broken by app id). Updating an
  existing id never evicts. A file with more than 200 is trimmed in memory on
  load.
- **Atomic write:** `fm_core::fsutil::save_atomic` (`fsutil.rs:19-81`) writes a
  uniquely named temp file in the same folder, fsyncs it, and renames it over
  the target; it creates the folder if needed. A failed write leaves the old
  file and removes the temp file.
- **A bad file:** `PickerState::load` returns the empty state and one message
  ("ignoring … because it does not parse (it will be replaced): …"). The next
  write replaces it. One wrong entry in an otherwise valid file is skipped;
  only a syntax error or a wrong-typed top-level section is a "does not parse".
- **Non-UTF-8:** `set_folder` refuses a folder that is not UTF-8 and not
  absolute; the dialog logs "not remembering …, the folder is relative or not
  UTF-8" and carries on. TOML strings are UTF-8, so the alternative would be a
  lossy name. A non-UTF-8 folder is therefore skipped, never mangled.
- **Who writes it:** only the picker. `config.toml` and the session file are
  untouched; P2am's read-only rule stands.

The state module is `crates/fm-core/src/picker_state.rs`, with its `pub mod`
line in `fm-core/src/lib.rs`.

## 5. The starting folder

`service::starting_folder_for(call, home, remembered)` now takes the remembered
folder and follows:

1. SaveFile's `current_file` folder, when it still is a directory;
2. the call's `current_folder`, when it still is a directory;
3. the remembered folder for this `app_id`, when it still is a directory;
4. home.

A remembered folder that vanished, or is now a file, falls through to home. The
lookup is done once in `PickerHost::open`; the pure function stays testable.

## 6. What is written on success

`Picker::answer` is the one success path. It computes the folder **before** the
reply, and writes only if `Request::accept` returns true (so a request already
answered by `Request.Close` cannot be remembered):

| Method | Remembered folder |
| --- | --- |
| OpenFile | the folder the grid was showing when Open was pressed (in folders-only mode, not the folder that was picked) |
| SaveFile | the answered path's folder |
| SaveFiles | the chosen folder (the answered folder itself when the call had no files) |

Cancel, Close, Escape and every refusal write no folder. Every save is a fresh
load-modify-save, so two dialogs in one picker process do not clobber each
other's entries.

## 7. The size memory

One size, shared by all apps. `dialog_size(remembered, scale)` (`picker.rs`) uses
the remembered `(width, height)` or `PICKER_W × PICKER_H` scaled, and lifts
either side to at least `PICKER_W/2` / `PICKER_H/2` scaled. The value is used as
the window's default size. When the dialog closes — accept, cancel, Close,
Escape, X — `remember_size` reads `window.width()/height()` and writes the
clamped value with the same atomic load-modify-save. A zero-size window writes
nothing. A remembered size is logical pixels, so it is used as-is; the floor
scales with the current look scale.

## API citations

- `gdk4-wayland-0.11.5:src/auto/wayland_toplevel.rs:9-16` (the type extends
  `gdk::Surface`), `:42-49` (`set_transient_for_exported`);
  `:src/wayland_toplevel.rs:9-57` (`export_handle`, used by the test).
- `gtk-4.22.4:gdk/wayland/gdktoplevel-wayland.c:2810-2851` (import v1, then v2),
  `:1018-1021` and `:1241-1252` (`xdg_wm_dialog`), `:1234-1238` (GTK-shell modal
  fallback).
- `~/mango/src/manage/client.c:255-268` (`client_get_parent`), `:270-276`
  (`client_has_children`, no callers), `:289-332` (`client_is_float_type`,
  parent at `:321-323`), `:1632-1637` (parent monitor and floating),
  `:1668-1670` (parent tags), `:1001-1052` and `:1739-1746` (centring).
- `~/mango/src/main.c:115-117`, `:906-910` (`xdg_foreign` v1 and v2).
- `fm-core/src/logfile.rs:64-75` (`state_dir` / `default_state_dir`).
- `fm-core/src/fsutil.rs:19-81` (`save_atomic`).
- `toml_edit-0.25.17`: `Document`/`DocumentMut`, `ArrayOfTables`, `Table`,
  `Value`; the output is round-tripped in the unit tests.
- `gio-0.22.10`: `GtkWindow::set_modal` and `Widget::surface` are reached through
  `gtk4-0.11.5`'s prelude; `GtkWidget::width`/`height` are the allocated size.

No new dependency and no `unsafe`. Nothing outside the brief's file list needed
a change: `logfile::default_state_dir` and `fsutil::save_atomic` were already
available inside `fm-core`.

## Tests and gates

`scripts/check.sh` with the 1.95.0 toolchain (`PATH="$T:$PATH"`) passes all five
gates: fmt, clippy with `-D warnings`, tests, release build and `cargo deny
check` (`advisories ok, bans ok, licenses ok, sources ok`). **[observed]**

`cargo test --workspace`: **1,158 passed, 0 failed, 30 ignored** (summed
`test result:` lines; P2at was 1,143 / 29). **[observed]**

New tests:

- `portal::tests::parent_handles_read_wayland_x11_and_nothing` — every form,
  empty Wayland handle, bad hex, zero XID, missing prefix.
- `portal::tests::starting_folder_for_follows_save_file_current_folder_remembered_and_home`
  — all four steps in order, a vanished folder, a file where a folder used to
  be, and a `current_folder` beating the remembered one.
- `portal::tests::modal_is_true_when_absent_and_false_only_when_the_call_says_so`.
- `portal::picker::tests::the_dialog_size_uses_the_remembered_one_and_never_goes_below_half`.
- `picker_state::tests`: round trip; missing/bad file; a bad file replaced on
  the next write; LRU at 201; updating at the limit; the empty-id slot and a
  near-lookalike real id; non-UTF-8 and relative folders refused; a failed write
  leaves no temp file; a failed write keeps the previous file (skipped if the
  test process can ignore the read-only folder); one bad entry keeps the rest;
  a wrong top-level section is a parse error.
- `portal::tests::real_save_dialog_answers_names_paths_and_collisions` (P2at's
  real-picker regression) now also checks, in its scratch `XDG_STATE_HOME`:
  - the last successful SaveFiles folder is remembered for `org.example.App`;
  - a second SaveFile from that id with no `current_folder` opens there, and is
    modal;
  - a different id opens at home;
  - `modal = false` leaves the window non-modal;
  - cancelling the new dialogs changes no remembered folder (the size may be
    rewritten, as designed).
- `portal::tests::real_open_dialog_attaches_to_a_wayland_parent` (+ `_inner`):
  a second real toplevel exports its own handle with
  `WaylandToplevel::export_handle` and passes `wayland:<handle>` to a real
  OpenFile. It asserts `set_transient_for_exported` returned true through the
  `transient_attached()` test hook. A display with no Wayland surface or no
  importer prints "skipped: …" and passes. On this machine it **attached**:
  the child log said `portal: attached to the app's parent window` and the
  assertion ran, not the skip branch. **[observed]**

## Live checks for Claude and Kyle

None of the visual checks was run here; no pointer or keyboard input is
allowed. The run script should use scratch `XDG_CONFIG_HOME` /
`XDG_STATE_HOME` and the `gdbus` byte-array form from P2am.

1. A picker for a Wayland parent floats centred over that parent's monitor
   instead of tiling beside it.
2. Move the parent to another workspace: the dialog is expected to stay on the
   workspace where it opened (tags are copied at map time). Confirm what Mango
   actually does.
3. An empty `parent_window`: the dialog opens exactly as after P2at, no error.
4. Save in folder A from app X; the next X dialog with no `current_folder`
   opens in A, while app Y opens at home. Cancel changes no folder.
5. Resize a picker and close it: the next picker opens at that size. Also check
   a hand-written tiny size is lifted to half the default.
6. `~/.local/state/gimbal-files/picker-state.toml` is the only new file; the
   main app's `config.toml` and session file are untouched. The main app never
   reads it.
7. `modal = false` leaves the window non-modal; on Mango, confirm that a modal
   dialog still behaves as the float does (no `xdg_wm_dialog` effect).

## Claude's review (Nov 21)

Read the notes and the diff. The handle parsing, the attach, `modal`, the state file (atomic,
LRU 200, the empty-id key, non-UTF-8 skipped) and the starting-folder order are as briefed and
well tested. I confirmed that no run wrote to Kyle's real `~/.local/state/gimbal-files/`; both
real-window tests set scratch `XDG_*` in their outer half.

**Fixed: the size memory saved compositor sizes.** `remember_size` stored whatever size the
window had when it closed. A dialog with no parent, or a rejected one, isn't floated by Mango;
it's tiled, so its size is the tile's, and that would become every later dialog's default. A
maximized or fullscreen dialog had the same problem. It now remembers only when the dialog was
attached (`transient == Some(true)`) and isn't maximized or fullscreen. Tests:
- the P2at save regression, whose dialogs have no parent, now asserts no size is remembered;
- the parent regression asserts the attached dialog's size is remembered.

**Observed here:** running the parent test's inner half with scratch folders logged
`portal: attached to the app's parent window`, a real `xdg-foreign` attach on Mango, and wrote
`[window] width = 1100, height = 700` to the scratch state.

**For Kyle's look:** `GIMBAL_HOLD_PARENT_TEST=1` makes the parent test leave the attached dialog
open, for up to two minutes, until it's closed by hand. The outer test then prints the
remembered size. `/tmp/gimbal-p2au/run.sh` uses it to check: the dialog floats, what happens
when the parent changes workspace, and a hand-chosen size being remembered.

**Gates:** `scripts/check.sh` exit 0, 1158 passed, 0 failed, 30 ignored. The count is the
agent's: the fixes added assertions to existing tests, not new tests.

**Fixed during Kyle's look:** the first held run ended after 2 seconds. The tests' D-Bus calls
time out at 2000 ms, so the held call gave up while the dialog was still open, and the size
check ran before anything was saved. `Bus` now has a `timeout_ms` (default 2000), and the held
call sets 150 s. The normal test path is unchanged.

**Kyle's live look (Nov 21):** the dialog floats centred on the screen, as Mango's
`client_center_geometry` predicts. Moving the parent to another monitor leaves the dialog on the
one it opened on: tags and monitor are copied at map time. Resizing works. Kyle saw the size not
carry into the next launch; that's expected, since each test run has a fresh scratch state
folder. The real picker uses one state file, and the regression shows the size written on
Cancel. Accepted as is.
