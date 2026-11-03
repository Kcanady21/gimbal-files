# P2n-r2: exact targets, scoped callbacks and bounded workers

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## What I built

- **[read] Exact execution targets.** The bridge now resolves an
  `ExecutionTarget { window, tab }` for every `run`: the requested window plus
  its explicit tab or the window's default focused tab. Validation stays pure;
  only after it passes does the bridge bind that model window and tab. The
  actual `Registry::dispatch` result is what becomes the reply. A deferred
  `nav.go` revalidates that the window and tab still exist and belong together
  after the lookup; removal or movement returns `invalid-target` without
  touching another window. Dry runs, including `file.paste` dry-run, use the
  target only for validation and never focus or change selection, folder, the
  Trash page or configuration.
- **[read] Scoped callback results.** Job reservations are updated by their
  full original token (`instance/jN`), not by a recycled local number.
  `fail_reserved`, `attach_fileops` and terminal history updates require an
  exact token match; a terminal job is never overwritten. A stale paste
  callback from an earlier enable cycle can neither fail nor attach to the new
  generation's `j1`, and its revoked ticket prevents any file submission.
- **[read] Process-wide transport workers.** `fm-agent` now owns one
  process-lifetime budget of eight client workers. Every `Server` draws from
  the same owner, so repeated Off/On cycles cannot allocate another eight while
  old resolver calls are blocked. At capacity a fresh client gets a typed
  `busy` reply. Off publishes revocation, stops accepting, shuts down every
  connected stream (`shutdown(Both)`) and removes only its own socket; peers see
  EOF promptly even when their worker is still inside a blocked resolver. No
  per-generation cleanup thread or unbounded detached worker was added.
- **[read] Final protection checks.** The file-op worker repeats the request
  and full planned-step protection after the answer/confirmation and
  immediately before the execution boundary. It checks the immutable ticket
  again after the blocking resolution and keeps the original roots. A
  destination alias changed while the decision was paused is refused before a
  write; an undo step whose folder became a protected alias is refused too.
  The documented final-query/filesystem-effect atomicity limit is unchanged.
- **[read] Truthful refusals and production seams.** A `PageRoute::Refuse`
  keeps the visible Trash explanation but returns `Outcome::Failed`; the socket
  reply is a typed error, not `ok`. Headless test seams use the real registry,
  handler, `FileOps` worker, queue and socket transport: no second command
  runner, no system clipboard and no GTK window. `FileOps` gains a manual-poll
  mode in tests so its GLib source cannot be iterated from another test thread.

## Gate output

`./scripts/check.sh` with the pinned 1.95.0 toolchain:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
fm-agent lib:              23 passed, 0 failed, 0 ignored
fm-agent socket.rs:        16 passed, 0 failed, 0 ignored
fm-cli main.rs:            12 passed, 0 failed, 0 ignored
fm-core lib:              306 passed, 0 failed, 0 ignored
fm-core trash.rs:           6 passed, 0 failed, 6 ignored
fm-core trash_browser.rs:  11 passed, 0 failed, 11 ignored
fm-ui main.rs:            299 passed, 0 failed, 0 ignored
doc-tests:                  0 passed
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **673 passed, 17 ignored, 0 failed**. P2n-r1 baseline was 657 passed,
17 ignored. Log: `/tmp/gimbal-p2n-r2-check.log`.

## New and changed regressions

- **[read]** `fm-agent` socket:
  `repeated_enable_cycles_share_one_fixed_worker_budget` opens five blocked
  resolver calls, stops that generation, starts another with three more,
  accepts no ninth worker, gets a typed `busy` for a fresh client, confirms
  peer EOF on both Offs and releases every owned gate to zero workers.
  `stop_is_not_blocked_by_a_blocked_filesystem_resolver` now uses separate
  entered/release gates, asserts Stop returns while the resolver is still
  blocked, asserts the peer read EOF before release, and only then releases.
- **[read]** `fm-ui` worker:
  `a_destination_alias_changed_after_planned_is_refused_before_the_write`,
  `revocation_while_the_final_resolution_is_paused_never_executes`,
  `move_rename_and_new_folder_are_revalidated_after_planned`, and
  `an_undo_plan_is_revalidated_after_the_decision_wait`.
- **[read]** `fm-ui` bridge:
  `tab_and_window_commands_bind_the_requested_window_without_a_tab`,
  `a_paused_lookup_whose_target_disappears_returns_invalid_target`,
  `forbidden_malformed_and_invalid_targets_leave_the_other_window_alone`,
  `contextual_refusal_and_config_save_failure_are_typed_errors`,
  `stale_paste_callbacks_cannot_change_a_new_generation_job`,
  `same_cycle_paste_failures_update_only_their_own_job`,
  `paste_dry_run_binds_no_target_and_reports_real_plan_lines`,
  `a_paste_into_a_protected_destination_fails_without_a_write`,
  `a_queued_agent_job_behind_ui_work_is_dropped_by_off`,
  `a_plan_arriving_after_off_never_prompts_or_executes`, and
  `a_job_waiting_for_a_conflict_is_cancelled_by_off_without_a_write`.

