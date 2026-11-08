# P2ae: logs, crash records and the bug-report command

## What I built

- **Always-on log** at `$XDG_STATE_HOME/gimbal-files/logs/gimbal.log`
  (`~/.local/state/gimbal-files/logs/gimbal.log`), opened once in append mode.
  Lines have the wall-clock prefix and the old run-time prefix:
  `2026-11-08T09:16:17.123 [gimbal 12.345] module: message`.
- **Rotation** at 5 MiB: `gimbal.log` -> `.1` -> `.2`, three files at most.
  The plan is `fm_core::logfile::plan_rotation`, a pure function; the UI applies
  it at startup and before a line that would cross the limit.
- **Two levels**:
  - `crate::log!` always writes the file, and stderr with `GIMBAL_LOG=1`;
  - `crate::log_verbose!` writes file and stderr only with `GIMBAL_LOG=1`.
  - A failed open or write falls back to stderr and never stops the app.
  - Only the primary instance opens the file. A forwarded launch and P2ac's
    `--gapplication-service` forwarding process reach no `startup`, so they
    keep their one line on stderr.
- **Run header** on every start: version, install kind, GTK, libadwaita and
  GLib runtime versions, `XDG_CURRENT_DESKTOP`, display type, and start time.
- **Panics**: a hook writes `panic: <message> at <location>` and
  `std::backtrace::Backtrace::force_capture()` to the log, then calls the
  previous hook. Debug builds honour `GIMBAL_TEST_PANIC=1`.
- **GLib messages**: `glib::log_set_writer_func` writes `glib:` lines with
  domain, level and message. Error/Critical/Warning always; Message/Info/Debug
  only with `GIMBAL_LOG=1`. `glib::log_writer_default` still prints normally.
- **Crash marker** `$XDG_STATE_HOME/gimbal-files/running` with PID, wall clock,
  Unix start time and the log size at start. It is removed in `quit_now` and
  `connect_shutdown`. At startup a stale marker saves the last 2,000 lines of
  `gimbal.log` (and `gimbal.log.1` when the run began there) to
  `crashes/<start>.log`, keeps the newest 10, and queues the one-time line
  `Gimbal closed unexpectedly last time. A crash record was saved; Settings →
  Report a bug.` for the first window.
- **Native signals**: SIGSEGV, SIGABRT and SIGBUS write one
  `fatal signal <n>` line with `write(2)` only, restore `SIG_DFL` and re-raise,
  so apport still gets a core.
- **Release debug info**: `[profile.release] debug = "line-tables-only"`;
  `build-deb.sh` splits with `objcopy --only-keep-debug`, `strip
  --strip-unneeded`, `objcopy --add-gnu-debuglink`, and packs
  `gimbal-files-dbgsym_<v>_amd64.deb` at
  `/usr/lib/debug/.build-id/xx/yyyy.debug`. `release.sh` adds it to the apt
  pool and the GitHub release; `test-deb.sh` checks debuglink, no
  `.debug_info`, build ID match and byte-identical rebuilds.
- **Settings**: under Integration, **Report a bug** with **Open log folder**
  (desktop URI handler) and **Copy report command** (clipboard). All strings
  live in `fm_core::logfile::text`.
- **`gimbal-files-cli bug-report`**: no socket. Writes
  `~/gimbal-files-bug-report-<YYYY-MM-DD-HHMMSS>.txt` (or `--output FILE`),
  prints the path and tells the user to read it. Six `##` sections: Gimbal,
  System, Last crash record, Log, Config, apport. `--lines N` and `--no-paths`.
  Home becomes `~`; `[pins] folders` become `<pin>`; no file contents,
  environment beyond `XDG_CURRENT_DESKTOP`, or `/var/crash` contents.
- **README and GitHub**: a Reporting a bug section, a bug-report issue form
  with the required report checkbox and a "I can't" option, and a config.yml
  with blank issues disabled.

