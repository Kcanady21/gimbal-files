# P2m-r4: preview ownership and bounded render work

Tags: **[observed]** the working tree and this session's commands/output; **[read]** pinned crate or installed CLI source/help; **[inferred]** behaviour that still needs Kyle's live display/interaction.

## What was built

**[observed] Durable Save As ownership.** `PreviewSession::save_finished` now
retains `saved_override` when a non-new-path save commits the document that
Save As already owns (the override path equals the saved path), and records the
new revision. A passive `apply_model_selection` of the old browser candidate
still returns no load, an own-write monitor `force_load`/`load_finished` of the
owned path updates the override revision, and `reconcile()` returns `None`
while ownership exists. The browser candidate and its status are untouched.
Explicit `select_user`, keyboard/pointer selection, Trash, tab/folder focus and
navigation still release the override; another window keeps its own session.
Discard/Reload of an owned editor reloads the owned path rather than
retargeting the older candidate. Save failures and conflicts remain bound to
the pinned editor and its immutable ticket.

**[observed] Keyboard focus is separate from the model selection.** The grid
now stores a `KeyboardFocus` name (`folder/mod.rs`), not a position. A key move
sets the target name before the synchronous model refresh; `apply_selection`
reapplies the cursor by name and only falls back to the first *visible*
selected item when that file was removed/filtered away. Folder replacement
resets deliberately. Pointer clicks set the clicked name. `nav_mode` separates
plain selection, Shift extension from the model's existing anchor, and Ctrl
focus-only movement (no Ctrl-click toggle). `visible_page_rows` uses the
scroller viewport and the measured realised row pitch, so multi-line selected
names do not make Page Up/Down use the full scrollable grid. Plain arrows read
`name_at(target)` directly; only Shift builds the visible order.

**[observed] PDF rasters are bounded before Poppler runs.** `pdf_raster_plan`
and `PdfRenderRequest::arguments` use the installed `pdftoppm -scale-to N`
inside a conservative square (`PDF_MAX_RASTER_EDGE = 4096`, whose square is the
16 MP `MAX_RENDER_PIXELS` bound). The plan preserves aspect and never passes
`-scale-to-x`/`-scale-to-y` alone. The post-render PNG/byte checks remain.
The preview records the actual returned geometry, lowers its display edge cap
for PDFs, clamps a displayed texture to the pixels actually rendered, and
re-requests once at the now-known fit size. The grid's PDF thumbnail uses the
same bounded plan.

**[observed] Thumbnail accounting and deferred demand.** The grid's separate
`thumb_pending` counter is gone. `Thumbnailer` owns live tokens, a bounded
coalesced deferred queue, and filters stale events in `try_next`, so a
cancelled A result cannot consume B's accounting. A full worker queue keeps
visible demand deferred and retries from the existing poll until capacity
opens; unbind/cancel forgets the exact token. `Drop for Thumbnailer` sets the
cancel flag on every live and deferred job, including jobs still in the pool
channel.

**[observed] Bounded, prioritised render work.** The preview panel now has a
one-slot newest-wins image queue (`LatestQueue`) for `Load`/`RenderImage` and a
separate serialized save/check/destination worker. A burst keeps at most one
in-flight decode plus one pending job; replaced pending jobs are cancelled;
decoded results go through a one-event bounded channel. Saves no longer wait
behind image decode. Poppler has selected and grid queues with selected
priority, per-panel coalescing of obsolete selected zooms, a reserved normal
slot, and a UI retry when the selected queue is temporarily full. Closing the
panel drops the pending image job. `quit_now` calls `pdf::shutdown`, which sets
cancellation on queued/running jobs and waits for each worker to kill and reap
its own child before the application leaves.

## API citations

- **[read]** Installed `/usr/bin/pdftoppm` (version 26.01.0, `/usr/bin/pdftoppm -h`):
  `-scale-to <int>` "scales each page to fit within scale-to*scale-to pixel
  box"; `-scale-to-x`/`-scale-to-y` scale one axis only. Generated tall
  (100×3515 pt), wide (5000×100 pt), rotated (600×800, `/Rotate 90`) and
  ordinary (600×800) PDFs were rendered in `/tmp/gimbal-preview-r4-probe`:
  `-scale-to-x 400` gave tall `400×7323` and ordinary `400×1667` (the other
  axis kept the default 150 DPI); both `-scale-to-x 400 -scale-to-y 400` gave
  a stretched square `400×400`; `-scale-to 400` gave tall `12×400`, wide
  `400×8`, rotated `400×300` and ordinary `300×400`. The production tests use
  the generated fixtures, not those probe files.
