# P2o-r2: live results and fuzzy matching

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## The actual missing-results failure

**[observed] Cause.** The failure was in the real header entry's `changed`
handler, not in the walker, matching or the release executable.

1. Clicking Search dispatched `search.open` with no query and opened the entry.
   Typing logged `search: typed "report"`; no scan started.
2. Enter emitted the real `GtkEntry::activate` signal and dispatched
   `search.open query=report`. The registry validated it and
   `SearchRuntime::submit` created generation 1.
3. `Frame::begin_search` calls `Header::open_search(Some(query))`.
   `GtkEditable::set_text` clears the old text before inserting the new text and
   emits `changed` with an empty string in between. A backtrace from the changed
   handler showed `gtk_editable_set_text` → `Header::open_search` at
   `header.rs:538`.
4. The header's changed handler saw `entry.text().is_empty()` while search was
   open and not marked programmatic, so it called `ui.clear_search()`. That
   cancelled the session that had just been submitted. The search page showed
   its Searching heading, but the session count was already zero and no batch
   could arrive. Any nonempty query, including exact file and folder names,
   behaved the same way.
5. Headless tests did not exercise `begin_search` or a real `GtkEntry`, so they
   never saw the transient empty `changed` and passed.

**[read] Fix.** `Header::open_search` sets `search_programmatic` around both
entry text assignments (the already-open and newly-revealed branches).
`close_search` already used the same flag. The changed handler now ignores the
intermediate empty value and only treats a non-programmatic empty entry as a
user clear.

**[observed] Regression.** `real_header_enter_shows_file_and_folder_rows_in_the_list_store`
runs a real `adw::ApplicationWindow`, header entry, content stack, `SearchPage`,
`gio::ListStore`, `ListView` factory and the production search poll in its own
process. It creates a direct `report.txt`, a nested duplicate
`nested/report.txt`, a matching `report-folder`, and an unmatched
`unrelated.txt`. It:

- opens the entry through `search.open` (no query),
- types `report` and asserts the Press Enter hint is visible and no scan starts,
- emits the entry's real `activate` signal (no desktop input injection),
- waits for the production timer/poll to reach Complete with three rows in the
  store, asserts the list is mapped and the stack's visible page is `search`,
- repeats Enter for the exact file `report.txt` and the exact folder
  `report-folder`, asserting the exact native paths survive.

Before the programmatic-flag fix this regression cancelled its session and
failed at "the real entry started a search". The test runs in its own process
because `gtk::init` owns the default GLib context for the process lifetime; an
in-process run made the unrelated preview timer tests fail. The outer test
spawns the inner test binary and is part of the normal gate run.

## What I built

- **[read] Deterministic matcher.** `fm-core/src/search/matcher.rs` matches
  basenames only. Valid UTF-8 names use simple Unicode `to_lowercase`; invalid
  UTF-8 names keep native bytes with ASCII folding. Exact name, exact stem,
  prefix, substring and fuzzy classes are distinct.
- **[read] Fuzzy classes.** For queries of at least three Unicode scalars,
  ordered-character subsequences and bounded optimal-string-alignment distance
  (insertion, deletion, substitution and adjacent transposition) are tried
  against the full name, its stem and separator-delimited tokens. The edit budget
  is one for 3–5 scalars and two for 6 or more. Queries under three scalars and
  whitespace-only queries stay literal.
- **[read] Bounded fuzzy work.** Fuzzy work uses at most 64 query scalars and 256
  candidate scalars per name/stem/token. Each name also has an 8,192-scalar
  fuzzy budget, so a pathological name with many tokens cannot dominate the
  scan. Longer names still get exact/prefix/substring checks over their full
  text; identities are never truncated. The distance implementation is banded
  and rejects length differences early.
- **[read] Scoring tuple.** Rank order is the lexicographic tuple
  `(class, edits, gaps, start)` followed by the native relative path:
  - `class`: exact name, exact stem, prefix, substring, fuzzy;
  - `edits`: typo edit count (zero for literal and subsequence matches);
  - `gaps`: unmatched scalars inside the tightened subsequence span;
  - `start`: scalar offset of the match (byte offset for native invalid UTF-8);
  - ties: native path bytes.
  This tuple is public in `MatchScore` and asserted by tests. There is no opaque
  model score.
- **[read] Bounded ranked retention.** The walker keeps at most
  `MAX_RESULTS = 10_000` best rows in a `BTreeSet` ordered by the tuple above.
  When full, it drops the worst row and continues scanning within the existing
  visited and time budgets. A late exact match displaces an early fuzzy one.
  `SearchReport` carries total matches, retained rows and `truncated`;
  `ResultLimit` is reported without stopping the scan.
