# P2m-r2: finish Save As and close/quit routing

## What was built

Tags: **[observed]** the code and tests in this working tree; **[read]** pinned
API or GTK/libadwaita source; **[inferred]** behaviour that needs Kyle's live
display/interaction.

- **[observed] Missing Save As destinations advance to the saving stage.**
  `apply_destination_reply` in `crates/fm-ui/src/preview.rs` records the
  worker's `Destination` result, then for `Destination::Missing` calls
  `confirm_save_as` followed by `start_save_as`. Each operation borrows the
  session separately; no borrow spans the next call. A real failure cancels
  that exact request so a stale `SaveAsRequest` cannot block later Save or
  Save As attempts. The backend still refuses an arriving file through
  `create_new` and the request id/stage/token checks are unchanged.
- **[observed] Replace confirmation no longer holds a borrow across the arms.**
  `confirm_and_start_save_as` stores the `confirm_save_as` result before it
  calls `start_save_as`. The Replace dialog callback is now:
  `confirm_and_start_save_as(&panel.session, id)` followed by
  `panel.start_save_ticket(...)`, so clicking Replace cannot panic with
  `RefCell already borrowed`. The stale arm cancels only that request id and
  performs no write or status change on another editor. The dialog keeps
  Cancel as its default and close response, and the core still binds a Replace
  confirmation to the destination path and exact `FileRevision`
  (`save_as_existing_requires_a_confirmation_bound_to_that_revision`).
- **[observed] Successful-save close continuations revalidate in their idle.**
  `finish_or_abort_close` now leaves the accepted `CloseRequest` in the
  session and schedules `apply_saved_close_idle`. That idle upgrades the same
  `PreviewPanel`; `complete_saved_close` rechecks the exact save ticket,
  matching close request, no active save, no saving editor and no new dirty
  buffer before taking the request and running its `proceed`. Anything else
  aborts and clears the wait. A request that names a different save job is left
  untouched.
- **[observed] Close-dialog replies are scoped to their request.** The
  saving and Save/Discard/Cancel dialog callbacks capture their `CloseRequest`
  and ignore a reply once the session no longer holds that request. The
  discard idle does the same before its `close_ready` check. `abort_close` and
  the saved-close idle settle session state before dismissing the stored
  dialog, so the `close_response` that `force_close` emits is ignored rather
  than cancelling a newer request.
- **[observed] Existing window closes report Busy instead of being adopted by
  Quit.** `CloseGuard` is `Free`, `Waiting` or `Busy`,
  `close_guard_decision` returns `Busy` for an already armed request without
  replacing its wait, and `guard_close` keeps the original dialog and
  continuation intact. `advance_quit` cancels its own attempt on `Busy`;
  ordinary repeated `window.close` requests return `Outcome::Done` and never
  fall through to closing the model.
- **[observed] Quit requests carry an identity.** `QuitCoordinator` stores
  `active: Option<u64>` and `handled`; `begin` returns the new request id,
  `cancel(request)` only clears that request, and `next(request, ...)` returns
  `QuitStep::Inactive` for anything else. `advance_quit` captures the id in
  its proceed/cancel closures and checks `is_active` before final `quit_now`.
  A cancelled or superseded callback is a no-op. The existing full-window
  rescan is preserved.
- **[observed] Reload keeps the actual pending selection.** `PreviewSession::
  reload_pinned` abandons the dirty pin and its pending Save As/close state but
  no longer writes the pinned path into `candidate`. It reconciles to the
  unchanged candidate: Reload A when A is still selected, load B when B is
  selected, or show the neutral empty/multiple/Trash state. `force_load`
  increments the generation, so a late result for the old pinned file is
  rejected.

## Correcting the P2m-r1 note

**[observed]** The P2m-r1 notes said explicit Reload "preserves the pending
selection". That claim was not supported by the code it described:
`reload_pinned` assigned `candidate = File(pinned_path)` before reloading, so a
dirty A reviewed from B or Trash pulled the preview back to A. P2m-r2 removes
that assignment and adds candidate/document/action-state regressions. The
earlier P2m and P2m-r1 notes and review history are left intact; this file
corrects the claim going forward.

