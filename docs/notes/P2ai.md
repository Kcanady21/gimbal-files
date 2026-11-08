# P2ai: create an empty file from the folder menu

## What I built

- **New File…** in the empty-space menu, directly after **New Folder…**. It
  opens the same name form through `MenuAction::NewFile { folder }`. The form
  keeps the menu's original click rectangle with `popover_at`, opens from the
  P2af close-then-idle hand-off, focuses the entry, and supports Enter,
  **Create** and **Cancel**. The folder captured when the menu opened is used
  as `in`, so a later focus change cannot move the file.
- `fm_core::fileops`:
  - `Request::NewFile { name, in_folder, scope }` and `Action::NewFile`;
  - `plan_new_file`, using `is_valid_name` for one component, the existing
    `PathFacts` destination check, and `path_exists` for collisions;
  - exclusive execution through
    `gfile(path, scope).create(gio::FileCreateFlags::NONE, ...)`, followed by
    `close`, with no write;
  - `EXISTS` during execution becomes an ordinary `Refused("the name is
    taken")`, so a file, directory or symlink that appears after planning is
    never opened, truncated or followed;
  - create and close errors use the normal `Failed` item path. A close failure
    says the file may already exist and leaves the path alone.
- `file.new-file` in the command catalogue, with the same `name` and
  current-folder `in` conventions as `file.new-folder`. It is a menu-only
  `Effect::Files` command: `UI_ONLY_MENU_COMMANDS` in the registry permits only
  `Caller::Ui`, so Hotkey, CLI and Agent are refused before a handler or dry
  run. Its `undo` flag is false, and creation adds no undo record, so the
  previous undo entry stays available.
- Search and Trash now list `file.new-file` in their existing file-action
  refusals.
- `fm-agent::protect` checks a new-file target like a new-folder target.
- `docs/agent-socket.md` lists `file.new-file` as unavailable and documents the
  UI-only rule and new-file target protection.

No new dependency, no new `unsafe`, and no session or config schema change.

There is no `phase2/RULES.md`; I read `phase1/RULES.md`, the repository's
`CLAUDE.md` (named as its replacement by `phase2/README.md`) and the whole P2ai
brief. **[read]**

## Files written

- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/mod.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-core/src/fileops/mod.rs`
- `crates/fm-core/src/fileops/tests.rs`
- `crates/fm-ui/src/fileops/dialogs.rs`
- `crates/fm-ui/src/fileops/menu.rs`
- `crates/fm-ui/src/fileops/mod.rs`
- `crates/fm-ui/src/fileops/progress.rs`
- `crates/fm-ui/src/fileops/worker.rs`
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/search/mod.rs`
- `crates/fm-ui/src/search/tests.rs`
- `crates/fm-ui/src/trash.rs`
- `crates/fm-agent/src/protect.rs`
- `docs/agent-socket.md`
- `docs/notes/P2ai.md` (this file)

No file outside the brief's Output list changed. **[read]**

## Behaviour

`plan_new_file` reports `InvalidName` for an empty name, `.`, `..`, a slash or
a NUL; `MissingDestination` when `in` is missing or not a folder; and
`NameTaken` when anything already exists at the target. The dry run is
`Plan::lines` and writes nothing. **[read]**

Execution repeats the plan's checks, then creates exclusively. GIO's
`FileCreateFlags::NONE` is `g_file_create`, which fails with `EXISTS` instead of
truncating like `g_file_replace`; the error becomes `ItemOutcome::Refused` with
the ordinary conflict text. A close error becomes a `Failed` item whose text
says the file may already exist and was left alone. **[read]** **[inferred]**
for the exact GIO backend behaviour on a dangling symlink; the exclusive
`create` flags and the collision tests cover it.

The normal summary is `Created file “name”`; the existing folder monitor and
`fileop_finished` path handle the visible folder update. **[read]**

## API citations

- `gio-0.22.10:src/auto/file.rs:296` — `FileExt::create`.
- `gio-0.22.10:src/auto/flags.rs:1845` — `FileCreateFlags`; `NONE` is on the
  next line.
