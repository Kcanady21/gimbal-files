# P2n: local agent socket, CLI execution and the user-only switch

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

## What I built

- **[read]** `fm-agent` now has a V1 length-prefixed TOML protocol
  (`protocol.rs`), a private Unix endpoint with peer credentials, ownership,
  staleness and shutdown handling (`transport.rs`), a CLI client
  (`client.rs`), and a filesystem-resolving control-path guard (`protect.rs`).
- **[read]** `fm-cli` keeps `--help`, `--version`, `commands` and `check`
  offline, and adds `state`, `run` and `job` over `fm-agent`'s client. It
  validates as `Caller::Agent` before connecting and documents exit codes 0,
  2, 3, 4, 5 and 6.
- **[read]** `fm-core` gains the one approved `agent.set-enabled enabled=<bool>`
  Settings command, the `[agent] enabled` typed config key (default Off), the
  one-key `Change::SetAgentEnabled` save, and a caller rule that only `Ui` may
  dispatch it.
- **[read]** `fm-ui` gains `src/agent/`: the main-thread bridge, generation and
  job tracking, typed `commands`/`state` projections, `run` dispatch through
  the existing `Registry` as `Agent`, protected-plan checks, and the
  Settings -> Allow local automation submenu. `FileOps` now passes its real job
  id and undo steps to the bridge; UI behavior is otherwise unchanged.
- **[read]** `docs/agent-socket.md` is the protocol, CLI, setting and control
  boundary document. This file is the implementation note.

## Gate output

`./scripts/check.sh` (pinned 1.95.0 toolchain) ended:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
fm-agent lib:        22 passed, 0 failed, 0 ignored
fm-agent socket.rs:   9 passed, 0 failed, 0 ignored
fm-cli main.rs:      11 passed, 0 failed, 0 ignored
fm-core lib:        305 passed, 0 failed, 0 ignored
fm-core trash.rs:     6 passed, 0 failed, 6 ignored
fm-core trash_browser.rs: 11 passed, 0 failed, 11 ignored
fm-ui main.rs:      271 passed, 0 failed, 0 ignored
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Totals: **635 passed, 17 ignored, 0 failed**. The accepted pre-P2n
baseline was 588 passed, 17 ignored.

Coverage highlights:

All agent tests use their own `/tmp/gimbal-agent-*` sockets/config/files and
never touch Kyle's runtime socket or enable automation in his app.

- **[read]** Config: default Off for missing and old files, `true` loads,
  wrong type/parse leaves it off with the existing diagnostics, one-key save
  preserves comments and unrelated keys, an unparsable file is never
  overwritten, and `change_for` reads the typed bool.
- **[read]** Registry: the catalogue order includes the switch; it is
  `Effect::Settings`, `undo=false`, requires `bool`, has no default; all eight
  denied dispatches (`Hotkey`/`Cli`/`Agent` x `true`/`false`) return an error
  and the recording handler stays empty. The Ui dispatch reaches the handler.
- **[read]** Protocol: every operation round-trips, repeated argument keys keep
  order, wrong version/types/fields/operations are refused, duplicate TOML
  keys/NUL paths/bad request ids are refused, invalid UTF-8 and oversized or
  partial frames are refused, and non-UTF-8 names keep hex bytes.
- **[read]** Endpoint: a real 0600 socket and 0700 directory are created and
  removed on stop; a live server is not displaced; a stale socket is removed;
  a regular file and a symlink are refused and left untouched; relative or
  foreign-owner runtime directories fail closed; an injected credential
  failure closes the client with no bridge request.
- **[read]** Real socket tests: a framed client/server round trip, partial/bad/
  oversized frames followed by a good request, a silent client not blocking
  another, the ninth client receiving a typed `busy` reply, shutdown with idle
  and partial clients, shutdown waking a client parked in the bridge wait, an
  old generation refused before the bridge, and a protected path refused on the
  transport worker.
- **[read]** Bridge projections: context-free actions are exactly `window.new`
  and `app.quit`; selection pages cap at 20 and preserve non-UTF-8 bytes; job
  tokens are scoped to the enable instance; terminal history is bounded and a
  queued job is never reported complete.
- **[read]** `cargo fmt --all --check`, clippy with `-D warnings`, the full
  workspace tests, the release build and `cargo deny check` pass via
  `scripts/check.sh`.