### Modules moved to `log_verbose!`

- `split`: the full `layout …` line.
- `look::blur`: every `blur` line (capabilities, fallback, surface layout,
  scale factor, opacity, placed rects).
- `folder::view`: `viewport …`, `restoring scroll anchor …`, every
  `folder-size` line, `thumb: request …`, `thumb: no thumbnail …`.
- `header`: `window … pills …`.
- `dnd`: `hover …`, `drag selected-action changed …`.
- `preview`: `monitor event …`, `parent monitor event …`, `model selection …`,
  `user selection …`, `load generation …`.

### Default volume

`[inferred]` Kyle's Gimbal was already running (`pgrep` found pid 1624369), and
CLAUDE.md forbids a plain launch then: it would hand the arguments to his
window. I could not run the normal-browse measurement, so this is a code-path
estimate, not a measurement.

After the moves above, everyday lines are per action only: opening a folder
logs about four lines, a copy about eight, a split about two, selection and
scrolling zero, hover zero, idle zero. A busy minute with an open, a copy, a
split and several selections is roughly 15–25 lines. A typical line is about
120 bytes, so 5 MiB is about 43,000 lines: roughly two to five days of normal
use, and much longer when idle. Claude should confirm with `wc -l` on a real
scratch run before calling the item done.

## API citations

- `glib::log_set_writer_func` — `glib-0.22.10:src/log.rs:456`.
- `glib::LogField::key` / `value_str` — `glib-0.22.10:src/log.rs:418` / `:440`.
- `glib::log_writer_default` — `glib-0.22.10:src/log.rs:1053`.
- `glib::check_version` — `glib-0.22.10:src/auto/functions.rs:58`. Called with
  an impossible `(99, 0, 0)` so the return is the runtime version string.
- `glib::DateTime::from_unix_local` / `format` —
  `glib-0.22.10:src/auto/date_time.rs:73` / `:302`.
- `gtk::major_version` / `minor_version` / `micro_version` —
  `gtk4-0.11.5:src/auto/functions.rs:162` / `:176` / `:169`.
- `adw::major_version` / `minor_version` / `micro_version` —
  `libadwaita-0.9.2:src/auto/functions.rs:146` / `:160` / `:153`.
- `gtk::UriLauncher::new` / `launch` — `gtk4-0.11.5:src/auto/uri_launcher.rs:24` / `:56`.
- `gdk::Clipboard::set_text` — `gdk4-0.11.5:src/auto/clipboard.rs:263`.

### `unsafe` (fm-ui only)

One feature, two syntactic blocks, both in `crates/fm-ui/src/log.rs`:

1. `unsafe extern "C" fn fatal_signal`: one `unsafe` block around `write(2)`,
   `signal(SIG_DFL)` and `raise(2)`. **SAFETY:** the handler uses only
   async-signal-safe calls, an atomic descriptor and a stack buffer; no
   allocation, lock or formatting.
2. `install_signal_handlers`: one `unsafe` block registering the handler for
   signals 11, 6 and 7. **SAFETY:** `signal(2)` only stores a static
   `extern "C" fn(c_int)` pointer for the life of the process.

`log.rs` carries `#![allow(unsafe_code)]`; no other fm-ui module changes its
lint. If the reviewer counts syntactic blocks rather than features, this is
two and should be noted in the review.

## Verified

All from the repo root with the rustup 1.95.0 toolchain.

