# P2o-r1: worker-backed result activation and root lifetime

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## What I built

- **[read] Deferred activation evidence.** `fm-ui/src/search/` now owns a
  second fixed, process-lifetime worker pool for short read-only evidence:
  two workers, a bounded queue of eight jobs, and one result channel. A click,
  Enter or double-click submits the captured window/tab/root/generation/index
  and the exact native result path. No activation thread is created per click,
  and no GTK-thread `is_dir`, `is_file`, `metadata` or `exists` call remains on
  the activation path.
- **[read] Bounded, de-duplicated activation state.** At most one activation is
  pending per window. Repeating the same result is idempotent (`AlreadyPending`);
  a different result while one is pending, or a full worker queue, returns a
  readable busy notice. Replacement, Escape/clear, tab/focus/folder/window
  changes, root loss, quit and window destruction cancel the pending job. A
  late worker result whose id or generation no longer matches is discarded.
- **[read] Full revalidation before dispatch.** A returned activation is checked
  against the current model window id, focused tab, tab folder/root, session
  generation, result index and exact native path. Only then does the app
  dispatch the existing `nav.go` through `Registry`. A vanished, unreadable or
  special result updates the page notice and stays on the page; it never
  navigates, closes search or selects a different item.
- **[read] Native bytes through the real `nav.go` handler.** Both UTF-8 and
  invalid-UTF-8 activation reach the same `nav.go` catalogue entry, validation
  and handler. For invalid UTF-8, the UI sends a reserved absolute placeholder as
  the required validated text path and a one-shot typed payload carrying the
  exact `PathBuf`/`OsString` target. That payload is private, is bound to the
  exact text the registry accepted and to `Caller::Ui`, and is consumed only by
  the `nav.go` handler. A mismatched caller or text fails instead of falling
  back to a lossy path or a different target. The existing agent worker evidence
  (`set_pending_nav_target`) keeps working with its original unbound form.
- **[read] Same-folder reveal stays exact.** After an accepted same-folder file
  activation, the model's no-op `nav_go` is followed by the existing exact-name
  file click. Directories and symlinks navigate to the worker-resolved exact
  path; a symlink to a directory is not mistaken for a file selection.
- **[read] Completed-root lifetime observation.** A session keeps a focused,
  nonrecursive `GFileMonitor` on its root object and on the root's parent while
  its results page is open, including completed and partial pages. A monitor
  event only marks the session dirty and wakes the app. One bounded worker then
  checks `metadata(root)` and, once the walk has finished, compares
  `canonicalize(root)` with `walked_root`. Removal, replacement by a non-directory
  or a retargeted alias revokes that generation's session, pending activation,
  root check, batches and monitors; the app restores ordinary content and shows
  `Search unavailable — …`.
- **[read] No second navigation implementation and no schema changes.** There
  is no new command, argument kind, caller permission, protocol field, config or
  session key, dependency, manifest or lockfile change. `RootFailure::Changed`
  is a small typed label addition in the existing core search enum.

## Production regressions and evidence

**[observed] `fm-ui` production routing tests** (all through the real registry
and `Process` dispatch, not direct `Workspace` calls unless stated):

- A gated activation leaves `nav.focus-path` and other control work responsive
  while the lookup is blocked; clear then makes its late completion inert
  without changing folder or selection. Repeating the same result is
  deduplicated; a different result reports busy and only one lookup enters the
  gate.
- Replacement query, a tab move that changes focus, and window close each make a
  blocked activation's late completion inert.
- A file removed while its activation lookup is blocked produces
  `Search result unavailable: … no longer exists`, leaves the results page open,
  keeps the hidden folder/selection unchanged and uses no navigation.
- A directory result and a symlink-to-directory result navigate through their
  exact original paths.
- Two invalid-UTF-8 parent directories hold the same invalid-UTF-8 basename.
  The search retains both, the exact right duplicate is activated, the real
  `nav.go` handler consumes validated native evidence (counted), and the model
  lands in the exact native parent with the exact native name selected.
- A relative path is refused by the registry before the handler; a different
  caller cannot consume UI evidence; a changed text target fails; the matching
  text reaches the handler once with the exact native selection.
- Existing split/window test: activation in window A leaves window B's focused
  tab, pills, folder and selection unchanged.

**[observed] Root-lifetime tests:**

- A completed real-filesystem search installs one monitor pair and revokes its
  page after its root is removed; the monitor count returns to zero.
- A partial scan (injected `max_visited = 1`) is likewise revoked.
- A completed search on a symlink alias is revoked when the alias itself is
  removed; the walker recorded the resolved root.
- Removing the root revokes a simultaneously blocked activation and later
  produces no navigation.
- Old root evidence arriving after navigation to another root and a replacement
  query does not close the replacement session.
- Monitor counts stay bounded through replacement (old monitor dropped before
  the new one is installed), clear and window destruction.

