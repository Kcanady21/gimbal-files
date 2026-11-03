# P2m-r1: preview lifecycle, pending actions and scrolled line numbers

## What was built

Tags: **[observed]** code and tests in this working tree; **[read]** pinned API
sources; **[inferred]** behaviour that needs Kyle's live display/interaction.

- **[observed]** **Monitor timer ownership.** `crates/fm-ui/src/preview.rs` now
  owns one `MonitorTimer` instead of a bare `SourceId`. `schedule` first cancels
  a live source, then stores the new one-shot. Its callback upgrades the timer,
  checks the generation, **takes** its stored handle before submitting the
  check, and drops that already-fired handle. `clear_monitors` cancels only a
  live source. `PreviewPanel::drop` cancels a live timer and drops the monitors.
- **[observed]** **Clean external reload.** `PreviewSession::force_load` and an
  accepted `load_finished` finish a clean, idle pin and clear its pending Save
  As state. The shown text, title, access and later save revision then come from
  the new `Loaded` result. Failed/unsupported results leave no editor. A dirty
  (or saving) pin is untouched. Explicit `reload_pinned` still abandons edits
  and preserves the pending selection.
- **[observed]** **Save As request binding.** `EditorToken` is allocated once per
  `begin_edit`; `ActionToken` adds the exact buffer revision.
  `SaveAsRequest` carries the token through `Choosing -> CheckingDestination ->
  AwaitingReplace -> Starting`. `mark_dirty`, Discard, Reload, document
  replacement, close and a cancelled/superseded request invalidate it.
- **[observed]** **Bound save results.** `SaveTicket` freezes the editor token,
  document revision, target path, new-path/Replace context and encoding.
  `save_finished`/`save_failed` accept only the session's active ticket; a stale
  or differently edited result cannot clear another editor's saving state.
- **[observed]** **Scoped close and quit continuations.** `CloseRequest` freezes
  the buffer token and, when saving, the exact save job id. The UI keeps the
  proceed/cancel closures beside that request. A successful unrelated save
  aborts the continuation; failure, conflict, Reload and Cancel invalidate it.
  Quit uses a `QuitCoordinator` that rechecks every current model window, keys
  answers by editor/document/buffer identity, and never finishes while a dirty
  buffer or active save remains.
- **[observed]** **Visible-coordinate gutter.** `draw_gutter` uses
  `TextView::visible_rect` (buffer coordinates), `line_at_y` for the visible
  first/last logical lines, `line_yrange` for their buffer tops, and
  `compute_bounds` to convert the text view's origin into gutter coordinates.
  `gutter_line_positions` numbers each logical line once; partially visible top
  lines stay in range with a clipped label.

## Action invalidation (plain rules)

- A new editing session gets a new `editor_id`; ids are never reused.
- A Save As reply is accepted only when the request id, stage, editor id, path,
  document revision and buffer revision all still match.
- Any text change bumps the buffer revision and drops a pending chooser /
  destination / Replace request. The reply performs no write; the user invokes
  Save As again.
- Discard, Reload, selecting another file with a clean editor, document
  replacement and close all drop pending action state.
- `Save As` to the original path is routed back through original Save, so its
  load-time identity/version checks still apply. Replace keeps the
  destination-version confirmation.
- A save result is applied only to the same active `SaveTicket`; a stale
  result is ignored rather than changing the current title/status/saving flag.
- A close continuation is bound to one `ActionToken` and one save job id. It is
  consumed only after that job reports success for that buffer. A failed or
  abandoned save clears it. Discard-based close waits for an idle revalidation
  (`close_ready`) so a fresh edit cannot be closed silently.
- Closing during a save offers only “Cancel close”; the in-flight write is not
  called cancelled or discarded. Success closes/wakes the quit; failure keeps
  the window and aborts the pending quit.
- Quit records handled `(window, editor, path, document revision, buffer
  revision)` keys. A newly dirty window, a newly created dirty window, or new
  edits in a handled window are asked again. One request is active at a time;
  Cancel and save failure clear it.

## API citations (pinned sources)

**[read]**

- `glib-0.22.10:src/source.rs:29-35`: `SourceId::remove` calls
  `g_source_remove` and unwraps `result_from_gboolean!`, so removing an
  already-fired one-shot panics. `glib-0.22.10:src/source.rs:455-470` shows
  `timeout_add_local` acquiring the default context and `g_timeout_add_full`.
- `gtk-4.22.4/gtk/gtktextview.c:2531-2543`: `gtk_text_view_get_iter_at_location`
  requires buffer coordinates. `:3324-3349`: `gtk_text_view_get_visible_rect`
  fills the visible region in buffer coordinates. `:2625-2652`:
  `get_line_yrange` is also in buffer coordinates. `:2655-2683`:
  `get_line_at_y` takes buffer `y` and returns the line start/top.
