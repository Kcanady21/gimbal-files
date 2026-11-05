# P2v: a daily update notice (no in-app install)

## What this change does

- Adds `fm-core::update`, a notice-only check for the latest release:
  - derives the download base from `CARGO_PKG_REPOSITORY`;
  - fetches `latest.toml` and `latest.toml.minisig` behind a `Fetcher` trait;
  - verifies the minisign signature and the exact trusted comment;
  - parses `latest.toml` with unknown keys refused;
  - compares `MAJOR.MINOR.PATCH` versions numerically.
- Adds install-kind detection from the canonical `current_exe()` and the P2u
  `install.toml` marker: `System`, `Tarball` or `Development`.
- Adds `[updates] check = true|false`, default `true`, parsed like `[agent]`.
- Adds `<state dir>/gimbal-files/update.toml` with `last_check` and `notified`,
  written atomically and separately from the session.
- Adds the startup check and the 24-hour timer, both skipped in development
  builds and when the setting is off.
- Adds the UI-only `updates.set-enabled enabled=<bool>` and `updates.check`
  commands. Both are `Effect::System`, so the local socket refuses them before
  dispatch, like `drive.unmount`.
- Adds the Settings → Updates group, the one-time summary line, and the
  tarball-only releases-page button.
- Replaces the README's "Updates" paragraph and lists the new commands in
  `docs/agent-socket.md`.

No session schema change.

There is no `phase2/RULES.md`. I read `phase1/RULES.md` and the repository's
`CLAUDE.md`, which `phase2/README.md` names as its replacement. **[read]**

## Files written

- `crates/fm-core/src/update.rs` (new)
- `crates/fm-core/src/lib.rs`
- `crates/fm-core/src/config.rs`
- `crates/fm-core/src/command/catalogue.rs`
- `crates/fm-core/src/command/tests.rs`
- `crates/fm-core/build.rs` (new; item 2 needs one)
- `crates/fm-core/Cargo.toml`
- `Cargo.toml`, `Cargo.lock`, `deny.toml`
- `crates/fm-ui/src/update.rs` (new; the glue is more than a screen)
- `crates/fm-ui/src/app.rs`
- `crates/fm-ui/src/header.rs`
- `README.md` (only the "Updates" paragraph)
- `docs/agent-socket.md` (the UI-only command list and one explanatory
  sentence)
- `crates/fm-core/tests/data/test-only*` (the test-only key material and
  signatures the brief's Tests section requires)
- `docs/notes/P2v.md` (this file)

No `packaging/minisign.pub` was created.

## Dependencies

Pinned in `[workspace.dependencies]`:

```toml
ureq = { version = "=3.5.1", default-features = false, features = ["rustls-no-provider", "_ring", "platform-verifier"] }
minisign-verify = "=0.3.0"
```

### `ureq`

An HTTPS client in `fm-core`. It uses rustls and the operating system's
certificate verifier, not a bundled root list. `rustls-no-provider` plus
`_ring` gives rustls with the ring provider; `platform-verifier` wires in
`rustls-platform-verifier`. I explicitly select `RootCerts::PlatformVerifier`
instead of relying on a default. No gzip, brotli, cookies or JSON features are
enabled.

The brief names ureq's `native-certs` option. That is a ureq 2 feature. I first
tried ureq 2.12.1 with `tls` + `native-certs`; `cargo deny check` then failed
on `RUSTSEC-2025-0134`, because `rustls-native-certs 0.7.3` still pulls the
unmaintained `rustls-pemfile 2.2.0`. ureq 3.5.1 with `platform-verifier` uses
`rustls-native-certs 0.8.5` and loads the same OS trust store, with no
`rustls-pemfile` in `Cargo.lock`. That is the current equivalent of the
brief's intent. **[observed: the advisory and the absence of `rustls-pemfile`
in the final `Cargo.lock`]**

API citations:

- `ureq-3.5.1:src/config.rs:761` — `timeout_global`.
- `ureq-3.5.1:src/config.rs:632` — `user_agent`.
- `ureq-3.5.1:src/config.rs:548` — `tls_config`.
- `ureq-3.5.1:src/tls/mod.rs:221` — `root_certs`.
- `ureq-3.5.1:src/tls/mod.rs:307` — `TlsConfigBuilder::build`.
- `ureq-3.5.1:src/body/mod.rs:504` — `BodyWithConfig::limit`.
- `ureq-3.5.1:src/body/mod.rs:607` — `read_to_vec`.
- `ureq-3.5.1:src/error.rs:14` — `Error::StatusCode(u16)`.
- `ureq-3.5.1:src/error.rs:68` — `Error::BodyExceedsLimit(u64)`.

### `minisign-verify`

Verifies minisign Ed25519 signatures and the signed trusted comment. It has no
external dependencies. The crate's verifier checks the global signature over
`signature || trusted comment`, which is why a rewritten trusted comment cannot
pass as a valid signed comment.

API citations:

- `minisign-verify-0.3.0:src/lib.rs:351` — `PublicKey::verify`.
- `minisign-verify-0.3.0:src/lib.rs:277` — `Signature::trusted_comment`.
- `minisign-verify-0.3.0:src/lib.rs:374` — `verify_stream` (not used; the
  notice is capped at 4 KB).

### Licenses

Two new licenses are allowed in `deny.toml`, each with the reason next to it:

- `BSD-3-Clause` (`subtle`, through rustls): permissive, OSI-approved, and
  GPL-compatible; only attribution plus warranty/endorsement terms.