## Regression evidence

- **[read] Did not reproduce before the fix in this tree:** the r1 review's
  two-window routing, recycled `j1`, per-server five-plus-five worker probe and
  stale-plan alias copy were established by the reviewer, not rerun against the
  old tree here. The new tests exercise the corrected production paths.
- **[read] Reproduced after the fixes:** all new tests above pass, including
  real TCP-free Unix sockets, the production `Server::stop`, the real
  `GenerationTicket`/`JobAuthorization`, the real `FileOps` worker and the real
  `Registry` handler. The pre-existing `fm-agent` and `fm-ui` tests still pass.
- **[read] Test isolation:** all runtime sockets, config, files and windows are
  under `/tmp/gimbal-agent-*`; clipboard data is injected; the system clipboard,
  Kyle's Trash, config and live socket are never touched. Bridge tests share a
  process-wide test lock because transport workers are deliberately
  process-wide.

## API and implementation notes

- **[read]** `std::net::Shutdown::Both` via the owned `UnixStream` is the
  prompt-disconnect primitive; `gio-0.22.10:src/socket.rs:24` remains the safe
  credential path.
- **[read]** `std::sync::OnceLock` + atomics own the process-lifetime worker
  budget; `JoinHandle::is_finished` still reaps finished handles.
- **[read]** The target binding uses `Workspace::focus_window` followed by
  `Workspace::focus_tab`; it deliberately does not call the UI's pre-validation
  focus wrapper and does not leave the Trash page, so Trash route decisions
  still see the real page.
- **[read]** No new dependency, lockfile change, registry command, caller
  permission or GTK-free exception was added. `Cargo.lock` is untouched.

## Not verified live

- **[inferred]** Settings On/Off/restart and the status label through the real
  GTK menu.
- **[inferred]** Two real windows with A targeted while B is focused, through
  the live compositor.
- **[inferred]** CLI `state`, scratch dry-run and a real copy job result.
- **[inferred]** Clean/dirty close and queued-work revocation in the live app.
- **[inferred]** The app was not launched and no live socket/config was used.

## Anything outside my files

- **[read]** None. The final narrow race between the last protection query and
  the filesystem effect remains documented in `docs/agent-socket.md`; no atomic
  snapshot or OS sandbox is claimed.

## Review and live check (ChatGPT, Nov 3, 2026): passes with reviewer fix

**[observed]** Initial host `scripts/check.sh` passed all five gates:
673 passed, 17 ignored. After the reviewer fix and new regression, final host
gates pass: **674 passed, 17 ignored**, release and deny pass. Final output:
`/tmp/gimbal-p2n-r2-review-final-check.log`. Tracked whitespace check passes.

**[read]** The five r2 groups are addressed: explicit window/default-or-explicit
tab routing is bound at dispatch; deferred navigation revalidates its target;
paste dry-run does not bind focus; callback updates match full job tokens and
ignore terminal/stale records; transport workers share one process-wide budget
and Off shuts down their streams; request and plan protection is repeated after
decisions and revocation rechecked before execution. Contextual Trash refusals
now return Failed. New tests drive real sockets, bridge handlers, injected paste
callbacks and FileOps queue/worker transitions with entered/release gates.

**Fix (ChatGPT): release the model borrow before focus events.**
**[read]** app.rs bind_execution_target matched directly on
workspace.borrow_mut().focus_window/focus_tab. The temporary mutable borrow
lasted through the match arm's apply_events. A changed tab emits FocusChanged;
the live frame/header/content refresh reads the workspace again and could panic
with a RefCell borrow error. Headless windows did not perform that UI read.
Both focus results now go into local variables before event delivery, ending
the borrow first. The new test-only event-delivery invariant checks that model
events can reborrow the workspace even without widgets. The existing headless
close-window helper received the same borrow-lifetime correction.

**[observed]** New regression
`binding_another_window_and_tab_releases_the_model_before_events` failed before
the fix with the invariant message and passes afterward. It changes both the
window and selected tab through the production binding method. Logs:
`/tmp/gimbal-p2n-r2-binding-before.log` and
`/tmp/gimbal-p2n-r2-binding-after.log`. This reproduces the model/event borrow
violation headlessly; no live GTK panic is claimed. Final UI count: 300 passed.
Protocol docs also received a wording correction for invalid config/default Off
and clarified that request-path resolution is worker work.

**Live:** Not performed by the reviewer. No app launch, Kyle socket/config,
system clipboard, user-data mutation, real Trash or desktop interaction. Existing
and new tests use their isolated fixtures. The previously accepted narrow
filesystem revalidation race remains documented; no atomic snapshot is claimed.