**[read] Source reasoning:** the root monitor callback and the test's
`root_changed` hook converge on the same `root_dirty` → one-worker-check path.
GTK only applies the owned result; the monitor callback never reads metadata
itself. Batches still use the accepted P2o queue and cancellation invariants.
The P2n model-event reborrow assertion remains active in all headless paths.

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
fm-ui main.rs:            328 passed, 0 failed, 0 ignored
doc-tests:                  0 passed
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **720 passed, 17 ignored, 0 failed**. P2o baseline was 707 passed,
17 ignored; this brief adds thirteen `fm-ui` tests. Log:
`/tmp/gimbal-p2o-r1-check.log`.

## API citations

- **[read]** `gio-0.22.10:src/auto/file.rs:1153` — `FileExt::monitor_file`,
  used for the root object itself with `FileMonitorFlags::NONE`.
- **[read]** `gio-0.22.10:src/auto/file.rs:1131` — `FileExt::monitor_directory`,
  used for the parent with `FileMonitorFlags::WATCH_MOVES`
  (`gio-0.22.10:src/auto/flags.rs:2027`).
- **[read]** `gio-0.22.10:src/auto/file_monitor.rs:29` — `FileMonitorExt::cancel`;
  `RootMonitor::drop` cancels both handles. Dropping a `FileMonitor` also stops
  it, but the explicit cancel makes teardown order independent.
- **[read]** `std::fs::metadata` follows the result/root symlink exactly as the
  existing `nav.go` path did, while `std::fs::canonicalize` is used only on the
  root comparison worker. `SearchSource::lstat` remains the walker's
  `symlink_metadata` boundary.
- **[read]** `std::sync::mpsc::sync_channel` + `SyncSender::try_send` bound the
  evidence queue; `Condvar` from P2o still bounds the scan batch queue.
- **[read]** `gtk4-0.11.5` `GFileMonitor` signal signature
  `connect_changed(|monitor, file, other_file, event| ...)` matches the pinned
  bindings and the existing preview/trash monitor uses.

No unsafe blocks were added. No dependency, feature, manifest or lockfile
changed.

## Not verified live

Live checks still needed (Claude, per CLAUDE.md):

- **[inferred]** Enter/double-click and single-click preview on real results.
- **[inferred]** Keyboard row movement and focus ownership.
- **[inferred]** Exact reveal for duplicate basenames and invalid native paths.
- **[inferred]** Same-folder file selection and directory/symlink navigation in
  the live grid.
- **[inferred]** Escape/clear restoration of ordinary content, history,
  selection, scroll and split layout.
- **[inferred]** Root removal after a completed/partial search through the real
  GTK main-loop monitor callback, including a symlink alias.
- **[inferred]** Rapid cancellation/replacement with two windows and split
  tiles while a real OS lookup is slow.
- **[inferred]** Busy/dedup activation notices under repeated clicks.

Automated tests exercise monitor installation/teardown and the production
root-event → worker → poll → revoke lifecycle, but headless tests do not run a
GTK main loop, so the actual `GFileMonitor::changed` callback delivery remains a
live check.

No app was launched, no live socket/config was used, and no user file, real
Trash/clipboard, mount, desktop action or input injection was touched. Fixtures
are under `/tmp/gimbal-search-*` and `/tmp/gimbal-search-ui-*` only.

## Ordinary limitation

Activation evidence is a bounded filesystem query, not an atomic snapshot. A
file can still disappear, be replaced or be retargeted between the worker's
successful metadata/canonicalize check and the model/GTK application of the
navigation. The pre-dispatch revalidation closes the asynchronous stale-target
gap in this design; it cannot make the final filesystem mutation and model
change atomic. Root monitoring likewise observes changes after the fact and can
miss a remove-and-recreate at the same path between evidence checks. No recursive
watch, whole-tree scan, index or immutable snapshot was added.

## Anything outside my files

- **[read]** No source outside the brief's Output list needed a change. The
  socket protocol, `fm-agent`, `fm-cli`, config/session schemas and command
  catalogue are untouched.
- **[read]** The P2o note's original activation exception and completed-root
  lifecycle claims are corrected in `docs/notes/P2o.md`; `docs/agent-socket.md`
  now describes deferred activation, busy/dedup behavior, native evidence and
  completed-root revocation.

## Review and live check (ChatGPT, Nov 3, 2026): static review passes

**[observed]** Host scripts/check.sh passes all five gates: **720 passed,
17 ignored**, release/deny pass. Output: /tmp/gimbal-p2o-r1-review-check.log.
Tracked whitespace check passes. No reviewer source fix or new regression added.

**[read]** All three r1 findings are addressed. Result activation uses bounded
process-lifetime evidence workers and an id/generation/window/tab/root/index/path
check before application. Missing results remain errors on the results page.
Both native and UTF-8 paths reach Registry validation and the same nav.go handler
with a one-shot, caller/text-bound native payload; no direct native-path model
branch remains. Search activation consumes that evidence instead of falling
through to GTK-thread path queries. The existing agent navigation payload path
is retained. These guarantees concern the search activation path; ordinary UI
nav.go still has its existing synchronous fallback.

