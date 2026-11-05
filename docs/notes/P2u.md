# P2u: install, launcher, README and a signed release

## What this change does

- Adds the GPL-3.0 text as `LICENSE`, copied byte-for-byte from
  `/usr/share/common-licenses/GPL-3`.
- Adds a per-user desktop entry, a scalable placeholder icon, and
  `scripts/install.sh` with `--prefix DIR`, `--uninstall`, XDG data handling,
  atomic copy-then-rename, desktop/icon cache refreshes, a PATH warning and the
  `install.toml` marker P2v will read.
- Adds `scripts/release.sh`: preflight checks, `scripts/check.sh`, a release
  build, the exact `dist/gimbal-files-<version>-x86_64-linux/` layout, the
  exact `dist/latest.toml`, tarball creation, two minisign signatures with the
  exact trusted comments, and a confirmed `gh release create`.
- Adds a user-facing `README.md` and the release installer test
  `scripts/test-install.sh`.
- Sets the window's default icon name to the app id in
  `crates/fm-ui/src/main.rs` after GTK initialization.
- Adds `dist/` to `.gitignore`. That is the one change outside the brief's
  Output list, as item 6 requires; it is noted below too.

No Rust dependency, config schema, session schema, command or socket change.

There is no `phase2/RULES.md`. I read `phase1/RULES.md` and the repository's
`CLAUDE.md`, which `phase2/README.md` names as its replacement.

## Files written

