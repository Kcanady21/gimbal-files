# Gimbal Files: local agent socket (V1)

<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

This document describes exactly what a local script may do, what it can see, and
what it can never do. It is the post-implementation contract for P2n.

## Turning it on

1. Open Gimbal Files.
2. Open the main menu and choose **Settings**.
3. Switch **Allow local automation** on.

The switch is Off for every old, missing, unreadable or unparsable config. It is
the only way to change the setting: there is no CLI flag, socket operation,
D-Bus/GApplication action, launch option or hotkey that can turn it on or off.
The command behind the switch is `agent.set-enabled enabled=<bool>` with
`Effect::Settings`; the registry refuses `Agent`, `Cli` and `Hotkey` before any
handler or config write. The switch dispatches it as `Caller::Ui`.

The setting is saved as exactly:

```toml
[agent]
enabled = true
```

The existing one-key `toml_edit` save keeps comments and unrelated settings.
The entire `[agent]` section is validated before its bool applies: an unknown
key, wrong shape or type, or an unreadable/unparsable document leaves automation Off
with the existing readable diagnostics, in every key order.

**On** means the config save succeeded **and** the listener started. If either
fails, the switch shows Off or Unavailable and no socket is active. Turning it
off closes the listener and every connected client immediately, even if saving
the preference fails; the menu then says Off for this session and that the
preference could not be saved.

## Endpoint

```
$XDG_RUNTIME_DIR/gimbal-files/agent.sock
```

- `$XDG_RUNTIME_DIR` must be an absolute, real folder owned by the current user.
- `gimbal-files/` is created `0700`; `agent.sock` is `0600`.
- Symlinked runtime/control paths, a foreign owner, a non-socket at the path,
  unsafe permissions or a live server are refused. A stale socket is removed
  only after its type and owner are verified and a connect attempt returns
  `ECONNREFUSED`. The app removes only the socket device/inode it created.
- Every connection's kernel peer credentials must name the server's uid before
  a frame is read. Credential failure is a refusal.
- There is no `/tmp` fallback, TCP/HTTP port, abstract namespace, network
  listener or client-started app. A second ordinary app launch keeps the
  accepted GApplication behavior and does not take this endpoint.

## Control boundary

Every socket connection, including `gimbal-files-cli`, executes as
`Caller::Agent`. The server chooses this; the wire schema has no caller, role or
privilege field, and unknown fields are rejected. The offline CLI `check`
command keeps `Caller::Cli`, but that does not make the transport trusted.

Unavailable through the socket (refused by the registry before any handler):

- `preview.edit`, `preview.discard`, `preview.save`, `preview.save-as`
- `file.open-with`, `file.set-default-app`
- `file.properties`
- `app.settings`
- `file.open-as-admin`
- `tab.open-as-admin`, `tab.leave-admin`
- `trash.empty`, `trash.delete-permanently`
- `drive.mount`, `drive.unmount`, `drive.eject`
- `agent.set-enabled`
- `updates.set-enabled`, `updates.check`
- `integration.set-show-in-folder`
- unknown commands, unknown/duplicate arguments, context-filled `Files` paths
  that an agent did not supply, and every malformed invocation

`file.properties` is the read-only Properties window and `app.settings` opens
the Settings submenu; both are `System` commands, so a socket client is refused
before any handler.

There is no generic eval, shell execution or raw config write. Clipboard,
`Launch` and `System` operations keep their existing policy; `System` is listed
as not available and refused before dispatch. `drive.mount`, `drive.unmount` and
`drive.eject` are those operations: their Devices-sidebar actions are UI-only,
and no socket state/query/navigation/startup path can mount, unmount or eject a
volume. P2s did not add a socket command, protocol, or remote drive support.
`updates.set-enabled` and `updates.check` are also Settings-only `System`
commands; the socket lists them as unavailable and refuses them before
dispatch, so a local script can neither change nor trigger the update check.
`integration.set-show-in-folder` follows the same rule: its Settings switch is
the only caller, and no socket request can take over or release the
`org.freedesktop.FileManager1` name.

Administrator tabs are listed by `state` with `admin = true`, but every `run`
that names one, or leaves the tab implicit while the window's focused tab is
one, is refused with the typed error `administrator tabs are UI-only`. This is
checked after the target resolves and before any handler, plan or filesystem
work. There is no command, argument or state field that grants a socket client
administrator access.