```
$ scripts/check.sh
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Test counts: 960 passed, 23 ignored, 0 failed. Per crate:

- fm-agent: 23 unit + 16 socket
- fm-cli: 13
- fm-core: 470 passed, 1 ignored
- fm-core trash: 6 passed, 6 ignored
- fm-core trash browser: 11 passed, 11 ignored
- fm-ui: 421 passed, 5 ignored

Targeted P2ae tests:

```
$ cargo test -p fm-core --lib logfile
running 12 tests ... test result: ok. 12 passed; 0 failed
$ cargo test -p fm-ui --bin gimbal-files log::tests
running 2 tests ... test result: ok. 2 passed; 0 failed
$ cargo test -p fm-cli
running 13 tests ... test result: ok. 13 passed; 0 failed
```

`bash -n` passes for `scripts/build-deb.sh`, `scripts/release.sh` and
`scripts/test-deb.sh`.

`scripts/test-deb.sh` passed end to end after building the release:

```
PASS: .deb, two-version signed repo and scratch apt all work
two builds are byte-identical: 70cf6ed6a40251a4e4e6d6fa000e2fea919336931ea5dcd703e1e9715e3d42af
```

Package sizes with the line-table debug split:

- `gimbal-files_0.1.2_amd64.deb`: 2,616,494 bytes (2.5 MiB)
- `gimbal-files-dbgsym_0.1.2_amd64.deb`: 10,340,984 bytes (9.9 MiB)

The dbgsym package has two files by build ID:
`5c/1696ee…e1f5.debug` and `50/0669f7…f3d.debug`; each matches its installed
binary's build ID.

Manual `bug-report` against scratch `XDG_STATE_HOME`/`XDG_CONFIG_HOME`/`HOME`
with a log, a crash record and a config produced all six sections, replaced
the scratch home with `~`, replaced both pin paths with `<pin>`, and named the
existing apport file `_usr_bin_gimbal-files.1000.crash` with its date but no
contents. With `--no-paths`, `grep` found no `/tmp`, `/home` or `/var/crash`
path; the log line still read `~/Documents`, and `ubuntu-bug <path>` replaced
the apport path.

## What needs a live session (Claude checks)

1. A normal session writes `logs/gimbal.log` with a header line and everyday
   lines, and no `split: layout` lines unless `GIMBAL_LOG=1`.
2. A scratch log padded to 5 MiB rotates to `.1` and `.2`, and never leaves
   more than three files.
3. Killing Gimbal with SIGSEGV writes `fatal signal 11`; the next start saves a
   crash record and shows the one-time line.
4. `GIMBAL_TEST_PANIC=1` in a debug build panics on startup; the log holds the
   message and a backtrace.
5. `gimbal-files-cli bug-report` writes all six sections, `~` instead of the
   home folder, and no file contents; `--no-paths` removes the other paths.
6. A release `.deb` plus its dbgsym `.deb`: gdb on an apport core shows Gimbal
   function names and lines.
7. Kyle checks the README section and the issue form after the push.

I could not run items 1–3 here: Kyle's instance was already running, and
CLAUDE.md forbids launching then. I also did not send SIGSEGV to any process.

## Outside my files

- No new dependency and no crate feature was needed.
- `packaging/debian/control` needed no dbgsym stanza: `build-deb.sh` writes the
  debug package's control directly, so the source package stays the same.
- The README's "Where your settings live" list does not mention `logs/`,
  `running` or `crashes/`; I left it alone because the brief asked only for a
  new "Reporting a bug" section. A later change could add them there.

## Claude's review (Nov 8, 2026)

**Code.**
- Writes and rotation share one mutex: rotation closes, renames and reopens
  under it.
- The fatal-signal handler uses only `write(2)`, `signal` and `raise` on an
  atomic descriptor.
- Panics go through a hook that calls the previous one.

**[read]** Known trade-off: replacing the SIGSEGV handler also replaces Rust's
own stack-overflow message. An overflow logs as `fatal signal 11` instead of
naming the overflow, but still leaves a crash record and a core.

### Fixes by Claude

1. **An ordinary SIGTERM counted as a crash (`log.rs`, `main.rs`).** Gimbal
   had no SIGTERM handler, so a `kill`, a logout or a shutdown killed it
   without removing the `running` marker. Every next start then saved a
   "crash record" (the first live run produced five) and would have shown
   "closed unexpectedly" after every logout. `install_quit_signals` registers
   SIGTERM, SIGINT and SIGHUP through GLib's `g_unix_signal_add` (declared
   beside the other C entry points: glib 0.22 moved its wrapper into
   `glib-unix`, which is not a dependency). The callback runs on the main loop
   and calls `quit_now` (session saved, workers stopped, marker removed),
   without the unsaved-edit questions, since the session is ending. It is
   installed only by the primary, from `startup`. **[observed]**
2. **The run header's GLib version read `GLib version too old (major
   mismatch)`.** `glib_check_version(99, 0, 0)` returns an error message, not
   the version. The header now reads GLib's exported `glib_major_version`,
   `glib_minor_version` and `glib_micro_version` (`nm -D`), which gives
   `GLib 2.88.0`. **[observed]**
3. **`--no-paths` left home-folder paths** such as `~/Documents/taxes.pdf`.
   File and folder names under home are the private part, so `--no-paths` now
   replaces `~/…` tokens with `<path>` too; a bare `~` stays. The test covers
   it, and the README wording now matches. **[observed]**
4. The panic line read `panic: panic: …` (module plus prefix); it now reads
   `panic: <message> at <location>`.
5. README "Where your settings live" now lists `logs/`, `crashes/` and the
   `running` marker (the agent flagged this).

New `unsafe` (fm-ui, `log.rs`), each with a `// SAFETY:` comment:
- reading the three GLib version statics;
- registering `g_unix_signal_add` sources.