- **[read] Bounded changes.** Workers emit at most 64 `SearchChange`
  inserts/removals per batch through the existing eight-event queue. Each change
  carries the retained index plus the exact native path. The UI applies changes
  in order; it never clones or rebuilds the whole list per match.
- **[read] Stable identities.** Activation is keyed by generation plus exact
  native path. A reorder moves the row index but an in-flight activation still
  resolves the original path; if ranking evicts that path the completion is
  invalid and the page is cancelled rather than activating the new occupant of
  the old index.
- **[read] Selection, preview and scroll.** `SearchPage` keeps the selected
  native path across a ranked insert/remove and reselects that path at its new
  index. The selection notify is suppressed during the update, so the preview is
  not repointed; the scroll adjustment value is restored after the batch.
- **[read] Pre-submission hint.** The header shows a small `Press Enter to search`
  label once a query is typed and hides it when the results page opens.
- **[read] Diagnostics.** With `GIMBAL_LOG=1`, submissions log window/tab/root/
  query length and generation; worker starts log generation and token; applied
  or stale batches log generation, token, change count and retained count; and
  terminal state logs generation, phase, retained/total counts and visited
  entries. No per-path or per-filename traversal logging was added.

## Production regressions and evidence

**[observed] Core matcher/walker tests (9 new; 8 run, benchmark ignored):**

- Exact/prefix/substring/fuzzy class order; case-insensitive exact and stem;
  folder suffix as part of the name; Unicode lowercase and one-substitution
  typo.
- Fuzzy tighter spans and earlier starts rank first. Subsequence `rpt`; 3–5
  scalar one-edit and 6+ scalar two-edit rules; short queries stay literal;
  whitespace-only queries do not match all.
- Native invalid UTF-8 names stay ASCII-folded literal only; a UTF-8 query never
  matches native `0xff` bytes.
- Long exact names and long substrings still match; a 600-scalar token with
  only a subsequence is outside fuzzy work.
- Late exact matches after 20 earlier fuzzy rows replace them with a small
  injected `max_results = 2`; total matches, retained rows and truncation are
  reported honestly and batches stay under 64 changes.
- Duplicate names tie-break by native relative path.
- Existing traversal, depth/entry/time limits, mount/device, symlink, root
  alias, cancellation and partial-error tests were updated for ranked changes.

**[observed] UI tests:**

- `visible_rows_rank_exact_then_prefix_then_substring_then_fuzzy` checks the
  visible row order `report`, `report.txt`, `reporting.txt`, `my-report.txt`,
  `reprot.txt`.
- `a_late_exact_match_replaces_early_fuzzy_rows_and_reports_truncation` uses 20
  fuzzy files and a late exact file with `max_results = 2`; the exact row is
  first, the total is 21, the retained count is 2, the phase is
  `Partial(ResultLimit)` and the status says `2 best of 21 matches`.
- `pending_activation_uses_the_native_path_not_the_reordered_index` blocks the
  activation worker, swaps the retained rows, releases the lookup and verifies
  the original path and filename are still the target. Evicting that path makes
  the late completion invalid.
- The real widget regression above also injects a better row above a selected
  row and removes a row above it; the selection follows the same native path.
- Existing split/window, duplicate native-path, same-folder reveal, dirty
  preview, root revocation, queue cancellation, generation replacement and
  monitor teardown tests still pass.

**[read] Not covered as an assertion:** actual scroll pixels are not asserted;
the test checks that the page order and selected path survive. Live visual
mapping of the hint and scroll position remains a live check.

## Matcher-only benchmark

Repeatable ignored test:
`cargo test -p fm-core --release matcher_benchmark_over_generated_corpus -- --ignored --nocapture`

**[observed] Corpus:** 100,000 generated names, owned by the test (no filesystem
access): 25% `report-<n>.txt`, 25% `notes-<n>.md`, 25% `reprot-<n>.txt`, and 25%
worst-length names alternating a 256-scalar `z` token and an 80-token `zz-`
separator-heavy token mix. Queries: `rpt` (3), `reprot` (6), `ab`×16 (32),
`ab`×32 (64), `a`×64 (64), plus a cancellation pass.

**[observed] Host/toolchain:** AMD Ryzen 9 9900X (12-core/24-thread), rustc
1.95.0, release build.

| Query scalars | Matches | Elapsed | Per name |
| --- | --- | --- | --- |
| 3 (`rpt`) | 50,000 | 111.3 ms | 1,113.3 ns |
| 6 (`reprot`) | 50,000 | 86.4 ms | 863.7 ns |
| 32 (`ab`×16) | 0 | 92.6 ms | 925.8 ns |
| 64 (`ab`×32) | 0 | 97.1 ms | 970.9 ns |
| 64 (`a`×64) | 0 | 97.5 ms | 974.7 ns |
| cancellation, 64-scalar query | — | 541 µs / 100k calls | — |