- **[read]** `gtk4-0.11.5:src/event_controller_key.rs:11` —
  `connect_key_pressed` returns `glib::Propagation`; the grid key controller is
  installed in the capture phase as in r3.
- **[read]** `gtk4-0.11.5:src/auto/grid_view.rs:89` (`max_columns`) and `:129`
  (`scroll_to`); the page row calculation now measures realised cells instead
  of using `grid.height()`.
- **[read]** `gio-0.22.10:src/auto/file_info.rs:144`, `:173`, `:256`, `:272`
  remain the metadata/revision accessors used by the grid bindings.
- **[read]** `glib-0.22.10:src/source.rs` (the r3 timer lesson): the thumbnail
  poll never removes an expired `SourceId`; it stores `None` from inside the
  callback. The new PDF retry uses the same `MonitorTimer` discipline.

## Production regressions tested

**[observed]** All tests drive production code, not a parallel model:

- `PreviewSession` real scratch chain: select/edit A → Save As B (real atomic
  save) → passive A reapply → edit/save B (ordinary Save, real atomic save) →
  `reconcile` and another passive refresh → conflict Reload/Discard of B →
  explicit A selection. Asserts B displayed, updated B revision, A bytes
  unchanged and no pending Save As/save/close action.
- `folder::KeyboardFocus` + `keyboard_move` + `Workspace::file_click`/
  `file_shift_click` + `PreviewSession`: three consecutive Shift moves,
  contraction, Ctrl focus-only movement, sorting, removal, and two independent
  view cursors. `visible_page_rows` is tested separately.
- `PdfRenderRequest::arguments`, `pdf_raster_plan`, and `render_for_test` on
  generated tall/wide/rotated/ordinary PDFs: actual PNG dimensions/pixels
  before decode, aspect ratio, source bytes and job-directory cleanup.
- `Thumbnailer`: stale cancelled A vs delayed B, live/deferred accounting,
  saturated worker queue with a controlled decode barrier, unbind/rebind,
  cache hit, decode failure, and Drop cancellation.
- `spawn_workers`/`LatestQueue`: burst while decode is held (outstanding ≤ 2,
  newest result wins), a real text Save completing behind the held decode, and
  close dropping the pending image job.
- `PdfScheduler`: selected priority over queued grid work, per-panel selected
  coalescing, full-selected-queue recovery after a cancelled owner, and
  `shutdown` killing/reaping a test-owned `sleep` child.

## Gates

**[observed]** Final `PATH=... ./scripts/check.sh` from the repository root
(pinned 1.95.0 toolchain; log `/tmp/gimbal-p2m-r4-check-final.log`):

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
test result: ok. 277 passed; 0 failed; 0 ignored
test result: ok. 6 passed; 0 failed; 6 ignored
test result: ok. 11 passed; 0 failed; 11 ignored
test result: ok. 246 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Total: **548 passed, 17 ignored** (baseline 531/17; 17 new tests). Counts by
target: agent 1, CLI 7, core 277, isolated Trash parents 6 + 11, UI 246.

## Environment

**[observed]** This gate ran on the host with the image loader's D-Bus access;
all image/PDF/thumbnail tests passed. The reviewer sandbox previously saw
`gly-loader-error ... Operation not permitted` for three UI image tests. No
private bus, system-sandbox change, desktop setting or dependency was created
or changed here.

**[observed]** No application smoke run was needed for the gate. No git, sudo,
downloads/install, real data/Trash action, clipboard write, default-app launch,
input injection, runner/spike change or manifest/lockfile change was made.
Generated files live only under `/tmp/gimbal-preview-*`,
`/tmp/gimbal-thumbnails-*` and the PDF test's own job directories.

## Not verified live

