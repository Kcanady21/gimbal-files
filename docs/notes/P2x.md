# P2x: answer "Show in folder" (org.freedesktop.FileManager1)

## What this change does

- Adds `packaging/dbus/io.github.kcanady21.GimbalFiles.FileManager1.service`
  with `Name=org.freedesktop.FileManager1` and
  `Exec=/usr/bin/gimbal-files --gapplication-service`.
- Installs it system-wide at `/usr/share/dbus-1/services/` in the `.deb`, and
  per-user at `$data_dir/dbus-1/services/` from `install.sh`. The installer
  rewrites `Exec` to the absolute installed binary, quoted for the D-Bus
  service-file parser, and `--uninstall` removes exactly that file.
- Stages it in the release tarball under `share/dbus-1/services/`.
- Adds `fm_core::filemanager1`, the pure URI-to-path decoding with refusal
  reasons, a 64-entry cap and exact-duplicate removal.
- Adds `[integration] show_in_folder = true|false`, default `false`, parsed
  like `[updates]` with unknown keys reported and byte-preserving saves.
- Adds the UI-only `integration.set-show-in-folder enabled=bool` command with
  `Effect::System`, matching the update commands.
- Adds `fm-ui/src/filemanager1.rs`: the D-Bus object, name ownership (including
  the queued/waiting state), the three methods through `open_arguments`, the
  xdg-activation startup id, the one-line call log, and the bus-started
  switch-off grace path.
- Adds the Settings → Integration switch and status line.
- Extends the README's "Make it the default file manager" section and lists
  the new command as unavailable through the agent socket.

No new dependency and no session schema change.

There is no `phase2/RULES.md`. I read `phase1/RULES.md` and the repository's
`CLAUDE.md`, which `phase2/README.md` names as its replacement. **[read]**

## Files written

- `packaging/dbus/io.github.kcanady21.GimbalFiles.FileManager1.service` (new)
- `scripts/build-deb.sh`, `scripts/test-deb.sh`, `scripts/install.sh`,
  `scripts/test-install.sh`, `scripts/release.sh`
- `crates/fm-core/src/filemanager1.rs` (new)
- `crates/fm-core/src/lib.rs`
- `crates/fm-core/src/config.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-ui/src/main.rs`
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/header.rs`
- `crates/fm-ui/src/filemanager1.rs` (new)
- `README.md` (only the "Make it the default file manager" section)
- `docs/agent-socket.md` (the UI-only list and one sentence)
- `docs/notes/P2x.md` (this file)

## The URI decoder

`fm_core::filemanager1::uris_to_paths(&[String]) -> (Vec<PathBuf>, Vec<Refused>)`
accepts only `file://` URIs with a local path. `glib::filename_from_uri`
(`glib-0.22.10:src/auto/functions.rs:272`) does the percent decoding, so a
percent-encoded non-UTF-8 byte stays the same byte in the `PathBuf`. The checks
are ordered:

1. a NUL in the D-Bus string is `NulByte`;
2. no `scheme:` or a non-scheme prefix (a relative path) is `RelativePath`;
3. a non-`file` scheme (`sftp://`, `trash:///`) is `UnsupportedScheme`;
4. `file:` without `//` is `RelativePath`;
5. GLib's local-host check; a different host is `NotLocal`;
6. a decoded path that is not absolute is `RelativePath`;
7. a decoded NUL is `NulByte`.

Entries at index 64 and above are `TooMany` and do not affect the first 64.
Exact duplicate URI strings are dropped keeping the first occurrence. Each
refusal carries a short reason, and the log summary never prints more than
eight reasons.

`glib::filename_from_uri` was checked against this machine's GLib 2.88.0:
`file://other-host/tmp/a` returns `/tmp/a` plus hostname `other-host`,
`file://localhost/tmp/a` returns `/tmp/a`, and `sftp://` / `file:relative`
return an error. **[observed]**

## Config and command

`[integration]` joins `KNOWN_TOP_LEVEL`. `read_integration` follows the
`read_updates` pattern: a wrong shape, unknown key or wrong type reports a
problem and leaves the default Off. `Change::SetShowInFolder` updates the
typed value and writes only `integration.show_in_folder` through `toml_edit`,
so comments and every other section survive.

`integration.set-show-in-folder` sits after `updates.check` in the catalogue.
It takes a required `enabled` bool and has `Effect::System`. The Settings
switch dispatches as `Caller::Ui`; the local socket already refuses every
`System` effect before dispatch, which is the same rule `updates.set-enabled`
uses. **[read]**

