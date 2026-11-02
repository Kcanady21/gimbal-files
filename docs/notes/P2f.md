# P2f: file operations

## What I built

- `crates/fm-core/src/fileops/mod.rs`: requests, plans, per-item conflict decisions,
  execution through GIO, the in-memory undo stack, and restoring from the trash by reading
  the freedesktop.org directories directly.
- `crates/fm-core/src/fileops/trashinfo.rs`: a pure parser for `.trashinfo` files
  (`[Trash Info]`, `Path`, `DeletionDate`).
- `crates/fm-core/src/fileops/tests.rs`: 36 unit tests for names, requests, plans,
  decisions, dry runs, copying, moving, renaming, new folders, cancellation, partial
  failure and the undo paths that do not need the trash.
- `crates/fm-core/tests/trash.rs`: 5 parent tests and 5 `#[ignore]` child tests. The parent
  re-runs the test binary as a child with a scratch `XDG_DATA_HOME`.
- `crates/fm-core/src/lib.rs`: `pub mod fileops;` only.
- `crates/fm-core/Cargo.toml`, the workspace `Cargo.toml` and `Cargo.lock`: `gio = "=0.22.10"`,
  which was already in the lock through GTK.
- `docs/notes/P2f.md`: this file.

`preview.save` and `preview.save-as` are untouched and still belong to the preview brief.
`[observed]`

### Requests and plans

`Request` is exactly the brief's six operations. `request_for` maps the six `Files`
commands that this brief owns and returns `Ok(None)` for every other command. Its signature
is `Result<Option<Request>, RequestError>`, not the literal `Option<Request>` in the brief:
`Option` cannot carry the required error when a handler has not filled a context path. For
`file.rename` with no `path`, for example, it returns
`MissingArgument("path")` rather than guessing. `[inferred]`

`FileOperations::plan` is read-only and gives each step its action, source, target and at
most one conflict. The five conflicts in the brief are all there:

- `TargetExists`: the only one the menu can decide;
- `FolderIntoItself` and `MissingSource`: refused for that item, never overridable;
- `InvalidName` and `NameTaken`: refused for that item.

I added three fatal conflicts the brief does not name, because the filesystem can present
them and silence would be worse:

- `AlreadyThere`: a move into the folder the item is already in;
- `MissingDestination`: the destination folder is missing or is not a folder;
- `NotEmpty`: undoing a new folder that now has contents.

Names are checked as the brief says: non-empty, no `/`, no NUL, not `.` or `..`, and at most
255 bytes (`str::len()` is bytes). `[observed]`

`Plan::lines()` is the dry-run text (`Plan::dry_run_outcome()` wraps it in
`Outcome::DryRun`). A dry run never calls the filesystem except for read-only metadata and
`read_dir`. `[observed]`

### Decisions

`Decision` is `Skip`, `KeepBoth`, `Replace` or `Cancel`. `Decisions` is a pure value:

- `answer(item, decision, apply_to_all)` records one answer;
- `apply_to_all` fills only conflicts with no answer, so an earlier different answer stays;
- `next_unanswered()` returns the next conflict, or `None` once every item has an answer or
  the user cancelled;
- any `Cancel` marks the run cancelled and ends the questions at once.

`execute` checks `Decisions::is_cancelled()` first and writes nothing. It then refuses the
whole operation with `ExecuteError::UnansweredConflicts(items)` while any conflict has no
answer. That is also what the CLI and agents get: there is no menu, and no new command
argument was added. The error is one line naming every conflicting item. `[observed]`

A copy into the item's own folder is planned with `keep_both = true` and no conflict, and
gets `name (copy).ext`, then `name (copy 2).ext`. Two sources with the same base name in one
run are planned in order, and execution also remembers names claimed earlier in the run, so
the second `KeepBoth` cannot reuse the first target. `[observed]`

### Doing it through GIO

- Trash: `gio::File::trash`.
- Copy: `gio::File::copy`. Folders are made with `make_directory` and their children copied
  recursively. A symlink is copied as a symlink, never followed.
- Move: `gio::File::move_`. On another filesystem a folder fails with
  `G_IO_ERROR_WOULD_RECURSE`; the fallback copies recursively and then **trashes** the
  source. GIO's own copy-and-delete fallback is disabled with `NO_FALLBACK_FOR_MOVE`.
- Rename: `set_display_name`. The undo path falls back to `move_` when the old name is not
  UTF-8, because `set_display_name` only takes UTF-8.
