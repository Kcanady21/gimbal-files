# P2aj: a setting to show hidden files

Tags: **[observed]** for a command or its output, **[read]** for source, and
**[inferred]** for anything that needs a real key, pointer or live feel.

## What I built

- **Config (`fm-core/src/config.rs`).** A new `[view]` section with one key,
  `show_hidden = true|false`, default `false`. It parses exactly like
  `[integration]`: `KNOWN_TOP_LEVEL` gains `view`; unknown keys, a wrong shape,
  a wrong type or a nested table inside `[view]` are reported and leave the
  default Off; a save writes just the one key with `set_root_value`, so the
  `toml_edit` document stays byte-preserving (comments, order and unrelated
  settings survive). `[view]` is layered under P2ac's system defaults file
  beside `[integration]`, `[updates]` and `[agent]`, so GimbalOS could ship
  `show_hidden = true` and the user's own file still wins. `[agent enabled]`
  keeps its unique "never from the defaults file" rule; nothing of that sort
  applies to `show_hidden`.
- **Commands (`command/catalogue.rs`).**
  - `view.set-hidden enabled=bool`, `Effect::Settings`, saved to the config
    like `view.set-sort` (the one `change_for` → `ConfigFile::apply` path);
  - `view.toggle-hidden` with no arguments: `change_for` flips the *saved*
    value (`!config.view.show_hidden`), so the hotkey, the menu entry, the
    Settings switch, the CLI and the agent all agree on one bit.
- **Applying it live (`fm-ui/src/folder/view.rs`).** Every folder view keeps
  its own `Rc<Cell<bool>>` (`show_hidden`), seeded from the config in
  `FolderView::new` (`Process::show_hidden`), and the `gtk::CustomFilter` now
  decides through the pure `folder::entry_visible(name, show_hidden)`. A
  change calls `filter.changed(LessStrict)` when names are admitted and
  `MoreStrict` when they are dropped — `FolderView::set_show_hidden`. There is
  no re-listing and no folder reload, and the first visible name is kept as a
  scroll anchor, as a sort change already does.
- **Dimmed hidden items (`look/css.rs`).** A shown hidden item's cell carries
  `hidden` next to `gl-cell`; the look module's new rule
  `.gl-cell.hidden { opacity: 0.55; }` dims it. **The token is
  `HIDDEN_OPACITY`** (`css.rs`, `0.55`). No colour, no fixed size: an opacity
  is the one value the brief asked for and the rule uses no theme colour. The
  class is added on `bind` (every shown hidden name, visible while the
  setting admits it) and removed on `unbind` with the recycled-cell clean-up.
- **Safe hiding (`fm-core/src/workspace/mod.rs`).**
  `Workspace::selection_drop_hidden(is_hidden)` drops every selected name the
  predicate marks from every tab of every window, clears a focused or anchor
  name that leaves with them, and returns the dropped count and one
  `SelectionChanged` per window whose selection actually changed. The predicate
  keeps the rule in the UI (`folder::is_hidden_name`): `fm-core` never names
  the dot rule. `Process::apply_show_hidden` runs it *before* the filters
  change, so no view ever holds an invisible selection: Delete, Cut and Move
  read the model, which by then holds only visible names. The status line and
  the preview follow the remaining selection (`apply_events` plus
  `sync_preview_user`), and one log line carries the count:
  `hidden: hiding hidden files deselected N invisible item(s)`.
- **Where to toggle it.**
  - **Ctrl+H**: `("<Control>h", "view.toggle-hidden")` in `default_hotkeys`;
    the key was free. It follows the existing hotkey rules: blocked by an open
    dialog, and not an entry editing key, so it works while the path bar or
    search field has focus (like F5 and `<Alt>Return` do). **[read]**
  - **Settings**: a **View** section in `open_settings_menu` with a
    **Show hidden files** switch row in the existing switch style (label,
    accessible name and description, `syncing` guard, dim explanation line).
    It dispatches `view.set-hidden`; `Header::refresh_show_hidden_switch`
    syncs the open row whenever another window, the hotkey, the menu, the CLI
    or the agent flips the bit — the same shape as the Automation, Updates and
    Integration rows.
  - **The empty-space menu**: a **Show Hidden Files** / **Hide Hidden Files**
    entry last in the Pin / Refresh / Disk Usage / Properties section, bound
    to `view.toggle-hidden` with no arguments. **I chose the two-state label**,
    as the brief allowed: `MenuItem` (`fileops/menu.rs`) carries label, command,
    enabled, icon, tooltip and separator only — no check state **[read]** — and
    the label avoids the check widget and the `dialogs.rs` change.
- **Counts.** Confirmed, no change needed: `count_listed` counts
  `self.selection.n_items()`, which is the count *after* the filter and the
  sort in the model chain, so `Files: N, Folders: M` always described what is
  shown, and hidden items count exactly while they are visible. `finished()`
  already logged the distinction as `N items (M visible)`. **[read]**