These are matcher-only measurements. Filesystem traversal, metadata and GTK
work are separate; there is no fixed real-filesystem latency promise. The test
is ignored by the correctness suite and has no wall-clock assertion. An earlier
run before the final token-budget cleanup gave 65–76 ms for the query passes; the
final separator-heavy corpus now examines all tokens within the per-name scalar
budget, and machine variance is ordinary.

## Gate output

`./scripts/check.sh`, pinned 1.95.0 toolchain, from the repo root:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
fm-agent lib:              23 passed, 0 failed, 0 ignored
fm-agent socket.rs:        16 passed, 0 failed, 0 ignored
fm-cli main.rs:            12 passed, 0 failed, 0 ignored
fm-core lib:              332 passed, 0 failed, 1 ignored
fm-core trash.rs:           6 passed, 0 failed, 6 ignored
fm-core trash_browser.rs:  11 passed, 0 failed, 11 ignored
fm-ui main.rs:            332 passed, 0 failed, 1 ignored
doc-tests:                  0 passed
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **732 passed, 19 ignored, 0 failed**. P2o-r1 baseline was 720 passed,
17 ignored. Log: `/tmp/gimbal-p2o-r2-check.log`. The two new ignored tests are
the explicit matcher benchmark and the inner process-owned GTK regression
(spawned by its normal test wrapper). One earlier full run hit the pre-existing
flaky `fm-agent` socket test `repeated_enable_cycles_share_one_fixed_worker_budget`
with a `BrokenPipe`; the same test and the complete five-gate run then passed,
and no `fm-agent` file changed.

## API citations

- **[read]** `gtk4-0.11.5:src/auto/editable.rs:287` — `EditableExt::set_text`;
  the observed intermediate empty `changed` comes from GTK's editable
  implementation, not from the application.
- **[read]** `gtk4-0.11.5:src/rt.rs:132` — `gtk::init` deliberately acquires and
  leaks the default GLib main context, which is why the widget regression owns a
  separate process.
- **[read]** `gtk4-0.11.5:src/auto/entry.rs:1520` — real `Entry::activate`
  signal; the regression emits this signal rather than synthesising input.
- **[read]** `gio-0.22.10:src/list_store.rs:58` — `ListStore::splice`, used for
  bounded insert/remove changes.
- **[read]** `gtk4-0.11.5:src/auto/single_selection.rs:86` and `:93` —
  `selected`/`selected_item`, used to reapply the selected native path.
- **[read]** `gtk4-0.11.5:src/auto/list_view.rs:203` — single-click activation
  remains off.
- **[read]** `glib-0.22.10:src/source.rs:455` — `timeout_add_local` uses the
  default main context, which the production poll and the process-owned test
  both exercise.
- **[read]** `std::sync::mpsc::sync_channel`, `Condvar::wait_timeout` and the
  existing Unix `OsStrExt`/`MetadataExt` boundaries are unchanged.

No unsafe blocks, dependency, manifest, lockfile, command, caller permission,
protocol, config or session schema changed.

## Not verified live

- **[inferred]** The live visual hint placement and header width at Kyle's
  scale/theme.
- **[inferred]** Live exact file/folder Enter results through the compositor.
- **[inferred]** Live exact-first order, typo/subsequence results and the
  retained/total truncation status on a large real folder.
- **[inferred]** Keyboard selection, exact preview, Enter/double-click reveal and
  same-folder selection through the live grid.
- **[inferred]** Selection/scroll preservation during a real ranked reorder that
  arrives while the user is browsing rows.
- **[inferred]** Escape and rapid replacement under a real slow filesystem,
  including two windows/split tiles.
- **[inferred]** Root removal after completion through the live GTK monitor
  callback.

No app was launched. The widget regression creates and presents one owned test
window in its own process for under a second. No live socket/config was used,
and no user file, real Trash/clipboard, mount, desktop action or input injection
was touched. Fixtures are under `/tmp/gimbal-search-*` and
`/tmp/gimbal-search-ui-*` only.

## Live checklist for Kyle

1. Exact file and folder: search the fixture `report` (or any exact filename),
   press Enter, and confirm both files and the folder appear.
2. Typo and subsequence: search `reprot` and `rpt`; confirm a transposed name and
   an `rpt` subsequence appear without a model or network request.
3. Exact-first ranking: search a stem with exact, prefix, substring and typo
   neighbours; confirm exact name/stem first, then prefix, substring, fuzzy.
4. Keyboard/preview/reveal: select rows with Up/Down/Home/End, check the exact
   preview follows, and Enter/double-click a duplicate to reveal its exact path.
