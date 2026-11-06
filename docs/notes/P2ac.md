# P2ac: forward "Show in folder"; system-wide defaults

## What this change does

- A bus-started `gimbal-files --gapplication-service` beside a running Gimbal
  no longer exits with `Failed to register: Unable to acquire bus name
  'io.github.kcanady21.GimbalFiles'`. `main` checks the argument before
  `run()`, registers without `IS_SERVICE`, and if the result is a remote
  instance takes a forwarding path instead.
- The forwarding process owns `org.freedesktop.FileManager1` with
  `DO_NOT_QUEUE`, exports the same three methods and introspection XML as the
  primary, answers each call with an empty result, and hands the accepted
  paths to the running Gimbal through GApplication's remote `open`. It makes
  no window, loads no session and reads/writes no config or state. It exits 0
  after a 2-second grace window (reset by each call), or after a 10-second
  idle wait if it is never called.
- Adds the read-only system defaults file
  `$XDG_CONFIG_DIRS/gimbal-files/config.toml` (first existing entry;
  `/etc/xdg/gimbal-files/config.toml` when the variable is unset). The user's
  `config.toml` wins; Settings writes only the user file.
- Layers `[integration]`, `[updates]` and `[agent]`. `[agent] enabled` from
  the defaults file is ignored with a problem line. `[look]`, `[sidebars]`,
  `[sections]`, `[pins]`, `[sort]` and `[hotkeys]` are not layered (see
  below).
- No new dependency and no session schema change.

There is no `phase2/RULES.md`. I read the repository's `CLAUDE.md` (which
`phase2/README.md` names as the rules replacing Phase 1's `RULES.md`) and the
whole P2ac brief. **[read]**

## Files written

- `crates/fm-ui/src/main.rs`
- `crates/fm-ui/src/filemanager1.rs`
- `crates/fm-core/src/config.rs`
- `README.md` (the two sections named in the brief)
- `docs/notes/P2ac.md` (this file)

`crates/fm-ui/src/header.rs` was not touched. The Settings row already shows
the effective `On`/`Off` value from the config the process loaded, so it does
not need a separate "came from the distro defaults" label. No file outside the
brief's Output list changed. **[read]**

## Forwarding

### The exact GApplication sequence

1. `main` reads `std::env::args_os()` and calls the pure
   `filemanager1::is_service_launch`, which looks for the exact
   `--gapplication-service` argument after the program name. GApplication
   parses that flag itself inside `run`, but by then a service launch beside a
   running instance has already failed to take the app id. **[read]**
2. When it is present, `main` builds the `adw::Application` with
   `HANDLES_OPEN` and **without** `IS_SERVICE`, then calls
   `app.register(None)`. `gio-0.22.10:src/auto/application.rs:304` is
   `ApplicationExt::register`, and `:252` is `is_remote`. **[read]**
3. `g_application_register` calls `g_application_impl_register`
   (`glib 2.88.0:gio/gapplication.c:2340-2361`). The impl attempts
   `RequestName` with `G_BUS_NAME_OWNER_FLAGS_DO_NOT_QUEUE`
   (`gapplicationimpl-dbus.c`, `g_application_impl_attempt_primary`). If the
   name is taken and `IS_SERVICE` is set, it returns
   `Unable to acquire bus name`; because this pre-check left `IS_SERVICE`
   clear, it instead drops the service-side objects and returns a remote
   implementation (`g_application_impl_register`, same file). **[read]**
4. `is_remote()` true: `main` returns `filemanager1::forward(app)` and never
   calls `run()`. `startup` never fires, so GTK is not initialized and no
   `Process` (config, session, windows) is built. **[read/inferred]**
5. `is_remote()` false: this is the primary. `startup` already ran during
   `register` with the explicit `service = true`, so
   `filemanager1::start(app, true)` installs the object and name exactly as
   P2x did, including the switch-off `service_off` grace path. `main` then
   calls `app.run()`. `run` parses `--gapplication-service`, sets
   `IS_SERVICE`, and its `g_application_register` call is a no-op because the
   app is already registered. At the end of `run`, GApplication installs its
   hard-coded 10-second inactivity timeout
   (`glib 2.88.0:gio/gapplication.c:2728-2734`). This is how the primary
   service path keeps P2x's timing. **[read]**

### The forwarding process

`forward` does this, in order:

1. Takes `application.dbus_connection()`; without one it logs and exits 0.
2. Registers `/org/freedesktop/FileManager1` with the existing
   `INTROSPECTION_XML` and a method handler
   (`gio-0.22.10:src/auto/dbus_connection.rs:465`, `register_object`;
   `:318`, `RegistrationBuilder::method_call`).
3. Calls `org.freedesktop.DBus.RequestName` synchronously with only
   `gio::BusNameOwnerFlags::DO_NOT_QUEUE`
   (`gio-0.22.10:src/auto/dbus_connection.rs:190`, `call_sync`;
   `gio-0.22.10:src/auto/flags.rs:296-297`). Reply `1` is
   `PRIMARY_OWNER` and `4` is `ALREADY_OWNER`; anything else, notably `3`
   (`EXISTS`), means another process owns the name, so it unregisters the
   object, logs one line, and returns exit 0 without a main loop.
4. Runs a local `glib::MainLoop` and arms the same 10-second idle exit as
   GApplication's service mode (`glib-0.22.10:src/source.rs:642`,
   `timeout_add_seconds_local_once`). A call cancels the idle exit by moving
   the grace generation above 0.
5. On each call: parse the member with the same `Method::parse`, decode the
   `(as s)` arguments, run the same `uris_to_paths` via `plan`, log the same
   `filemanager1:` call line (`counts` and refusal reasons; with accepted
   paths it appends `; forwarded to the running Gimbal`), and answer the
   caller with `return_value(None)` **before** the hand-off.
6. Only `plan.paths` are forwarded. `plan.refused` is never passed on. An
   empty accepted list answers the caller and arms the grace window but never
   calls the running Gimbal.
7. The hand-off is `adw::Application::open(&files, hint)` on the registered
   remote instance (`g_application_open`;
   `gio-0.22.10:src/auto/application.rs:284`). The remote call is
   `g_application_impl_open`, which sends
   `org.gtk.Application.Open (as uris, s hint, a{sv} platform-data)`
   (`gapplicationimpl-dbus.c`, `g_application_impl_open`). The primary's
   existing `open` handler receives it and runs
   `open_arguments_with_startup`.
8. Each call starts a new grace generation and a new 2-second timeout. An
   older timeout checks the generation and returns; only the newest one
   releases the name, unregisters the object and quits the loop.

### The activation token

GTK 4.22's remote platform data is built by
`gtk_application_add_platform_data`
(`gtk-4.22.4:gtk/gtkapplication.c:390`). It reads
`gdk_get_startup_notification_id()` and adds both `activation-token` and
`desktop-startup-id` (`:404-406`). That value is the process's
`XDG_ACTIVATION_TOKEN` (preferred) or `DESKTOP_STARTUP_ID`, copied and
unset by a GDK constructor at library load
(`gtk-4.22.4:gdk/gdk.c:185-203`, `:399-404`). There is no public per-call
setter for that static value; the Wayland display setter
(`gdk_wayland_display_set_startup_notification_id`) changes the display
field, not the source GTK reads for platform data. **[read]**

The brief asks to pass the call's `startup_id` along as the activation token.
The forwarding process passes it in the `hint` argument of
`g_application_open` instead of platform data, because the platform-data
token can only come from the environment captured before `main`. The primary
already treats the open hint as its activation token path: `main.rs` sees a
non-empty hint and calls `Process::open_arguments_with_startup`, which calls
`gtk_window_set_startup_id` before `present`
(`gtk4-0.11.5:src/auto/window.rs:951`;
`gtk-4.22.4:gtk/gtkwindow.c:5446`;
`gtk-4.22.4:gdk/wayland/gdktoplevel-wayland.c:1219-1221`). That is the
same token hand-off P2x uses, so niri focuses the running window. Ordinary
second launches pass an empty hint and keep their previous
`open_arguments` path. **[read]**

### Logging

- One line per call, for example:
  `filemanager1: :1.42 called ShowItems: 2 accepted, 1 refused (unsupported scheme); forwarded to the running Gimbal`.
- A call with no accepted paths logs the same call line without the
  `forwarded` suffix.
- One exit line: either `released org.freedesktop.FileManager1 after the grace
  window; the forwarding process exits`, the already-owned line, or the idle
  line. **[read]**

### Tests

`crates/fm-ui/src/filemanager1.rs` adds pure tests:

- `only_the_accepted_paths_are_forwarded` — a `file://` folder and file are
  forwarded, an `sftp://` URI is not.
- `refused_uris_are_never_forwarded` — an all-refused call never reaches the
  `RemoteOpen` recorder.