- `gio-0.22.10:src/auto/output_stream.rs:31` — `OutputStreamExt::close`.
- `glib-0.22.10:src/error.rs:90` — `glib::Error::matches`, used to recognise
  `gio::IOErrorEnum::Exists`.
- `gtk4-0.11.5:src/auto/popover.rs:514` — `Popover::set_pointing_to`, already
  used by the P2af `popover_at`.
- `gio-0.22.10:src/auto/file.rs:1756` — `File::query_info`, used by the existing
  `PathFacts` checks for administrator paths.

## Tests

Added or extended tests cover:

- the catalogue row, its `name`/`in` arguments, the menu-only caller rule, and
  refusal of Hotkey, CLI and Agent;
- menu order (New File directly after New Folder) and the folder binding;
- zero-byte creation, a dry run that writes nothing, invalid names, a missing
  destination, and collisions with a regular file, a directory and a dangling
  symlink;
- a collision introduced between planning and execution, preserving the
  existing contents;
- the previous undo entry remaining available after creation;
- search and Trash refusals;
- a worker-level plan/execute race refusal;
- the New File… → Create GTK subprocess regression, checking the click
  rectangle, the form closing, and a zero-byte file on disk.

## Verification

`scripts/check.sh` exited **0**. The five gates are `cargo fmt --all --check`,
clippy with `-D warnings`, the workspace tests, the release build and
`cargo deny check`. The tail of the log (`/tmp/gimbal-p2ai-check.log`) is:

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test totals: **999 passed, 25 ignored, 0 failed** — 23 + 16 + 13 + 492 + 6 + 11
+ 438 passing, with 1 + 6 + 11 + 7 ignored. P2af's reviewed baseline was 976
passed / 24 ignored, so this change adds 8 passing tests and one ignored
subprocess-only GTK test. **[observed]**

No live pointer or keyboard interaction was possible for this agent, so the
menu clicks and the written file are Claude and Kyle's checks. **[read]**

## Live checks for Claude and Kyle

Use scratch folders, for example:

```bash
SCRATCH=/tmp/gimbal-p2ai-live
mkdir -p "$SCRATCH/config/gimbal-files" "$SCRATCH/state" "$SCRATCH/data" "$SCRATCH/home"
XDG_CONFIG_HOME="$SCRATCH/config" XDG_STATE_HOME="$SCRATCH/state" \
XDG_DATA_HOME="$SCRATCH/data" HOME="$SCRATCH/home" \
  ./target/debug/gimbal-files "$SCRATCH/home"
```

1. Right-click empty space in an ordinary tab and create `empty.txt`. Confirm it
   appears, opens as an empty file and has size zero.
2. Try the same name again. Confirm the refusal, then Cancel a second form and
   check that clicking the grid still works.
3. Create a file from a non-focused split tile and check it lands in that
   tile's folder. Re-check **New Folder…** from the same menu.

## Outside the Output list

- No file outside the brief's Output list changed.
- `docs/agent-socket.md` already describes the file-request protection list; I
  added new-file targets and the UI-only command line there.
- The command is not offered by any hotkey. It is deliberately absent from
  `default_hotkeys`; the registry would refuse a Hotkey caller anyway.

## Known limits

- The New File form uses the existing simple name entry. It does not expand
  `~`, create parent folders, or accept a path with a slash; those are the same
  rules as New Folder.
- Creation is not itself undoable; that is intentional so the previous undo
  entry stays available. A created file can be deleted through the ordinary
  Trash command.
- The GTK regression checks the menu, form and created file, not the full
  visual grid refresh; the live checks above cover that.

## ChatGPT's review (Nov 8, 2026)

- **[read]** The menu-only command uses the existing worker. The exclusive
  create covers races after planning and leaves the previous Undo entry
  available. The shared name form now refuses a destination path that the
  command cannot represent in UTF-8. Lossy conversion could otherwise
  target a different folder.
- **[observed]** The GTK subprocess activated **New File…** and **Create**,
  then checked a zero-byte file on disk. The worker regression refused a
  file that appeared after planning. The combined gates all exited 0 with
  999 passed, 25 ignored and 0 failed.
- **[observed]** Kyle confirmed the scratch desktop re-check passed for
  creating an empty file, preserving an existing file on a name collision
  and creating a file in the non-focused split tile's folder.