## The D-Bus service

### Owning the name

`filemanager1::start` runs from GApplication's `startup` signal. It always
keeps a runtime when it has the application's own D-Bus connection, so the
Settings switch can turn the service on later without a restart. It exports
`/org/freedesktop/FileManager1` and requests
`org.freedesktop.FileManager1` in `startup` when either the switch is on or
the process was started with `--gapplication-service`.

The object is registered before the name is requested. The name request uses:

```rust
gio::bus_own_name_on_connection(
    &connection,
    BUS_NAME,
    gio::BusNameOwnerFlags::ALLOW_REPLACEMENT | gio::BusNameOwnerFlags::REPLACE,
    acquired,
    lost,
)
```

`gio-0.22.10:src/dbus.rs:52` is `bus_own_name_on_connection`; the two flags are
`gio-0.22.10:src/auto/flags.rs:293` and `:295`. The object and method handler
are `gio-0.22.10:src/dbus_connection.rs:465` (`register_object`) and `:318`
(`RegistrationBuilder::method_call`); `:370` is `build`.

GIO handles the queue. If the current owner does not allow replacement,
`RequestName` returns `IN_QUEUE`, GIO calls the lost callback, and it calls the
acquired callback when the owner quits. The code's lost callback schedules an
idle `GetNameOwner`, compares the unique name with the application connection,
and sets `Waiting(<comm>)` or `Owned`. The owner command is
`org.freedesktop.DBus.GetConnectionUnixProcessID` followed by
`/proc/<pid>/comm`, with `another app` when either step fails. Turning the
switch off calls `bus_unown_name` and unregisters the object. It never touches
the installed activation file. **[read]**

State changes log one `filemanager1:` line each (`Owned`, `Waiting`, `Off`).
The status row reads `On`, `Waiting: <owner> is handling it until it closes`,
or `Off`; with the switch off it always reads `Off`, even if a bus-started
process is briefly serving one call.

### Calls

The three methods are in one introspection XML constant. Each takes `(as uris,
s startup_id)`. The call log is one line:

```text
filemanager1: :1.42 called ShowItems: 2 accepted, 1 refused (unsupported scheme)
```

It never prints a full URI list; `refusal_summary` caps the reason list at
eight. Malformed parameters return
`org.freedesktop.DBus.Error.InvalidArgs`; an unknown method returns
`org.freedesktop.DBus.Error.UnknownMethod`.

`ShowFolders`, `ShowItems` and `ShowItemProperties` all hand their accepted
paths to the same `open_arguments`. `group_arguments` already opens a folder
as a folder and a file as its parent with the item selected, so `ShowFolders`
with a non-folder path is the `ShowItems` behavior by construction.
`ShowItemProperties` additionally logs
`properties are not supported; showing the item`.

The pure `plan(method, uris)` mapping is in `fm-ui/src/filemanager1.rs` and is
tested for all three methods.

### Startup id

After `open_arguments`, the focused window calls `gtk_window_set_startup_id`
and then `present`. On this GTK (4.22.4) the call stores the id and hands it to
`gdk_toplevel_set_startup_id`; the Wayland backend calls
`xdg_activation_v1_activate(startup_id, surface)`, so the token is used for an
already-running window too:

- `gtk4-0.11.5:src/auto/window.rs:951` — `WindowExt::set_startup_id`.
- `../gimbal-refs/gtk-4.22.4/gtk/gtkwindow.c:5446` — the documented function;
  its body starts at `:5467`.
- `../gimbal-refs/gtk-4.22.4/gdk/wayland/gdktoplevel-wayland.c:1219` — the
  `xdg_activation_v1_activate` call.

### Bus-started with the switch off

A `--gapplication-service` launch never activates, so `startup` is the only
place that can install the object. When the switch is off, the runtime still
requests the name for the call that the bus started. After the first call it
logs exactly:

```text
filemanager1: started by the bus with the switch off; served one call
```

and releases the name after a 2-second grace window (reset by each call in a
burst). A window opened by the call holds the application open afterwards, so
the process keeps running without the name.

GApplication's service-mode inactivity timeout is hard-coded to 10,000 ms in
the run loop (`glib 2.88.0:gio/gapplication.c:2728-2734`). The bus activation
call arrives immediately, and once a call creates a window GTK holds the
application, so no explicit `hold()` was needed. The 2-second release is
shorter than that timeout, and the idle that builds the process runs before
the timeout source.

## Verification

### Gates

