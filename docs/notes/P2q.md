# P2q — Recent folders across restarts

## What I built

- **One shared list in the workspace model.** `Workspace` now owns
  `recent_dirs`, capped by `RECENT_LIMIT = 5`. `nav_go`, `nav_back` and
  `nav_forward` record their destination through the same private
  `record_recent` call, after the tab actually changes. A revisit removes the
  old entry and inserts at the front; the oldest entry is evicted at five.
  Only absolute paths are recorded, and comparison is the existing native
  `Path`/`PathBuf` equality. **[read]**
- **No other navigation triggers it.** `window_new`, `tab_new`, `focus_window`,
  `Workspace::restore`, startup, failed `nav_go` (including `NoWindow`) and
  search submission do not record. Search reveal already dispatches `nav.go`,
  so its successful `nav_go` records through the same path. **[read]**
- **Session persistence.** `session::document_for` writes
  `recent_dirs = [ ... ]` after `focused_window` when the list is non-empty;
  an empty list omits the optional field. `session::load` reads it with the
  existing string-or-`{ bytes = [ … ] }` path codec. `Workspace::restore`
  then keeps the first absolute distinct paths, in order, up to five. Thus a
  missing field is empty, and malformed elements, relative paths and
  duplicates are skipped individually without affecting window/tab parsing.
  Atomic writes, debounce, `.bad` recovery and the blocking quit save are
  unchanged. **[read]**
- **Every sidebar renders the shared list.** `Sidebar::new` seeds from
  `Process::recent_dirs()`, a copied-out workspace value. `Event::Navigated`
  makes the app copy the list once, then refresh every open `WindowUi`
  sidebar; the workspace borrow is released before any GTK call. Labels,
  tooltips and the existing `attach_click` → `Process::nav_go_path` activation
  are unchanged. Rendering uses no `is_dir`/`is_file` probe, so unavailable
  removable-drive folders stay in the list. **[read]**
- **Persistence scheduling.** The navigation event already makes
  `Process::finish` call `schedule_save`; a real list change always produces a
  `Navigated` event, so it uses the existing debounced writer. A same-folder
  navigation emits no event and does not save. **[read]**

## Saved session excerpt (rich round trip)

**[observed]** from `GIMBAL_P2E_PRINT=1 cargo test -p fm-core --lib
session::tests::a_rich_workspace_round_trips -- --nocapture`:

```toml
version = 1
focused_window = 1
recent_dirs = ["/home/kyle/work/a", "/home/kyle/work/b"]

[[window]]
pills = [0, 1]
```

The two entries are the result of the fixture's `nav_go(a)`, `nav_go(b)` and
`nav_back()`; restore keeps that order even though the restored tabs include
other folders. **[observed]**

## Automated tests

All fixtures are owned scratch folders under `/tmp/gimbal-*` and remove
themselves. No real state directory is touched.

### `fm-core` — workspace model

- `recent_dirs_follow_successful_navigations_newest_first_distinct_and_capped`
  covers ordering, revisit promotion, the five-entry eviction and ignoring a
  relative path.
- `recent_dirs_follow_back_and_forward_but_not_focus_or_failed_navigation`
  covers back/forward promotion, same-folder no-op, focus and `NoWindow`.
- `restore_keeps_the_stored_recent_order_and_sanitizes_the_list` restores
  `[b, a, b, relative, c, d, e, f]` to `[b, a, c, d, e]` while the restored
  tabs are `a` and `b`; it verifies restore does not promote tabs over the
  saved order.

### `fm-core` — session

- `recent_dirs_are_optional_and_bad_entries_are_ignored_individually` saves
  without the field, loads an empty list, then loads a fixture containing a
  relative path, an integer, an inline table without bytes, a duplicate and
  ten entries. It expects the first five valid distinct paths. The `exists`
  closure panics if the history path is probed, proving loading performs zero
  availability probes.
- `a_malformed_recent_field_leaves_the_windows_restorable` uses
  `recent_dirs = "not a list"` and still restores the window, group and tab.
- `a_non_utf8_folder_and_file_name_round_trip` now also asserts the non-UTF-8
  recent folder survives the byte codec.
- `a_rich_workspace_round_trips` asserts the saved recent order survives.

### `fm-ui` — production paths

