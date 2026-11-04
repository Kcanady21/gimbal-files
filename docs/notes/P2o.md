# P2o: recursive filename search

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## What I built

- **[read] A GTK-free core walker.** `fm-core/src/search/` adds the typed
  `SearchQuery`, `SearchRequest`, `SearchMatch`, `SearchReport` and
  `SearchOutcome` contract, the literal matcher, the cooperative walker, the
  native source, the internal limits and 18 tests. It has no GTK, no unsafe and
  no dependency outside the existing `fm-core` crates.
- **[read] A fixed search worker pool.** `fm-ui/src/search/` owns two
  process-lifetime threads in a `SearchService`. Each job has its own
  cancellation flag, progress counters and a bounded queue of eight events.
  The queue waits on a condition variable with a short timeout, so a cancelled
  session wakes a worker blocked on a full queue. `SearchService::drop` stops
  accepting work and never joins a worker that may be inside a blocked native
  filesystem call.
- **[read] Per-window sessions.** `SearchRuntime` keeps at most eight admitted
  window sessions and at most one current scan plus one latest pending scan in
  a window. A submission captures the exact window, tab, folder and generation.
  A replacement cancels the old scan and abandons its queue before starting the
  new query. `poll` applies at most four events (256 rows) per main-loop tick,
  validates the full job token and session generation, and never borrows the
  runtime while the GTK view updates.
- **[read] A transient results page.** `SearchPage` is a `GtkListView` over a
  virtualized `SingleSelection` list. It shows the searched root, the query
  actually searched, the scope note, progressive counts and a bounded status.
  Rows append in discovery order with a theme icon, full-name tooltip, relative
  parent, type and available size/modified text. The page sits in the existing
  content stack above the folder/split page, which stays parented with its
  selection, history, scroll anchor and layout intact.
- **[read] Routing and guard integration.** `search.open` still has its
  existing optional `Text` argument and caller validation. No query opens or
  focuses the entry only; a nonempty query (including Enter in the entry)
  starts a scan. Escape, clearing the entry, closing the entry, tab/folder
  changes, entering Trash, root removal, window destruction and query
  replacement cancel the session and restore the ordinary page. Folder Refresh
  restarts the visible query. A single result click asks the existing preview
  for that exact path; Enter or double-click runs `nav.go` with the exact path
  and captured window/tab. File actions and `preview.edit` are refused with a
  readable notice while results are shown. Search never navigates the model
  until activation, and never writes session/config state.
- **[read] The results view is read-only.** It has no drag source, no context
  menu, no file-operation controls, no editable preview and no Stop button.
  Escape is the only cancellation control.

## Matching and traversal scope

- A literal substring of the basename, including the extension, never the
  parent path or file contents. No regex, glob, shell, external find/grep or
  index.
- Valid UTF-8 names compare Unicode lowercase forms (`to_lowercase`), with no
  locale, normalisation or full case-folding promise. Spaces stay literal, and
  an all-whitespace query is literal rather than match-all.
- Invalid UTF-8 names keep their native bytes and compare them with ASCII case
  folding against the query's UTF-8 bytes. Lossy text is only display.
- The walker resolves a symlink root alias once on the worker, never follows a
  child symlink directory, and stays on the root device. A mounted directory
  can match but is not entered; the scope note reports how many were skipped.
  Depth is bounded at 128 directories, root excluded from results.
- Per scan: 250,000 visited entries, 10,000 retained matches, 30 seconds of
  cooperative elapsed work and batches of at most 64 rows. Counts advance for
  non-matches too. Skipped counts and at most five example errors are retained.
  A failed root is `Search unavailable`; only a fully completed scan can say
  `No matches`.

## Production regressions and evidence

**[observed] Core tests** (`fm-core search`): literal extension/space matching,
Unicode lowercase, native invalid UTF-8 names, hidden files, duplicate
basenames, empty folder, symlink cycle, nested device skip with the mount point
itself matched, depth/entry/result/time limits, partial permission/changed/
unreadable errors with capped examples, batch size and discovery order, root
alias resolution, cancellation between entries, and root failure versus
`No matches`.

**[observed] Worker/session tests** (`fm-ui search`):
`repeated_blocked_scans_keep_two_workers_for_the_process_lifetime` records the
worker thread ids and peaks at two with a blocked third job; a replacement
query, clearing and closing remove sessions; a full queue wakes within the
cancellation bound; a ninth window gets a readable busy error.