- `the_empty_result_goes_back_before_the_hand_off` — the answer event is
  recorded before the open event, and an empty call records only the answer.
- `every_call_resets_the_grace_window` — a second call moves the 2-second
  deadline.
- `a_forwarded_call_uses_the_newest_generation` — the newest generation owns
  the exit.
- `only_the_service_argument_wins` — the raw argument check is exact.

The hand-off is behind `RemoteOpen`; production uses
`RunningGimbal` (`Application::open`), tests use a recorder. **[read]**

## System-wide defaults

### Path

`defaults_config_paths(Some("a:b"))` builds `a/gimbal-files/config.toml` and
`b/gimbal-files/config.toml`; relative entries are ignored as the XDG Base
Directory specification requires. `defaults_config_path` returns the first
one that `is_file()`. With `XDG_CONFIG_DIRS` unset or empty the list is
`/etc/xdg/gimbal-files/config.toml`. `ConfigFile::load` uses the environment;
`ConfigFile::load_with` remains a no-defaults test entry point and
`load_with_defaults` takes the path explicitly. **[read]**

### Layering and what is not layered

`load_with_defaults` starts from `Config::with_defaults`, reads the defaults
file, then reads the user file over it. A defaults problem carries the
defaults file's path and never stops loading. A user-file problem keeps its
existing behaviour.

Layered: `[integration] show_in_folder`, `[updates] check`, `[agent] enabled`
(user file only). Not layered: `[look]`, `[sidebars]`, `[sections]`,
`[pins]`, `[sort]`, `[hotkeys]`. A known non-layered section in the defaults
file produces a `NotLayered` warning and is ignored. This is the fallback the
brief allows for sections whose shape is not a simple key/value pair; I did
not widen it. **[read]**

A missing key in a section the user did set keeps the defaults value. An
invalid user section keeps the P2x rule: its value falls back to the built-in
default and a problem line is reported.

The agent exception is enforced in `read_agent`: in the defaults file,
`[agent] enabled` is never applied, even when false, and produces an
`IgnoredInDefaults` warning with key `agent.enabled`. The user's own
`config.toml` still turns automation on.

`filemanager1::config_enabled()` calls `ConfigFile::load`, so the switch read
at `startup` uses the same layered value.

Settings writes continue to go through the `ConfigFile` whose `path` is the
user's file. A write after a defaults value therefore creates or edits only
the user file; the defaults file is read-only and untouched. **[read]**

### GimbalOS

The distro file is outside this repository. For Kyle's GimbalOS build:

```toml
# /etc/xdg/gimbal-files/config.toml (shipped by GimbalOS)
[integration]
show_in_folder = true
```

### Tests

`crates/fm-core/src/config.rs` adds:

- default only, user only, and both set (the user wins);
- the defaults file in the second `XDG_CONFIG_DIRS` entry;
- a parse-broken and a wrong-typed defaults file (problems carry the
  defaults path, Gimbal still loads);
- `[agent] enabled = true` in the defaults file is ignored with a problem,
  and the user can still enable it;
- `[updates]` layers while `[look]` is reported `NotLayered` and ignored;
- a Settings write after a default leaves the defaults bytes untouched and
  writes `show_in_folder = false` to the user file, which survives a reload.

## Verification

### Gates