Remote file operations must use explicit absolute paths. A relative path is
rejected, never resolved against Gimbal's working directory. A `dry-run=true`
`Files` request returns the real plan and writes nothing.

A socket file request may not touch:

- Gimbal's config file,
- its containing control directory (`.../gimbal-files`),
- the active runtime control/socket directory (`$XDG_RUNTIME_DIR/gimbal-files`).

Sources, destinations, rename targets, new-folder parents, undo/restore plan
steps and ancestor directories are checked. `..` and symbolic-link aliases are
resolved; an unresolvable path fails closed. The declared request is checked on
the transport worker before the bridge sees it. The file-op worker checks the
actual request (including clipboard sources/destination) before `Planned`, then
repeats the request and full planned-step protection after the answer or
confirmation and immediately before the execution boundary. A destination
alias that changes during a paused decision is therefore refused without a
write. The same immutable ticket (with its original roots) is checked again
after resolution; request-path resolution runs off the GTK thread.

### The honest limitation

"Only the user can change it" means **unchanged through this socket, the CLI or
the registry**. A process with separate same-user shell, filesystem, debugger or
desktop-automation authority can edit the config file, connect to the socket
when it is on, or click the UI. This is not an OS sandbox against arbitrary code
running as the same user, and it does not promise an unbreakable same-account
boundary. There is no sudo/polkit layer.

## Wire format

One request is one frame:

```
4-byte big-endian payload length | UTF-8 TOML document
```

- Maximum frame: 1 MiB.
- No JSON, line splitting, debug formatting or hand-written TOML parser;
  `toml_edit 0.25.17` encodes and parses.
- Unknown versions, unknown operations, unknown fields, wrong types, duplicate
  keys, invalid UTF-8, NUL bytes, invalid request ids and oversized frames are
  refused before the bridge.
- Maximum 128 `args` entries. Request ids, strings, arrays and pages are
  bounded.
- Eight clients at once, **four** parsed requests waiting for the UI, 15 s idle
  read timeout, 10 s reply-write deadline, 30 s bridge wait. Admission is a
  nonblocking `try_send`; a full queue gets a typed `busy` reply and is never
  dispatched later.
- One connection may use at most **64 request ids**. The duplicate-id list is
  never trimmed: the 65th request gets `session-exhausted` and the connection
  closes. Reusing an id on the same connection is `duplicate-request`. A client
  that needs more opens a fresh explicit connection; the CLI never replays a
  write automatically.
- A request whose bridge wait expires or whose connection stops is marked
  abandoned. The bridge checks that flag immediately before dispatch and drops
  it without running a command or file effect.
- One process-lifetime budget of **eight** transport workers covers every
  enable cycle. A worker still blocked in a filesystem resolver after Off keeps
  its slot, so a later On cannot allocate another eight. At capacity a fresh
  client receives a typed `busy` reply; the Settings switch and Off remain
  usable. Completed workers are reaped instead of retained in an unbounded
  handle vector, and no cleanup thread is started per generation.
- Off publishes revocation, closes the listener and shuts down every connected
  stream immediately, including a client whose worker is blocked in a resolver.
  The peer sees EOF promptly; a late resolver result cannot reach the UI.
- Parsing, encoding, socket reads and socket writes run on transport workers.
  The GTK thread only receives typed messages.

### Request schema

Common fields:

| field | type | required | meaning |
| --- | --- | --- | --- |
| `version` | integer | yes | exactly `1` |
| `request_id` | string | yes | 1–128 printable ASCII bytes; scopes the reply and replay detection |
| `operation` | string | yes | `commands`, `state`, `run` or `job` |
| `instance` | string | required for `run`/`job` | reject if it is not this enable cycle |
| `generation` | integer | required for `run`/`job` | reject if it is not this enable cycle |

`commands` and `state` are bootstrap operations and may omit `instance` and
`generation`. `run` and `job` must name both; an unbound write is refused with
`missing-field` before the bridge. The CLI performs a `state` bootstrap on each
invocation, then sends the discovered identity with every `run`/`job`.

`commands` and `state`:

| field | type | default |
| --- | --- | --- |
| `offset` | integer, 0..=1000000 | `0` |
| `limit` | integer, 1..=100 | `20` |

`run`:

| field | type | required | meaning |
| --- | --- | --- | --- |
| `window` | string | for window-bound commands | `state` window id |
| `tab` | string | no | `state` tab id, must belong to the window |
| `command` | string | yes | registry name |
| `args` | array of inline tables | no | `{ key = "...", value = "..." }`, repeated keys keep order |
| `wait` | boolean | no | CLI hint; the server answers with a job id |

`job`:

| field | type | required |
| --- | --- | --- |
| `id` | string | yes |

Example `commands` request:

```toml
version = 1
request_id = "cli-1234-1"
operation = "commands"
offset = 0
limit = 5
```

Example `run` request:

```toml
version = 1
request_id = "cli-1234-2"
instance = "8123-...-g3"
generation = 3
operation = "run"
window = "8123-...-g3/w4"
command = "file.copy"
args = [
  { key = "paths", value = "/tmp/gimbal-agent-example/a.txt" },
  { key = "to", value = "/tmp/gimbal-agent-example/dest" },
  { key = "dry-run", value = "true" },
]
wait = false
```

### Reply schema

Common fields: `version`, `request_id`, `operation`, `status`
(`ok`/`accepted`/`error`), `instance`, `generation`.

An error also has:

```toml
[error]
code = "protected-path"
message = "..."
```

Documented codes: `malformed`, `unsupported-version`, `unknown-operation`,
`unknown-field`, `bad-type`, `bad-value`, `bad-request-id`, `missing-field`,
`nul-byte`, `too-long`, `too-many-arguments`, `duplicate-request`, `stale`,
`disabled`, `unknown-command`, `forbidden`, `invalid`, `invalid-target`,
`protected-path`, `unsupported`, `busy`, `unknown-job`, `timeout`,
`failed`, `session-exhausted`.

`commands` data:

```toml
[data]
offset = 0
total = 57
next = 5

[[data.command]]
name = "file.copy"
summary = "Copy files into a folder."
effect = "files"
undo = true
allowed = true

[[data.command.argument]]
name = "paths"
kind = "paths"
presence = "from selection"
required = true
```

`group.open` is the newest registry entry. It takes repeated
`paths=<absolute folder>` arguments and opens them as one split group after the
focused pill; it is available to every caller, like `group.split`. A path that
is not an existing folder refuses the whole command.

`state` data:

```toml
[data]
offset = 0
total = 1

[data.socket]
status = "on"
path = "/run/user/1000/gimbal-files/agent.sock"
generation = 3

[[data.window]]
id = "8123-...-g3/w4"
focused = true
focused_tab = "8123-...-g3/t9"
groups_total = 1
groups_truncated = false

[[data.window.group]]
id = "8123-...-g3/g5"
number = 0
focused = true
split = false
tabs_total = 1
tabs_truncated = false

[[data.window.group.tab]]
id = "8123-...-g3/t9"
folder = "/tmp/example"
folder_hex = "2f746d702f6578616d706c65"
focused = true
selection_count = 1
selection_truncated = false

[[data.window.group.tab.selection]]
text = "a.txt"
hex = "612e747874"

[[data.job]]
id = "8123-...-g3/j7"
state = "running"
summary = "Copy files into a folder."
lines = []
errors = []
```

- Window, group and tab ids are opaque and scoped to one enable cycle; an id
  from an earlier enable or process instance is rejected. A `run` binds the
  exact requested window and its requested/default tab after validation and
  immediately before the effect. A refused, unknown or malformed request keeps
  the original focus and model state. A deferred lookup revalidates that the
  window and tab still exist and belong together; if either disappeared or
  moved, the reply is `invalid-target` and another window is untouched.
- A dry run, including `file.paste` dry-run, uses a read-only target context:
  it never focuses a tab or changes selection, folder, the Trash page or
  configuration.
- One state reply pages windows and caps groups at 64 per window and tabs at
  128 across the page; omitted groups/tabs keep their real total and a
  `*_truncated` flag. A selection shows its real count and at most 20 names. Non-UTF-8 names keep
  lossless hexadecimal bytes and omit `text`.
- No preview buffer, clipboard content, secret or file body is ever returned.

`run` data:

```toml
[data]
kind = "dry-run"      # or "done" or "job"
job = "8123-...-g3/j7" # only for kind = "job"
lines = ["dry run: copy 1 item(s) to /tmp/...", "copy /tmp/a -> /tmp/dest/a"]
```