**[observed] Headless production routing** (`fm-ui app`):
`split_tile_search_activates_the_exact_duplicate_in_the_focused_tile` builds a
real two-tile split in window A plus an unrelated window B, starts search in
the focused tile only, checks both duplicate `report.txt` paths, activates the
right one, and confirms A lands in that exact parent while B's focused tab, pill count, folder and
selection stay unchanged. Other tests cover the same-folder file selection,
the exact preview selection without hidden selection mutation, the dirty-preview
refusal via the real `PreviewSession`, `file.undo`/`preview.edit` refusals,
`nav.refresh` generation replacement, opening the entry without scanning,
tab/folder/window revocation while a worker is blocked, and a removed root
closing the session. The existing model-event reborrow invariant runs in these
headless paths and never panicked.

**[read] Source reasoning for GTK-only behavior:** the entry's `activate`
signal ignores an empty text (the registry's nonempty rule is untouched), the
`changed` signal only cancels when the user clears a nonempty search, Escape
calls the same `Process::close_search`, and one window key controller owns
Escape. The results list has no second Return/Escape handler; its built-in
activation is the only one. The list is not focused when it first appears, so
no row is auto-selected. Global hotkeys leave entry/list navigation keys to the
widgets through `search_entry_owns_key`/`search_results_own_key`.

## Gate output

`./scripts/check.sh`, pinned 1.95.0 toolchain, from the repo root:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
fm-agent lib:              23 passed, 0 failed, 0 ignored
fm-agent socket.rs:        16 passed, 0 failed, 0 ignored
fm-cli main.rs:            12 passed, 0 failed, 0 ignored
fm-core lib:              324 passed, 0 failed, 0 ignored
fm-core trash.rs:           6 passed, 0 failed, 6 ignored
fm-core trash_browser.rs:  11 passed, 0 failed, 11 ignored
fm-ui main.rs:            315 passed, 0 failed, 0 ignored
doc-tests:                  0 passed
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **707 passed, 17 ignored, 0 failed**. P2n-r2 baseline was 674 passed,
17 ignored. Log: `/tmp/gimbal-p2o-check.log`.

**[observed]** One earlier full run hit the pre-existing flaky
`fm-agent` test `repeated_enable_cycles_share_one_fixed_worker_budget` with a
`BrokenPipe` at `socket.rs:729`. The exact test passed alone and the complete
five-gate run above then passed; no `fm-agent` file changed.

## API citations

- **[read]** `glib-0.22.10:src/boxed_any_object.rs:79` — `BoxedAnyObject`,
  used to put a typed `ResultRow` in the `gio::ListStore`.
- **[read]** `gio-0.22.10:src/list_store.rs:58` — `ListStore::splice`, used to
  append one whole batch without rebuilding earlier rows.
- **[read]** `gtk4-0.11.5:src/auto/list_view.rs:203` and `:225` —
  `set_single_click_activate(false)` and `connect_activate`, so a single click
  selects only and a double click/Enter activates.
- **[read]** `gtk4-0.11.5:src/auto/single_selection.rs:101` and `:109` —
  `set_autoselect(false)` and `set_can_unselect(true)`, so arrival does not
  select a result.
- **[read]** `gtk4-0.11.5:src/lib.rs:49` — `GTK_INVALID_LIST_POSITION`.
- **[read]** `glib-0.22.10:src/auto/date_time.rs:73` and `:302` —
  `from_unix_local` and `format`, used for the row's modified time.
- **[read]** `std::sync::Condvar::wait_timeout` — the bounded full-queue wait;
  cancellation and abandonment both notify the same condition variable.
- **[read]** `std::os::unix::ffi::OsStrExt::as_bytes` and
  `std::os::unix::fs::MetadataExt` — native invalid UTF-8 bytes and Unix
  device ids; the metadata conversion reuses `folder_size::entry_meta`.

## Not verified live

- **[inferred]** Enter in the header entry, typing without scanning, the
  result page's visual layout and theme icons.
- **[inferred]** Progressive/partial status under a slow real filesystem.
- **[inferred]** Two live windows and a split tile through the compositor,
  including exact-path duplicate activation.
- **[inferred]** List Up/Down/Home/End/Enter focus ownership and double-click.
- **[inferred]** Result reveal into the parent and exact file selection.
- **[inferred]** Escape restoration of selection, history, scroll and split.
- **[inferred]** Dirty-preview refusal notice in the live preview.
- **[inferred]** Rapid cancellation/replacement under a blocked OS call.

The app was not launched, no live socket/config was used, and no user file or
desktop action was touched. All fixtures are under `/tmp/gimbal-search-*` and
`/tmp/gimbal-search-ui-*`, owned by the tests, which remove their own trees.

## Anything outside my files

- **[read]** None required. `nav.go`'s same-folder branch returns without
  selecting a file, so result activation selects the exact file name through
  the existing model file-click path after the accepted command. No registry
  command, caller permission, dependency, manifest, lockfile, protocol or
  schema changed.
- **[read] Correction (P2o-r1):** the original P2o return wrongly treated
  result activation as an acceptable one-path GTK lookup and the non-UTF-8
  branch as a direct model call. Both are superseded. Activation now resolves
  the exact path on a bounded process-wide worker, revalidates the session and
  target, and reaches the existing `nav.go` handler through registry validation
  for UTF-8 and invalid UTF-8 alike. The exact native target travels as trusted
  one-shot UI evidence; lossy text is never used as identity. See
  `docs/notes/P2o-r1.md`.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Host scripts/check.sh passes all five gates: **707 passed,
17 ignored**, release/deny pass. Log: /tmp/gimbal-p2o-review-check.log. No
production source fix, app launch, live socket/config, desktop input, real
clipboard/Trash or user-file action in this review.

**[read]** Positive findings: native filename identity and matching, bounded
worker/batch/session/result counts, cooperative cancellation, virtualized rows,
exact duplicate-path tests, window/tile capture and hidden-selection command
refusals are present. The original output scope is respected; Zen changes remain
separate. Three completion issues block acceptance:

- **[P2] Activation can block GTK.** app.rs search_activate/nav_go_exact uses
  normal nav.go's is_dir/is_file or direct fs::metadata for native paths on the
  UI thread. The brief requires metadata/path resolution on workers. This is
  explicitly acknowledged in the return, but is not an accepted exception.
  Use bounded deferred evidence and revalidate the full session/target before
  any navigation. Cover cancellation and target changes during blocked lookup.
- **[P2] Native activation bypasses the registry.** nav_go_exact directly calls
  Workspace::nav_go for non-UTF-8 paths. Preserve native bytes using trusted
  typed navigation context through the existing registry/handler, rather than
  adding a parallel command path or using lossy text. Current headless routing
  tests do not establish registry dispatch for that branch.
- **[P2] Completed searches do not observe root removal.** Core walk.rs does
  final missing-root checks before Finished. Search polling stops when there
  is no current/pending work, and no search root lifetime monitor is installed.
  A completed/partial page can therefore outlive removal of its root/alias.
  Add bounded nonrecursive lifetime observation and generation-safe teardown,
  with completed-scan/root-removal regressions. This does not require recursive
  watching or an immutable filesystem snapshot.

These are source-verified findings, not claimed live reproductions. Follow-up:
P2o-r1-result-activation-and-root-lifecycle.md. Live acceptance waits for that
revision; then check Enter/results, exact-path reveal, keyboard/preview, Escape
restoration, completed-root removal and split/multiwindow cancellation. No commit
or push. The unrelated Zen note/capture files remain excluded.

## P2o-r1 correction (Nov 3, 2026)

**[read]** The completed-root claim above is superseded. A finished or partial
results page now keeps a focused, nonrecursive root/parent `GFileMonitor`; an
event schedules one bounded worker metadata/canonicalize check. Removal or a
retargeted alias revokes the session, pending activation and batches, restores
the ordinary content page and reports `Search unavailable`. No recursive
watching or whole-tree scan was added. Details and tests are in
`docs/notes/P2o-r1.md`.

## P2o-r2 correction (Nov 3, 2026)

**[read]** The literal-substring-only matching and discovery-order result claims
above are superseded. The matcher now adds exact/prefix/substring plus ordered
subsequences and bounded typo distance for queries of at least three scalars.
The walker retains at most 10,000 best-ranked rows and continues scanning after
the cap; a late exact match replaces an early fuzzy one. A full retained set is
reported as `Partial results — result limit` with retained-versus-total counts.
See `docs/notes/P2o-r2.md`.

**[observed]** The P2o/P2o-r1 live missing-results failure was not a matching
deficiency. `GtkEditable::set_text` can emit `changed` with an empty string while
replacing text, and the header's changed handler treated that transient empty
value as the user clearing search. `begin_search` sets the entry text again after
starting the session, so the cancellation closed the just-created scan before
any batch arrived. Details, the regression and the fix are in
`docs/notes/P2o-r2.md`.