- `navigating_in_one_window_updates_every_sidebar_recent_list` spawns an
  ignored inner GTK test in its own process. The inner test builds two real
  windows, navigates A to `one`, `two`, then `one` again, and reads the rendered
  rows from both sidebars. Both show `one, two` with full-path tooltips. It
  then renders a nonexistent remembered path and asserts the row remains,
  covering zero probes while rendering. **[observed]**
- `a_recent_change_survives_quit` spawns an ignored inner test that restores a
  session, navigates, and calls `request_quit`. `quit_now` uses the existing
  blocking save barrier. The parent then loads the actual `session.toml` with
  `session::load` and asserts `recent_dirs == [visited]`. Running it in a child
  process keeps quit's global worker shutdown away from the other tests.
  **[observed]**

## Five gates

**[observed]** `./scripts/check.sh` from the repo root, all five passed:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.03s
==> cargo test --workspace
    ...
    test result: ok. 23 passed; 0 failed; 0 ignored
    test result: ok. 16 passed; 0 failed; 0 ignored
    test result: ok. 12 passed; 0 failed; 0 ignored
    test result: ok. 348 passed; 0 failed; 1 ignored
    test result: ok. 6 passed; 0 failed; 6 ignored
    test result: ok. 11 passed; 0 failed; 11 ignored
    test result: ok. 362 passed; 0 failed; 4 ignored
    test result: ok. 0 passed; 0 failed; 0 ignored
    test result: ok. 0 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
    Finished `release` profile [optimized] target(s) in 5.94s
==> cargo deny check
    advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **778 passed, 0 failed, 22 ignored** across the workspace. The change
adds 7 passing tests and 2 ignored inner-process tests over the accepted
771/20.

## API citations

Non-obvious GTK calls used only by the sidebar test helper:

- `gtk4-0.11.5:src/auto/widget.rs:431` — `WidgetExt::first_child`.
- `gtk4-0.11.5:src/auto/widget.rs:626` — `WidgetExt::next_sibling`.
- `gtk4-0.11.5:src/auto/widget.rs:812` — `WidgetExt::tooltip_text`.
- `gtk4-0.11.5:src/auto/label.rs:225` — `Label::text`.

No new dependencies, no new `unsafe`, and no GTK additions to `fm-core`.

## Live checks for Kyle

These are **not automated**; automated coverage stops at the GTK widget test
and the owned session/quit fixtures above. **[inferred]**

1. Visit several folders in one window, then revisit one already in Recent
   Directories. Confirm it moves to the front and the fifth entry falls off.
2. With two windows open, navigate in one and confirm the other window's
   Recent Directories list updates to the same order without focusing it.
3. Restart Gimbal and confirm the restored order matches the last saved order
   and is not reordered by the restored tabs.
4. Click each restored recent row and confirm it opens the folder through the
   normal navigation path; if a removable drive is absent, confirm the row
   stays in the list and the normal navigation failure feedback appears.
5. Confirm labels and tooltips still match the previous behavior (`Home` for
   the home folder, full path in the tooltip).

I did not launch the app, so no `XDG_CONFIG_HOME`/`XDG_STATE_HOME` run was
made. The GTK regression ran in a test process with an owned non-unique
application and no desktop input injection. **[observed]**

## Files changed

- `crates/fm-core/src/workspace/mod.rs` and `workspace/tests.rs`
- `crates/fm-core/src/session.rs`
- `crates/fm-ui/src/app.rs`, `frame.rs`, `sidebar.rs`
- `docs/notes/P2q.md`

Nothing outside the brief's Output list needed a change; the saver contract
already carries the whole `Workspace`, so `save.rs` was untouched.

## Review (ChatGPT, Nov 4, 2026)

**[read]** The five-entry list is shared workspace state and uses the existing
session codec and writer. Restore keeps saved order and renders every sidebar
without probing remembered paths.

**Reviewer fix.** Recent rows previously used the unchecked native-path
navigation helper directly. Activating a missing folder could put the tab at
that path. The Recent click now checks for a directory before navigating and
shows Folder unavailable on failure. The widget regression also exercises the
unavailable activation path and checks that workspace and history stay intact.

**[observed]** All five gates passed on the final tree with 778 passed and
22 ignored. Final log is /tmp/gimbal-p2q-review-final.log. Kyle's restart
and multiwindow live check is pending.

## Live acceptance (Kyle, Nov 4, 2026)

Kyle reports that the multiwindow, restart and restored-row checks all passed. P2q is accepted. Final gates remain 778 passed and 22 ignored.