- **Reach to every view (`app.rs`).** The brief's Output list does not include
  `content.rs`, `split.rs` or `frame.rs`, and none of them lets a caller walk
  to the live `FolderView`s. So `Process` keeps the grids itself:
  `FolderView::new` registers a `Weak<FolderView>` with the process (dead ones
  are pruned on every pass), and `Process::apply_show_hidden` updates the
  model, every live grid and every open Settings switch. A new window, tab or
  split tile seeds its view from the config, so late views join already
  applied. This is exactly how the existing `scale` and `sort` broadcasts work
  — one config bit, fanned out to every window — with the registry replacing
  a widget-tree walk I was not meant to add accessors for.
- **Search is unchanged.** The recursive search already includes hidden
  entries and says so in its scope line
  (`crates/fm-ui/src/search/mod.rs:1677`); the brief keeps that, and the
  search page's own list is not the folder filter. **[read]**
- **README.** One bullet under "Where your settings live".

No new dependency, no session schema change, no file outside the brief's
Output list changed.

## Which caller rule applies (the brief asks me to say)

Both commands are ordinary `Effect::Settings` `view.*` entries. They follow
the existing rule for that family (`command/mod.rs::caller_allowed`): the two
exceptions (`UI_ONLY_SETTINGS_COMMANDS`, `UI_ONLY_MENU_COMMANDS`) and the
preview-commands rule do not name them, so **all four callers — Ui, Hotkey,
Cli and Agent — may run them**, exactly like `view.set-sort` and
`view.toggle-sidebar`. `crates/fm-ui/src/agent/mod.rs` refuses only
`Effect::System` before dispatch (`:909`, `:1152` — the System checks), so the
socket reaches both like it reaches the other view commands. The brief's rule
holds: a caller that can already re-sort or hide a sidebar changes which names
are listed, and touches no file. `docs/agent-socket.md` needed no line: it
lists the commands that are *unavailable* through the socket, and these are
available. **[read]**

## The GTK facts behind the live filter

- `gtk4-0.11.5:src/custom_filter.rs:11` — `CustomFilter::new` takes a
  `Fn(&glib::Object) -> bool + 'static`; the closure holds the view's
  `Rc<std::cell::Cell<bool>>` clone, so the filter reads the current value on
  every decision with no re-wiring. **[read]**
- `gtk4-0.11.5:src/auto/filter.rs:29` — `FilterExt::changed(change)`;
  `gtk4-0.11.5:src/auto/filter_list_model.rs:62` — `filter()`, the getter the
  view uses to reach its own filter. **[read]**
- `gtk4-0.11.5:src/auto/enums.rs:4719` — the pinned enum's names are
  `Different`, `LessStrict`, `MoreStrict` (the brief's `MoreLoose`/`MoreStrict`
  are `GTK_FILTER_CHANGE_LESS_STRICT` / `GTK_FILTER_CHANGE_MORE_STRICT` at
  this version, aliases at `:4722` and `:4724`). **[read]**
- `gtk-4.22.4:gtk/gtkfilterlistmodel.c:803` — the filter's `changed` signal
  runs `gtk_filter_list_model_refilter`. `:753-795` — `LESS_STRICT` keeps the
  current matches and re-checks the items filtered out (only the hidden names
  re-enter), `MORE_STRICT` re-checks the current matches (only the hidden
  names leave). **[read]**
- `gtk-4.22.4:gtk/gtkfilterlistmodel.c:475-481` and `:860-862` —
  `GtkFilterListModel:incremental` defaults **false** and a non-incremental
  model filters to completion synchronously inside `changed`; the view never
  turns it on (the `set_incremental(false)` at `view.rs:224` is the
  *SortListModel*). So the rebuild, selection re-assert, counts and anchor
  scroll that `set_show_hidden` runs right after `changed` see the final
  model. `gtk-4.22.4:gtk/gtkcustomfilter.c:62` — a `CustomFilter`'s
  strictness is always `GTK_FILTER_MATCH_SOME`, so the synchronous
  `MATCH_ALL`/`MATCH_NONE` shortcuts are not the path taken. **[read]**