`scripts/check.sh` on the final tree exited 0
(`/tmp/gimbal-p2ac-check-final.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test totals: **921 passed, 23 ignored** (23 + 16 + 12 + 442 + 6 + 11 + 411
passing; 0 + 0 + 0 + 1 + 6 + 11 + 5 ignored). P2aa's baseline was 908 passed
/ 23 ignored, so P2ac adds 13 passing tests: 7 in `fm-core/src/config.rs` and
6 in `fm-ui/src/filemanager1.rs`. **[observed]**

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, the workspace tests and the release build all ran inside
`scripts/check.sh` and passed. **[observed]**

### What was not run

No GUI launch and no D-Bus ownership run. This agent cannot use
`dbus-run-session`, and a real session-bus instance would take "Show in
folder" from Kyle's running desktop while it ran. Claude runs the live list
below. **[read]**

## What Claude checks live

Debug build, scratch `XDG_CONFIG_HOME`, `XDG_CONFIG_DIRS`, `XDG_STATE_HOME`
and `XDG_CACHE_HOME` under `/tmp/gimbal-p2ac-*`. Each run under 10 seconds and
stopped by PID with SIGTERM.

1. **Forwarding.**
   - Start a primary Gimbal with `show_in_folder = false`.
   - Start `gimbal-files --gapplication-service` as a second process. It
     should own `org.freedesktop.FileManager1` and log
     `forwarding to the running Gimbal while
     org.freedesktop.FileManager1 is held`.
   - Send
     `gdbus call --session --dest org.freedesktop.FileManager1
     --object-path /org/freedesktop/FileManager1 --method
     org.freedesktop.FileManager1.ShowItems
     "['file:///tmp/gimbal-p2ac/a b (c)/file.txt']" ""`.
     The **primary's** window must open that folder with `file.txt` selected;
     the second process makes no window, logs one call line ending
     `forwarded to the running Gimbal`, releases the name about 2 seconds
     later, and exits 0.
   - Repeat with a third `--gapplication-service` process while the name is
     still held: it must log the already-owned line and exit 0 without a
     window.
2. **The service path is unchanged.** With no primary running,
   `gimbal-files --gapplication-service` behaves exactly as P2x's live check
   4: it owns the name, serves one call, logs
   `started by the bus with the switch off; served one call`, releases the
   name after about 2 seconds, and keeps running for its window.
3. **Defaults.**
   - A scratch `XDG_CONFIG_DIRS` whose `gimbal-files/config.toml` sets
     `[integration] show_in_folder = true`, and an empty user config:
     Settings shows the switch on; Gimbal owns the FileManager1 name.
   - Switch it off. Only the scratch user file gains
     `show_in_folder = false`; the defaults file is unchanged. After a
     restart the switch stays off.
   - A defaults file with `[agent] enabled = true` leaves
     **Allow local automation** off and logs an `ignored in the defaults
     file` problem for `agent.enabled`.

## Outside the Output list

- No file outside the brief's Output list changed.
- `crates/fm-ui/src/header.rs` needed no change; the row shows the effective
  value.
- The GimbalOS defaults file is a distro file, not a repository file; its
  one-line contents are above.
- No new dependency and no `Cargo.toml` change.

## Claude's review (Nov 6, 2026)

**Code.** The return matches the brief. Two deviations are justified and
documented above:
- the activation token rides in the `open` hint, because GTK reads platform
  data only from the environment captured at load;
- only `[integration]`, `[updates]` and `[agent]` are layered, which is the
  fallback the brief allowed.

The primary's `open` uses the hint only when it is non-empty, so ordinary
launches are unchanged. **[read]**

**Gates.** `scripts/check.sh`: 921 passed, 23 ignored. **[observed]**

No fixes were needed.

### Live checks (Claude, Nov 6)

Debug build, scratch XDG dirs under `/tmp/gimbal-p2ac` (and `/tmp/gimbal-p2x`
for the service path), AT-SPI actions only, `gdbus`/`busctl` for D-Bus. Each
run was stopped by PID with SIGTERM, or exited on its own.

1. **Forwarding: pass.**
   - With a primary running and the switch off, a second
     `--gapplication-service` process owns `org.freedesktop.FileManager1` and
     logs `forwarding to the running Gimbal…`.
   - A third exits 0 at once: `already has an owner; the forwarding process
     exits quietly`.
   - `ShowItems` on `a b (c)/file.txt` makes the **primary** open that folder
     with `file.txt` selected (`command line selected 1 file(s)`). The second
     process has no niri window, logs `… forwarded to the running Gimbal`,
     releases the name about 2.6 s after the call, and exits 0.
2. **Service path unchanged: pass.** With no primary running, P2x's check 4
   repeats exactly: it owns the name, serves one call, logs `served one call`,
   releases the name, and keeps running for its window. An uncalled service
   launch exits on its own after 10.1 s with code 0 (GApplication's inactivity
   timeout).
3. **Defaults: pass.**
   - `XDG_CONFIG_DIRS=<sysA>:<sysB>`, with the file only in `sysB` setting
     `show_in_folder = true`, and an empty user config: Gimbal owns the name
     and the Settings switch is on.
   - Switching it off releases the name. The user file becomes
     `[integration]\nshow_in_folder = false\n`, and the defaults file is
     byte-identical.
   - After a restart the switch stays off and nothing owns the name.
   - A defaults file with `[agent] enabled = true` leaves `Allow local
     automation` off and logs `agent.enabled: warning: automation stays the
     user's own choice; this key is ignored in the defaults file`. No listener
     started.

**For GimbalOS** (a distro change, outside this repo): ship
`/etc/xdg/gimbal-files/config.toml` containing `[integration]` and
`show_in_folder = true`.
