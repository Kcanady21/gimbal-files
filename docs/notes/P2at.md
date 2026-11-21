# P2at (portal F3): Save and SaveFiles

Tags: **[read]** for source, **[observed]** for a command and its output,
**[inferred]** for anything that needs a live session.

Starts from P2an on main at `42f8b0b` (Release 0.1.6). Status: code, tests and
all five gates are complete. The live checks at the end are for Claude and
Kyle; none was run here (Kyle's main Gimbal is running, and this agent may not
inject pointer or keyboard input).

## Options: one widened type, renamed

I widened the existing options struct rather than adding a second type, and
renamed it `CallOptions` (`portal/service.rs:125`). The two new methods carry
three fields Open never sees (`current_name`, `current_file`, `files`), and one
type keeps the parser, the per-method known-key lists and the picker's reads in
one place. `OpenOptions` would have been a lie once it held a filename.

`parse_call_options(method, options)` (`service.rs:351`) reads only the keys
that method knows:

| Method | Known keys | Still ignored |
| --- | --- | --- |
| OpenFile | `accept_label`, `modal`, `multiple`, `directory`, `filters`, `current_filter`, `choices`, `current_folder` | `modal` |
| SaveFile | `accept_label`, `modal`, `filters`, `current_filter`, `choices`, `current_name`, `current_folder`, `current_file` | `modal` |
| SaveFiles | `accept_label`, `modal`, `choices`, `current_folder`, `files` | `modal` |

A key another method knows is unknown here and stays silent: OpenFile does not
read `current_name`, and SaveFile does not read `multiple`. A known key with the
wrong type is read as absent and named once in `malformed`; the tests pin both
orders. `current_folder`, `current_file` and every `files` entry are `ay` and
reuse `folder_from_bytes` (now `service.rs:446`), so a missing NUL, an empty
value and a non-UTF-8 path behave as they do for Open. `files` is `aay`, read as
`Vec<Vec<u8>>` and validated later.

`starting_folder_for` (`service.rs:471`) adds SaveFile's one rule: when
`current_file` has a parent that is still a directory, open that folder; else
follow the Open rule. `Call::accept_label` (`service.rs:237`) now returns
"Save" for both Save methods unless the app named a label.

Results (`results_for`, `service.rs:540`) take the method: `writable` is added
for OpenFile only, `current_filter` for OpenFile and SaveFile only, and
SaveFiles gets `uris` and `choices` alone.

## `save_target`: every outcome and its test

`portal/save.rs` is the pure decision. The lookup is a closure, so the tests
need no files. The outcomes, in the order the function checks them:

| Situation | Outcome | Test |
| --- | --- | --- |
| empty or whitespace-only | `Refused("Type a file name")` | `an_empty_or_whitespace_only_name_is_refused_before_a_lookup` |
| not `is_valid_name` (`/`, `.`, `..`, NUL, > 255 bytes) | `Refused("That name can't be used")` | `an_invalid_name_is_refused_and_a_long_one_too` |
| an existing folder | `Enter(folder/name)` | `an_existing_folder_is_entered_not_answered` |
| an existing regular file, or a symlink to one | `Overwrite(path)` | `an_existing_file_or_a_link_to_one_asks_before_overwriting`, `a_symlink_is_overwritten_and_a_dangling_link_is_refused` |
| something else (socket, device, dangling link) | `Refused("That name is taken by something that isn't a file")` | `something_else_with_the_name_is_refused` |
| the folder is not writable | `Refused("You can't save in this folder")` | `a_folder_that_cannot_be_written_refuses_both_new_and_existing_files` |
| otherwise | `Save(folder/name)` | `a_missing_name_saves_in_the_folder` |

Names are used exactly as typed apart from the empty-name check:
`a_name_keeps_its_spaces_and_its_case`. Writability is asked once per shown
folder through GIO's `access::can-write` (`folder_writable`, `save.rs:306`);
the picker never asks on every keypress. The writable check comes before
`Overwrite` and `Other` deliberately: `/usr` refuses every save, not just a
new name. `Enter` still works there, because navigating is not saving.

`inspect` (`save.rs:287`) uses `symlink_metadata` first so a dangling link is
`Other`, then `is_dir` / `is_file` so a link to a file or folder follows its
target.

## Extension follows the filter