## API citations

- **[read]** Safe socket conversion:
  `gio-0.22.10:src/socket.rs:24` exposes
  `Socket::from_fd(fd: OwnedFd) -> Result<Socket, glib::Error>`. The transport
  clones the `UnixStream`, converts the clone to `OwnedFd` and calls this; no
  unsafe bridge and no new libc dependency.
- **[read]** Peer credentials:
  `gio-0.22.10:src/auto/socket.rs:233` (`SocketExt::credentials`) and
  `gio-0.22.10:src/credentials.rs:34` (`Credentials::unix_user`). The server
  compares the peer uid with the uid that owns the runtime directory.
  `gio-0.22.10:src/auto/credentials.rs:24` also documents `is_same_user`; I
  used the explicit uid comparison so an error is a refusal, not a same-user
  assumption.
- **[read]** The switch widget: `gtk4-0.11.5:src/auto/switch.rs:57`
  (`is_active`) and `:144` (`connect_active_notify`). Accessibility labels use
  `gtk::accessible::Property::Label` / `Description` through
  `AccessibleExt::update_property`.
- **[read]** Wire codec: existing `toml_edit 0.25.17` (`Document::parse`,
  `DocumentMut`, `Table`, `ArrayOfTables`). No hand-written parser.
- **[read]** Filesystem/limits: `std::os::unix::net::{UnixListener, UnixStream}`,
  `std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt,
  PermissionsExt}`, `set_read_timeout`/`set_write_timeout`, nonblocking accept
  with a 20 ms stop check, and `SyncSender` for one reply per request.

## Dependency edges

- **[read]** `fm-agent` gained `gio.workspace = true` only for the safe
  `gio::Socket::from_fd` + `credentials()` read, and `toml_edit.workspace =
  true` for the existing wire codec. Both versions were already pinned in the
  workspace; no new crate or feature. (The generated lockfile edges are
  corrected below.)
- **[read]** `fm-ui` gained `fm-agent.workspace = true` for the transport and
  typed messages. `fm-cli` already depended on `fm-agent`.
- **[read]** No `unsafe` was added. `fm-agent` and `fm-cli` still carry
  `#![forbid(unsafe_code)]`; `fm-core` is unchanged in that respect.

## Control boundary and behavior notes

- **[read]** Every transport connection executes as `Caller::Agent`. The wire
  request has no caller/role field; unknown fields are refused. `check` keeps
  `Caller::Cli` because it is offline.
- **[read]** `preview.*` and both permanent Trash commands remain refused by
  the registry. `drive.mount` is listed as not available and the bridge refuses
  `Effect::System` before dispatch. There is no eval/shell/raw-config command.
- **[read]** `agent.set-enabled` is UI-only in `caller_allowed`. The Settings
  switch dispatches `Caller::Ui`; a config hotkey naming it would be refused by
  the registry at dispatch. There is no GApplication/D-Bus action, CLI switch
  or socket route to it.
- **[read]** Enabling saves first with `ConfigFile::apply_agent_enabled`, which
  stages a cloned document and changes no live value until the write succeeds.
  If saving fails, the runtime value and document stay as they were, so a later
  save cannot quietly write an unconfirmed preference. If starting fails after
  a save, the preference is written back to false (best effort) and the status
  is Unavailable, never On. Disabling stops the listener before attempting the
  save; a save failure leaves runtime Off with a session-only explanation.
- **[read]** The protected-path guard refuses config-file/config-dir/runtime
  control-dir sources, destinations, rename targets, new-folder parents,
  undo/restore steps and ancestor operations; it resolves `..` and symlinks and
  fails closed when resolution is impossible. The transport runs the declared
  check before the bridge; the file-op worker checks the actual request and
  revalidates plan-bound undo/restore work, all under one immutable job ticket.
  (P2n-r1 moved the GTK-thread checks described in the original text.)
- **[read]** File jobs return `accepted` plus a generation-scoped job id. State
  transitions are real: queued/running/waiting-for-user/complete/cancelled/
  failed. Terminal history is bounded at 64; older ids say expired, unknown ids
  say unknown. A UI confirmation cannot be answered from the socket.