- `gtk4-0.11.5:src/auto/text_view.rs:675` (`iter_at_location`), `:742`
  (`line_at_y`), `:758` (`line_yrange`), `:878` (`visible_rect`), `:516`
  (`buffer_to_window_coords`, consulted); `gtk4-0.11.5:src/auto/widget.rs:166`
  (`compute_bounds`).
- Existing save/ETag APIs are unchanged from P2m:
  `gio-0.22.10:src/auto/file.rs:2096` (`replace_contents`) and
  `gio-0.22.10:src/auto/file.rs:1756` (`query_info`).

## Gates

**[observed]** `./scripts/check.sh` from the repository root, pinned toolchain,
passed on Nov 3, 2026. Full log: `/tmp/gimbal-p2m-r1-check.log`.

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
test result: ok. 264 passed; 0 failed; 0 ignored
test result: ok. 6 passed; 0 failed; 6 ignored
test result: ok. 11 passed; 0 failed; 11 ignored
test result: ok. 198 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Total: **487 passed, 17 ignored** (baseline was 464/17; 14 new core and 9 new
UI tests).

## Named regressions

**[observed]** Core (`crates/fm-core/src/preview/tests.rs`):

- clean reload: `a_clean_external_reload_drops_the_old_pin_and_uses_the_new_revision`
  (two real revisions; later `begin_save_original` uses the new revision).
- reload failures: `failed_and_unsupported_reloads_leave_no_editor`.
- dirty preservation: `a_dirty_external_change_keeps_the_pinned_buffer`.
- Save As binding: `a_destination_reply_for_discarded_editor_a_cannot_start_b_through_a`,
  `old_chooser_and_replace_replies_are_rejected_after_reload_or_replacement`,
  `editing_while_a_destination_check_is_pending_refuses_the_stale_reply`,
  `duplicate_or_in_flight_save_as_requests_are_refused`,
  `saving_over_the_original_path_routes_through_the_original_checks`,
  `invalidating_actions_on_close_drops_a_pending_save_as`.
- close: `close_waits_for_the_accepted_save_and_document`,
  `a_failed_conflict_reload_cannot_close_a_later_edit_and_save`,
  `cancel_and_repeated_close_requests_do_not_leave_a_competing_continuation`,
  `close_while_saving_captures_the_in_flight_job`,
  `a_close_discard_is_ready_only_while_no_new_edit_exists`.

UI (`crates/fm-ui/src/preview.rs`, `app.rs`):

- `an_expired_monitor_timer_can_be_rescheduled_without_removing_it_again`
- `an_expired_monitor_timer_survives_a_document_change`
- `a_monitor_timer_cancelled_before_expiry_never_fires`
- `a_rescheduled_timer_generation_rejects_the_obsolete_callback`
- `gutter_numbers_follow_a_nonzero_visible_origin_and_wrap_once`
- `quit_rechecks_a_clean_window_dirtied_during_another_prompt`,
  `a_handled_window_edited_again_is_asked_again`,
  `a_newly_created_dirty_window_joins_a_pending_quit`,
  `repeated_quit_and_cancel_are_scoped_to_one_request`,
  `final_quit_requires_no_dirty_or_saving_window`,
  `a_save_failure_cancels_the_whole_pending_quit`.

The old test-only `CloseChoice` response lookup was removed; close tests now
drive `PreviewSession::arm_close`, `bind_close_to_save`, `close_satisfied_by`,
`close_ready` and the save ticket lifecycle. Timer tests call the production
`MonitorTimer::schedule`/`cancel` on a GLib main context.

## App smoke run

**[observed]** `pgrep -a gimbal-files` was empty before the run. The release
binary ran for three seconds with
`XDG_CONFIG_HOME=/tmp/gimbal-preview-r1-smoke/config`,
`XDG_STATE_HOME=/tmp/gimbal-preview-r1-smoke/state` and `GIMBAL_LOG=1`, then
its own app PID was stopped with SIGTERM. No panic; the log shows a built
window, `preview: selection None`, folder listing and a scratch session save.
No `file.open`, `app.disk-usage`, clipboard write, real Trash operation or
desktop setting was triggered.

## Not verified live

**[inferred]**

- Gutter visual alignment at a long file's top, middle and bottom with a
  narrow sidebar, wrapped lines, blank/final lines, Unicode and live font/scale
  changes. The range/coordinate logic is unit-tested, but the actual pixel
  alignment against the text baseline is not.
- The monitor timer in a real GTK main loop (the tests use a GLib main context;
  the app smoke run did not edit externally).