## Production callbacks and adapters tested

**[observed]** The tests drive the same functions the UI calls, not a parallel
model:

- `apply_destination_reply` (called by `PreviewPanel::apply_destination`) for
  Choosing -> CheckingDestination -> Missing -> confirm -> accepted
  `SaveTicket`, with a real `/tmp/gimbal-preview-r2-*` source and destination.
- `confirm_and_start_save_as` (called by the Replace dialog callback and the
  missing-destination branch) with a `RefCell<PreviewSession>`: valid Replace
  and a stale reply after A was discarded and B became the pinned editor.
- `complete_saved_close` (called by `apply_saved_close_idle`, scheduled from
  `finish_or_abort_close`) for normal success, fresh edit before idle, new save
  before idle, cancelled request, and a superseded newer close.
- `close_guard_decision` (called by `PreviewPanel::guard_close`, used by
  `window.close` and `advance_quit`) with a `RefCell<PreviewSession>` for an
  existing close then Quit, and after Cancel/Save/Discard/save failure.
- `QuitCoordinator::begin`, `next(request, ...)`, `cancel(request)` and
  `is_active(request)` for repeated Quit, rescanning and stale continuations.
- `PreviewSession::reload_pinned` for B, Trash, no selection, multiple
  selection and same-file reload, including a late A result.

## Design details

**[observed]** `close_guard_decision` checks an existing `CloseRequest` or
wait before it checks whether the preview is dirty or saving. That ordering is
deliberate: a repeated ordinary close (and a Quit that reaches the same
window) must stay `Busy` even during the short interval after a discard when
the buffer is already clean but the idle has not yet consumed the request.
`Busy` never mutates the session or stored wait.

**[observed]** `saved_close_decision` only considers a close request that names
the completed ticket (`close.save == Some(ticket.id)` and matching token). If
the panel now holds a different close request, the late save idle returns
`SavedClose::None` and leaves the newer request and wait in place. An orphaned
wait with no request is cleared.

**[read]** `libadwaita-0.9.2:src/auto/dialog.rs:376` provides
`DialogExt::force_close`; the C implementation is
`../gimbal-refs/libadwaita-1.9.1/src/adw-dialog.c:1979`. An `AdwAlertDialog`
emits its `close-response` when closed by the system
(`adw-alert-dialog.c:346-357`), so the close dialogs are force-closed only
after the session has taken or invalidated their request; the scoped callback
then sees no matching request and ignores the response.

**[read]** `glib-0.22.10:src/source.rs:353` is `idle_add_local_once`; the saved
close uses it so the window can be closed outside the save-result callback and
outside every `RefCell` borrow.

## Named regressions

**[observed]** Core (`crates/fm-core/src/preview/tests.rs`):

- `reload_of_dirty_a_previews_the_selected_b_instead_of_pulling_a_back`
- `reload_of_dirty_a_on_the_trash_page_stays_neutral_and_clears_actions`
- `reload_with_no_or_multiple_selection_keeps_that_neutral_state`
- `reloading_the_still_selected_file_uses_a_fresh_load_of_that_file`

UI preview (`crates/fm-ui/src/preview.rs`):

- `a_missing_save_as_destination_is_confirmed_started_and_written`
- `a_valid_replace_reply_confirms_and_starts_without_reentering_the_borrow`
- `a_stale_replace_reply_performs_no_write_or_mutation_of_a_later_editor`
- `a_saved_close_waits_for_its_idle_and_proceeds_exactly_once`
- `a_fresh_edit_before_the_saved_close_idle_keeps_the_window_open`
- `a_new_save_before_the_saved_close_idle_keeps_the_window_open`
- `a_cancelled_saved_close_never_runs_its_continuation`
- `a_superseded_close_is_not_aborted_by_a_late_save_idle`
- `a_late_save_result_only_aborts_a_continuation_it_owns`