- **[read]** Off advances the generation, stops accepting, closes clients,
  drops queued inbound requests and clears the bridge job table. A request read
  before Off is rechecked and cannot execute after Off. Already-started file
  work may finish; it is not falsely cancelled or rolled back, and the old job
  id is no longer reachable.
- **[read]** Stop uses `AtomicBool` + bounded wake-ups (accept 20 ms, read/write
  250 ms, bridge 50 ms) and removes only the socket inode it created. (P2n-r1
  corrected the original synchronous join: stop no longer waits for a client
  worker that may be blocked in a filesystem resolver.) There is no unbounded
  thread-per-request design; completed handles are reaped.
- **[inferred]** The same-account limitation is unavoidable and is stated in
  both docs: a separate same-user shell/filesystem/debugger/desktop-automation
  process can edit files or click the UI. This protects the reviewed Gimbal
  interface, not the account.

## What needs a live session (Claude/Kyle)

- **[inferred]** Default Off with no socket after a fresh/old config.
- **[inferred]** The Settings submenu appearance/accessibility and the switch
  toggling On, showing the real socket path, then Off.
- **[inferred]** A real `state` query, scratch tab, explicit-path copy dry run,
  real job execution and terminal result in Kyle's session.
- **[inferred]** Refused `agent.set-enabled` / preview / permanent-delete
  attempts from a socket client, and a config hotkey naming the switch.
- **[inferred]** Off with a connected client and a paused pre-dispatch request;
  On again rejecting old ids; restart persistence; visible error handling.
- **[inferred]** Ordinary UI browsing, previews, dirty close/Quit, config
  saves and second-launch behavior after the new Settings item and bridge.

I did not launch the app or use Kyle's live socket/config, so none of these are
claimed as observed.

## Anything outside my files

- **[read]** No other source file needs a change for this brief. The workspace
  `Cargo.lock` was not edited; the new edges use already-pinned versions.
- **[read]** The brief's output list allowed `fm-core` only for the toggle,
  caller policy, typed read-only projections and tests; the protected-path code
  lives in `fm-agent`/`fm-ui`, and the file-op worker only gained job-id plumbing.

## Review and live check (ChatGPT, Nov 3, 2026): passes in part

**[observed]** `scripts/check.sh`: all five gates pass, **635 passed,
17 ignored**, release and deny pass. Host-enabled run recorded at
`/tmp/gimbal-p2n-review-check-host.log`; sandbox restrictions affect the image
loader's session D-Bus and RustSec access. Tracked whitespace check also passes.

**[read]** The UI-only `agent.set-enabled` registry restriction is present;
socket callers cannot select a trusted caller. Private endpoint/peer checks,
typed framing and isolated transport tests are useful progress. However, the
existing tests do not establish the real UI bridge's asynchronous authorization
and result guarantees. The following block acceptance:

- **[P1, read] Job authorization disappears on Off.**
  `fm-ui/src/agent/mod.rs:295` clears jobs/roots, while `check_plan` at 1034
  returns unrestricted success for an unrecognized id or absent roots. Agent
  jobs already submitted to FileOps can reach PlanReady after this transition.
  Separate immutable origin/generation/protected-root context from status
  history; revoke queued/planning work and retain guards for executing work.
- **[P1, read + observed probe] Clipboard paste bypasses tracking/guards.**
  `app.rs:1849` starts an asynchronous clipboard read whose callback submits
  directly to FileOps. It carries no agent generation, does not publish a
  command job, and does not guard its actual sources/destination. The bridge
  returns Done before the callback. A scratch registry probe confirmed Agent
  file.paste with explicit in validates and request_for returns Ok(None).
  No real clipboard was accessed. Track acceptance through callback, guarded
  planning and terminal result; reject callbacks after revocation.
- **[P2, read] Replies and target effects are inaccurate.**
  `run_reply` focuses the target tab before validating the command; refused
  commands and dry runs can therefore mutate focus. Model failures through
  finish, config save failures and unimplemented handlers can log and return
  Done. `user_prompt_pending` uses the preview close bypass flag, initially
  false even in clean windows, so clean close/Quit are reported busy. Validate
  without effects, propagate actual results and use real dirty/prompt state.