5. Escape and rapid replacement: Escape restores the folder/split with its
   selection, scroll and history; quickly replace queries; old rows must not
   reappear.
6. Truncation on a very large tree: confirm the status says retained versus total
   matches and `result limit` instead of claiming every match is shown.
7. Completed-root removal still closes the page with `Search unavailable`.

Acceptance waits for Kyle's live report. Unobserved items above remain
explicitly unverified.

## Anything outside my files

- **[read]** No source outside the brief's Output list needed a change. The
  socket protocol, `fm-agent`, `fm-cli`, config/session schemas and command
  catalogue are untouched. `docs/agent-socket.md` now describes fuzzy/ranked
  results and total-versus-retained truncation.

## Review and live check (ChatGPT, Nov 3, 2026): passes with reviewer scoring fix

**[observed]** Initial host scripts/check.sh passes: 732 passed, 19 ignored.
Final gates after the reviewer correction/regression pass all five checks:
**733 passed, 19 ignored**, release/deny pass. Logs:
/tmp/gimbal-p2o-r2-review-check.log and
/tmp/gimbal-p2o-r2-review-final-check.log. Whitespace check passes.

**[read + observed automated test]** Missing-results cause is now established: a
programmatic GtkEntry text replacement emitted transient empty changed and
cancelled the newly submitted session. Both open_search text-assignment paths
now use the programmatic guard. The gate's subprocess regression uses the real
entry activation, frame/content stack, production poll, list store and mapped
list rows, including files, a folder and exact-name resubmission. It has a
separate NON_UNIQUE test application id and scratch fixtures; it cannot hand
arguments to Kyle's Gimbal instance. This closes the earlier headless coverage
gap. Kyle's live workflow still needs a fresh report.

**[read]** Matching and ranked retention implement the requested local fuzzy
classes/budgets without a model, dependency or network request. Exact results
can displace earlier fuzzy rows at the bounded retention limit. Ranked changes
remain capped, and pending activation tracks generation plus native path rather
than mutable row index. The real widget test verifies selected path survives
insert/remove; scroll pixels are not asserted. Live scroll-anchor behavior
remains explicitly unverified. Existing activation/root/caller tests pass.

**Fix (ChatGPT): correct subsequence spans and stem origin.**
**[read]** The first implementation only considered the first completed greedy
span; r___p___t__r_pt for rpt scored gaps=6/start=0 rather than the later tighter
span gaps=1/start=11. It also used the stem's character length as the stem's
origin, although the stem begins at offset zero. The new bounded dynamic prefix
scan finds the tightest span with earliest-start ties; native stem origin is
zero. Cancellation is checked during scanning; candidate/query bounds remain.

**[observed]** New production matcher regression
fuzzy_scores_use_the_best_span_and_the_stems_actual_origin failed before the
fix on the span assertion and passes afterward on both span and stem assertions.
The stem-origin fault was independently source-verified, not separately claimed
as a pre-fix red test. Logs: /tmp/gimbal-p2o-r2-score-before.log and
/tmp/gimbal-p2o-r2-score-after.log.

**[observed]** Re-ran the existing ignored release matcher benchmark after the
correction: 100k generated names took 108.4 ms (rpt), 95.9 ms (reprot), 221.9 ms
(32-scalar alternating query), 360.9 ms (64-scalar alternating query), and
395.8 ms (64 repeated a); cancellation pass 566 microseconds. Log:
/tmp/gimbal-p2o-r2-review-benchmark.log. These final numbers supersede the earlier
matcher measurements for this tree. They exclude traversal, ranked retention
and GTK; no fixed end-to-end latency is claimed.

**Live:** Reviewer ran the isolated owned widget window through the test gate;
no ordinary Gimbal app launch, live socket, user config/files, clipboard, Trash
or desktop input action. Actual Kyle-session acceptance, visual hint/scroll,
rapid replacement and completed-root callback behavior remain unverified live.

**Still open:** Relaunch the current release and repeat the live checklist above,
starting with exact file AND folder names, then reprot/rpt, ranked selection/
preview/reveal and Escape. No further revision brief from this review. No commit
or push; unrelated Zen note/captures remain separate.

## Live acceptance (Kyle, Nov 3, 2026): accepted through r2

**[Kyle reports]** Search is working now, following the exact file/folder, fuzzy
matching, ranking and preview/reveal/scroll/Escape checklist. P2o through r2 is
accepted. This is Kyle's overall acceptance report, not a separate reviewer
observation of each UI case; unreported stress/root-removal scenarios retain
their earlier coverage limits. Latest gates 733 passed, 19 ignored. No source
change or gate rerun. No commit/push authorized in this update; Zen separate.