`filter::follow_extension(name, old, new)` (`portal/filter.rs:236`) finds the
new filter's first simple `*.ext` glob, then changes the name only if the old
filter has a glob containing a dot that matches the whole name ASCII
case-insensitively. A bare `*`, a MIME-only filter on either side, no extension,
or an extension the old filter does not list leaves the name alone.

Examples, all in `the_extension_follows_the_filter_only_when_the_old_one_claimed_it`:

- `Untitled.txt`, Text (`*.txt`) → Markdown (`*.md`, `*.markdown`) becomes
  `Untitled.md`;
- `notes` stays `notes`;
- `report.pdf` typed under Text stays `report.pdf`;
- a MIME-only target filter changes nothing;
- a bare `*` on either side changes nothing;
- `notes.TXT` matches `*.txt` and becomes `notes.md`;
- a new filter whose first glob is bracketed falls through to its first simple
  glob (`*.rst`).

The picker applies it in `Picker::filter_changed` (`picker.rs:1173`) before it
updates the rules, so the old filter is still the one being compared.

## SaveFiles naming

- `save_files_names` (`save.rs:115`) takes each `ay` field's last path
  component; one that is not a legal component returns its index. The host
  (`PickerHost::open`, `picker.rs:2223`) then logs one line naming the index and
  answers response 2 without opening a dialog.
- `clashing` (`save.rs:136`) reports a target that exists in the folder or a
  name that appeared earlier in the same answer.
- `resolve_names` (`save.rs:165`) preserves the sent order. Keep Both renames
  every clash with the first free `name (2).ext`; Replace keeps existing names
  and renames only duplicates within the list. `free_save_name` reserves every
  name already handed out, so one answer cannot name the same path twice.
- `save_files_summary` (`save.rs:235`) is the footer line: `Saving 3 files:
  a.txt, b.png, c.pdf`, then `and N more`.
- `collision_text` (`save.rs:257`) is the dialog body, with the count, the
  folder and up to three names.

Tests: `savefiles_names_take_the_last_component_and_reject_an_invalid_one`,
`clashes_are_a_folder_hit_or_an_earlier_duplicate`,
`keep_both_numbers_every_clash_and_replace_only_duplicates`,
`two_clashes_with_the_same_name_get_two_numbers`,
`numbers_keep_the_extension_and_skip_names_already_reserved`,
`the_footer_lists_three_names_and_then_a_count`,
`the_collision_text_names_the_count_the_folder_and_the_names`.

No files: `save_files_now` (`picker.rs:1615`) answers the chosen folder's own
URI, as `nautilus-portal.c:262` does.

## The window

`Picker::new` builds the same window for all three methods. SaveFile adds the
Name entry first in the footer, before the filter dropdown and the choices
(`picker.rs:646`); its initial text is `current_file`'s basename, else
`current_name`; it takes the spare width. `present` focuses it and selects the
stem with `save::stem_selection` (`save.rs:228`), the same `file_stem` rule as
`rename_prefill` (`fileops/dialogs.rs:188-199`): `Untitled.odt` selects 8,
`archive.tar.gz` 11, `.bashrc` all 7. SaveFiles builds a folders-only grid (via
`Mode::for_call`), no dropdown and a footer summary instead of the entry.

The Name entry is in the same "am I typing" check as the address and search
entries (`picker.rs:936`) and uses `fileops::keys::entry_focused`, never
`has_focus()`.

A click on a file changes the model selection; `follow_selection`
(`picker.rs:1495`) copies its name into the entry once. A folder click leaves
the entry alone, and the entry is always the answer. A double-click / Enter on a
file (grid or search result) takes that file's own folder and name and shows the
overwrite alert (`save_file_path`, `picker.rs:1343`).

The overwrite alert is an `adw::AlertDialog` (`confirm_overwrite`,
`picker.rs:1668`): `“name” already exists in “Folder”. Saving will replace its
contents.`, Replace destructive, Cancel the default and close response. The
SaveFiles alert (`confirm_collisions`, `picker.rs:1709`) offers Keep Both (the
default), Replace and Cancel.

Typing an absolute path or `~/` in the entry moves the dialog on Save: the
folder is `parent`, the entry keeps `file_name`, and nothing is saved. A
missing folder refuses with `That folder doesn't exist`; any other slash refuses
with `A name can't contain /` (`typed_path`, `picker.rs:1534`).

## New Folder