UI app (`crates/fm-ui/src/app.rs`):

- `a_quit_does_not_take_over_an_existing_window_close_request`
- `quit_works_again_after_an_existing_close_is_cancelled`
- `quit_works_again_after_an_existing_close_is_saved`
- `quit_works_again_after_an_existing_close_is_discarded`
- `a_window_close_save_failure_clears_its_continuation_and_allows_a_new_quit`
- `stale_quit_continuations_cannot_finish_a_cancelled_or_new_request`

The existing Quit rescan tests
(`quit_rechecks_a_clean_window_dirtied_during_another_prompt`,
`a_handled_window_edited_again_is_asked_again`,
`a_newly_created_dirty_window_joins_a_pending_quit`,
`repeated_quit_and_cancel_are_scoped_to_one_request`,
`final_quit_requires_no_dirty_or_saving_window`,
`a_save_failure_cancels_the_whole_pending_quit`) were updated to the request
id API and still pass.

## Gates

**[observed]** `./scripts/check.sh`, pinned toolchain, from the repository
root on Nov 3, 2026. Log: `/tmp/gimbal-p2m-r2-check.log`.

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored
test result: ok. 7 passed; 0 failed; 0 ignored
test result: ok. 268 passed; 0 failed; 0 ignored
test result: ok. 6 passed; 0 failed; 6 ignored
test result: ok. 11 passed; 0 failed; 11 ignored
test result: ok. 213 passed; 0 failed; 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Total: **506 passed, 17 ignored** (baseline 487/17; four new core and fifteen
new UI tests). Counts by target: agent 1, CLI 7, core 268, isolated Trash
parents 6 + 11, UI 213.

## App smoke run

**[observed]** `pgrep -a gimbal-files` was empty first. The release binary ran
for three seconds with `XDG_CONFIG_HOME=/tmp/gimbal-preview-r2-smoke/config`,
`XDG_STATE_HOME=/tmp/gimbal-preview-r2-smoke/state` and `GIMBAL_LOG=1`, opened
`/tmp/gimbal-preview-r2-smoke/note.txt`, selected it, started load generation 1
and built the window. It was stopped with SIGTERM on its own app PID. No panic.
The log also shows the environment's existing accessibility-bus warning and a
Vulkan `VK_SUBOPTIMAL_KHR` warning; neither came from these changes. No
`file.open`, `app.disk-usage`, clipboard write, real Trash operation or desktop
setting was triggered.

## Not verified live

**[inferred]** No agent interaction was injected. These remain for Kyle:

- new-name Save As through the real `GtkFileDialog` and worker, including bytes
  and the unchanged original;
- existing-name Replace and Cancel, including the default Cancel and a changed
  destination;
- long/wrapped gutter scrolling at top, middle and bottom in a narrow sidebar;
- clean and dirty external changes twice, including the change dialog;
- Reload while browsing B or Trash, same-file reload, and late results;
- dirty close Save/Discard/Cancel and the saved-close idle against a fresh edit
  made in the idle interval;
- two-window Quit where A already owns an ordinary close dialog, then Cancel,
  Save, Discard or save failure in A; retry Quit; repeated Quit; stale
  continuations;
- the original P2m checklist and the accepted Trash/DnD regressions.

## Remaining limitations / outside this brief

- **[read]** The accepted filesystem revalidation/commit race is unchanged.
  Save As to a missing path still creates a zero-length placeholder with
  `create_new` before the conditional GIO replacement, and only its own
  placeholder is removed on failure.
- **[read]** `force_close` on an `AdwAlertDialog` emits its `close_response`;
  correctness here depends on the session request being taken or invalidated
  before the dialog is dismissed. That ordering is implemented in
  `complete_saved_close`, `abort_close` and the dialog callbacks.
- **[inferred]** The two-window Quit sequence and the dialog timing are
  covered by state-level production tests, not by live GTK dialogs.
