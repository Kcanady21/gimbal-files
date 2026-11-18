# P2as: edit keyboard shortcuts in Settings

## What I built

Settings → Keyboard Shortcuts… now opens a `adw::Dialog` on the window. It lists every
command a hotkey can run, grouped by area, with each current accelerator drawn as an
`adw::ShortcutLabel` and a small remove button. **Add** puts the row into "Press the new
shortcut…", **Reset** appears only when the row differs from the defaults, and **Reset All**
sits in the dialog header behind a confirmation. A search field filters by readable name or
by key (`ctrl` matches `<Control>t`). A hint at the top says a desktop shortcut (Mango, niri)
never reaches Gimbal.

The dialog counts in the header's `dialogs` counter, so the window's hotkeys are blocked while
it is open. That is what lets Capture own the key: Ctrl+T pressed behind the dialog does
nothing.

## The capture rule

`hotkeys::capture(key, modifiers, default_mask)` is pure and tested. GTK passes its
`accelerator_get_default_mod_mask()`; the tests pass the same four modifiers directly.

```
Escape / Backspace                 -> Cancel
Shift, Control, Alt, Super, Meta,
Hyper, ISO_Level3/5, Caps/Num Lock -> Ignore (keep waiting)
printable with no Ctrl/Alt/Super    -> Refused("Add Ctrl, Alt or Super to a letter or number.")
Tab / Return / KP_Enter, no modifiers -> Refused("Tab and Return need Ctrl, Alt or Super.")
other named key, no modifier        -> Refused("Use a function, arrow, editing or media key…")
otherwise                           -> Allowed(key.to_lower(), masked modifiers)
```

The default mask is applied and Shift is kept explicitly. The allowed bare set is F1–F35,
Delete, Insert, Home, End, Page Up/Down, the four arrows, and the common media, brightness,
Eject, Sleep/Suspend/Wake, screen-saver, battery, Bluetooth, WLAN/WWAN and RF-kill keys.
The accelerator string comes from `gtk::accelerator_name(lowercase key, masked modifiers)`.

Conflict: if the new accelerator already runs another command, the row shows
"Ctrl+T is used by New tab" plus **Replace** and **Cancel**. Replace dispatches `hotkey.set`,
which moves the key. A binding that names a command outside the editor's list gets that
command's catalogue summary rather than a raw command name.

## The saved file

`U` is the user's `[hotkeys]` table, `D` is `default_hotkeys()`, and what runs is `D` with `U`
on top (`""` removes). The four new `Change` variants edit the parsed `toml_edit` document, so
comments and the order of untouched keys survive.

| Operation | File rule |
| --- | --- |
| Bind `a` to `c` | If `D[a]` is `c`, delete `U[a]`; otherwise `U[a] = "c"`. |
| Remove `a` | If `a` is in `D`, `U[a] = ""`; otherwise delete `U[a]`. |
| Reset `c` | Delete every `U[a]` that binds `c`, every `U[a] = ""` that removed one of `c`'s defaults, and every `U[a]` that took one of `c`'s default keys for another command. The last case gives the key back to `c`. |
| Reset All | Reset every bindable command. Entries for commands the editor does not list stay untouched. |

Concrete examples, all covered by tests:

- `"<Alt>Left" = "nav.forward"`, bind `nav.back` to `<Alt>Left` → the entry is deleted,
  because `<Alt>Left` is already `nav.back` in `D`.
- Remove `<Alt>Left` → the file gets `"<Alt>Left" = ""`.
- `"<Alt>Left" = "nav.forward"` and `"F6" = "nav.back"`, reset `nav.back` → both entries go;
  `<Alt>Left` runs `nav.back` again and `nav.forward` keeps `<Alt>Right`.
- Reset All on a file with `"<Control>1" = { command = "tab.focus", index = 0 }` leaves that
  entry exactly as written. It removes `"F6" = "nav.refresh"`, `"<Alt>Left" = ""` and the
  editor's other entries.
- A hand-written `# keep this comment` is still there after every operation.

Writes go through `ConfigFile::apply`. A file that did not parse is never written and, for the
four hotkey changes only, the in-memory config is left untouched too, so the dialog can say
"Can't save: config.toml has an error" and nothing moves. (Other changes keep their older
behavior: memory changes, nothing is written.) The dialog checks `Process::config_writable()`
before dispatching and renders any save failure in the row or in a global line above the list.