The header gets a New Folder button only in the two Save modes
(`picker.rs:520`). It opens a small popover with a name entry, Cancel and
Create, modelled on `fileops/dialogs.rs:219` and `open_name_form` (`:261`), but
without a `WindowUi` (`open_new_folder`, `picker.rs:1760`). Create dispatches
`file.new-folder` with `name` and `in` as `Caller::Ui` (`create_folder`,
`picker.rs:1821`), then polls briefly for the folder and navigates into it.

`picker_accepts` in `app.rs` gains exactly `file.new-folder` (`app.rs:5022`).
The allowlist test now permits that one `Effect::Files` command and still
refuses every other one, including `file.new-file` and `file.paste`; the
shortcut test asserts no key is bound to it. The comment above the function
says why it is the only exception: New Folder creates exactly the folder the
user asked for.

## API citations

- `adw::AlertDialog::connect_response` takes a response filter and
  `(dialog, &str)`: `libadwaita-0.9.2/src/auto/alert_dialog.rs:704`.
- `gio::FileInfo::boolean` is the getter for `access::can-write`:
  `gio-0.22.10/src/auto/file_info.rs:63-67`.
- `nautilus-portal.c:236-262` (SaveFiles results and the empty list's folder
  URI), `:485-520` (`current_file` / `current_name` / `current_folder` /
  `files`).
- `nautilus-file-chooser.c:183-343` (filename entry, overwrite confirmation,
  selection-follows-name) and `:411-510` (the suffix following the filter).
- `fm-core/src/fileops/mod.rs:312` (`is_valid_name`) and `:340-360`
  (`free_copy_name`, the shape of the collision naming).
- `fileops/dialogs.rs:188-199` (`rename_prefill`) and `:219`, `:261`
  (`open_name_form`).
- `docs/notes/P2am.md` and `P2an.md` for the picker's hosting, rules and
  search.

No new dependency and no `unsafe`.

## Tests and gates

`scripts/check.sh` passes with the 1.95.0 toolchain: fmt, clippy
(`-D warnings`), test, release build and `cargo deny check` all pass.

**[observed]** `cargo test --workspace`: **1,142 passed, 0 failed, 28 ignored**
across the suites. The ignored ones are the existing subprocess/GTK and
environment-gated tests (10 of them in `fm-ui`).

New tests outside the Save tables above:

- option parsing for `current_name`, `current_file`, `current_folder`, `files`,
  wrong types and per-method unknown keys;
- `starting_folder_for` with `current_file`;
- D-Bus SaveFile: success, one URI, no `writable`, the filter returned;
- D-Bus SaveFiles: every URI in order, no `writable`, no `current_filter`;
- SaveFile and SaveFiles: response 1 on Cancel, response 2 on Close (and the
  Request object is unexported), response 2 when the host cannot open.