- No dependency, manifest/lockfile, command, caller-policy, config/session
  format, runner, spike or desktop change. No file outside the brief's Output
  was edited.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** Static review passes; Kyle's live acceptance is pending. Reviewer
ran the pinned `scripts/check.sh`: fmt, clippy, tests and release passed in the
sandbox; RustSec refresh then failed to resolve github.com. The network-enabled
rerun passed all five gates: **506 passed, 17 ignored** (core 268, UI 213, CLI 7,
agent 1, isolated Trash parents 6 + 11). `cargo deny` ended `advisories ok, bans
ok, licenses ok, sources ok`, followed by `all checks passed`. Logs:
`/tmp/gimbal-p2m-r2-review-check.log` and
`/tmp/gimbal-p2m-r2-review-check-network.log`. `git diff --check` passed.

**[read/observed tests]** All five r2 findings are addressed:

- `apply_destination_reply` drives Missing through confirmation and Starting,
  then issues the accepted SaveTicket. Its production-adapter scratch test
  writes/reads the new file, checks unchanged original bytes and clears active
  request/save state after completion. Exclusive-create backend checks remain.
- `confirm_and_start_save_as` releases its confirmation borrow before starting
  the save. Replace uses that helper; valid and stale RefCell adapter tests pass.
  The ticket preserves the confirmed destination revision and stale replies
  do not change the later editor.
- A saved-close request remains in the session until the idle. The production
  `complete_saved_close` validates the exact save/close plus current dirty/save
  state before taking it. Fresh edits, a newer save, cancellation and a newer
  close are covered; normal success proceeds exactly once. Session state is
  settled before force-closing the stored dialog, and callbacks check the
  request before acting. Verified force_close/close-response behavior against
  local libadwaita 1.9.1 source, adw-dialog.c:1979 and
  adw-alert-dialog.c:346–357.
- Existing ordinary close returns Busy without replacing its continuation.
  Window-close treats Busy as blocked; Quit cancels only its own attempt.
  Request ids reject cancelled/superseded Quit callbacks, and the full current
  window rescan remains. Production guard/coordinator tests cover retry after
  Cancel/Save/Discard/failure, repeated Quit and stale callbacks.
- `reload_pinned` preserves the actual candidate and invalidates abandoned
  actions. Tests check B/Trash/none/multiple/same-file results, document paths
  and rejection of a late A load. The incorrect r1 preservation claim is
  explicitly corrected above.

**[read]** No additional blocker found in these revised paths. No new command,
dependency, manifest/lockfile, config/session format, runner or spike changes.
The accepted filesystem revalidation/commit limitation remains unchanged.
Existing unrelated P2l-r3 notes and Zen captures remain separate and untouched.

**Live: [observed]** No reviewer app launch or desktop interaction. The reviewer
ran the automated tests, including their owned scratch-file operations; no
user-data/real-Trash/clipboard operation. The agent's smoke run above does not
establish actual GTK dialog, key-routing or gutter behavior.

**Still open: [inferred] Kyle's live acceptance.** Use disposable files and the
latest release binary. Suggested numbered report:

1. Select text with blank/Unicode/wrapped lines; scroll a long file to its
   middle/bottom and narrow the sidebar. Numbers stay aligned and count logical
   lines once. Empty text works; binary/oversized text shows a refusal.
2. Select images and unsupported/media files. Images fit without forcing window
   width; cached media thumbnails or the unavailable fallback show correctly.
3. Alt-click text, edit, Undo/Redo, select/copy/paste and Delete/Backspace. These
   affect text, not file selections. Save persists the edit and returns read-only.
4. Save As to a new name; original bytes stay unchanged. Save As over an existing
   disposable file: Cancel leaves it unchanged, Replace saves without a crash.
5. Leave A dirty, select B, browse/open Trash and collapse/reopen the sidebar.
   A stays named and pinned. Discard returns to the actual selected/neutral view.