`job` data uses the same fields as a `[[data.job]]` entry. Job states:
`queued`, `running`, `waiting-for-user`, `complete`, `cancelled`, `failed`.
An older terminal job outside the bounded history reports `expired`; a foreign
or malformed id reports `unknown`. `waiting-for-user` means an existing
Gimbal confirmation dialog must be answered in the UI; socket clients cannot
answer or impersonate it. A job is never reported complete merely because it
was queued.

## File search (view-only)

`search.open` is the existing V1 command. Its optional `query` argument keeps
the registry's nonempty `Text` rule and the existing caller validation.

- `search.open` without `query` only opens or focuses that window's search
  entry. It starts no traversal and changes no model state.
- A nonempty `query` starts a recursive filename search in the exact focused
  tab folder captured for that window. In a split group it is the focused
  tile's folder only. The `run` reply means the entry was opened or the scan
  was accepted; it does **not** mean traversal finished.
- Results are transient view state over the workspace. `state` continues to be
  the underlying workspace projection: it never lists search results, and
  opening or scanning never changes the reported folder, selection, focus,
  groups or tabs. Activating a result is an ordinary `nav.go` and is visible as
  that navigation. A search is not a file-operation job and never appears in
  `[[data.job]]`.
- Matching is filename-only: case-insensitive exact name/stem, prefix and
  substring first; for queries of at least three Unicode scalars it also accepts
  ordered-character subsequences and bounded typo distance. Results are ranked
  by `(class, edits, gaps, start)` then native relative path. At most 10,000 best
  rows are retained while the scan continues; the page and the accepted `run`
  reply do not claim a fixed latency. A full retained set is reported as partial
  with `result limit` and retained-versus-total matches; no result list is
  exported over the socket.
- A second `search.open` query in the same window replaces the first. A search
  in a ninth window can return the typed busy failure. Tab/folder changes,
  entering Trash, root removal, query replacement and window close revoke the
  old session; late batches are neither exported nor able to change another
  tab or window. Removal of a completed or partial scan's root (including a
  user-selected symlink alias) is observed by a focused root/parent monitor and
  a bounded worker check, then revokes the page as `Search unavailable`.
- Result activation is deferred to a bounded process-wide worker. One lookup
  may be pending per window; a second different result reports busy, and a
  repeated identical activation is deduplicated. Clear/Escape, replacement,
  tab/folder/window changes and shutdown revoke it. The worker resolves the
  exact native path; the UI revalidates the window, tab, root, query generation
  and result before dispatching the existing `nav.go` through registry
  validation. A vanished result explains itself without navigating, closing the
  page or selecting another item.
- The native bytes of an invalid-UTF-8 result never travel as a text command
  argument. The UI dispatches the existing `nav.go` with an absolute validated
  text path and a one-shot typed payload bound to that text and `Caller::Ui`;
  the handler consumes the payload after registry acceptance. Wire and CLI
  callers cannot supply or select this payload, and a mismatched target fails
  rather than falling back to a lossy path or another item.
- A search is ordinary UI view state, not a socket generation. Turning
  automation Off, restarting it, or a client disconnect does not create or
  cancel a search. The page remains for the user to close with Escape.
- Opening, clearing or closing the entry and Escape are UI lifecycle events,
  not commands. V1 has no socket operation to cancel, close or page search
  results, and no search-results export. A protocol search query waits for a
  later brief.
- Results are read-only. Existing file commands and `preview.edit` are refused
  with a readable explanation while the results page is shown.

## CLI

```
gimbal-files-cli --help
gimbal-files-cli --version
gimbal-files-cli commands
gimbal-files-cli check <command> [key=value ...]
gimbal-files-cli state [--offset N] [--limit N]
gimbal-files-cli run [--window ID] [--tab ID] [--wait] [--timeout SECS] \
    <command> [key=value ...]
gimbal-files-cli job <job-id> [--wait] [--timeout SECS]
```

`commands` and `check` work with no running app. `state`, `run` and `job` use
the socket and print the reply as typed TOML.

`run` validates the invocation offline as `Caller::Agent` before connecting.
It performs a `state` bootstrap to discover the current instance/generation,
then binds the write to that identity. `job` does the same bootstrap. A missing
or stale binding is refused; the CLI does not guess or reuse an old cycle.
Window-bound commands require `--window`; the server never falls back to
whatever window became focused while a request waited. Context-free
`window.new` and `app.quit` may omit it.

Exit codes:

| code | meaning |
| --- | --- |
| 0 | success, or a job reached a terminal state |
| 2 | usage, invalid command, missing/forbidden arguments |
| 3 | Gimbal is not running, or local automation is off |
| 4 | transport failure; a submitted request's outcome may be unknown |
| 5 | Gimbal refused the request |
| 6 | `--wait` timed out; the job may still be running |

A timeout or disconnect after submission is **outcome unknown**, never proof of
failure. The CLI never replays a `run` after reconnecting, because that could
duplicate a file operation. A missing socket says Gimbal is not running or
automation is off and points at Settings. There is no CLI enable/disable
command and no automatic server launch.

## Worked example (post-implementation behavior)

```sh
# 1. In Gimbal: Settings -> Allow local automation = On.
# 2. See the ids and socket state.
gimbal-files-cli state

# 3. Open a scratch tab (use the window id printed above).
gimbal-files-cli run --window '<window id>' tab.new path=/tmp

# 4. Validate a copy with explicit paths; nothing is written.
mkdir -p /tmp/gimbal-agent-example/dest
printf 'hello\n' > /tmp/gimbal-agent-example/a.txt
gimbal-files-cli run --window '<window id>' file.copy \
  paths=/tmp/gimbal-agent-example/a.txt \
  to=/tmp/gimbal-agent-example/dest \
  dry-run=true

# 5. Execute it and follow the real job.
reply=$(gimbal-files-cli run --window '<window id>' file.copy \
  paths=/tmp/gimbal-agent-example/a.txt \
  to=/tmp/gimbal-agent-example/dest)
printf '%s\n' "$reply"
job=$(printf '%s\n' "$reply" | sed -n 's/^job = "\(.*\)"$/\1/p')
gimbal-files-cli job "$job" --wait

# 6. In Gimbal: turn the switch Off. Connected clients close immediately and
#    a later state/run fails with exit code 3.
gimbal-files-cli state
```

The accepted reply is TOML; the snippet reads its `data.job` line. A client
may instead use the wire format above.

## Disable, restart and already-started work

- Off advances the generation, stops accepting, closes connected clients and
  drops undelivered UI requests. A request read before Off cannot execute after
  Off; the bridge rechecks enabled state and generation immediately before
  dispatch.
- Re-enabling creates a new instance/generation. Window, job and request ids
  from the previous cycle are stale and refused.
- A file operation that has already started may finish through the existing
  file-op worker and is not rolled back or falsely cancelled. Off stops further
  control; the user still has the ordinary UI Cancel button. Its job stops
  being reachable through the old generation.
- Authorization is an immutable ticket captured at acceptance, not a lookup in
  the client-visible job table. It carries the enable generation, instance and
  resolved protected roots through queueing, planning, confirmation, execution
  and terminal cleanup. Off revokes the ticket before any cleanup; queued work
  and plans waiting for a decision never start. A job whose worker has already
  published its execution boundary keeps its original protected-path checks.
- `file.paste` is a tracked asynchronous operation. The reply is `accepted` and
  a queryable job id before the clipboard read begins. The callback carries the
  original ticket, rechecks revocation before submission, and the file-op
  worker resolves and guards the actual Copy/Move sources and destination.
  Empty/invalid clipboard data, a read failure, a planning failure and a dry
  run all update that job's `failed`/`complete` result. A dry run returns the
  real plan lines and does not clear the clipboard; a successful cut still
  clears it under the existing UI rule.
- Every callback result is scoped to its full original job token (instance,
  generation and job number), never a recycled local number. A callback from an
  earlier generation cannot fail, attach to, revive or overwrite a job in the
  current table, and a terminal job is never overwritten by an unrelated late
  result.
- Filesystem path resolution, dry-run planning and undo/restore plan
  revalidation run on bounded workers. The GTK thread does only in-memory
  validation and state application. A late result from a previous generation
  is rejected before dispatch.
- Model/config/handler failures are returned as `error`/`failed`, never as
  `ok`. A contextual page refusal such as `nav.up` or `search.open` while Trash
  is visible keeps its visible explanation and is also a typed failure, not a
  successful completion. A clean `window.close`/`app.quit` is allowed; a dirty,
  saving or dialog-owning window reports `busy` and keeps the human close guard.
- Stop wakes accept/read/write/bridge waits with condition-variable-style
  flag checks and bounded timeouts, so idle listeners and queued clients do
  not hang application shutdown. There is no thread per unbounded request.
