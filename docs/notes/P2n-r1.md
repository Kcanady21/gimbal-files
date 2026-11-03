# P2n-r1: authorization, real results and bounded transport

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## What I built

- **[read] Immutable job authorization.** `JobAuthorization` captures the
  enable generation ticket, instance and resolved protected roots when an agent
  operation is accepted. `FileOps` carries the same value through its queue, the
  worker's planning/confirmation/execution path and the final result. Off
  revokes the `GenerationTicket` immediately, drops queued agent jobs, cancels
  an agent job that has not published its execution boundary, and clears the
  bridge's undelivered requests. A worker that has already entered execution
  keeps its original policy and its protected-path checks. Unrelated UI jobs and
  the ordinary UI Cancel button are unchanged.
- **[read] Clipboard paste lifecycle.** `file.paste` now reserves a bridge job
  before the clipboard read, returns `accepted` plus its token, and passes the
  original authorization into the callback. The callback rechecks revocation
  before creating a request. The file-op worker resolves and checks the actual
  Copy/Move sources and destination, and revalidates the resulting plan. Empty
  or invalid clipboard data, read failures, planning failures and dry runs
  update that job (`failed`/`complete`). A dry run returns real plan lines and
  does not clear the clipboard; a successful cut paste still clears it.
- **[read] Real command outcomes and target purity.** The bridge validates the
  command, caller, arguments and tab ownership before any focus change. A
  refused, unknown or malformed command and every dry run leave focus,
  selection, folder and config untouched. `Outcome::Failed` is new in
  `fm-core`'s registry; model errors, config-save failures and unimplemented
  branches propagate through the real handler and become typed socket errors.
  `user_prompt_pending` uses real dialog/dirty/saving state, not the preview's
  initially-false close-bypass flag.
- **[read] Filesystem work and shutdown.** Dry-run planning, `nav.go` path
  resolution and plan work run on a fixed two-worker pool with a 32-job bounded
  queue. The file-op worker performs request and plan protection checks off
  GTK. `Server::stop` publishes Off first, stops the accept loop and removes the
  owned socket without joining a client worker that may be blocked in a
  filesystem resolver. Completed client handles are reaped.
- **[read] Config and transport boundaries.** The whole `[agent]` section is
  validated before its bool applies; unknown keys, wrong shape/type and
  read/parse failures leave automation Off with the existing diagnostics, in
  every key order. The inbound bridge queue is bounded at four with nonblocking
  admission, requests have a 64-id session budget, abandoned deliveries are
  dropped, `run`/`job` require instance and generation, and the CLI discovers
  both with a `state` handshake.

## Gate output

`./scripts/check.sh` (pinned 1.95.0 toolchain) ended:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
fm-agent lib:              23 passed, 0 failed, 0 ignored
fm-agent socket.rs:        15 passed, 0 failed, 0 ignored
fm-cli main.rs:            12 passed, 0 failed, 0 ignored
fm-core lib:              306 passed, 0 failed, 0 ignored
fm-core trash.rs:           6 passed, 0 failed, 6 ignored
fm-core trash_browser.rs:  11 passed, 0 failed, 11 ignored
fm-ui main.rs:            284 passed, 0 failed, 0 ignored
doc-tests:                  0 passed
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **657 passed, 17 ignored, 0 failed**. Accepted P2n baseline was 635
passed, 17 ignored.

## New tests

- **[read]** `fm-agent` protocol:
  `run_and_job_require_an_explicit_instance_and_generation`.
- **[read]** `fm-agent` real sockets:
  `a_full_bridge_queue_returns_typed_busy_without_reaching_the_ui`,
  `a_timed_out_delivery_is_abandoned_and_never_dispatched`,
  `completed_sessions_do_not_accumulate_unbounded_thread_handles`,
  `duplicate_ids_and_the_session_request_budget_are_explicit`,
  `run_requires_a_matching_instance_and_generation`,
  `stop_is_not_blocked_by_a_blocked_filesystem_resolver`.
- **[read]** `fm-core` config:
  `an_invalid_agent_section_is_off_in_every_key_order` (key-first,
  enabled-first, scalar/array shape, mixed nested type and string bool).
  `automation_reads_true_and_wrong_types_leave_it_off_with_a_problem` now
  asserts Off for an unknown key plus `enabled = true`.
- **[read]** `fm-ui` bridge:
  `one_generation_ticket_revokes_all_queued_and_pending_contexts`,
  `an_agent_job_keeps_its_original_policy_after_a_later_enable`,
  `a_revoked_clipboard_callback_has_no_effect_and_no_new_job`,
  `a_paused_clipboard_callback_dies_at_off_and_is_not_revived_by_on`,
  `capability_projection_marks_system_and_ui_only_commands_unavailable`,
  `a_dry_run_result_is_complete_but_writes_nothing`.
- **[read]** `fm-ui` bounded work:
  `a_full_queue_is_refused_without_blocking`,
  `results_keep_the_request_generation_and_late_evidence_is_visible`.