- `README.md` (new)
- `LICENSE` (new; unchanged GPL-3 text)
- `packaging/io.github.kcanady21.GimbalFiles.desktop` (new)
- `packaging/icons/io.github.kcanady21.GimbalFiles.svg` (new placeholder)
- `scripts/install.sh` (new)
- `scripts/release.sh` (new)
- `scripts/test-install.sh` (new; required by the brief's Tests section)
- `crates/fm-ui/src/main.rs` (one icon call)
- `.gitignore` (`dist/`; required by item 6)
- `docs/notes/P2u.md` (this file)

No key was generated and no `packaging/minisign.pub` was created.

## README minimums

The README's minimums come from the workspace manifest:
`Cargo.toml:10` sets `rust-version = "1.92"`; `Cargo.toml:16` pins
`gtk4 0.11.5` with feature `v4_22`; `Cargo.toml:17` pins
`libadwaita 0.9.2` with feature `v1_9`. GTK 4.22 and libadwaita 1.9 are
therefore the real minimums, not guesses. **[read]**

## Desktop entry and app id

`niri msg --json windows` on a scratch run showed:

```
"title":"Gimbal Files","app_id":"io.github.kcanady21.GimbalFiles"
```

The run used a scratch `HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME` and
`XDG_DATA_HOME` under `/tmp/gimbal-p2u-appid`, and was stopped with SIGTERM.
The desktop entry therefore sets:

```
Exec=gimbal-files %U
Icon=io.github.kcanady21.GimbalFiles
StartupWMClass=io.github.kcanady21.GimbalFiles
StartupNotify=true
```

`desktop-file-validate` is installed at `/usr/bin/desktop-file-validate`; it
reported no output and exited 0 on the committed desktop file. **[observed]**

## App icon

`packaging/icons/io.github.kcanady21.GimbalFiles.svg` is a **placeholder**
based on `crates/fm-ui/icons/gimbal-folder-symbolic.svg`. Its own XML comment
says Kyle will replace it. It uses fixed colours because the hicolor theme
loads it outside Gimbal's CSS palette, where the symbolic `gpa:fill` slots do
not apply. **[read]**

The window icon is set in `ensure_process`, not in `main` before the app runs:
calling `gtk::Window::set_default_icon_name` before GTK initialization panics
with "GTK has not been initialized" (observed on the first launch attempt).
GtkApplication initializes GTK in its startup vfunc, so the call is safe from
`activate`/`open`, both of which pass through `ensure_process`.

API citations:

- `gtk4-0.11.5:src/auto/window.rs:90-94`:
  `gtk::Window::set_default_icon_name(name)`.
- GTK 4.22.4 C source `gtk/gtkwindow.c:3651-3667`:
  `gtk_window_set_default_icon_name` stores the fallback icon name.
- GTK 4.22.4 C source `gtk/gtkapplication.c:304-315`: `GtkApplication`
  already sets the default icon name to the application id when the icon
  theme has that name. The explicit call makes the intent local and does not
  depend on that fallback.
- GTK 4.22.4 C source `gtk/gtkwindow.c:3523-3525`: the stored name is looked
  up in the icon theme.

## install.sh

- `set -euo pipefail`, SPDX header, executable.
- Detects a release tarball by `install.sh`, `bin/gimbal-files` and
  `latest.toml` at the script's root. Otherwise it treats itself as
  `scripts/install.sh` in a checkout and uses `target/release/`. It never
  runs cargo, and refuses with a clear message when the release binaries are
  missing. **[observed]**
- `--prefix DIR` sets the prefix and always puts data under `DIR/share`.
  Without it, the prefix is `~/.local`, the bin directory is
  `~/.local/bin`, and `XDG_DATA_HOME` (when absolute) replaces
  `~/.local/share`. A relative `XDG_DATA_HOME` falls back to the prefix.
- Files are copied to `.<name>.tmp.$$` in the destination folder, chmodded,
  then renamed with `mv -f`, so a running Gimbal keeps its old binary until
  restart.
- Installs both binaries (0755), the desktop entry (0644) and the icon
  (0644) to the paths in item 4.
- Writes `<data dir>/gimbal-files/install.toml` atomically with exactly
  `bin_dir`, `prefix` and `version`. `bin_dir` is recorded with `pwd -P`, so
  P2v's canonicalized `current_exe` comparison sees the real location. The
  prefix stays the absolute directory the caller chose.
- Version comes from the tarball's `latest.toml` in tarball mode and from the
  `[workspace.package]` section of `Cargo.toml` in checkout mode.
- Runs `update-desktop-database` and `gtk-update-icon-cache` when present and
  ignores their absence or failure. Neither is required.
- Warns (does not fail) when the bin directory is not on `PATH`.
- `--uninstall` removes only the two binaries, the desktop entry, the icon
  and the marker. It never removes directories and never touches
  `~/.config/gimbal-files` or `~/.local/state/gimbal-files`.
- Needs no sudo. It never runs `xdg-mime`; the README documents that step.

## release.sh

- Refuses unless the tree is clean (tracked and untracked), HEAD is `main`,
  local `main` equals `origin/main` (after `git fetch origin main`), and tag
  `v<version>` exists neither locally nor on `origin`.
- Runs `scripts/check.sh` itself, then `cargo build --workspace --release`.
- Stages `dist/gimbal-files-<version>-x86_64-linux/` with `bin/`,
  `share/applications/`, `share/icons/hicolor/scalable/apps/`, `install.sh`,
  `README.md`, `LICENSE` and `latest.toml`.
- Writes `dist/latest.toml` exactly as required and verifies every tar entry
  stays under the single top-level folder.
- Signs the tarball with trusted comment `gimbal-files <version>
  x86_64-linux` and `latest.toml` with `gimbal-files <version> latest`, using
  `${MINISIGN_KEY:-$HOME/.minisign/gimbal-files.key}`.
- Prints the four files, asks `Publish v<version>? [y/N]`, and only then runs
  `gh release create v<version> --target main --title v<version>
  --notes-file <file>` with the tarball, `latest.toml` and both `.minisig`
  files.
- `--dry-run` does everything through the tarball and leaves `dist/`, but
  does not require minisign and does not sign or publish.
- `--sign-only` signs and stops before the prompt.
- `--notes FILE` overrides a default one-line note; the default note is
  `Gimbal Files <version>`.

## Verification

### Gates

`scripts/check.sh` on the final tree exited 0 (`/tmp/gimbal-p2u-check.log`):

```
all checks passed
```

Test totals: **825 passed, 23 ignored** (23 + 16 + 12 + 367 + 6 + 11 + 390
passing; 1 + 6 + 11 + 5 ignored). This matches the brief's latest baseline.
**[observed]**

`bash -n` passes for `install.sh`, `release.sh` and `test-install.sh`.
**[observed]**

`shellcheck` is **not installed** on this machine (`command -v shellcheck`
empty), so `install.sh` and `release.sh` were not shellchecked. **[observed]**

### install.sh tests

`scripts/test-install.sh` passed. It installs from the checkout into a
`mktemp -d /tmp/gimbal-p2u-test.*` prefix, checks all five files and their
modes (755/755/644/644/644), checks the marker's three values and the
workspace version, checks that no `.*.tmp.*` file remains, damages the
installed binary and reinstalls to prove copy-then-rename restores it,
uninstalls, and confirms that only its own five files disappeared. Sentinel
files in the shared folders, plus a fake user config and session, all
survived. **[observed]**

Additional scratch checks passed:

- Tarball mode: the `--dry-run` tarball was unpacked and its `install.sh`
  installed into a `/tmp/gimbal-p2u-tarball.*` prefix; the marker read
  `0.1.0` and `--uninstall` removed exactly the five files. **[observed]**
- Default mode with a scratch `HOME` and `XDG_DATA_HOME`: binaries landed in
  `$HOME/.local/bin`, data in `XDG_DATA_HOME`, and uninstall removed only
  the five installed files. **[observed]**
- Relative `--prefix relative-prefix` resolved against the current directory
  and uninstalled correctly. **[observed]**
- `--prefix` with no value and `--prefix --uninstall` are both refused.
  **[observed]**
- The PATH warning appeared for every scratch prefix, as intended.
  **[observed]**

### release.sh tests

A scratch clone (`/tmp/gimbal-p2u-release/work`) was made from the committed
tree, given a bare local `origin`, and the working changes were committed
there. No real repository history was changed.

- `scripts/release.sh --dry-run` exited 0, ran all five gates locally, built,
  staged the required layout, wrote the exact two-line `latest.toml`, and
  produced a tarball whose entries all start with
  `gimbal-files-0.1.0-x86_64-linux/`. **[observed]**
- Refusal paths, all exit 1 with the intended message: dirty tracked tree,
  untracked file, branch `side`, local tag `v0.1.0`, remote-only tag
  `v0.1.0`, and local `main` ahead of `origin/main`. **[observed]**
- `--dry-run --sign-only` is refused. **[observed]**
- Real `--sign-only` cannot run here because **minisign is not installed**
  (`command -v minisign` empty); the script's error says
  `sudo apt install minisign`. **[observed]**
- A **stub** `minisign` and `gh` in `/tmp/gimbal-p2u-fake/bin` were used only
  to check argument shape. `--sign-only` called minisign twice with:
  `-t gimbal-files 0.1.0 x86_64-linux -m ...tar.gz` and
  `-t gimbal-files 0.1.0 latest -m ...latest.toml`. The publish path called
  `gh release create v0.1.0 --target main --title v0.1.0 --notes-file ...
  <tarball> <latest.toml> <both .minisig files>`. These are **not**
  cryptographic verifications. **[observed]**

What could not be run: real signing, real signature verification, a real
GitHub upload, a real tag, and the real `~/.local` install (Kyle has not said
to do that). No `sudo` was used and nothing was downloaded.

## P2v-facing formats

- Tarball: `gimbal-files-<version>-x86_64-linux.tar.gz`, one top-level
  folder `gimbal-files-<version>-x86_64-linux/`, containing the layout in
  item 6 and a copy of `latest.toml`.
- `latest.toml`:
  ```
  version = "<version>"
  tarball = "gimbal-files-<version>-x86_64-linux.tar.gz"
  ```
- Trusted comments: `gimbal-files <version> x86_64-linux` and
  `gimbal-files <version> latest`.
- Marker: `<data dir>/gimbal-files/install.toml` with `bin_dir`, `prefix`
  and `version`, as TOML strings.
- `install.sh --prefix <prefix from marker>` works from the tarball and
  rewrites the marker in `<prefix>/share/gimbal-files/install.toml`.

One consequence of the brief's literal marker rule: a default install that
used a custom `XDG_DATA_HOME` records `prefix = ~/.local` but its data lives
outside `~/.local/share`. P2v's update runs `install.sh --prefix ~/.local`,
which writes data to `~/.local/share`. This matches the brief's definition,
but P2v should read the marker location it originally found if it wants to
support that case. **[inferred]**

## Kyle's one-time steps

1. Install minisign: `sudo apt install minisign`.
2. Make the key pair, with the secret outside the repo:

   ```sh
   minisign -G -p packaging/minisign.pub -s ~/.minisign/gimbal-files.key
   ```

3. Commit **only** `packaging/minisign.pub`. The secret key stays at
   `~/.minisign/gimbal-files.key`; `scripts/release.sh` honours
   `MINISIGN_KEY` if it lives elsewhere. No key was generated by this agent.
4. When ready, make `Kcanady21/gimbal-files` public. Until then, release
   downloads need a token and P2v's daily check fails quietly, as planned.

`scripts/release.sh` refuses to sign until `minisign` and the secret key
exist. Its `--dry-run` works without either.

## What Claude checks live

1. Run `scripts/install.sh` into a scratch prefix, then the real `~/.local`
   install once Kyle says so.
2. Open Noctalia's app list: the launcher shows with the icon, and opening it
   starts Gimbal.
3. Run the installed binary with Kyle's real config.
4. Run `--uninstall` and confirm `~/.config/gimbal-files` and
   `~/.local/state/gimbal-files` are untouched.
5. Have Kyle build a `--sign-only` release and verify both files with
   `minisign -V -p packaging/minisign.pub -m <file>`.

## Outside the Output list

- `.gitignore` gains `dist/`, required by item 6 and noted here.
- `scripts/test-install.sh` is required by the brief's Tests section even
  though the Output list names only `install.sh` and `release.sh`.
- No plan file or other outside file was changed. The unrelated uncommitted
  `docs/notes/P2s.md` change in the checkout was left alone.
- The placeholder icon needs Kyle's real artwork before release; the desktop
  entry and README already use the final name.

## Review (Claude, Nov 5, 2026)

**Passes with five small fixes.** The scripts match the brief, and the agent's test coverage
(refusal paths, stub signing, tarball mode) is thorough.

Fixes made by Claude:

- `install.sh`: the installed desktop entry now runs the binary by absolute path
  (`Exec="<bin_dir>/gimbal-files" %U`). A launcher no longer depends on the session's PATH holding
  `~/.local/bin`. Paths are quoted per the desktop-entry spec. `&` and `|` are escaped for sed. A
  path containing `"`, `` ` ``, `$`, `\` or a newline keeps the plain `gimbal-files`. [observed:
  `desktop-file-validate` passes on an install under `pre fix & (x)`]
- `install.sh`: a new `--data-dir DIR`, plus `data_dir` in the marker. This fixes the case the agent
  flagged, where an update to an install using a custom `XDG_DATA_HOME` would write a second marker
  under the prefix. P2v's brief now passes `--prefix` and `--data-dir` back from the marker.
- `release.sh`: `gh release create --target <the commit it checked>` instead of `main`, so a push
  during the multi-minute build can't change what gets tagged.
- `release.sh`: a relative `--notes FILE` resolves against the caller's folder, not the repo root.
- `release.sh`: the tarball records owner `0/0` (`--owner=0 --group=0 --numeric-owner`), not
  Kyle's user name. [observed with GNU tar]
- `test-install.sh`: the scratch prefix now has spaces, `&` and parentheses. It checks the Exec
  line, `data_dir` and desktop validation, and adds a `--data-dir` install/uninstall case.

Verification:

- `scripts/test-install.sh` passes. [observed]
- `scripts/check.sh` exit 0, 825 passed / 23 ignored. [observed]
- `bash -n` is clean for both edited scripts. `shellcheck` is still not installed. [observed]

Not run yet:

- the real `~/.local` install and the launcher in Noctalia (waiting on Kyle's go);
- real signing (minisign isn't installed, and there's no key yet).