**[inferred]** These remain for Kyle's live run:

- repeated Shift/Ctrl navigation in single and split grids, including Page
  Up/Down with multi-line selected names and editor/path/dialog key ownership;
- Save As followed by editing and ordinary-saving the new file, then a
  passive refresh and explicit old-file reselection;
- large/rapidly scrolled thumbnail folders, queue pressure, and deferred
  thumbnails becoming visible without leaving the viewport;
- readable portrait/landscape PDFs at fit-width, 100% and zoom, plus the
  rendering-limit label and panning; the conservative square PDF bound is a
  deliberate quality/safety trade-off for extreme page aspects;
- image zoom, r3 names/hover/status/pinned click/header/notice, and the
  accepted editor/close/Quit/Trash/DnD checks.

## Remaining limits / outside this brief

- **[read]** Poppler's internal memory during a render is still not hard-capped.
  The 4096 square bounds the requested raster edge/pixels, not Poppler's own
  allocations; the 10 s child deadline remains.
- **[inferred]** The selected PDF queue is bounded at 16 owners and retries
  through a 50 ms timer; a very large number of concurrently rendering windows
  could still see a short delay. Grid PDF queue pressure is reported as a
  thumbnail failure, not deferred by the PDF scheduler.
- **[read]** The first PDF pass uses a conservative geometry probe and a second
  pass at the known fit size; this costs one extra small Poppler run for a PDF
  preview.
- **[read]** No new registry command, config/session field, dependency,
  manifest/lockfile, desktop, runner or spike change was made.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Static review passes with the reviewer fix below; Kyle's live
acceptance remains pending. Initial reviewer host-enabled `scripts/check.sh`
passed **548 tests, 17 ignored**. After the reviewer source fix/test, the final
host-enabled run passed all five gates: **549 passed, 17 ignored** (core 277,
UI 247, CLI 7, agent 1, isolated Trash parents 6 + 11), including release and
deny. Final output: `advisories ok, bans ok, licenses ok, sources ok` and
`all checks passed`. `git diff --check` passed. Logs:
`/tmp/gimbal-p2m-r4-review-check-host.log` and
`/tmp/gimbal-p2m-r4-review-check-final.log`.

**[read/observed tests]** The five prior findings are addressed:

- Save As ownership survives subsequent ordinary Save, reconciliation, passive
  model reapplication and own-write reload. The actual scratch save chain checks
  updated B revisions and unchanged A bytes; explicit selection releases ownership.
  Discard/Reload of the owned editor stays on that document intentionally.
- KeyboardFocus persists by filename across the synchronous model refresh;
  repeated Shift extension/contraction and Ctrl focus-only moves are tested.
  Plain navigation reads its target directly; page distance uses viewport and
  measured row pitch. Text-entry/editor key routing remains in place.
- PDF argv now uses aspect-preserving `-scale-to` inside a conservative 4096
  square, bounding requested edges/pixels before rasterization. Tests render
  generated tall/wide/rotated/ordinary pages through the installed helper and
  inspect output geometry/source preservation. The post-render checks remain.
- Thumbnail work belongs to live/deferred tokens; stale/cancelled events no
  longer double-decrement a separate counter. Deferred demand retries without
  rebind, cancellation is token-scoped and Drop cancels jobs. Controlled tests
  exercise stale A/delayed B and queue saturation/recovery.
- The preview image queue coalesces to one in-flight plus one pending job with
  bounded results; saves use a separate serialized worker. Tests hold decoding
  behind a barrier while a real text save completes. Selected PDF priority,
  superseding/retrying demand and shutdown/own-child reap are exercised.

**Fix (ChatGPT): keep the PDF wait predicate locked until Condvar::wait.**
**[read]** PdfScheduler::worker dropped its queue mutex, reacquired it, then
waited without checking the predicate again. A submit or shutdown between those
operations could notify before the worker slept, leaving queued work asleep
until another notification. Replaced that unlock/relock gap with
`self.ready.wait(state)`, which releases the predicate mutex and waits atomically.
No scheduler policy, timeout, priority or child ownership change.