- New folder: `make_directory`.

**Metadata flags:** `NOFOLLOW_SYMLINKS | ALL_METADATA` for copies and moves, plus
`NO_FALLBACK_FOR_MOVE` for moves. Nautilus always passes `NOFOLLOW_SYMLINKS`, and adds
`ALL_METADATA` to the flags it hands `g_file_copy`/`g_file_move` on the move path
(`nautilus-file-operations.c:4142-4149`, `:4685`, `:5421`). I use the same pair for
user-requested copies so a copy keeps timestamps, permissions and extended attributes just
as a move's copy fallback does. I do not pass `TARGET_DEFAULT_PERMS`: Nautilus only uses it
when it deliberately resets permissions, which this brief does not ask for. I also do not
call `g_file_copy_attributes(ALL_METADATA)` on a copied directory: Nautilus makes the
directory and copies its children, and GLib 2.8x rejects that attribute set on a directory
with `Setting attribute standard::size not supported`. `[observed]`

Progress: every run takes a `&gio::Cancellable` and a callback receiving
`Progress { items_done, items_total, bytes_done, bytes_total }`. Regular-file copies feed
GIO's own byte callback into `bytes_done`; the total comes from metadata before the run.
Cancellation is checked before each item, so the current item finishes and what finished
stays. The undo record is built from the items that actually completed. `[observed]`

A failed item is recorded as `ItemOutcome::Failed` and the loop continues. A refused item is
`Refused`, a skipped one `Skipped`, and a cancelled remainder `Cancelled`. `[observed]`

`execute` only makes blocking GIO calls and never touches a `MainContext`, so the UI can put
it on a worker thread as the brief plans. `[inferred]`

### Undo

`FileOperations` owns a `VecDeque<UndoRecord>` capped at 50. Each completed operation pushes
what actually happened:

- trash: every original path plus the trash entry found for it;
- copy: every copy and every `Replace`d target;
- move: every `(source, target)` pair and every `Replace`d target;
- rename: the old and new paths;
- new folder: the folder path.

`file.undo` plans from the newest record and checks the filesystem again: the moved item is
at its target, the copy is still there, the old name is free. An item that changed is
refused and reported while the others still undo. The record is then popped; there is no
redo. A copy undo trashes the copies, and also restores a target that `Replace` had trashed,
which keeps `Replace` reversible. A new folder is trashed only if it is still empty. `[observed]`

### Restoring from the trash

`trashinfo::parse` follows Trash Specification 1.0:

- **Section 5, "Trash directories":** the home trash is `$XDG_DATA_HOME/Trash` (via
  `glib::user_data_dir()`); another mount uses `$topdir/.Trash/$uid` when that directory
  exists and is not a symlink, otherwise `$topdir/.Trash-$uid`.
- **Section 6, "Contents of a trash directory":** `files/` holds the items and `info/` holds
  one `<name>.trashinfo` per item. The file starts with `[Trash Info]`; `Path` is the
  original pathname percent-escaped as in RFC 2396 section 2; `DeletionDate` is
  `YYYY-MM-DDThh:mm:ss` local time. The first occurrence of a key wins. The spec's own
  example uses `YYYYMMDDThh:mm:ss`, so both forms are accepted.

On trash, the item's `DeletionDate` is matched against the time just before `g_file_trash`,
with Nautilus's two-second epsilon (`nautilus-file-undo-operations.c:44` and `:1747`). The
matched `files/<name>` is remembered in the undo record, so a later restore is exact even if
another entry for the same original path appears later; the `Path`/date lookup remains the
fallback. Restoring checks that the original path is free, renames `files/<name>` back
without overwriting, then removes the `.trashinfo`, which is the trash's bookkeeping rather
than a user file. A taken original path refuses only that item. `[observed]`

The top-directory trash path was implemented from the spec but not exercised by the tests:
the brief's cross-filesystem test moves *into* `/tmp`, and GIO refuses to trash anything on
`/tmp` ("Trashing on system internal mounts is not supported"). The home-trash path is fully
exercised. `[inferred]`

## Verified

From `$PROJ` with the 1.95.0 toolchain on `PATH` (`scripts/check.sh`). Exit status 0.
`[observed]`

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
running 1 test
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 7 tests
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 172 tests
test result: ok. 172 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
running 10 tests
test result: ok. 5 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
==> cargo build --workspace --release
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