**[read]** Root and parent monitors remain installed on completed/partial pages.
Their callbacks only mark generation-owned dirty state and wake polling; bounded
worker evidence determines removal/alias changes. Teardown cancels monitors and
pending activation/check work; stale evidence cannot revoke the new session.
Tests cover actual routing and the production event/check/revoke adapters,
including duplicate invalid-UTF-8 paths, stopped activations and root removal.
The existing model-event borrow invariant passes. No new dependency, schema,
command or caller permission; unrelated Zen files remain separate.

**Live:** Not performed by the reviewer. Monitor installation and production
root-event handling are tested, but actual changed-signal delivery through GTK's
main loop is still a live check, as the return correctly states. No app launch,
live socket/config, desktop input, user-file mutation or real Trash/clipboard.
The documented final-query/filesystem-change race remains an ordinary limitation.

**Still open:** Kyle's live acceptance below. No further revision brief from
this review, commit or push.

### Live checklist for Kyle

Close the old build and launch ./target/release/gimbal-files from the repo.
For a disposable fixture, run in one terminal:

```sh
SCRATCH=$(mktemp -d /tmp/gimbal-search-kyle-XXXXXX)
mkdir -p "$SCRATCH/tree/one" "$SCRATCH/tree/two" "$SCRATCH/tree/report-folder"
printf 'First report\n' > "$SCRATCH/tree/one/report.txt"
printf 'Second report\n' > "$SCRATCH/tree/two/report.txt"
printf '%s\n' "$SCRATCH/tree"
```

1. Open the printed tree path in Gimbal. Open search, type report, and press
   Enter. Typing alone should not scan. Results should include both report.txt
   entries with their different parent locations and report-folder. The query,
   root, progress/completion and scope should be readable.
2. Click the results and use Up/Down/Home/End in the list. The preview should
   follow the exact selected file. Enter/double-click the second report: its
   parent opens with that exact file selected. Activate the folder result in
   another search: it should open the folder.
3. Search again, then Escape or clear the entry. Normal content, selection,
   history/scroll and any split layout should return. Rapidly replace/close
   searches; old results must not reappear. Delete/Cut/Paste/Undo and Alt-click
   editing while results are visible must be refused without file changes.
4. With two windows and a split group, search the focused tile's folder.
   Results/reveal should affect only the captured tile/window. Switching tabs
   should close the old search. No edits to another window's selection/folder.
5. In ordinary browsing, edit a disposable text preview without saving, then
   search and select another file. The existing dirty-buffer notice should
   prevent preview replacement. Save/discard that disposable edit afterward.
6. Search report in the original tree again and wait for completion. In the
   same fixture terminal run `mv "$SCRATCH/tree" "$SCRATCH/tree-moved"`.
   The completed search should close and explain Search unavailable. This
   changes only the disposable fixture; no real folder needs deletion.

Native invalid-UTF-8 activation, partial/error limits and blocked-lookup timing
have automated coverage; live observations should be reported separately.

## Kyle live report (Nov 3, 2026): fails; search not accepted

**[Kyle reports]** Search does not surface folders or files, including exact
filenames. Confirmed Enter was pressed; this is not merely typing without
submission. Kyle also requests fuzzy search with low latency and asks whether a
small language model would help.

**[observed]** Read-only host process inspection found running Gimbal PID
2160658's executable device/inode matches target/release/gimbal-files. A stale
release executable is therefore not supported as the explanation at this check.
No live socket, UI injection, app restart or user-file action was performed.
The missing-results cause is not yet identified; static/headless gates passing
did not establish that the real entry-to-visible-row handoff works.

**[decision]** Wrote P2o-r2-live-results-and-fuzzy-matching.md: reproduce and fix
the actual GTK handoff first; add local subsequence and bounded typo-distance
matching, exact-first ranking and bounded streaming with stable native-path row
identities. No model/inference/dependency. Existing match rules/ranking caps
are explicitly superseded where needed. No production patch, new gates, commit
or push; previous source gates remain 720/17, not live acceptance.

## P2o-r2 follow-up (Nov 3, 2026)

**[observed]** The missing-results cause is now identified and fixed. The real
header `GtkEntry` emits `changed` with an empty string during
`GtkEditable::set_text`; the header changed handler read that as a user clear and
cancelled the session. `Frame::begin_search` set the entry text again after
submitting, so the new scan was cancelled before its first batch. The
programmatic flag now wraps every entry text assignment, and a process-owned GTK
entry/list/store/poll regression covers files and folders. Details are in
`docs/notes/P2o-r2.md`. The activation/index claims above remain valid as
corrected by r1; activation now carries the exact native path identity rather
than a row index because ranking can reorder rows.
