# P2r: sort from the Settings menu, with per-folder overrides

## What I built

`[observed]` unless a line says otherwise.

- **One command, `view.clear-folder-sort`** (`fm-core`):
  - It is an ordinary `Effect::Settings` command with no arguments and no undo.
    `Caller::Ui`, `Hotkey`, `Cli` and `Agent` all reach it exactly as they reach
    `view.set-sort`; there is no new caller exception.
  - `change_for` maps it to `Change::ClearFolderSort { folder }` using the
    focused tab's folder, the same context `view.set-sort scope=folder` uses.
  - The clear matches paths after `~` expansion and lexical normalization, the
    same comparison `sort_for` uses. A rule stored as `~/Downloads` clears from
    the absolute path, and an absolute rule clears through `~/Downloads`.
  - If the folder has no rule, `ConfigFile::apply` returns `Ok` before touching
    the in-memory config, the parsed document or the disk. It is a true no-op.
  - With a rule, only its one `[[sort.folder]]` table is removed (`toml_edit`'s
    `ArrayOfTables::remove`, or `Array::remove` for the inline-array form). The
    rest of the document, including comments and the other folder rules, is
    written byte-for-byte. The test compares the entire file.
  - `Config::has_folder_sort` reads the same last matching entry that
    `sort_for` chooses, so the menu checkbox and the effective rule agree when
    a folder is listed more than once.
- **The Sort section in the Settings popover** (`fm-ui`):
  - It sits below the "Settings" heading and above "Allow local automation",
    under its own "Sort" heading, using the existing `gl-menu-item` class. No
    new size, colour or CSS class was needed.
  - Rows: Name, Date modified, Size, Type; then the order labels for the chosen
    key (A → Z / Z → A, Oldest first / Newest first, Smallest first / Largest
    first); then "Only for <folder>".
  - Current rows get a `✓` prefix. The rule is never stored in the widget:
    every refresh reads `Process::config` and calls `sort_for` for the focused
    folder.
  - Choosing a key keeps the shown order and choosing an order keeps the shown
    key. Both dispatch `view.set-sort` as `Caller::Ui`, with
    `scope=folder` when the check is set and `scope=global` otherwise.
  - Checking the folder row dispatches `view.set-sort scope=folder` with the
    rule currently shown. Unchecking dispatches `view.clear-folder-sort`.
  - The Sort rows do not call `take_down`; the popover stays open, and each
    click refreshes the labels, checks and sensitivity from the saved config.
  - Trash and search views report no folder: the rows show the global rule and
    the folder check is insensitive, with a tooltip explaining why. A folder
    row for a normal view names that folder (for example "Only for Downloads").
  - Every GTK call happens after the config borrow is dropped. Click closures
    hold a weak reference to the row widgets, so the menu's widget graph has no
    `Rc` cycle back to its buttons.
- **Re-sorting**:
  - `Change::ClearFolderSort` uses the same `WindowUi::sort_changed` /
    `Content::refresh_sort` path as `SetSort`.
  - The existing `Content::refresh_sort` already does the right thing and was
    not changed: a folder change touches every tile showing that folder; a
    global change touches all tiles, and each tile then reads its own
    `sort_for` rule. Tiles with no displayed folder are skipped.
- **Logging**: every applied `SetSort`/`ClearFolderSort` logs one
  `[gimbal ...] sort: scope ... folder ... rule ...` line. The clear line names
  the global rule the folder fell back to.
- `docs/agent-socket.md` lists the new command and updates the sample
  `commands` total from 53 to 54.

Choices where the brief leaves room:

- The order labels name the key's direction: Name and Type use A → Z / Z → A,
  Modified uses Oldest/Newest first, Size uses Smallest/Largest first.
- The empty-folder label stays "Only this folder" (with the explanation in the
  tooltip), because there is no folder name to append.
- A clear that finds no rule does not re-sort the grid. Nothing changed, so
  there is nothing to re-sort and no log line.
- If duplicate entries name the same folder, the clear removes the last one,
  the effective entry `sort_for` uses. `SetSort` already updates that same
  entry.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the rustup 1.95.0 toolchain on `PATH`; exit 0.

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile ... in 3.50s
==> cargo test --workspace
    fm-agent: 23 unit + 16 socket passed, 0 ignored
    fm-cli:   12 passed, 0 ignored
    fm-core:  359 passed, 1 ignored; trash 6 passed, 6 ignored;
              trash_browser 11 passed, 11 ignored
    fm-ui:    378 passed, 5 ignored
    total:    805 passed, 23 ignored
==> cargo build --workspace --release
    Finished `release` profile ... in 6.13s
==> cargo deny check
    advisories ok, bans ok, licenses ok, sources ok
    all checks passed
```

The only deny output before "all checks passed" is the pre-existing
`system-deps 7.0.8` / `9.0.1` duplicate warning that P2a, P2b and P2c also
reported. No dependency, manifest, session schema or socket protocol changed.

New tests:

- `fm-core` catalogue: `view.clear-folder-sort` is a zero-argument
  `Effect::Settings` command accepted for Ui, Hotkey, Cli and Agent, like
  `view.set-sort`.
- `fm-core` config: `every_settings_command_has_a_change_for` now includes the
  new command; clear with no rule writes nothing; both path forms clear each
  other; the full commented file stays byte-for-byte equal; reloaded
  `sort_for` returns the global rule after a clear.
- `fm-ui` header: the Downloads example (global name ascending, Downloads
  modified descending); another folder shows the global rule and checking
  creates its override; no folder shows the global rule with an insensitive
  check; the check marks and order words.
- `fm-ui` split (in `app.rs`, the file that owns the new `Change` hook): a real
  GTK subprocess builds a split view with two Downloads tiles and one Documents
  tile, dispatches the actual `view.set-sort` command, and reads each tile's
  listed names. A folder change re-sorts both Downloads tiles and leaves
  Documents alone; a global change touches every tile while Downloads keeps its
  override and Documents follows the new global rule. It then dispatches
  `view.clear-folder-sort` and checks that both Downloads tiles fall back to
  the global rule while Documents stays put. This runs through `dispatch` →
  `settings_change` → `after_change` → `sort_changed` → `refresh_sort`, not a
  copied decision.

## API citations

Read in the pinned crate source under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`:

- `toml_edit 0.25.17`: `ArrayOfTables::remove` at
  `src/array_of_tables.rs:130`, with `len` at `:67` and `get` at `:82`;
  `Array::remove` at `src/array.rs:305`. `[read]`
- `gtk4 0.11.5`: `WidgetExt::set_sensitive` at `src/auto/widget.rs:1375` and
  `WidgetExt::set_tooltip_text` at `src/auto/widget.rs:1412`. `[read]`

The GTK popover, button, label and `gl-menu-item` patterns are P2h's existing
`open_settings_menu`, `menu_item` and `close_on_closed` code. No new GTK API was
needed beyond the two setters above. `[read]`

## What needs a live session

I did not launch the app: `pgrep -a gimbal-files` showed Kyle's instance running
as PID 122626, and CLAUDE.md says a plain launch would hand arguments to his
window. The following are for Claude and Kyle:

1. Settings → Sort shows Name, A → Z (Ascending), and "Only this folder" is not
   checked on a fresh config.
2. In Downloads, check "Only this folder", choose Date modified and Descending:
   newest file first. Go to another folder: still by name. Back in Downloads:
   still newest first.
3. Change the global sort to Size from another folder; Downloads keeps its own
   rule.
4. Restart Gimbal: both rules survive, and
   `~/.config/gimbal-files/config.toml` has one `[[sort.folder]]` entry for
   Downloads and untouched comments.
5. Uncheck "Only this folder" in Downloads: it re-sorts to the global rule at
   once and the entry is gone from the file.
6. Split view with Downloads in one tile and another folder in the other: each
   follows its own rule, and changing the focused tile's sort leaves the other
   alone.

Also worth checking live:

- The Settings popover stays open across a key click and then an order click,
  and the check marks move without reopening it.
- In Trash or search results, the folder row is insensitive and its tooltip
  explains that there is no folder.
- With `GIMBAL_LOG=1`, one `sort:` line appears per applied change.

## Anything outside my files

Nothing is required outside the brief's Output list. Two observations for a
later brief:

- The Settings popover's rows only refresh on their own clicks. If a hotkey
  changes the sort while the popover is open, the rows keep their old labels
  until reopened. The brief did not ask for config-change notifications into an
  open popover.
- `fm-core` still keeps `Config::current_folder` out of the config; `change_for`
  receives the focused folder from the UI, so P2r did not reintroduce UI state
  into the config struct.

The split view needed no production change: `Content::refresh_sort` already
re-sorts every tile on the named folder and lets each tile read its own
`sort_for` on a global change. The required split regression therefore lives in
`crates/fm-ui/src/app.rs` (the allowed re-sort-hook file), not in `split.rs`.

The catalogue's existing order and caller tests live in
`crates/fm-core/src/command/tests.rs`, so that test file gained the new name
and the caller coverage. Everything else stayed within the brief's Output list.
`spike/` was read only. `[observed]`

## Review (Claude, Nov 4, 2026)

**Code review passes. Live checks pending (Kyle).**

- Gates: `scripts/check.sh` exit 0 on the combined P2r + P2s tree, 805 passed / 23 ignored.
  [observed]
- The menu's decisions are pure functions (`sort_menu_state`, `sort_key_action`,
  `sort_order_action`, `sort_folder_toggle_action`) that always re-read the config; every click is a
  `Caller::Ui` dispatch. [read]
- `ClearFolderSort` removes the last matching entry in both the in-memory list and the document,
  each matched by normalised path, so the two can't drift. `SpecialFolders` only feeds default pins,
  so `~`/absolute is the full set of path forms. [read]
- The split regression goes through the real dispatch → `refresh_sort` path. [read]
- Nit for Kyle's look check: unchecked rows are padded with three spaces to line up with "✓ ".
  That aligns only in some fonts. If it looks ragged, a fixed-width check slot (an icon or an empty
  label of the token size) is the fix.
- Agreed follow-up: an open Settings popover doesn't refresh when a hotkey changes the sort.

## Fix after Kyle's live check (Claude, Nov 4, 2026)

- **Symptom:** picking a new key re-sorted the files, but the order rows kept the old key's text
  ("A → Z" after choosing Date modified) until the menu was reopened. [observed, Kyle]
- **Cause:** `open_settings_menu` built `SortRows` in an `Rc` that was a local. Each click handler
  held only a `Weak`, so after the function returned `upgrade()` always failed and `refresh` never
  ran on a click. The ✓ marks were stale for the same reason. [read]
- **Fix:** the header owns the open menu's rows (`Header::sort_rows`), and the popover's `closed`
  handler clears the slot. `WindowUi::sort_changed`, which every window runs after a
  `SetSort`/`ClearFolderSort`, now calls `Header::refresh_sort_menu`. That also closes the review
  item about an open menu not following a hotkey sort change. Gates: `scripts/check.sh` exit 0.
  Not yet re-checked live.