## Commands and the caller rule

Three new `Effect::Settings` commands in `fm-core`'s catalogue:

- `hotkey.set` (`accelerator`, `command`) → `Change::BindHotkey`
- `hotkey.remove` (`accelerator`) → `Change::RemoveHotkey`
- `hotkey.reset` (optional `command`; none means all) → `Change::ResetHotkey` or
  `Change::ResetAllHotkeys`

All three are in `UI_ONLY_SETTINGS_COMMANDS`, so the registry refuses `Caller::Agent`,
`Caller::Cli` and `Caller::Hotkey` before any handler or write. The `app.rs` handler repeats
the refusal in the style of `tab.open-as-admin`, as a second guard. An agent cannot rebind
Delete.

`fm-core` has no GTK, so `change_for` checks only that the target is bindable and that the
accelerator is a non-empty `Text`. The dialog validates the accelerator with
`gtk::accelerator_parse` before it dispatches. A non-bindable target makes `change_for` return
`None`, so `hotkey.set` fails and changes nothing.

## What is bindable

`fm_core::command::bindable_commands()` is the one rule: no required argument, a hotkey may
actually run it (`caller_allowed(..., Caller::Hotkey)`), it is not `hotkey.*`, and its
`app.rs` handler does not refuse non-UI callers. The handler-only exclusions are
`file.open-with`, `file.open-as-admin`, `file.set-default-app`, `tab.open-as-admin`,
`tab.leave-admin`, `updates.set-enabled`, `updates.check` and
`integration.set-show-in-folder`. `fm-ui::hotkeys::bindable_commands()` re-exports it.

The final set (38):

Navigation: `nav.back`, `nav.forward`, `nav.up`, `nav.refresh`, `nav.focus-path`

Tabs and windows: `window.new`, `window.close`, `tab.new`, `tab.close`,
`tab.close-selected`, `tab.open-in-new-window`, `tab.open-in-own-tab`, `group.split`,
`group.close`

Files: `file.open`, `file.properties`, `file.trash`, `file.undo`, `file.cut`,
`file.copy-to-clipboard`, `file.paste`, `trash.open`

Selection: `selection.all`, `selection.none`

View: `view.zoom-in`, `view.zoom-out`, `view.clear-folder-sort`, `view.toggle-hidden`

Search: `search.open`

Preview: `preview.edit`, `preview.discard`, `preview.save`

App: `app.quit`, `app.disk-usage`, `app.settings`, `pins.add`, `picker.accept`,
`picker.cancel`

The readable names live in one table, `hotkeys::NAMES`, in `fm-ui`. A test asserts that its
keys are exactly the bindable set (so no command lacks a name and no name is left over), that
no label is used twice, and that every default hotkey targets a bindable command. `pins.add`
and `app.disk-usage` qualify because their path argument is context-filled, not required.

## Applying a change live

`WindowUi` keeps its `gtk::ShortcutController` in a new `hotkeys` field.
`commands::install_hotkeys` builds the controller with the old code, removes the previous
controller from the window, stores the new one and adds it last. `Process::after_change`
handles all four `Change` variants by calling `ui.reinstall_hotkeys()` for every open window.

The controller order P2ao recorded is preserved: the window's Escape controller is added in
`WindowUi::wire` before `install_hotkeys`, and every reinstall removes the old hotkey controller
and adds the new one at the end, so the hotkey controller is still last.

## What I verified

- `PATH="$T:$PATH" ./scripts/check.sh` → `all checks passed` (exit 0).
  - `cargo fmt --all --check` passed.
  - `cargo clippy --workspace --all-targets -- -D warnings` passed.
  - `cargo test --workspace`: fm-agent 23 passed; fm-agent `tests/socket.rs` 16 passed;
    fm-cli 13 passed; fm-core 526 passed + 1 ignored; fm-core `tests/trash.rs` 6 passed +
    6 ignored; fm-core `tests/trash_browser.rs` 11 passed + 11 ignored; fm-ui 517 passed +
    9 ignored; doc-tests 0.
  - `cargo build --workspace --release` passed.
  - `cargo deny check` → `advisories ok, bans ok, licenses ok, sources ok`.
- New fm-core tests cover each file rule, comments, the unparsable file, the caller refusals,
  the bindable target and Reset All's untouched indexed `tab.focus` entry.