The deny dependency tree is elided; it is the same `system-deps 7.0.8` / `9.0.1` duplicate
warning P2a, P2b and P2e recorded. `[observed]`

Test count: `fm-core` has 172 unit tests (36 new `fileops` tests), the CLI has 7, the agent
has 1, and the trash integration binary has 5 parent tests plus 5 ignored child helpers, so
185 tests ran. `[observed]`

### Cleanup checks

After the full run:

```
$ test -e ~/.cache/gimbal-tests && echo present || echo absent
absent
$ find /tmp -maxdepth 1 -name 'gimbal-p2f-*' -print
$ find ~/.local/share/Trash -mindepth 1 -maxdepth 1 -printf '%P\n' | sort | sha256sum
89918530475c251a2e5a82a5a9b3cc81826ae134f6f39af468fb59671979ec20  -
```

The same hash before and after the trash tests:

```
before 89918530475c251a2e5a82a5a9b3cc81826ae134f6f39af468fb59671979ec20  -
after  89918530475c251a2e5a82a5a9b3cc81826ae134f6f39af468fb59671979ec20  -
```

Each parent test also compares `~/.local/share/Trash` before and after its child and fails
if anything changed. The children set `GIO_USE_VFS=local`; without it, `gvfsd-metadata`
recreates `<scratch>/data/gvfs-metadata` after the parent removes the folder. That is a test
harness setting only; the app does not set it. `[observed]`

### Dry-run plan and reports

The sample below is the actual output of
`GIMBAL_P2F_PRINT=1 cargo test -p fm-core --test trash replace_trashes -- --nocapture`.
The child replaced an existing `a.txt`, then undid it. `[observed]`

```
== copy dry run (conflict) ==
dry run: copy 1 item(s) to /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst
copy /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/src/a.txt -> /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt [the target exists]
== copy report ==
copied /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/src/a.txt -> /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt
== copy undo dry run ==
dry run: undo the last file operation
trash /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt
restore /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/data/Trash/files/a.txt -> /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt
== copy undo report ==
trashed /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt
restored /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/data/Trash/files/a.txt -> /home/kyle/.cache/gimbal-tests/p2f-568390-0-replace/scratch/dst/a.txt
```

The `undo` plan's first step trashes the copy; the second restores the old target that
`Replace` had trashed. The old target is back after the undo. `[observed]`

## API citations