`scripts/check.sh` on the final tree exited 0
(`/tmp/gimbal-p2x-check-final.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test totals: **871 passed, 23 ignored** (23 + 16 + 12 + 405 + 6 + 11 + 398
passing; 1 + 6 + 11 + 5 ignored). P2v's baseline was 850 passed / 23 ignored;
P2x adds 21 passing tests (16 in `fm-core`, 5 in `fm-ui`). **[observed]**

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, the workspace tests and the release build are all inside
`scripts/check.sh` and passed. **[observed]**

### Installer tests

`scripts/test-install.sh` passed (`PASS: install, reinstall and uninstall ...`).
It covers:

- the service file's mode 0644;
- `Exec='<prefix>/bin/gimbal-files' --gapplication-service` under a prefix
  containing spaces, `&` and parentheses;
- `Name=org.freedesktop.FileManager1`;
- `--uninstall` removing the service file and only it, with a sentinel left in
  the same `dbus-1/services` folder;
- the `--data-dir` install and uninstall of the service file.

`scripts/test-deb.sh` passed. The `dpkg-deb --contents` expected list now
includes `./usr/share/dbus-1/`, `./usr/share/dbus-1/services/` and the service
file at mode 0644, owner root/root. It also extracts the file and checks
`Name=` and `Exec=/usr/bin/gimbal-files --gapplication-service`. The package
still has no maintainer scripts, and two builds remain byte-identical
(`bf08e0a21d376c1e5f9c1d3d219d14dd7adde8d4bb9dd90aa116ce24da37fed7`).
**[observed]**

`bash -n` passes for all five changed scripts. `shellcheck` is still not
installed. **[observed]**

### What was not run

- No GUI launch and no D-Bus ownership run. Kyle's `gimbal-files`
  (PID 22812) was already running, so `CLAUDE.md` forbids a plain launch, and
  the agent environment has no session bus address. A debug instance that
  owned the real name would also take "Show in folder" from Dolphin while it
  ran. Claude runs the live list below.
- No `gdbus` call against the real session bus.
- `scripts/release.sh --dry-run` on a scratch clone. That needs git commands,
  which this agent may not run.

## What Claude checks live

1. **Owning the name.** A debug build with a scratch config whose switch is
   on, while no Dolphin is running: `busctl --user status
   org.freedesktop.FileManager1` names the gimbal-files PID.
   - `gdbus call --session --dest org.freedesktop.FileManager1 --object-path
     /org/freedesktop/FileManager1 --method org.freedesktop.FileManager1.ShowItems
     "['file:///tmp/gimbal-p2x/a b (c)/file.txt']" ""` opens that folder with
     `file.txt` selected.
   - `ShowFolders` opens the folder.
   - An `sftp://` URI is refused, and only logged.
2. **Waiting.** With Dolphin running first, Settings shows the Waiting line.
   When Dolphin quits, Gimbal takes the name and the status changes to On.
3. **Switch off.** The name is released. The installed activation file is
   untouched.
4. **A bus-started Gimbal with the switch off.** Run it as
   `gimbal-files --gapplication-service` with a scratch config, then send one
   `ShowItems` call. It serves that call, then releases the name.
5. **Kyle, after the next release.**
   - `dpkg -L gimbal-files` lists the service file.
   - With Gimbal switched on and Dolphin closed, "Show in folder" in his
     browser opens Gimbal with the file selected.
   - With Gimbal closed, the same click starts whichever file manager
     directory order picks, until the others are removed.

A debug instance that owns the name on the real session bus takes "Show in
folder" away from Dolphin while it runs. That is expected during the
10-second live runs.

## Outside the Output list

- No file outside the brief's Output list was changed. In particular,
  `packaging/debian/copyright` needs no crate update because P2x adds no
  dependency.
- The `Waiting` owner query uses the application's own bus connection and a
  500 ms deadline. It is a short synchronous call from an idle, never from the
  D-Bus method callback itself.
- `filemanager1::start` keeps the runtime even when the switch is off so the
  Settings switch can turn it on later without restarting. The object and
  name are only exported when the switch is on or the bus started the process.

## Claude's review (Nov 6, 2026)

**Code.** The return matches the brief. The service, the three methods through
`open_arguments`, the queued Waiting state, the switch-off grace path, the
startup-id hand-off and the UI-only command are all as specified. **[read]**

**Gates.** `scripts/check.sh`: 871 passed, 23 ignored. `test-install.sh` and
`test-deb.sh` pass. **[observed]**

**Fix: the installer's `Exec` quoting (`scripts/install.sh` `dbus_quote`).**
dbus-daemon 1.16.2 reads a service file in two passes. Its desktop-file parser
handles backslash escapes first, and rejects the *whole file* on an unknown one.
Only then is `Exec` split with shell quoting. The returned `'\''` escape for an
apostrophe therefore made dbus-daemon drop the file: a per-user install under a
path with `'` would install a service that never starts. Tested on a private
bus with stand-in services:

| `Exec` form | Result |
| --- | --- |
| `'…/sp ace/fake'` | ran |
| `"…/sp ace/fake"` | ran |
| `'…/it'\''s/fake'` (returned) | file rejected: `ServiceUnknown` |
| `'…/it'"'"'s/fake'` | ran |
| `'…/back\\slash/fake'` (doubled) | ran, argv has one `\` |
| `#` comment line before `[D-BUS Service]` | accepted |

`dbus_quote` now doubles each backslash, then writes an embedded `'` as
`'"'"'`. `test-install.sh` gains a prefix with both `'` and `\`; it checks the
exact `Exec` line and that `'\''` never appears. The file `install.sh` writes for
`it's a \ (x) & dir` activates its stand-in binary on a private bus.
**[observed]**

**Minor, left as is:** `install_and_own` logs `waiting; another app is handling
it` before the asynchronous `NameAcquired` arrives, so every switch-on shows one
false Waiting line before `now owns`. The Settings row corrects itself within a
main-loop turn. Worth an initial `Requesting` state the next time this file is
touched. **[read]**

**Known limit, not a bug:** with Gimbal running and its switch off, if the bus
picks Gimbal's activation file (directory order, while other file managers are
installed), the `--gapplication-service` launch meets an already-registered app
id. That case is checked live below. **[inferred]**