I did **not** add an in-process widget regression that types a name and saves.
The existing widget harness (`app.rs`'s `gtk_test_lock` / child-process tests)
builds a main `WindowUi`; a picker is created by `Process::portal` from a
`DialogHost` call with its own application and request. Driving that path is a
larger harness than this brief needs, so Claude checks it live.

## Live checks for Claude

None was run here: Kyle's main Gimbal is running, and no pointer/keyboard
injection is allowed. The run script should use `gdbus`, a scratch config/state
under `/tmp/gimbal-*`, and the byte-string syntax from P2am
(`<@ay [0x2f, …, 0x00]>`).

1. SaveFile with `current_name` `Untitled.odt`: Name focused, `Untitled`
   selected. Type `report`, Enter; answer `…/report.odt`; nothing on disk.
2. SaveFile with `current_file` in a scratch folder: folder opens, file
   selected, name in the entry. Save asks to replace; Replace answers, Cancel
   returns with the entry focused.
3. Text (`*.txt`) and Markdown (`*.md`, `*.markdown`) with `notes.txt`: switch
   to Markdown → `notes.md`; type `notes.rst`, switch back → unchanged.
4. Save on an existing folder's name enters it. `~/<scratch>/sub/x.txt` moves
   to `sub` and leaves `x.txt`.
5. `/usr`: Save disabled with `You can't save in this folder`.
6. New Folder `exports`: the dialog opens `exports`; Save there answers
   `…/exports/<name>`; only the folder was created.
7. SaveFiles `a.txt`, `b.txt`, `a.txt` into a folder holding `b.txt`: the
   collision dialog appears. Keep Both answers `a.txt`, `b (2).txt`,
   `a (2).txt`; Replace answers `a.txt`, `b.txt`, `a (2).txt`.
8. Close on the handle answers **2** and closes the window; Escape answers 1;
   choices come back in the results. (The brief's line says 1, but Nautilus
   answers `RESPONSE_OTHER` for `Request.Close`
   (`nautilus-portal.c:295-307`) and P2am/P2an already pinned 2; changing it
   would also break item 10's "Open unchanged". I kept 2.)
9. Ctrl+A and Delete edit the Name text and touch no file; Ctrl+H still toggles
   hidden files.
10. P2an's Open run script passes unchanged.

## Claude's review (Nov 21)

Read the notes and the whole diff. The design matches the brief, and the pure halves are well
tested. Keeping Close at response 2 was right: Nautilus answers `RESPONSE_OTHER` there, and P2am
and P2an pinned it.

**A real-picker regression, written in review.** The agent stopped short of driving the real
picker. `portal::tests::real_save_dialog_answers_names_paths_and_collisions` (+ `_inner`, in a
child process with scratch `XDG_CONFIG_HOME` / `XDG_STATE_HOME`) does it:
- It builds the real `Process::portal`, the real `PickerHost`, and the service on the tests'
  private D-Bus server.
- It calls SaveFile and SaveFiles over that bus.
- It drives the dialog through test-only hooks (`picker::testing`): it sets the Name entry and
  emits its `activate`, picks a filter on the real dropdown, answers the real alert by its
  response id, calls `create_folder` and `follow_selection`, and reads the status labels.

It checks the brief's live items 1–8 against the D-Bus replies and the disk:
- the stem is selected, Enter answers, and nothing is written;
- `current_file`: Cancel returns to the dialog and Replace answers, with the file's contents
  untouched;
- the extension follows the filter, and a typed one stays;
- a folder name enters the folder; a typed absolute path moves and keeps its last part; "a/b"
  is refused;
- a clicked file fills the name;
- New Folder creates and opens the folder, and nothing else appears;
- `/usr` disables Save and says why;
- Close answers 2 and takes the window down;
- SaveFiles shows the summary; Keep Both answers `a.txt, b (2).txt, a (2).txt`, and Replace
  answers `a.txt, b.txt, a (2).txt`; nothing is written.

It passed 3/3 after the fix below.

**Fixed: a Save refusal was covered by the folder's counts.**
- Opening Save in `/usr` disabled the button but showed "Files: 0, Folders: 12": the listing
  finished after `show_status` and wrote its counts over the reason. A refusal typed right after
  a navigation was covered the same way.
- In the two Save modes, a refusal now turns off the grid's status gate
  (`FolderView::set_status_gate`, the switch split view already uses), and an allowed answer
  turns it back on. `refuse` does the same for Save.
- Open is unchanged since P2an.
- The test no longer waits for a listing before typing, so it proves the fix.

**Corrected: P2ao's `entry_focused` comment.** It said `FOCUS_WITHIN` holds whether or not the
window is active. GTK clears it when an active window deactivates and restores it on
reactivation (`gtk-4.22.4:gtk/gtkwindow.c:6153-6185`). The guard's behaviour is right, because
an inactive window gets no keys; only the comment was wrong. The test's `name_focused` hook asks
the window's focus widget instead, since a test window on Kyle's desktop may never be activated.

**A note for running these tests:** the real-GTK tests open windows on Kyle's screen. If Mango
focuses one while he types, his keys land in it. One early run caught a stray space: the Name
read " .txt". The picker has no file-changing keys, so it's harmless, but a run can fail for
that reason.

**Gates:** `scripts/check.sh` exit 0. 1143 passed, 0 failed, 29 ignored: the agent's 1142/28,
plus the outer and inner halves of the new regression.

**Left for Kyle** (`/tmp/gimbal-p2at/run.sh`): real keys in the Name entry (Ctrl+A and Delete
edit the text, Ctrl+H toggles hidden files), the look of the footer and the New Folder button,
the filter and choice values in a real reply, and an Open dialog still behaving as after P2an.

**Kyle's live run (Nov 21):** all passed: typing in the Name entry, Ctrl+H, the look, a real reply with filter and choice, and Open unchanged.