**[observed]** Added production scheduler test
`a_single_job_wakes_an_idle_worker_without_another_notification`: 256 sequential
requests each complete from their own notification, using missing paths inside
an owned scratch directory (no child/image decode needed), then shutdown returns.
The focused test passed before the fix as well; this is not claimed as a
deterministic red/green reproduction of the narrow race. The bug/fix are supported
by the source's mutex/condition-variable ordering. The new regression and all
other scheduler/child tests pass in the final suite. Focused pre-fix log:
`/tmp/gimbal-p2m-r4-review-wakeup-before.log`.

**Live: [observed]** No reviewer app launch or desktop interaction. Tests used
their owned scratch files/generated PDFs and test-owned children, with host
image-loader D-Bus access as established in r3. No user-data/real-Trash/clipboard
operation, private bus or desktop configuration change. No new dependency,
command/caller policy, manifest/lockfile, config/session schema, runner or spike
changes. Existing unrelated P2l-r3 notes and Zen captures remain untouched.

**Still open: [inferred] Kyle's live acceptance.** Use the latest release and
disposable files:

1. Arrow/Home/End/Page navigation updates preview/status in single/split views.
   Repeated Shift moves extend/contract; Ctrl moves focus without toggling files.
   Text/path/dialog keys still stay with their widgets.
2. Save As A to B, edit B and Save again, then refresh: B stays in preview and
   A stays unchanged. Explicitly selecting A again switches back normally.
3. Scroll a large image/PDF thumbnail folder quickly, then stop: visible
   thumbnails fill without requiring another scroll/rebind; no wrong-file image.
4. Images and portrait/landscape PDFs stay readable at Fit/100%/zoom, retain
   aspect, pan correctly and report a rendering limit when reached.
5. Check full selected names, hover details/highlight, bottom selected name/size,
   wrapping preview title and double-click compact/expanded toggle.
6. Pinned single-click opens once; reorder/drop still work without navigating.
   Dirty edits stay pinned with a prominent persistent notice that pulses on a
   fresh selection attempt.
7. Recheck Save/Save As Replace/Cancel/Discard, external-change warnings, dirty
   close Save/Discard/Cancel, two-window Quit and accepted Trash/DnD behavior.

**[read]** No further blocker found in the revised paths. Poppler's other internal
allocations still lack a hard cap; the conservative square can limit detail for
extreme page aspect ratios, and actual visual quality remains for Kyle to judge.

**[observed]** Updated Phase 2 index and handoff log. No further brief written,
commit or push. Wait for Kyle's live report before accepting/committing P2m.

## Live acceptance (Kyle report, Nov 3, 2026): passes

**[observed: Kyle reports]** “All that works and behaves as expected” in
response to the seven-item r4 live checklist. P2m through r4 is accepted,
including keyboard/range navigation, durable Save As preview, thumbnails,
visual rendering, file details/header, pinned activation/notice and the listed
editor/close/Quit/Trash/DnD regressions.

**[observed: Kyle reports]** Folder hover does not show total contents size;
Kyle asks about its complexity. **[read]** Recursive folder totals were
explicitly excluded from r3's tooltip/status scope. They require a contents
walk rather than the folder entry's own metadata size. Track as a possible
separate background feature, not a failed P2m acceptance item. No new brief or
implementation authorization is inferred from this question.

**[observed]** Updated index/handoff log. No source changes or gate rerun for
the acceptance report; latest gates remain 549 passed, 17 ignored. No commit
or push; wait for Kyle's instruction. Zen remains separate/open.

## Commit/push authorization and next feature (Nov 3, 2026)

**[observed: Kyle instruction]** Kyle authorized committing and pushing the
accepted P2m stack, and requested a brief for background folder-hover size so
he can evaluate the UX. Wrote `P2m2-folder-size-hover.md` in the prompts folder:
delayed main/split-grid tooltip totals, contents/reported on-disk bytes, partial
and progress states, bounded cooperative scans and 30-second memory caching.
This is a separate implementation brief; no folder-size feature source is part
of the accepted P2m commit. The unrelated P2l-r3/Zen notes and captures stay out.
No code changes since the final 549-passed/17-ignored review gates; no rerun
needed for these documentation updates. Commit/push result is recorded in the
external phase index/handoff log after it succeeds.