### Live checks (Claude, Nov 8; scratch XDG dirs, each launch < 10 s)

1. **Log: pass.** A normal session writes `logs/gimbal.log` with the header
   (`Gimbal Files 0.1.2; install kind development; GTK 4.22.4; libadwaita
   1.9.1; GLib 2.88.0; XDG_CURRENT_DESKTOP=niri; display=Wayland`) and 45
   lines, with no `split: layout` or `blur:` lines.
2. **Rotation: pass.** Padding the log past 5 MiB three times leaves
   `gimbal.log`, `.1` and `.2`, never more.
3. **SIGSEGV: pass.** It exits with 139, the log has `fatal signal 11`, and
   the marker remains. The next start saves exactly one crash record. A
   SIGTERM stop leaves no marker and no record (after fix 1).
4. **Test panic: pass.** The log has `panic: GIMBAL_TEST_PANIC=1 at
   crates/fm-ui/src/log.rs:243:9` and a forced backtrace with file and line
   numbers. It's followed by Rust's "panic in a function that cannot unwind",
   because the panic happens inside a GTK callback.
5. **`bug-report`: pass.** All six sections. The default shows `~` for home
   (0 occurrences of the real home path) and `<pin>` for pinned folders.
   `--no-paths` gives `moved <path> -> <path>` and `ubuntu-bug <path>`.
6. **Symbols: pass.** A release binary, split exactly as `build-deb.sh` does
   (7 MB stripped, no `.debug_info`, plus a 45 MB `.debug` via debuglink),
   killed with SIGSEGV under gdb, shows `main () at
   crates/fm-ui/src/main.rs:80` and GIO frames with file and line.
7. **For Kyle after the push:** the README "Reporting a bug" section and the
   issue form on GitHub.

**Gates:** `scripts/check.sh` 960 passed, 23 ignored, on two consecutive
runs after `cargo fmt`. `test-install.sh` and `test-deb.sh` pass, and two
builds are byte-identical. **[observed]**

One earlier run failed `volumes::tests::removal_and_reinsertion_churn_keeps_four_outstanding_calls`
with GLib's "default main context already acquired by another thread"
(`glib-0.22.10:src/source.rs:463`): two test threads used the default main
context at once. It passes alone (5 of 5) and in the two full runs above. It
is an existing test-isolation flake, not P2ae; worth serializing those tests
later. **[observed]**