**Still open:** Kyle's live acceptance below. No further revision brief needed
from this review. No commit/push; unrelated Zen notes/captures remain separate.

### Live checklist for Kyle

1. Close the old Gimbal process and launch the newly built binary from the repo:

   ```sh
   cd '/home/kyle/Documents/03.) Code-Projects/gimbal-files'
   ./target/release/gimbal-files
   ```

   In another terminal in the same repo, run
   `./target/release/gimbal-files-cli state`. With Settings → Allow local
   automation Off, it should explain that automation is off/app unavailable.
   Turn the switch On; the menu should show On and the socket path, and state
   should print windows/tabs and the current instance/generation.

2. Create a disposable fixture in that terminal:

   ```sh
   SCRATCH=$(mktemp -d /tmp/gimbal-agent-kyle-XXXXXX)
   mkdir -p "$SCRATCH/dest"
   printf 'socket live check\n' > "$SCRATCH/example.txt"
   ```

   Open two Gimbal windows, A and B, and use state to identify A's window id.
   Keep B focused. Set WID below to A's full id printed by state, then run:

   ```sh
   WID='<window id for A from state>'
   ./target/release/gimbal-files-cli run --window "$WID" nav.go path="$SCRATCH"
   ```

   A should navigate to the disposable folder; B's folder/tabs stay unchanged.

3. Run a copy dry run, then execute and follow its job:

   ```sh
   ./target/release/gimbal-files-cli run --window "$WID" file.copy \
     paths="$SCRATCH/example.txt" to="$SCRATCH/dest" dry-run=true
   ./target/release/gimbal-files-cli run --window "$WID" --wait file.copy \
     paths="$SCRATCH/example.txt" to="$SCRATCH/dest"
   ```

   Dry run should print plan lines while dest remains empty. The real command
   should report the job reaching complete; dest contains example.txt and the
   original remains. A failed request must not claim completed success.

4. Try `./target/release/gimbal-files-cli run agent.set-enabled enabled=false`.
   It should be refused and the Settings switch should remain On. Turn it Off
   yourself; state should fail promptly. Turn On again: the previous WID should
   be stale/refused; a fresh state gives new ids. Update WID from that fresh state.
   Repeat the copy into the same destination to open a conflict, then turn Off
   from B's Settings before answering. No replacement or extra copy should happen,
   and the app should remain responsive.

5. Check normal browsing, previews and clean window close. With a disposable
   text buffer edited but unsaved, close/Quit must retain the human save/discard
   guard; a client close/Quit should report busy rather than bypass it. Finally
   turn automation Off, close/relaunch, and confirm it remains Off. On persistence
   can also be checked by restarting once while On, then finishing with Off.

## Authorized live socket check (ChatGPT, Nov 3, 2026): checks so far pass

**[observed]** Kyle explicitly enabled automation and asked the reviewer to test
the live socket. That supersedes the earlier review-only prohibition for these
checks. Host socket access was needed because the execution sandbox refuses
Unix socket connections. No desktop input injection or system clipboard access.

**[observed]** State reports On. A copy dry run aimed at a nonfocused tab returns
real plan lines, writes nothing and preserves the complete window/tab/focus/
selection projection. Real copy of an owned scratch file returns accepted; job
polling reaches complete with no errors, matching bytes and the original intact.
A direct framed socket request for agent.set-enabled enabled=true is forbidden;
automation remains On. This checks server policy rather than only CLI validation.

**[observed]** Created two scratch-only windows, B initially focused. Navigation
targeting A without a tab affects A and leaves B's folders/tabs unchanged. Tab
new/close also affect A. Both owned windows then closed successfully, and a fresh
state projection exactly matches the original windows, folders, focus and
selections. No original window closed or user file changed.

Fixtures/results: /tmp/gimbal-agent-live-review-ilin3l0r (results.json,
multiwindow-results.json and job-result.toml; the latter contains parsed result
JSON despite its suffix). No cleanup deletion or Trash operation. A connected
client is waiting for Kyle to manually turn Off; disconnect/fresh-query refusal
are pending. Restart persistence, dirty-close interaction and conflict-time Off
remain unverified live. No new source changes, gate rerun, commit or push.

**[observed, Off completed]** Kyle reports turning the switch Off. The existing
client disconnected and a fresh CLI state returned exit 3 with the socket absent.
The watcher completed successfully; off-result.json records it in the fixture
directory above. Leave automation Off; restart/dirty-close/conflict-time Off are
still unverified live.

## Live acceptance (Kyle, Nov 3, 2026): accepted through r2

**[Kyle reports]** The automation setting persists across restart. Closing with
unsaved changes stops the close and presents the expected popup. Together with
the authorized live socket checks above, P2n through r2 is accepted. Latest
source gates remain 674 passed, 17 ignored; no source change or gate rerun.
Conflict-time Off was covered by automated production-path tests but was not
separately observed live. No commit/push in this update; Zen remains separate.