6. Change the file externally twice: clean previews reload without crashing;
   dirty previews retain edits and warn. While B or Trash is visible, Reload
   abandons A's edits and returns to B or neutral Trash. Cancel retains edits.
7. Dirty close: Cancel keeps edits, Save persists/closes, Discard closes. With
   A's close question open, Quit from B leaves that question intact; after it is
   resolved, retry Quit works. A window dirtied during another Quit question is
   also prompted. Recheck sidebar/theme/floating behavior and accepted Trash/DnD.

**[observed]** Updated the Phase 2 index and handoff log. No reviewer production
fixes, commit or push. No further brief written; await Kyle's live report.

## Kyle's live report and next brief (ChatGPT, Nov 3, 2026)

**[observed: Kyle reports]** Long/wrapped text scrolling and line numbers work;
images fit the sidebar. Alt-click editing, text Undo/Redo/Delete and Save work.
Save As writes successfully, but the preview returns to the old file instead
of staying on the new one. Dirty navigation preserves edits. Closing with
unsaved changes produces the expected warnings. This report does not establish
all close-dialog outcomes, two-window Quit, Replace/Cancel or external-change
cases from the prior checklist.

**[observed: Kyle reports]** Remaining issues and requested behavior:

1. Keyboard movement through files does not update preview as pointer clicks do.
2. Visual previews (PDF/images) are too low-resolution to read.
3. Main-grid icons have no content thumbnails.
4. Main-grid filenames cannot be read in full.
5. Hovering main-grid icons gives no useful feedback. In clarification Kyle
   explicitly chose a full details tooltip and highlight, not a large hover preview.
6. A pinned folder should open with one click, replacing double-click activation.
7. Selected filename and size should appear in the bottom status bar.
8. Preview filename header should wrap fully by default; double-click toggles
   a compact single ellipsized line and back.
9. Text gutter/scroll and image fitting are accepted as above; keep them.
10. Save As should keep preview ownership on the newly saved path.
11. Dirty pinning is accepted, but its explanation should be larger and blink
    on a fresh selection attempt to draw attention.
12. Unsaved-close warnings work; preserve them.

**[read]** Source inspection supports several gaps: cells are explicitly
non-selectable to avoid duplicate custom click selection; only pointer selection
is connected to the model in the inspected factory. Grid cells bind type icons,
cap labels at two lines and set no details tooltip. pin_release explicitly maps
one click to Select and double-click to Open. Status currently formats counts
only. Preview title always ends with ellipsize End; pinned explanations use the
small expiring status. Save As's actual passive-refresh ownership failure still
needs a production-path trace; no definitive cause is claimed here.

**[read/observed helper inventory]** GIO 0.22.10 exposes normal/large/xlarge/
xxlarge thumbnail path and validity constants; current preview code tries the
generic path first. Native image decode caps its longest edge at 2048, and no
zoom/re-render flow exists. Reviewer read installed `/usr/bin/pdftoppm -h`:
Poppler 26.01.0 supports first/last/singlefile, PNG, scale and crop options.
`pdftocairo` is also installed; poppler-glib pkg-config was unavailable. No PDF,
user file or renderer output was opened/created by that inventory command.

**[observed]** Wrote `P2m-r3-preview-and-file-details.md` to cover the requested
keyboard/model updates, readable image/PDF rendering with local zoom, grid
thumbnails, full selected names/details hover, pinned single-click, selected
status, wrapped/toggleable header, durable Save As preview ownership and larger
pulsing dirty notice. This deliberately expands the previous cache-only PDF
scope in response to Kyle's request, using installed optional pdftoppm with
bounded first-page jobs and no new Rust dependency; helper absence falls back
clearly. Production adapter, resource/cancellation and refresh-chain tests required.

**[observed]** No application source edit, test rerun, commit or push for this
report/brief. Latest reviewed gates remain 506 passed, 17 ignored. Updated index
and handoff log. P2m remains open; preserve the specifically accepted behavior,
and review r3 before committing or moving to the next phase brief.