Read in the pinned sources under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`:

- `gio-0.22.10:src/auto/file.rs:64` `File::for_path`.
- `gio-0.22.10:src/auto/file.rs:684` `File::find_enclosing_mount`, used to find the
  top directory of the original path's mount; `src/auto/mount.rs:146` `Mount::root`.
  A device-number walk is the fallback when the mount is not local.
- `gio-0.22.10:src/auto/file.rs:222` `File::copy` (the progress callback is
  `Option<&mut dyn FnMut(i64, i64)>`).
- `gio-0.22.10:src/auto/file.rs:916` `File::make_directory`.
- `gio-0.22.10:src/auto/file.rs:1331` `File::move_`.
- `gio-0.22.10:src/auto/file.rs:2624` `File::set_display_name`.
- `gio-0.22.10:src/auto/file.rs:2881` `File::trash`.
- `gio-0.22.10:src/auto/flags.rs:1748` `FileCopyFlags`; `NOFOLLOW_SYMLINKS` at `:1756`,
  `ALL_METADATA` at `:1758`, `NO_FALLBACK_FOR_MOVE` at `:1760`.
- `gio-0.22.10:src/auto/enums.rs:2146` `IOErrorEnum::WouldRecurse`.
- `gio-0.22.10:src/auto/cancellable.rs:21`, `:43`, `:56` `Cancellable::new`, `cancel`,
  `is_cancelled`.
- `glib-0.22.10:src/auto/functions.rs:476` `glib::user_data_dir` (honours
  `XDG_DATA_HOME`).
- `glib-0.22.10:src/auto/date_time.rs:20`, `:476` `DateTime::new` and `to_unix`.
- `glib-0.22.10:src/auto/time_zone.rs:36` `TimeZone::local`.

Nautilus 50.2.2 references:

- `nautilus-file-operations.c:4142-4149` the copy/move flags; `ALL_METADATA` only on the
  move path.
- `nautilus-file-operations.c:4417` `test_dir_is_parent`, used at `:4635` for
  "copying/moving a folder into itself".
- `nautilus-file-operations.c:4685` and `:5421` the copy and move flag pairs.
- `nautilus-file-undo-operations.c:44` `TRASH_TIME_EPSILON 2`; `:1747` the absolute
  difference check; `:1700-1811` the trash-restore thread.

Rust standard library: `symlink_metadata` for type checks that never follow a symlink, and
`std::os::unix::fs::MetadataExt::{dev, uid}` for the mount top and the trash `$uid`.
`[read]`

## What needs a live session

Nothing in the core needs a display: all calls are blocking GIO, and no `MainContext` is
used. Nothing is wired to a menu yet, so there is no UI to click. When the UI brief lands it
has to run the plan → decide → execute flow on the right threads:

- on the main thread: `request_for`, `plan`, then a menu over `plan.decisions()` using
  `Decisions::next_unanswered()` and `answer(..., apply_to_all)`;
- on a worker thread: `execute(&plan, &decisions, &cancellable, progress)`;
- for `dry-run=true`: `plan.dry_run_outcome()` and no `execute`.

The trash test's `/tmp` destination proves the cross-filesystem fallback, but the
top-directory trash path (`$topdir/.Trash-$uid`) has no automated test because a second real
filesystem is not available here. That is the one thing worth a live check on a real mounted
drive. `[inferred]`

## Anything outside my files

Nothing needs changing now. For the next briefs:

- The UI handler should own a `FileOperations`, because the undo stack is in-memory and not
  saved across restarts.
- `file.undo`'s dry run is just `operations.plan(&Request::Undo)`; no command argument is
  needed.
- The CLI/agent path deliberately cannot answer conflicts. A future catalogue change for
  Kyle to review could add a `file.*` decision argument, but this brief leaves it out.
- `request_for` returns `Result<Option<Request>, RequestError>` because `Option` cannot
  carry the missing-context error the brief requires. If a literal `Option<Request>` is
  wanted, the missing-context case would have to be a separate function.
- The test children set `GIO_USE_VFS=local` so `gvfsd-metadata` does not outlive them; the
  app should leave the environment alone.

No file outside the brief's Output list was created or changed. `spike/` was read only.
`[observed]`

## Review (Claude, Nov 2, 2026): passes, with one safety fix by Claude

`scripts/check.sh` from `/tmp`: exit 0. The tests are 172 `fm-core` unit tests, 6 trash parents
with their 6 child tests, 7 CLI and 1 agent, and deny is clean. Kyle's real
`~/.local/share/Trash` was snapshotted before the run (7 items) and was byte-identical after,
timestamps included. No `~/.cache/gimbal-tests` or `/tmp/gimbal-p2f-*` folder was left.

- **The trash harness really runs:** with `GIMBAL_P2F_PRINT=1`, each parent shows its child
  running "1 passed" and trashing into `…/p2f-*/data/Trash`, not the real Trash. The "0.00s"
  timings are only rounding.
- **A stray child can't hit the real Trash:** a child test asserts `GIMBAL_P2F_CHILD=1` first,
  so `cargo test -- --ignored` can't run one outside the harness.
- **Every removal in the app code** was checked:
  - restoring an item removes only its `.trashinfo`, the trash's own bookkeeping;
  - the restore checks that the original path is free before the `rename`. A race between the
    check and the rename is possible, but the window is tiny.

**Safety fix (Claude):**
- **The problem:** when a cross-filesystem folder move had copied the folder but then couldn't
  trash the source, `remove_own` deleted the new copy with `remove_dir_all` / `remove_file`. The
  source was still intact, so no data was lost, but it was permanent-delete code in the app,
  against this brief's rule. And it would have been destructive if the target had ever pointed
  at something that existed before.
- **The fix:** `remove_own` is gone. The item now fails with "copied to <target> but could not
  trash the source, so both copies remain: <GIO error>", and nothing is deleted.
- **The new test, `an_untrashable_source_keeps_both_copies`:** a child moves a folder from `/tmp`
  (GIO refuses to trash there) to the home filesystem. It checks the message, that both copies are
  intact, and that nothing reached the trash.

Still to check later (not blocking):
- The top-directory trash (`$topdir/.Trash-$uid`, for USB drives) is implemented from the spec
  but untested. Try it live with a removable drive when the UI exists.
- `request_for` returns `Result<Option<Request>, RequestError>` rather than the brief's
  `Option`. That's right: a missing context path must be an error, not a silent `None`.