- New fm-ui tests cover the capture rule (refused/allowed, Shift kept, lowercase keyval, lone
  modifiers, Escape/Backspace), the search matcher, and the names/bindable set.

The reinstall itself is **not** tested headless. Firing a shortcut requires a key event, and
this brief forbids key injection. The existing `fileops/keys.rs` child-process pattern could
build the controller widgets, but it cannot prove that a key fires without injecting one. So
Claude checks the live behavior below.

## API citations

- `adw::ShortcutLabel::new` / `set_accelerator` — `libadwaita-0.9.2:src/auto/shortcut_label.rs:37`.
  The crate is pinned at `=0.9.2` with `features = ["v1_9"]`, so the 1.8+ widget is present.
- `adw::DialogExt::present` / `set_title` — `libadwaita-0.9.2:src/auto/dialog.rs:471`, `:561`.
- `gtk::accelerator_name` — `gtk4-0.11.5:src/functions.rs:54`.
- `gtk::accelerator_get_default_mod_mask` — `gtk4-0.11.5:src/auto/functions.rs:35`.
- `gtk::accelerator_parse` — `gtk4-0.11.5:src/functions.rs:86`.
- `gdk::Key::to_lower`, `to_unicode`, `name` — `gdk4-0.11.5:src/keys.rs:60`, `:94`.
- `toml_edit::Key::default_repr` quotes keys such as `<Control>t`
  (`toml_edit-0.25.17+spec-1.1.0:src/key.rs:80-90`); the new-entry test checks the exact
  `"<Control>j" = "nav.back"` output.

**[read]** All API names above were checked against the pinned crate sources.
**[observed]** The test suite and `check.sh` results above.
**[inferred]** The live key behavior and the dialog's look need a real session.

## What needs a live session (Claude)

1. Settings → Keyboard Shortcuts opens. Ctrl+T while it is open does nothing behind it.
2. Bind Back to Super+Left: close the dialog, Super+Left goes back, or Mango takes it (which
   proves the hint). Bind Back to Ctrl+Shift+B: it works at once in every open window.
3. Remove Alt+Left from Back: the file gets `"<Alt>Left" = ""`, and Alt+Left does nothing.
4. Give a command the key Ctrl+T: the conflict message names New tab; Replace moves it and New
   tab loses it.
5. Bind plain `a`: refused with the inline message. Bind F6: allowed.
6. Reset Back: the file loses its Back entries and Alt+Left works again. Reset All: the file's
   `[hotkeys]` keeps only the hand-written entries the editor does not list.
7. A hand-written comment in the scratch `config.toml` survives all of this.
8. Break the scratch config by hand, then open the dialog: it says it can't save; nothing
   changes.
9. Check the dialog tiled and in a floating window for layout and the shortcut chips.

## Outside my files

Nothing else needed a change. I did not add a stylesheet rule: the dialog uses
`gimbal-look`, the built-in `dim-label`, `error`, `accent`, `card`, `heading` and
`destructive-action` classes, and the look tokens for spacing. No new dependency.

## Claude's review (Nov 17)

Read the notes and the whole diff, including all of `hotkeys.rs` and the config and registry
changes. The design is right:
- the three `hotkey.*` commands are refused for every caller but the UI, at the registry and
  again in the handler;
- the file edits match the in-memory changes for every operation, including a stolen default
  key and inline-table entries;
- an unparsable file leaves both the file and memory unchanged;
- a reinstall keeps the hotkey controller last.

Three fixes:

1. **`picker.accept` and `picker.cancel` were bindable.** The file chooser answers only its own
   fixed keys and never reads `[hotkeys]`. The main window doesn't accept these commands either
   (`app.rs`, `picker_accepts` is the portal's routing), so a binding would never fire.
   `bindable_commands()` now excludes `PICKER_COMMANDS`. Their two `NAMES` rows are gone, and
   `bindable_commands_take_no_required_argument_and_a_hotkey_may_run_them` asserts both are
   excluded. The set is 36, not 38.
2. **Conflicts compared spellings, not keys.** A hand-written `"<Control><Shift>j"` and the
   captured `<Shift><Control>j` (GTK's order) are the same key. They were missed as a conflict
   and saved as two entries with the same trigger. `finish_capture` now matches by
   `gtk::accelerator_parse` and reuses the existing spelling, so Replace rebinds that one entry.
   Capturing a key the row already has is a no-op.
3. **Four fixed pixel sizes** in the row (spacing 4 and 2, margins 2) are now `GAP_S / 2` and
   `GAP_S / 4` times the scale (CLAUDE.md: no fixed sizes).

**Gates after the fixes:** `scripts/check.sh` exit 0, 1112 passed, 0 failed, 27 ignored, the
same count, since the fixes changed tests rather than adding them. fmt and clippy are clean, and
`cargo deny` shows only the old duplicate warnings.

**Live run:** `/tmp/gimbal-p2as/run.sh`, with a scratch config that has a hand-written comment,
a `tab.focus` entry with an index, and `"<Control><Shift>j" = "nav.refresh"` to exercise fix 2.
Part 2 breaks the scratch file and checks that it isn't rewritten.

## Claude's live test (Nov 18)

Kyle asked Claude to run the live checks. This session has no desktop-control tool, and
CLAUDE.md forbids key and pointer injection. So the checks run in-process, as a real-GTK widget
regression in the pattern of the search test (`app.rs`):
`app::tests::real_shortcut_editor_saves_and_reinstalls_in_every_window` (+ `_inner`).

- It builds two real windows and opens the real dialog on one.
- It clicks the dialog's own buttons with `emit_clicked`.
- It feeds key presses through the same `Editor::key_pressed` that the dialog's capture
  controller calls.
- After each step it checks the saved scratch `config.toml` and each window's installed
  `ShortcutController` triggers (`WindowUi::hotkey_triggers_for_tests`).

Test hooks: `hotkeys::testing::Driver` and `WindowUi::hotkey_triggers_for_tests`, both
`#[cfg(test)]`.

What it covers from the run script:
1. Hotkeys stay blocked while the dialog is open, and unblock when it closes.
2. Binding Back to Super+Left and Ctrl+Shift+B saves both, and both windows' controllers get
   them. A lone modifier keeps waiting.
3. Removing Alt+Left writes `"<Alt>Left" = ""`, and the key leaves both windows.
4. Searching "refresh" shows only Refresh, with F5 and the hand-written Ctrl+Shift+J. Capturing
   Ctrl+Shift+J on Back says "… is used by Refresh". Replace leaves exactly one entry, in the
   hand-written spelling (review fix 2).
5. A plain `a` is refused with the message and capture keeps waiting. F6 binds and reaches the
   second window. Ctrl+T conflicts with New tab, and Cancel and Escape both write nothing.
6. Reset Back removes every Back entry and the `""`. Alt+Left comes back and Ctrl+Shift+B goes,
   in both windows. Reset All removes F6.
7. The hand-written comment and the `tab.focus` entry with an index survive, and Ctrl+1 is still
   installed.
8. With a config that doesn't parse, the row says "Can't save: config.toml has an error", the
   file is byte-for-byte unchanged, and no trigger moves.

**Found and fixed by looking.** With `GIMBAL_TEST_SNAPSHOTS=<folder>`, the test renders the
dialog to PNGs (`Driver::snapshot`, after the open fade and scroll animations settle). On a row
far down the list (Zoom in), the refusal message sat below the visible area.
- The cause: GTK's viewport scrolls only to the focused Cancel button, so the message line
  under it stayed hidden. A conflict's Replace button would have been hidden the same way.
- `Editor::reveal` now scrolls the whole active row into view once it has been laid out.
- It uses `set_value`, which ends GTK's running focus-scroll animation
  (`gtk-4.22.4:gtk/gtkadjustment.c:525-540`). `clamp_page` writes the value under that
  animation, which then carried on to its own target. That was the first attempt, and the
  snapshot showed it didn't work.
- Confirmed by a new snapshot: the refusal line shows under the capturing row.

**Not covered in-process:**
- Real key delivery through the compositor (does Mango take Super+Left?).
- A real key press reaching the controller. The test checks the installed triggers, not a
  firing.
- Real hand scrolling.

These need Kyle at the keyboard. Seen in the snapshots: a row with three shortcuts (Back with
Ctrl+Shift+J, Ctrl+Shift+B and Super+Left) already uses the full dialog width. A fourth would
make the dialog wider rather than wrap. That isn't fixed; it's a small follow-up if it matters.

**Gates:** `scripts/check.sh` exit 0, 1113 passed, 0 failed, 28 ignored (+1 test, +1 ignored:
the outer and inner halves of the widget regression).