- `CDLA-Permissive-2.0` (`webpki-root-certs`, in the platform verifier's tree):
  a permissive data license. It allows use, modification and sharing, imposes
  no condition on results, and asks only that the agreement accompany shared
  copies of the data.

`cargo deny check` passes:
`advisories ok, bans ok, licenses ok, sources ok`. **[observed]**

## The release key

`crates/fm-core/build.rs` looks for `packaging/minisign.pub` relative to
`CARGO_MANIFEST_DIR`, emits `cargo:rerun-if-changed=…`, and, when the file
exists, bakes its second line (the base64 key) into
`GIMBAL_RELEASE_PUBKEY` with `cargo:rustc-env`.

`fm_core::update::release_key()` reads it with `option_env!`. A missing file
means the variable is absent, `option_env!` is `None`, and the build still
succeeds. The check then returns `Failed(NoKey)` before any fetch, whose text
is exactly `This build has no release key`.

In debug builds only, `GIMBAL_UPDATE_PUBKEY` names a file that replaces the
embedded key. That branch is behind `#[cfg(debug_assertions)]`, so release
builds ignore it.

Citations:

- Cargo Book, "Build Scripts": `cargo::rustc-env=VAR=VALUE` sets an environment
  variable for the compile, and `cargo::rerun-if-changed=PATH` reruns the script
  when the path changes.
  <https://doc.rust-lang.org/cargo/reference/build-scripts.html#cargorustc-envvarvalue>
- Rust standard library: `option_env!` evaluates to `None` when the variable
  is not set at compile time.
  <https://doc.rust-lang.org/std/macro.option_env.html>

`packaging/minisign.pub` does not exist in this checkout, so this build has no
release key. The build still passes. **[observed]**

## The check

- Base URL: `https://github.com/Kcanady21/gimbal-files/releases/latest/download/`,
  built from `CARGO_PKG_REPOSITORY`.
- `GIMBAL_UPDATE_URL` replaces it in any build. Plain `http://` is accepted
  only when the host is `127.0.0.1` or `localhost` (with an optional port).
- Both files are fetched with `User-Agent: gimbal-files/<version>` and a 15 s
  global timeout. Each response is limited to 4 KB plus one byte; the extra
  byte detects a body exactly over the cap.
- Signature first, then the trusted comment. The comment must be exactly
  `gimbal-files <version> latest`, using the version parsed from the same
  bytes. A replayed older signed release therefore verifies but is `UpToDate`
  when it is not newer than the running version.
- `latest.toml` is parsed with `toml_edit`; unknown keys, missing keys, wrong
  types, an empty tarball name and malformed versions are a parse failure.
- Results are `UpToDate`, `Available(Version)` and `Failed(FailedReason)`.
  `FailedReason` covers offline/DNS, an HTTP status (404 included), too large,
  a bad signature, a bad trusted comment, a parse error, no key and an invalid
  URL. Each has one short text in `update::text`.

### Install kind

`install_kind(canonical_exe, marker)` is pure:

- `/usr/bin/gimbal-files` → `System` (the `.deb`);
- `canonical_exe == marker.bin_dir/gimbal-files` → `Tarball`;
- anything else, including `target/`, → `Development`.

Production canonicalizes `current_exe()` and the marker's `bin_dir`. The
tarball marker path follows `XDG_DATA_HOME` (or `~/.local/share`). A
`--prefix` install whose data is under `DIR/share` is not found unless that
`DIR/share` is the current data directory; that is the same marker-location
limit P2u recorded, and the README's normal tarball install is covered.

Development builds never check; Settings says
`Development build: update checks are off`.

### State and schedule

`update.toml` is at `$XDG_STATE_HOME/gimbal-files/update.toml`, or
`~/.local/state/gimbal-files/update.toml`; it is written through the existing
`fsutil::save_atomic` path. It contains only:

```toml
last_check = 1700000000
notified = "0.1.1"
```

`should_check` treats a missing timestamp as due, a timestamp of 24 h or more
as due, and any timestamp in the future as stale (also due). `seconds_until_check`
gives the remaining part of the 24 hours, so a launch 23 hours after the last
check waits one hour rather than a whole day. The one-shot timer is re-armed
from the state after startup, after each finished check, and after the switch
changes. Every finished check, successful or failed, writes `last_check`. A
missing or malformed state file loads as "never checked" and is repaired by the
next check.

## Commands

Catalogue order now includes:

```text
updates.set-enabled enabled=bool
updates.check
```

Both are `Effect::System`. The local socket already refuses every `System`
effect before dispatch, so no socket request can change or trigger the check.
The Settings rows dispatch as `Caller::Ui`; the handler also refuses any caller
other than `Ui` as a second guard.

The ordered-name test and a caller/shape test were updated in
`crates/fm-core/src/command/tests.rs`; `catalogue.rs` has a focused test for the
two new entries.

## UI

`crates/fm-ui/src/update.rs` owns the runtime: the state file, install kind,
one in-flight check, a 100 ms result poll, the 24-hour timer, the status
snapshot, the summary, and the command handler. It follows the existing
worker pattern: a background thread does the blocking request and sends one
result over an `mpsc` channel; the GTK main loop drains it.

`header.rs` adds the Settings group under the automation row:

- `Gimbal Files <version>`;
- a `Check for updates` switch (`updates.set-enabled`);
- a `Check now` button (`updates.check`, disabled while checking or in a
  development build);
- a status line: `Up to date`, `Checking…`, the failure text, the available
  how-to, or the development text;
- for a tarball install with an available version, `Open releases page`.
  The button uses `gtk::UriLauncher`:
  `gtk4-0.11.5:src/auto/uri_launcher.rs:24` (`new`) and `:56` (`launch`).

The one-time summary is shown in the focused window as one line:
`Gimbal Files <v> is available.` followed by the same how-to sentence. The
version is recorded in `notified` after the summary, so a restart with the same
`notified` stays quiet.

All user-facing update strings live in `fm_core::update::text`. The UI only
chooses a string and renders it.

## Logging

With `GIMBAL_LOG=1`, every check start writes one line:

```text
[gimbal …] update: check started (Gimbal Files 0.1.0, tarball install)
```

and every finished check writes one line:

```text
[gimbal …] update: check finished: 0.1.1 available
```

A failure uses `check finished: failed: <short text>`. Skipped checks
(development build, setting off, not due) write no `update:` line and make no
request.

## Tests

All tests are in `crates/fm-core/src/update.rs`, `config.rs`,
`command/tests.rs`, `catalogue.rs`, and `crates/fm-ui/src/update.rs`. No test
uses the network.

- Version parsing and comparison: equal, older, newer, `0.10.0` vs `0.9.0`,
  and malformed forms.
- `latest.toml` parse: valid, unknown key, missing key, wrong type, bad
  version, empty tarball, non-table and invalid TOML.
- Trusted comment: exact match, wrong version, missing suffix, trailing space,
  wrong case.
- Announcement: a version newer than `notified` is shown once; equal and older
  are quiet.
- Schedule: missing, recent, exactly 24 h, older, and future timestamps, plus
  the remaining seconds used to arm the one-shot timer.
- Install kind: `/usr/bin`, marker match, marker mismatch, `target/`, and a
  real symlinked scratch path canonicalized to the marker's real `bin_dir`.
- Marker and state paths under XDG and home, absolute `bin_dir`, and an atomic
  state round trip with no temp file left.
- Config: `[updates]` default true, explicit false/true, wrong type, unknown
  key, wrong shape, parse error, and a byte-preserving save that keeps
  comments and `[agent]`.
- Catalogue: exact order, `System` effect, no undo, required bool for
  `updates.set-enabled`, no arguments for `updates.check`, and the UI handler
  path.
- Signature verification with a committed test-only key pair.
- The fetch seam with an in-memory fake: HTTP 404, an oversized body, a bad
  signature, and success (`0.1.1` available from a `0.1.0` build).
- A replayed signed `0.1.0` notice is `UpToDate` from a `0.1.1`/`0.1.2` build.

### The test-only key pair

`crates/fm-core/tests/data/` contains:

- `test-only.pub` — minisign public key format;
- `test-only.key` — the 32-byte Ed25519 seed in base64, with a comment saying
  it is test-only and must never sign a release;
- `test-only-latest-0.1.0.toml` and `.minisig`;
- `test-only-latest-0.1.1.toml` and `.minisig`;
- `test-only-wrong-comment.minisig` (a valid signature over `0.1.1` whose
  trusted comment names `9.9.9`);
- `test-only-bad.minisig` (one bit changed in a valid signature).

They were made offline with Python's `cryptography` Ed25519 and
`hashlib.blake2b`, following the minisign file format:

- public key: `Ed` + 8-byte key id + 32-byte public key, base64;
- signature line: `ED` (pre-hashed) + the same 8-byte key id + 64-byte
  Ed25519 signature over `blake2b-512(content)`;
- trusted-comment line: `trusted comment: <text>`;
- global signature: Ed25519 over `signature || <text>`, base64.

The key id is the ASCII `testonly`. These are **not** release keys.

## Verification

### Gates

`scripts/check.sh` on the final tree exited 0
(`/tmp/gimbal-p2v-check-sh-final9.log`):

```text
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

Two earlier runs failed in `fm-agent --test socket` with `BrokenPipe` or a
socket test timing out, followed by "a previous test leaked transport workers"
knock-ons. That is the existing timing flake P2w recorded; P2v changes no agent
code. Each immediate full rerun passed. **[observed]**

Test totals: **850 passed, 23 ignored** (23 + 16 + 12 + 389 + 6 + 11 + 393
passing; 1 + 6 + 11 + 5 ignored). The P2u/P2w baseline was 825 passed / 23
ignored; P2v adds 25 passing tests. **[observed]**

`cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings` pass, and
`cargo build --workspace --release` is part of `scripts/check.sh`. **[observed]**

### What was not run

- No GUI launch. The live settings layout, the switch, the releases button,
  the one-time summary and the restart-quiet rule need Claude. I used no
  pointer or keyboard injection.
- No real minisign key or signature. `packaging/minisign.pub` and
  `minisign` are both absent here.
- No real GitHub request. The check is tested through the fake fetcher, and
  the real network path was not exercised.
- No `scripts/test-install.sh` or `scripts/test-deb.sh` rerun; neither file
  changed. P2v does not run an installer.

## What Claude checks live

1. **Debug build, scratch key, local server, fake tarball install.**
   Build `target/debug/gimbal-files`. Make a scratch prefix, for example:

   ```sh
   S=/tmp/gimbal-p2v-live
   mkdir -p "$S/bin" "$S/data/gimbal-files" "$S/state" "$S/config" "$S/www"
   cp target/debug/gimbal-files "$S/bin/"
   cat >"$S/data/gimbal-files/install.toml" <<EOF
   bin_dir = "$S/bin"
   prefix = "$S"
   data_dir = "$S/data"
   version = "0.1.0"
   EOF
   ```

   Put a signed `latest.toml` for `0.1.1` in `$S/www`, serve it with
   `python3 -m http.server 8000 --directory "$S/www"`, and start the binary
   with:

   ```sh
   XDG_CONFIG_HOME="$S/config" XDG_STATE_HOME="$S/state" \
   XDG_DATA_HOME="$S/data" GIMBAL_LOG=1 \
   GIMBAL_UPDATE_URL="http://127.0.0.1:8000/" \
   GIMBAL_UPDATE_PUBKEY="$S/scratch.pub" \
   "$S/bin/gimbal-files"
   ```

   Check now should show `0.1.1 is available. Download it from the releases
   page.` and the releases button. The summary `Gimbal Files 0.1.1 is
   available. Download it from the releases page.` should appear once; after a
   restart with `$S/state/gimbal-files/update.toml` still holding
   `notified = "0.1.1"`, it must not appear again. The log must show one
   `update: check started` and one `update: check finished` line.

   Because `minisign` is not installed here, use either the installed
   `minisign -G`/`minisign -S -t "gimbal-files 0.1.1 latest"` pair, or the
   test-only seed and the Python format above. The latter is a scratch key, not
   a release key.

2. **Tampered notice.** Change one byte in `$S/www/latest.toml` after signing
   it. Check now must show the signature failure text
   (`The update notice is not signed correctly.`) and must not show a summary.

3. **Replayed older notice.** Sign a `0.1.0` `latest.toml` with the same
   scratch key and serve it from the same URL. Check now must show `Up to
   date` and no summary.

4. **Development build.** Start `target/debug/gimbal-files` from the checkout,
   with scratch XDG folders. Settings must say
   `Development build: update checks are off`, the switch and Check now must be
   insensitive, and `GIMBAL_LOG=1` output must contain no `update:` request
   lines. Use SIGTERM by PID within 10 seconds.

5. **Real private repository.** With `packaging/minisign.pub` present, start a
   normal build and point it at the real
   `https://github.com/Kcanady21/gimbal-files/releases/latest/download/` URL.
   While the repository is private, the one quiet result should be a failure in
   the log only, with no dialog and no summary, and `update.toml` must still
   gain `last_check`.

6. **Switch off.** Put `[updates] check = false` in the scratch config and
   restart. Settings shows `Update checks are off.`, and the log must show no
   request at startup.

## Outside the Output list

- `crates/fm-core/tests/data/test-only*` is required by the brief's Tests
  section, although the Output list names only source and documentation files.
- `crates/fm-core/build.rs` is item 2's build script.
- `crates/fm-ui/src/update.rs` is allowed when the UI glue is more than a
  screen; it is declared from `app.rs` with
  `#[path = "update.rs"] pub mod update;` because `main.rs` is not in the
  Output list.
- `packaging/debian/copyright` is a committed snapshot of `Cargo.lock`'s
  crates (P2w). It now needs the new crates (`ureq`, `minisign-verify`,
  `rustls`, `rustls-platform-verifier`, `webpki-root-certs`, and the rest)
  before the next real release. That file is outside this brief, and
  `build-deb.sh` does not run cargo.
- The README's "Where your settings live" section does not mention the update
  state file; the brief allows only the "Updates" paragraph, so it was left
  alone. A later edit could add
  `$XDG_STATE_HOME/gimbal-files/update.toml`.
- A custom `--prefix` tarball install whose marker lives under `DIR/share` is
  not discovered unless that directory is the current `XDG_DATA_HOME`; the
  normal `~/.local` install and any marker under the current data directory
  are found. This is the marker-location limit P2u's notes recorded.
- I did not add an `apply_updates_enabled` rollback like the agent switch. A
  failed config write is reported as `Outcome::Failed`, but the in-memory
  value follows the existing `ConfigFile::apply` behavior.

## Claude's review (Nov 5, 2026)

Verdict: **ready to commit**, with the copyright fix below.

### Gates

`scripts/check.sh` passes first time: **850 passed, 23 ignored**. **[observed]**

### Code

- **Dependencies.** ureq 3 with `platform-verifier` instead of ureq 2's `native-certs` is the
  right call, given the `rustls-pemfile` advisory. On Linux, `cargo tree --target
  x86_64-unknown-linux-gnu -i webpki-root-certs` finds nothing, so no bundled root list is
  compiled in and the system store is used. `CDLA-Permissive-2.0` is in `deny.toml` only because
  cargo-deny checks all targets. I'm satisfied it's compatible, and it doesn't ship on Linux.
  **[observed]**
- **Check order.** The size cap comes first, then the parse, then the signature over the exact
  bytes, then the trusted comment, which must match the version in the same bytes. A replayed
  older notice is `UpToDate`. **[read]**
- **Threads.** One worker thread per check, results over `mpsc`, and the 100 ms poll runs only
  while a check is in flight. No `RefCell` borrow is held across `finish` or `show_summary`.
  **[read]**
- **Module placement.** `#[path = "update.rs"] pub mod update;` inside `app.rs` makes it
  `crate::app::update`, so it can reach `Process`'s private `update` field. It's unusual, but it
  works and keeps `main.rs` out of the change. **[read]**

### Live checks

Run on a debug build copied to a scratch fake tarball install (marker `bin_dir` pointing at
it), with scratch XDG dirs, a fresh scratch key (minisign format, signed with Python
`cryptography`), and `python3 -m http.server` on `127.0.0.1`. The window was driven through
AT-SPI actions, because niri reported no screen position for the tiled window and pointer
clicks missed. Every run stayed under 10 s and was stopped with SIGTERM. **[observed]**

| # | Case | Result |
|---|---|---|
| 1 | Signed 0.1.1, fresh state | Startup check: `check started` then `0.1.1 available`. The summary label `Gimbal Files 0.1.1 is available. Download it from the releases page.` shows, and `notified = "0.1.1"` is saved. Settings shows `Gimbal Files 0.1.0`, the switch, Check now, **Open releases page**, and `0.1.1 is available. Download it from the releases page.` |
| 1b | Restart with `notified = "0.1.1"` and the check due | The check runs again, finds 0.1.1, and shows no summary |
| 2 | One byte changed in `latest.toml` | Check now: `The update notice is not signed correctly.` No summary, `notified` untouched |
| 3 | Signed 0.1.0 replayed | Check now: `Up to date` |
| 3b | Signed 0.1.1 whose comment names 9.9.9 | `The update notice's signed comment does not match.` |
| 4 | `target/debug` from the checkout | Settings: `Development build: update checks are off`. Switch and Check now are insensitive, there are no `update:` log lines, and no `update.toml` is written |
| 5 | Real GitHub URL (repo private) | One quiet `failed: The update server returned HTTP 404.` in the log only. `last_check` written |
| 6 | `[updates] check = false` | Settings: `Update checks are off.` and no `update:` lines |
| 7 | No key (no `GIMBAL_UPDATE_PUBKEY`, no `packaging/minisign.pub`) | `failed: This build has no release key`, with no request |

Not covered live: the releases button actually opening a browser. I didn't press it, because
that would open Kyle's real browser. A System (`/usr/bin`) install can't be faked without root,
so the `sudo apt upgrade` how-to text is covered only by the unit tests. Both are worth a look
after the first real release.

### Fixed by Claude

- **`packaging/debian/copyright` was stale.** P2v adds 53 crates. A new
  `scripts/copyright-crates.py` compares the list with `cargo metadata`, and `--write` replaces
  it. I ran `--write`, then added the full BSD-3-Clause text (`subtle`, with its copyright
  notice, which BSD requires in binary distributions) and the full CDLA-Permissive-2.0 text
  (`webpki-root-certs`). `python3-debian` parses the file as 11 paragraphs. `release.sh` now runs
  the check after the release build and refuses a stale list, with the regenerate command in the
  error. `scripts/test-deb.sh` still passes. **[observed]**
- **README.** "Where your settings live" now lists `update.toml`.

### Smaller points (no change)

- The summary is skipped, and `notified` isn't recorded, when no window is focused or trash or
  search is active. The next check announces it instead. That's fine.
- `ureq` follows redirects (GitHub's `latest/download` redirects to its object store). The
  signature, not the transport, is what's trusted, so that's fine.
- The `fm-agent` socket flake from P2w didn't recur here, and this run passed first time.