**Fix: the Settings switch did nothing (`crates/fm-ui/src/app.rs`).**
`integration.set-show-in-folder` is an `Effect::System` command, and System
commands need their own branch in the UI dispatcher (as `updates.set-enabled`
has). It had none, so every toggle failed with `integration.set-show-in-folder
is not implemented by this version`: the config was never written and the name
never released. Found in live check 3. The command now routes to
`settings_change`, which saves the key and runs `after_change`'s existing
`SetShowInFolder` arm. New test
`filemanager1::tests::the_settings_switch_saves_show_in_folder_and_only_the_ui_may_use_it`
dispatches through the real `UiHandler` on a headless process. It fails with the
"not implemented" outcome when the branch is removed, and passes with it.
**[observed]**

### Live checks (Claude, Nov 6; debug build, scratch XDG config/state/cache)

Driver: AT-SPI actions only, `gdbus`/`busctl` for D-Bus, each run under 10 s and
stopped by PID with SIGTERM. Kyle's Gimbal was closed.

1. **Owning the name: pass.** `busctl` names the Gimbal PID.
   - `ShowItems` on `a b (c)/file.txt` opens that folder with `file.txt`
     selected (`command line selected 1 file(s)`).
   - `ShowFolders` opens the folder.
   - `sftp://…` and a relative path are refused and only logged
     (`0 accepted, 2 refused (unsupported scheme, relative path)`).
   - Settings shows the switch on and status `On`.
2. **Waiting: pass.** With Dolphin started first, Dolphin owns the name and
   Settings reads `Waiting: dolphin is handling it until it closes`. After
   SIGTERM to Dolphin, Gimbal owns the name within about a second and the
   status reads `On`.
3. **Switch off: pass after the fix above.** The switch writes
   `show_in_folder = false`, the name is released (`busctl`: no owner), and the
   status reads `Off`. No activation file was touched (none is installed in the
   bus's directories before the next release).
4. **Bus-started with the switch off: pass.** `gimbal-files
   --gapplication-service` owns the name, serves one `ShowItems` (folder opened
   with the file selected), logs `started by the bus with the switch off;
   served one call`, releases the name about 2 s later, and keeps running for
   its window.
5. **Service launch beside a running Gimbal with the switch off: fails, as
   predicted.** The second process exits 1: `Failed to register: Unable to
   acquire bus name 'io.github.kcanady21.GimbalFiles'`. If the bus picks Gimbal's
   activation file while Gimbal is open with the switch off, that "Show in
   folder" call fails. Not fixed here: it needs a design decision (Kyle).
6. **Kyle, after the next release:** `dpkg -L` lists the service file; "Show in
   folder" from the browser with the switch on.