- After the change, `apply_selection` re-asserts the model's selection:
  GtkMultiSelection shifts selected positions under `items-changed` on its
  own, but the model is the truth (P2i's rule), so the view pins it again.
  **[read] [inferred]** for what that feels like under the pointer.

## The 3003adf stall check (the brief asks me to confirm)

Confirmed: it cares about nothing the filter does. **[read]**
`schedule_stall_check` (`view.rs:800-852`) collects names from its own
`gio::FileMonitor`, then compares them against `view.dir_list` — GTK's
**unfiltered** `DirectoryList` (`n_items`/`item` on `dir_list`, never
`filtered`/`sorted`/`selection`). `set_show_hidden` changes no `dir_list`,
its attributes or its file, and never calls `fresh_dir_list`; `reload` and
`set_folder` keep using the same filter through the replacement list
(`fresh_dir_list` wires `filtered.set_model`, and the filter's own cell
survives). So a rename while hidden files are shown, then hidden, is checked
against the unfiltered listing exactly as before. The live re-check (Done
item 7) is Kyle's.

## Tests

- `config.rs`: `[view] show_hidden` default and both reads; the invalid-
  section matrix in every key order (unknown key first/last, wrong shape,
  array shape, wrong type, nested, unparseable); one-key byte-preserving save
  with comments and `[updates]` intact, then back to `false`; `change_for` for
  `view.set-hidden` both ways and `view.toggle-hidden` flipping the saved
  value from both states; layering under a defaults file (default applies,
  user `false` wins, bad default reported, save touches only the user's file);
  `every_default_matches_the_brief` now expects 20 hotkeys with
  `<Control>h → view.toggle-hidden` and `view.show_hidden = false`;
  `every_settings_command_has_a_change_for` gained both commands.
- `catalogue.rs`: both entries' effect, argument shape and summaries.
- `command/tests.rs`: the catalogue order test gained both names; a new test
  runs both commands through all four callers (`Effect::Settings`), checks the
  typed `Value::Bool`, the missing-argument error and the non-bool rejection.
- `workspace/tests.rs`: a mixed selection in every tab of every window loses
  only its hidden names — the hidden focused/anchor names clear, a tab and a
  whole window with nothing hidden change nothing and get no event; a fully
  hidden selection leaves a clean tab; `check_invariants` passes.
- `folder/mod.rs`: `dot_names_are_hidden` kept as it was; the new
  `entry_visible` test covers `(name, show_hidden) → visible` for visible,
  dotted-file and dotted-folder names in both states.
- `folder/view.rs` through `app.rs`: a headless bridge test dispatches
  `view.toggle-hidden` twice through the real registry and handler — the
  config file gains `[view] show_hidden = false` and both tabs keep only
  `visible.txt` in the model selection; the log line
  `hiding hidden files deselected 2 invisible item(s)` appeared. **[observed]**
- `fileops/menu.rs`: the background-menu shape now ends the folder section
  with `Show Hidden Files` before the separator; a new test checks the label
  states the setting, the entry binds the no-argument `view.toggle-hidden`,
  and it sits last in its section in both states.
- `look/css.rs`: `.gl-cell.hidden` carries exactly
  `opacity: 0.55` from `HIDDEN_OPACITY` in both stylesheets, with no literal
  colour.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the 1.95.0 toolchain on `PATH`. The final run's exit
status is **0**. **[observed]**

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
running 23 tests
test result: ok. 23 passed; 0 failed; 0 ignored
running 16 tests
test result: ok. 16 passed; 0 failed; 0 ignored
running 13 tests
test result: ok. 13 passed; 0 failed; 0 ignored
running 502 tests
test result: ok. 501 passed; 0 failed; 1 ignored
running 12 tests
test result: ok. 6 passed; 0 failed; 6 ignored
running 22 tests
test result: ok. 11 passed; 0 failed; 11 ignored
running 449 tests
test result: ok. 442 passed; 0 failed; 7 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **1012 passed, 25 ignored, 0 failed** — agent 23, agent socket 16,
CLI 13, core 501 (+1 ignored), trash 6 (+6), trash browser 11 (+11), UI 442
(+7). Against P2ah's reviewed 999 passed / 25 ignored, P2aj adds 13 passing
tests: 5 config, 1 catalogue, 1 command, 2 workspace, 1 `entry_visible`, 1
css rule, 1 menu shape, and 1 headless bridge test. (One earlier green run
of the same script showed the same rows with the UI line at 441; the bridge
test came after it.) **[observed]**

The new UI tests are pure or headless: `entry_visible`, the css rule, the
menu shapes, and the bridge test. Nothing new needs a display, and
`cargo test` ran without one. I did not launch the app: `pgrep -a
gimbal-files` found Kyle's instance, and CLAUDE.md says not to launch then.

## Live checks (Claude and Kyle, scratch XDG dirs)

A scratch folder with `.hidden-file`, `.hidden-folder/` and `visible.txt`,
debug build, `XDG_CONFIG_HOME`/`XDG_STATE_HOME` under `/tmp/gimbal-*`:

1. By default only `visible.txt` shows and the status line says 1 file.
2. Ctrl+H shows both hidden items, dimmed, counts now 2 files 1 folder;
   Ctrl+H again hides them.
3. The Settings switch and the empty-space entry toggle the same bit; all
   three stay in sync, including across two windows and with the menu entry's
   label flipping between **Show Hidden Files** and **Hide Hidden Files**.
4. Split view of the folder in two tiles: one toggle updates both tiles; a
   tile scrolled away from the top keeps its place.
5. Select `.hidden-file` and `visible.txt`, then hide: only `visible.txt`
   stays selected — the status count, the preview and the log line agree, and
   Delete would act on it alone. Check without deleting.
6. Restart: the setting and the dimmed state are remembered; the session's
   selection contains no hidden name.
7. Rename `visible.txt` while hidden files are shown, then hide. The 3003adf
   stall recovery still keeps the grid true (no stale row).
8. The dimmed look in light and dark, files and folders, selected and not,
   including a rubber-band highlight over a dimmed cell.
9. With hidden names shown, Ctrl+H while the path bar or the search entry has
   focus still toggles (it is not an editing key); a non-UTF-8 name like
   the bytes `.` + `é` dims and hides with the rest.
10. A CLI/agent `view.toggle-hidden` flips the grids, the switches and the
    menu label everywhere.

## Not verified live (Claude checks these)

- Everything in the list above: I had no display and Kyle's instance was
  running. The filter's live feel (batch removals on a huge folder, the
  anchor's position mid-change), the dim level's legibility in both themes,
  the Settings row and section heading's look, the menu label's feel, and the
  Ctrl+H feel while typing all need real keys and eyes. **[inferred]**

## Questions for Kyle (the brief asks)

- GNOME's per-folder `.hidden` file lists and backup files ending in `~`
  stay out of scope, as the brief says — do you want either as its own
  setting later?

## Known limits

- A hidden name selected *while shown* leaves `focused = None` when the
  setting hides it (the model does not guess a replacement focus), matching
  how a Ctrl-drag that removes the focused hit behaves.
- Only the app's own toggles clean the selection. Manually editing
  `show_hidden = false` in the file *between* sessions can leave a stored
  session selection naming a now-invisible item until the next click; the
  in-session paths all deselect.
- The setting covers the folder grid. The Trash browser and the search page
  keep their own lists and policies (search already includes hidden entries
  and says so).

## Anything outside my files

Nothing changed outside the brief's Output list. Two notes for review:

- `Process` now holds the live `FolderView` weak registry (`app.rs`,
  registered from `FolderView::new`). I chose it over adding walk methods to
  `content.rs`, `split.rs` and `frame.rs`, which the brief does not list. If
  the next brief wants a different broadcast (per-window theme or density
  changes), the registry is where it plugs in, and one `Vec<Weak<FolderView>>`
  prunes itself on every pass.
- The `HIDDEN_OPACITY` token lives in `look/css.rs`, not `look/tokens.rs`,
  because the brief scoped the look change to `css.rs` ("only the dimmed
  class"). If Kyle prefers every numeric token in `tokens.rs`, it is a
  one-line move.

## Claude's review (Nov 16, 2026)

The return matches the brief. Select All draws on the visible names only, the
3003adf stall check still compares against GTK's unfiltered list, and both
out-of-list notes (the `FolderView` weak registry in `Process`, and
`HIDDEN_OPACITY` in `css.rs`) are fine as they are. **[read]**

### Fix

1. **A restored selection could name an invisible item
   (`crates/fm-ui/src/app.rs`).** The second "Known limits" item above: quit
   with a hidden file selected while hidden files were shown, and start with
   the setting off (a system defaults file or a hand edit), and the restored
   session kept the now-invisible item selected, so Delete would act on it.
   `start()` now calls `drop_restored_hidden_selection()` right after
   `restore_session()`. When hidden files are off it runs the same
   `selection_drop_hidden` the toggles use, applies the events, follows the
   preview, logs the count and schedules a session save. Test
   `a_restored_selection_loses_hidden_names_when_hidden_files_are_off` covers
   both the off case (deselected) and the on case (kept). **[observed]**

### Gates

`scripts/check.sh`: exit 0; 1013 passed, 25 ignored (the agent's 1012 plus
the new test). **[observed]**

### Live checks (Kyle, Nov 16, scratch XDG dirs, `/tmp/gimbal-p2aj`)

All passed: default hides the dot items and the count says 1 file; Ctrl+H,
the Settings switch and the empty-space menu entry flip one setting and stay
in sync; both split tiles follow one toggle; hiding with `.hidden-file` and
`visible.txt` selected leaves only `visible.txt` selected; the setting
survives a restart (the scratch config holds `[view] show_hidden = true`);
the dimmed look reads in light and dark. The scratch log has no panic, GTK
warning or crash record (one unrelated Vulkan loader message). **[observed]**

### Open for later

Kyle has not asked for GNOME `.hidden` lists or `~` backup files to be hidden;
both stay out of scope.
