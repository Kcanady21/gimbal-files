# P2a: the Cargo workspace skeleton

## What I built

- Root `Cargo.toml`: `members = ["crates/*"]`, `exclude = ["spike"]`, resolver 3,
  shared `[workspace.package]`, the pinned GTK crates, path entries for the four
  crates, and `rust.unsafe_code = "deny"` in `[workspace.lints]`.
- `fm-core`: library with `#![forbid(unsafe_code)]`, the crate doc, and
  `APP_ID = "io.github.kcanady21.GimbalFiles"` plus a test for the niri prefix
  and the GLib GApplication ID rule.
- `fm-ui`: binary `gimbal-files`, an `adw::Application` with `fm_core::APP_ID`
  that presents an empty `adw::ApplicationWindow` titled "Gimbal Files". No size
  is set in code. With `GIMBAL_LOG=1` it writes one activate line.
- `fm-agent`: library with `socket_path(runtime_dir)` returning
  `<runtime_dir>/gimbal-files/agent.sock`, never reading the environment, plus
  a unit test.
- `fm-cli`: binary `gimbal-files-cli` with the pure `action_for` decision
  (`--help`/none -> usage, `--version` -> version, anything else -> status 2)
  and four unit tests.
- `deny.toml`, `scripts/check.sh`, and this file.

No file outside the brief's Output list was created or changed. `spike/` was
read only; I ran its build and tests without writing to its sources.

The brief says to read `phase2/RULES.md`, but that file does not exist. The
phase-2 `README.md` and the brief both say the repo's `CLAUDE.md` replaces
Phase 1's `RULES.md`, so I followed `CLAUDE.md` **[observed]**.

## Verified

All commands used the brief's toolchain:

```
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
T="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
```

`rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`
**[observed]**. `cargo-deny 0.20.2` **[observed]**. `deny.toml` was generated
and validated against 0.20.2; an `init` template in `/tmp` confirmed the field
names, and the real config was run with `cargo deny check` **[observed]**.

### `scripts/check.sh`

Run from the repo root; exit status 0. Compile progress is omitted here, but
the step order, test results, deny verdict and final line are the exact output:

```
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.38s
==> cargo test --workspace
     Running unittests src/lib.rs (target/debug/deps/fm_agent-...)

running 1 test
test tests::socket_lives_under_the_runtime_directory ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/main.rs (target/debug/deps/gimbal_files_cli-...)

running 4 tests
test tests::anything_else_is_a_usage_error ... ok
test tests::help_shows_usage ... ok
test tests::no_arguments_shows_usage ... ok
test tests::version_shows_the_version ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/lib.rs (target/debug/deps/fm_core-...)

running 1 test
test tests::app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

     Running unittests src/main.rs (target/debug/deps/gimbal_files-...)

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

   Doc-tests fm_agent: 0 passed
   Doc-tests fm_core: 0 passed
==> cargo build --workspace --release
    Finished `release` profile [optimized] target(s) in 24.90s
==> cargo deny check
warning[duplicate]: found 2 duplicate entries for crate 'system-deps'
   ... (duplicate warnings only; multiple-versions = "warn")

advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

The script `cd`s from `BASH_SOURCE`, quotes every path, exports the 1.95.0
toolchain directory on `PATH`, and uses `set -euo pipefail`. A second run from
`/tmp` (not the repo root) also exited 0 and ended `all checks passed`.
`[observed]`

### Spike still builds on its own

From `spike/`:

```
$ cargo build --release
    Finished `release` profile [optimized] target(s) in 0.04s
$ cargo test
test result: ok. 4 passed; 0 failed; ...
test result: ok. 76 passed; 0 failed; ...
test result: ok. 13 passed; 0 failed; ...
```

Both exit 0 **[observed]**. No file under `spike/` was written. Files touched
(all outside `spike/`): root `Cargo.toml`, `Cargo.lock`, the four crates'
`Cargo.toml` and source files, `deny.toml`, `scripts/check.sh`,
`docs/notes/P2a.md`.

### The app, 5 seconds, SIGTERM

```
$ cd /tmp/gimbal-p2a && GIMBAL_LOG=1 \
    '/home/kyle/Documents/03.) Code-Projects/gimbal-files/target/release/gimbal-files'
[gimbal 0.841] fm_ui: activated
pid=464483 exit=143
```

`exit=143` is SIGTERM. Stderr had only the log line: no GTK warning, and no
`VK_SUBOPTIMAL_KHR` notice in this run **[observed]**.

### CLI

```
$ gimbal-files-cli --version
gimbal-files-cli 0.1.0
exit=0

$ gimbal-files-cli --help
Usage: gimbal-files-cli [--help | --version]

Commands come later, through the command registry; none are available yet.
exit=0

$ gimbal-files-cli            # no arguments
(same usage, stdout)
exit=0

$ gimbal-files-cli bogus
stdout: (empty)
stderr:
Usage: gimbal-files-cli [--help | --version]

Commands come later, through the command registry; none are available yet.
exit=2
```

**[observed]**

## License list

`cargo deny list` on the lockfile reports these licenses, and `deny.toml`
allows exactly them **[observed]**:

| License | Crates | Why it is GPL-3.0-or-later compatible |
| --- | --- | --- |
| `GPL-3.0-or-later` | the four workspace crates | The project's own license. |
| `MIT` | 63, including gtk4-rs, glib-rs, cairo-rs, pango-rs | Permissive; GPL-compatible. |
| `Apache-2.0` | 35, including the toml/system-deps/quote toolchain | Permissive; the FSF lists Apache-2.0 as GPLv3-compatible. |
| `Apache-2.0 WITH LLVM-exception` | `target-lexicon` | Apache-2.0 with an additional permissive exception. |
| `Unicode-3.0` | `unicode-ident` | Permissive MIT-style Unicode license. |
| `Unlicense` | `memchr` (also MIT) | Public-domain dedication; GPL-compatible. |

## Bans proof

I temporarily added `gtk.workspace = true` to `crates/fm-core/Cargo.toml`, then
ran `cargo deny check bans`. It failed with status 2 **[observed]**:

```
warning[unmatched-wrapper]: direct parent 'fm-core = 0.1.0' of banned crate
'gtk4 = 0.11.5' was not marked as a wrapper
   ├── fm-core v0.1.0