- The actual Save As / Replace / changed-on-disk / dirty-close / Quit dialogs,
  and the two-window Quit sequence with new edits.
- Save As/Replace cancellation and stale replies driven through the real file
  chooser and worker.
- Alt-click editing, text-key ownership, image/media previews and the original
  P2m live checklist (still Kyle's acceptance run).

## Remaining limitations / outside this brief

- **[read]** `SourceId::remove` is tied to the default main context. The
  production timer runs on the app's main context; the tests serialise access
  to that context rather than constructing a GTK window.
- A wrapped logical line whose first row is above the viewport keeps its label
  clipped at the line start; continuation rows are deliberately not numbered.
  If Kyle wants a continuation row numbered while its start is off-screen, that
  is a new look decision, not implemented here.
- The existing query/delete and ETag check-to-replace residual races are
  unchanged. No new dependency, manifest/lockfile, command, config, session
  format or runner change was made.
- No file outside the brief's Output was edited.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Reviewer ran the pinned `scripts/check.sh`. The sandbox run
passed fmt/clippy/tests/release, then failed RustSec refresh with github.com DNS
resolution. The network-enabled rerun passed all five gates: **487 passed,
17 ignored** (core 264, UI 198, CLI 7, agent 1, isolated Trash parents 6 + 11).
`cargo deny`: `advisories ok, bans ok, licenses ok, sources ok`; script ended
`all checks passed`. Logs: `/tmp/gimbal-p2m-r1-review-check.log` and
`/tmp/gimbal-p2m-r1-review-check-network.log`. `git diff --check` also passed.

**[read/observed tests]** The expired monitor handle is now taken before its
callback fires the check. Production MonitorTimer tests cover expiry/reschedule,
document change and cancellation. Clean force_load drops the old idle pin, and
the real-file reload test verifies a subsequent save ticket uses the new revision.
Document/buffer tokens reject stale Save As core transitions. Gutter now derives
the visible range from buffer coordinates. Quit rescans current window activities.
These address substantial parts of the prior review.

**Live: [observed]** No reviewer app launch or desktop interaction. Original
P2m and r1 visual/dialog acceptance remains pending. No user data/Trash/clipboard
operation. Independent Rust probes used only files under the reviewer-owned
`/tmp/gimbal-preview-r1-review-kib6521u`; source and `result.log` remain there.

**Still open:**

- **[observed state probe; read UI] P1, Save As to a new name always fails.**
  UI preview.rs:1373 calls start_save_as immediately after save_as_destination
  sets AwaitingReplace. start_save_as requires Starting; the probe reproduced
  `Err("That Save As reply is stale; start Save As again.")`. The missing-path
  branch must confirm then start, with the same validated request context. The
  failed branch also leaves the pending request blocking future Save/Save As.
- **[observed callback-pattern probe; read UI] P1, Replace panics.**
  UI preview.rs:1650 matches directly on session.borrow_mut().confirm_save_as(id)
  and mutably borrows again in both arms. A probe against the actual core and
  this exact borrow pattern caught `RefCell already borrowed`. Store the result
  before matching. Existing core tests do not exercise the UI's borrow lifetime.
- **[read] P2, successful-save close still skips idle revalidation.**
  finish_or_abort_close takes the close request/wait, then schedules an
  unconditional proceed; window.close's proceed uses close_now to bypass the
  guard. New edits/save between completion and idle are not checked. Keep and
  revalidate the accepted request in the idle, as the Discard path already does.
  Old dialog replies also need to be scoped to the request they belong to.
- **[read] P2, Quit can be stranded by an ordinary close already in progress.**
  With A's dirty window-close prompt open, Quit from B selects A. guard_close
  returns true for A's existing CloseWait, dropping Quit's callbacks. Cancel A
  leaves Quit active; Save/Discard closes A without advancing Quit while B stays
  open. Future Quit requests are ignored. Distinguish Busy from owning the Quit
  continuation and cancel this attempt without disturbing A. Inactive/stale
  advances must also be no-ops, not QuitStep::Finish.
- **[observed core probe] P2, explicit Reload overwrites pending selection.**
  reload_pinned assigns candidate to the old pinned path. A dirty -> Trash ->
  Reload probe returned File(A) rather than Trash. The same applies to B/no/
  multiple selection. Preserve the actual candidate and reconcile after
  abandoning edits. The earlier implementation notes' preservation claim above
  is not supported by this code path.

**[observed]** Wrote `P2m-r2-save-and-close-routing.md`, updated the Phase 2
index and handoff log. No production source fixes, commit or push. Existing
unrelated P2l-r3 notes and Zen captures were left untouched. Review remains open;
the next revision needs production callback/adapter tests, not only manually
sequenced core state tests.