- **[read]** `fm-ui` execution boundary:
  `a_revoked_ticket_never_publishes_the_execution_boundary`,
  `an_active_ticket_publishes_execution_once_and_keeps_its_context`.
- **[read]** `fm-ui` prompt predicate:
  `clean_close_and_quit_need_no_human_prompt`,
  `dirty_or_saving_or_dialog_windows_block_close_and_quit`.
- **[read]** `fm-ui` clipboard seam:
  `the_injected_reader_delivers_success_and_failure_without_gtk`.
- **[read]** `fm-cli`:
  `write_requests_name_the_discovered_instance_and_generation`.
- **[read]** Existing job-history tests were updated to the new reserved job id
  and file-op attachment model; they still prove generation scoping, bounded
  terminal history and that queued work is not reported complete.

These exercise production paths: real Unix sockets and the real transport
worker, the same `GenerationTicket`/`JobAuthorization` used by the UI, the same
`begin_execution` called by the file-op worker, the same fixed work pool and the
real config loader. The blocked-resolver socket test uses an injected hook and
a barrier, calls the production `Server::stop`, then verifies the client worker
exits and the socket is removed.

## Regression evidence

- **[read] Source reasoning, not a reproduced pre-fix failure:** the review's
  probes established the old invalid-section, unbound-parser and clipboard
  `Ok(None)` behavior. I did not rerun those probes against the pre-fix tree.
- **[read] Reproduced after the fixes:** the new socket tests exercise actual
  bounded queue admission, abandoned delivery, session exhaustion, missing/stale
  binding, duplicate ids, blocked resolver shutdown and worker reaping.
- **[read] Reproduced after the fixes:** the new config regression loads an
  owned temporary file and observes Off plus diagnostics for both key orders.
- **[read] Not reproduced as a live race:** the submit → Off → PlanReady and
  pause-read → Off → deliver paths are covered through the production ticket,
  worker boundary and work-pool gates, not a timed GTK window.

## API citations

- **[read]** `gio-0.22.10:src/socket.rs:24` and
  `src/auto/socket.rs:233` remain the safe credential path; no new unsafe code.
- **[read]** `std::sync::mpsc::SyncSender::try_send` is the bounded,
  nonblocking admission primitive. `std::thread::JoinHandle::is_finished`
  drives worker reaping.
- **[read]** `gtk4-0.11.5:src/auto/switch.rs:57` / `:144` still back the Settings
  switch; no GTK API changed for this round.
- **[read]** `toml_edit 0.25.17` still does all wire encoding/parsing. The new
  binding rule is checked after the operation field, before arguments are read.
- **[read]** `fm_core::preview::EditorActivity` (`src/preview.rs:702`) supplies
  the real `dirty`/`saving` state used by `prompt_pending`.

## Not verified live

- **[inferred]** Settings On/Off/persistence across restart, including the
  status label and visible save/start failures.
- **[inferred]** CLI `state`, a disposable copy dry run, a real scratch copy
  job with terminal results, and job polling through an actual On cycle.
- **[inferred]** Clean and dirty `window.close`/`app.quit` through the real
  window/dialog stack.
- **[inferred]** Responsiveness and revocation while a real queued job or
  conflict dialog is open, and the actual clipboard round trip.
- **[inferred]** The app was not launched and no live socket/config was used.
  All tests use owned `/tmp/gimbal-agent-*` fixtures.

## Anything outside my files

- **[read]** `Cargo.lock` retains the accepted generated edges (`gio` and
  `toml_edit` for `fm-agent`, `fm-agent` for `fm-ui`). No package or version was
  added.
- **[read]** No new registry command, caller permission, dependency, `unsafe`
  or GTK-free rule exception was added.
- **[read]** The documented limitation remains: this protects the reviewed
  Gimbal interface, not the same-user account. A separate same-user shell,
  debugger or desktop-automation process can still edit files or click the UI.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** `scripts/check.sh`: all five gates pass, **657 passed,
17 ignored**, release and deny pass. Host-enabled output is
`/tmp/gimbal-p2n-r1-review-check.log`. No production source patch in this review.
P2n's previous 635/17 was a reviewed baseline, not an accepted implementation.

**[read] Addressed:** Invalid agent-section loading now fails closed; Run/Job
require instance/generation; inbound admission and per-session ids are bounded;
completed handles are reaped within a server. FileOps carries immutable
JobAuthorization and revokes queued/planning jobs separately from job history.
Paste reserves a queryable job, and the worker checks its actual file request.
The close predicate uses dirty/saving activity, and model/config errors have a
Failed outcome. These are substantial fixes, but the following remain:

- **[P1, read] Explicit window targets can affect another window.**
  agent/mod.rs run_prepare and finish_pending construct UiHandler directly,
  focusing the model only when an optional tab is supplied. app.rs normal
  dispatch_for_window first focuses the model window; the socket path does not.
  Core tab_new/tab_close/window_close/nav_go use the model's focused window.
  With B focused, a request naming A without a tab can act on B, including
  closing it. Deferred navigation also needs target revalidation on completion.
  Additionally paste dry-run falls through run_prepare's unconditional target
  tab focus, violating dry-run model/focus purity.