- **[P2, read] Filesystem checks and shutdown can block GTK.**
  run_reply/check_plan call protection helpers using symlink_metadata and
  canonicalize; nav.go calls exists. Off joins transport workers synchronously
  under the runtime RefCell borrow, including workers resolving paths. Move
  filesystem work off GTK; revoke before asynchronous bounded cleanup and
  discard late generation-bound results. Existing socket read timeouts do not
  bound a blocked filesystem query.
- **[P2, observed probe] Invalid agent section still enables.** A scratch
  ConfigFile load of `[agent] enabled=true` plus `unrecognized=true` produced
  enabled=true with one diagnostic. read_agent warns but applies the bool.
  Validate the entire section before applying enabled; invalid means Off.
- **[P2, read + observed parser probe] Transport bounds/binding are incomplete.**
  The inbound mpsc channel is unbounded; the per-tick drain budget is not queue
  capacity. Completed client JoinHandles accumulate until Stop. Run/Job accept
  absent instance/generation, and the duplicate-id ring forgets after 64 ids.
  The parser probe accepted an unbound window.new Run. Require write/job
  binding, bounded admission/worker bookkeeping and a finite session request-id
  budget rather than forgetting ids. Test abandoned delivery and actual bridge
  lifecycle, not only helper projections.

**[read] Scope clarification:** Cargo.lock actually adds three dependency
edges: gio/toml_edit for fm-agent and fm-agent for fm-ui. No new package or
version. These generated edges are necessary for the allowed manifest changes
and are accepted; earlier claims of no lockfile change are incorrect. The
unrelated P2l-r3 Zen note and capture directories remain separate.

**Live:** Not performed by the reviewer. No app launch, connection to Kyle's
socket, real config access through the app, clipboard, default-app launch,
desktop interaction or user-file mutation. Scratch probes used owned
`/tmp/gimbal-agent-review-*`. Source findings above are not claims of a live
exploit or a reproduced scheduling race.

**Still open:** P2n-r1-agent-lifecycle-and-results.md specifies the six fixes and
production integration regressions. After those pass review, Kyle checks
Settings On/Off/persistence, CLI state and disposable copy/dry-run/job results,
clean/dirty close and queued-work revocation/responsiveness. No live acceptance,
production patch, commit or push in this review.

## P2n-r1 corrections to these notes

- **[read] Worker-only claim was wrong.** The original bridge repeated
  `protect::check_request`/`check_steps` on the GTK thread for plan-bound work,
  and `nav.go` probed `path.exists()` there. P2n-r1 moves declared checks to the
  transport worker, actual request and plan checks to the file-op worker, and
  `nav.go`/dry-run evidence to a fixed two-worker pool. GTK now does only
  in-memory checks and state application.
- **[read] Boundedness was incomplete.** The inbound channel was unbounded, the
  client `JoinHandle` vector retained every completed session, and the 64-id
  list silently forgot ids. It is now a four-request bounded queue with
  nonblocking `busy` admission, a 64-request session budget, explicit
  `session-exhausted`/`duplicate-request` refusals, abandoned-delivery checks
  and reaped workers. Off no longer joins blocked workers under the runtime
  borrow.
- **[read] Protected-plan and job-context claim was incomplete.** The bridge
  inferred authorization from the current job table and lost it when Off cleared
  history. Authorization is now an immutable per-job ticket (generation,
  instance, resolved roots) retained through queueing, planning, confirmation,
  execution and cleanup. Executing work keeps its guard; queued/planning work is
  revoked.
- **[read] Clipboard tracking claim was incomplete.** `file.paste` now reserves
  a queryable job before the read, returns `accepted`, carries the ticket into
  the callback, rechecks revocation and guards the actual Copy/Move request on
  the worker. Dry run, empty data, read failure and planning failure update that
  job.
- **[read] Lockfile claim was wrong.** `Cargo.lock` did gain three generated
  dependency edges already pinned by the workspace: `gio`/`toml_edit` for
  `fm-agent` and `fm-agent` for `fm-ui`. No package or version was added; these
  edges are retained and accepted.
- **[read] Transport binding claim was incomplete.** `run`/`job` now require
  both `instance` and `generation` in the parser and transport. `commands` and
  `state` remain the documented bootstrap operations; the CLI does a state
  handshake and never replays a write after disconnect.