error[banned]: crate 'gtk4 = 0.11.5' is explicitly banned
   ┌─ deny.toml:44:16
44 │ { crate = "gtk4", wrappers = ["fm-ui", "libadwaita"], reason = "only fm-ui may use GTK" },
   └─ gtk4 v0.11.5
```

I reverted the line and re-ran `cargo deny check`: `bans ok` and the full
`advisories ok, bans ok, licenses ok, sources ok` line above.

Note on syntax: cargo-deny 0.20.2 checks `wrappers` against **direct parents**.
`libadwaita` lists only `fm-ui`; `gtk4` lists `fm-ui` and `libadwaita`
(libadwaita wraps gtk4), and every lower toolkit crate lists the toolkit
crates directly above it (`gtk4`, `gdk4`, `gsk4`, `libadwaita`, and their
`-sys` crates). Any non-`fm-ui` crate that adds GTK, directly or through the
toolkit, still fails, as the proof above shows: `fm-core` was not an allowed
parent of `gtk4`. `[read]` from the 0.20.2 bans docs, `[observed]` in the two
runs.

## API citations

- GApplication ID rule: GLib's `g_application_id_is_valid()`, documented at
  <https://docs.gtk.org/gio/func.Application.id_is_valid.html>; the binding is
  `gio-sys-0.22.9/src/lib.rs:10027`, aliased at
  `gio-0.22.10/src/auto/application.rs:53`. The same rule is enforced by GTK at
  `gtk-4.22.4/gtk/gtkapplication.c:822` (`../gimbal-refs/`). `[read]`
- `adw::Application::builder()`: `libadwaita-0.9.2/src/auto/application.rs:30`;
  `.application_id(...)`: `:82`. `[read]`
- `adw::ApplicationWindow::builder()`:
  `libadwaita-0.9.2/src/auto/application_window.rs:59`. The inherited
  `.application(...)` and `.title(...)` builder methods are
  `gtk4-0.11.5/src/auto/application_window.rs:95` and `:237`. `[read]`
- `adw::Application` is a `gio::Application`, so `run()` returns
  `glib::ExitCode`; `gtk::glib` is the re-export used in `fm-ui`. `[read]`
- cargo-deny 0.20.2 `[bans]` schema, including `wrappers` semantics:
  <https://embarkstudios.github.io/cargo-deny/checks/bans/cfg.html>. `[read]`

## Not verified live

For Claude to check in a live niri session **[inferred]**:

1. The window actually appears under niri and shows the title "Gimbal Files"
   with no content. The process ran and logged `activated`, but I did not look
   at the screen.
2. niri's focus-ring/blur rule matches this window because its app ID is
   `io.github.kcanady21.GimbalFiles`. Only the string prefix and GLib validity
   were unit-tested; the blur itself needs eyes on niri.

## Anything outside my files

Nothing needs changing. Two notes for review:

- The brief's `phase2/RULES.md` is missing; `CLAUDE.md` is the effective rules
  file, as the phase-2 README says.
- `scripts/check.sh` uses the absolute rustup 1.95.0 path from `CLAUDE.md`
  (`$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin`) instead of
  `rustup run`. It exits with a clear error if that toolchain is missing,
  rather than silently falling back to the distro cargo.

## Review and live check (Claude, Nov 2, 2026): passes

The code matches the brief. The four crates are small, `fm-core` has no dependencies, and the
non-UI crates forbid `unsafe`. The CLI's decision and the socket path are pure functions with
tests. (The brief never mentioned a `phase2/RULES.md`; `CLAUDE.md` is the rules file, as the agent
assumed.)

- **`scripts/check.sh` from `/tmp`:** exit 0. fmt and clippy are clean, the tests pass
  (1 + 4 + 1), the release build works, and deny reports
  `advisories ok, bans ok, licenses ok, sources ok`.
- **The spike:** `cargo build --release` still works from `spike/`, and git shows no change there.
- **A second ban check:** with `gdk4 = "=0.11.5"` added straight to `fm-cli`,
  `cargo deny check bans` reported `error[banned]: crate 'gdk4 = 0.11.5' is explicitly banned`.
  Restored afterwards (`bans ok`).
- **Live under niri:** the window opens tiled, titled "Gimbal Files", with app ID
  `io.github.kcanady21.GimbalFiles`, which Kyle's `draw-border-with-background false` rule
  matches. The only stderr was the log line and the allowed `VK_SUBOPTIMAL_KHR` notice.
- **Single instance:** a second launch hands off to the running instance through GApplication,
  and `activate` opens a second window in the same process. That suits a file manager, but P2d's
  workspace model should own what a second launch does (new window, or a new tab in the focused
  window).

For later briefs:
- When `fm-ui` takes `gdk4-wayland` for the blur, it needs its own `deny.toml` entry (wrapper
  `fm-ui`), and `gdk4`'s wrappers gain `gdk4-wayland`. Otherwise bans fails.
- The GIO crates (`gio`, `glib`) are not banned; `fm-core` will use them for file I/O.