- **[P2, read] Stale clipboard failure can corrupt a new job.** enable and
  disable reset next_job; the callback captures a numeric reserved id and calls
  fail_job even on the revoked branch. fail_reserved looks up only that number
  in the current runtime. Old generation j1 can therefore fail new generation
  j1 after Off/On. Scope every callback update to its full original identity.
- **[P2, observed isolated socket probe] Workers are bounded per Server, not
  across enable cycles.** A production Server/ResolverHook probe held five
  resolver calls blocked, stopped that server, then repeated on the same owned
  runtime path. It observed five and then ten blocked workers after the two
  stops. All exited after the release gate opened. Server::stop detaches the
  old workers and a new Server starts with a fresh budget. Stop also cannot
  close a resolver-blocked client's stream with its current ownership model.
  Use a process-lifetime bounded worker owner and explicit client shutdown.
  The existing blocked-resolver test's single Barrier rendezvous releases the
  resolver before Stop, so it does not establish the claimed blocked-stop case.
- **[P1, read + observed isolated plan probe] Protection is stale after a
  decision wait.** worker.rs checks request/plan before Planned, then waits;
  begin_execution checks only revocation. A scratch probe used the production
  planner/protection/executor in that order, changed the owned destination
  directory to a symlink into its protected fixture between guard and execute,
  and observed a copy into the protected fixture. A fresh guard refused it.
  This was a staged component reproduction, not a GTK race or user-data test.
  Recheck paths after the answer immediately before execution; the accepted
  narrow final-query/filesystem-effect race remains and needs no atomic snapshot.
- **[P2, read] Contextual refusals still report success.** app.rs
  PageRoute::Refuse returns Done; nav.up/search.open on the Trash page are
  examples. Return an actual refusal while retaining the UI message. The r1
  tests also omit the required real bridge invalid-tab/save-failure cases,
  queue-to-PlanReady transitions and actual injected paste callback. Ticket and
  job-table helper tests do not establish those handoffs. Add production-path
  regressions around the failures above, including two windows and Off/On.

**[observed] Probe isolation:** Socket probe executable and source are in
`/tmp/gimbal-agent-r1-review-xp4mbqzd`; the host-enabled run printed
`blocked client workers=5` then `=10` and confirmed all workers exited after
release. The initial sandbox invocation could not bind its scratch Unix socket.
The staged plan probe and result.log are in
`/tmp/gimbal-agent-r1-plan-review-67d7d44z`. Only owned scratch directories/files
were created/renamed/copied. No Trash, system clipboard or external app launch.

**Live:** Not performed. No app launch or access through Kyle's live socket or
configuration. The live checklist remains deferred until the lifecycle/routing
fixes pass static review. No commit or push. Unrelated Zen notes/captures remain
separate.

**Still open:** P2n-r2-targets-callbacks-and-worker-lifetime.md gives the five
fix groups and deterministic production integration coverage. Preserve all
correct r1 caller/config/transport fixes; no permission expansion is needed.

## Corrections after P2n-r2 (Nov 3, 2026)

- **[read] Worker bound:** r1 said completed client handles were reaped within a
  server and Stop stayed bounded. That was true per `Server`, but each new
  enable cycle started a fresh eight-client budget. A blocked resolver worker
  from an old cycle therefore survived without an owner, and two cycles could
  hold ten workers. The final implementation uses one process-lifetime budget
  of eight, refuses fresh clients with `busy` at capacity, and shuts down every
  connected stream on Off. `stop_is_not_blocked_by_a_blocked_filesystem_resolver`
  now uses separate entered/release gates; the old single Barrier released the
  resolver before Stop, so it did not prove the claimed case.
- **[read] Callback identity:** r1 said a revoked clipboard callback had no
  effect and no new job. That covered its file submission, but its failure path
  still called `fail_job` with a numeric reserved id. After Off → On a new
  generation's `j1` could be failed by the old callback. Failure, attachment
  and terminal updates now require the full original job token; a terminal job
  is never overwritten.
- **[read] Target and dry-run claims:** r1 said a refused/malformed command and
  every dry run left focus untouched. Registry validation did, but an accepted
  command naming window A without a tab still acted on focused window B, and
  `file.paste` dry-run fell through the target-tab focus. The final bridge binds
  an exact window/default-tab target after validation, revalidates it after
  deferred evidence, and gives every dry run a read-only target context.
- **[read] Contextual refusals:** r1 still described model/config failures as
  typed errors, but `PageRoute::Refuse` returned `Done`. It now keeps the
  visible Trash explanation and returns `Failed`.
- **[read] Production-test claim:** several r1 UI tests exercised ticket
  predicates or a synthetic job table rather than the real callback or bridge.
  P2n-r2 adds real-socket bridge tests, an actual injectable paste callback,
  queue/PlanReady/conflict Off paths, and real worker revalidation with owned
  scratch fixtures. The final-revalidation race remains the documented narrow
  query/effect window.
