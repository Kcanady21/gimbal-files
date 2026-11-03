# Gimbal Files: rules for agents

Gimbal Files is an open-source Linux file manager in Rust on GTK4 + libadwaita, for Wayland. Kyle
owns it and approves every change. Agents write most of the code from briefs. Claude reviews each
return, runs the live checks and commits. The plan is `Rust File Manager Plan for Claude Code.md`
in this folder. Read its Phase 2 section and the brief you were given before writing code.

## Layout

```
Cargo.toml          the workspace (P2a onward)
crates/fm-core      commands, workspace model, config, file operations. No GTK.
crates/fm-ui        the GTK4 + libadwaita app
crates/fm-agent     the local agent socket
crates/fm-cli       the command-line front end
spike/              Phase 1's prototype. Frozen reference: read it, never edit it.
docs/research/      Phase 0 and Phase 1 reports
docs/notes/         one notes file per brief (Phase 2 onward)
tools/              test helpers (the Phase 0 drag runner)
```

The spike is the reference for the look: `spike/src/bin/look-header/`, `spike/src/bin/look-body/`,
`spike/src/look_shared/` and `spike/src/theme.rs`. The look's written rules are in
`../gimbal-files-prompts/phase1/look-style-spec.md`, and its measured values are in
`sketch-values.md` next to it.

## Paths and toolchain

```
PROJ="/home/kyle/Documents/03.) Code-Projects/gimbal-files"
T="$HOME/.rustup/toolchains/1.95.0-x86_64-unknown-linux-gnu/bin"
```

Quote every path: the folder names contain a space and parentheses. The distro `cargo` 1.93.1 has
no `fmt` or `clippy`, so run every cargo command with `PATH="$T:$PATH"`.

## Done means (every change)

From `$PROJ`, all of these pass:

```
PATH="$T:$PATH" cargo fmt --all --check
PATH="$T:$PATH" cargo clippy --workspace --all-targets -- -D warnings
PATH="$T:$PATH" cargo test --workspace
PATH="$T:$PATH" cargo build --workspace --release
PATH="$T:$PATH" cargo deny check
```

`scripts/check.sh` (from P2a) runs all five in order and stops at the first failure.

## What you may run

- The commands above. Cargo may download the crates pinned in `Cargo.lock` from crates.io. Nothing
  else may be downloaded.
- The app, **only** on folders under `/tmp/gimbal-*` that you created. Run it for at most 10 seconds
  at a time, and stop it by its PID with SIGTERM. Never leave it running.
- Whenever you run the app, set `XDG_CONFIG_HOME` and `XDG_STATE_HOME` to folders under
  `/tmp/gimbal-*`, so it never reads or writes Kyle's real `~/.config/gimbal-files` or
  `~/.local/state/gimbal-files`. If Kyle's instance is already running (`pgrep -a gimbal-files`),
  a plain launch hands its arguments to his window (GApplication) and opens tabs there. Don't
  launch then: write in your notes what still needs a run. Never use `dbus-run-session` to get
  around it: each private session leaves about 15 services running (accessibility bus, account
  and volume monitors, portals), and nine such launches used up Kyle's inotify limit, which broke
  file watching for every app on his desktop.
- Read-only commands: `niri msg --json …` reads, `pkg-config`, `ls`, `grep`, and reading files
  anywhere.

## What you must never do

- Only ChatGPT (the primary reviewer) and Claude may run git commands. They may
  run read-only git commands for reviews. Git commands that change files, staging,
  commits, branches or remotes require Kyle's explicit approval; push only when Kyle
  says “push”. Other coding agents must run no git commands, including read-only ones.
  No `sudo` for any agent.
- No pointer or keyboard injection of any kind: no `/dev/uinput`, `wtype`, `ydotool` or `xdotool`.
  Claude does every live interaction.
- No `niri msg action …`, no changes to `~/.config` (Noctalia, GTK, niri), and no `gsettings set`.
- **Never delete, move or trash a user file.** Tests use scratch folders under `/tmp/gimbal-*`
  that they create and remove themselves. In the app, deletes go to the trash by default.
- **Trash tests are the one exception to `/tmp`.** GIO refuses to trash anything on `/tmp`
  ("Trashing on system internal mounts is not supported"). A trash test works under
  `~/.cache/gimbal-tests/<test>-*`, which it creates and removes. It runs in a child process with
  `XDG_DATA_HOME` pointed inside that folder, so the trash it uses is the scratch folder's, never
  Kyle's real `~/.local/share/Trash`.
- Don't edit a file your brief doesn't list under Output, and never edit `spike/`. If something
  outside your files needs a change, say so in your notes.

## How to write the code

- **Verify, don't recall.** GTK4, libadwaita, GIO and Wayland APIs change. Check every name against
  the pinned crate source in `~/.cargo/registry/src/*/<crate>-<version>/`, or docs.rs at that exact
  version. When you use something non-obvious, cite `crate-version:path:line` in your notes. GTK's
  own C source is in `../gimbal-refs/gtk-4.22.4/`, and libadwaita's in
  `../gimbal-refs/libadwaita-1.9.1/`.
- **Small changes.** One feature or fix per brief, with a summary Kyle can review in minutes.
- **No new dependency without a reason.** Write the reason in your notes. Pin it in the workspace's
  `[workspace.dependencies]` with `=x.y.z`, and keep its license GPL-3.0-compatible
  (`cargo deny check` enforces this).
- **`fm-core` has no GTK.** It may use GIO for file I/O, trash, mounts and watching, but never
  gtk4, gdk4 or libadwaita (`deny.toml` enforces this). Only `fm-ui` uses GTK.
- **Every action is a named command.** Menus, hotkeys, the agent socket and the CLI only ever call
  commands from `fm-core`'s registry, so agents can do only what Kyle has reviewed.
- **License header.** Every source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`
  (`#` for shell, TOML and YAML).
- **Attribution.** Ideas from Thunar, Dolphin or Nautilus are fine. Adapted code keeps its origin
  and license in a comment.
- **No `unsafe`** in `fm-core`, `fm-agent` or `fm-cli` (`#![forbid(unsafe_code)]`). In `fm-ui`,
  each `unsafe` block needs a `// SAFETY:` comment and a line in your notes.
- **Tests.** Unit tests for everything in `fm-core`. Put UI logic (layout arithmetic, hit tests,
  state transitions) in pure functions with tests as well. Kyle checks the rest by hand.
- **Logging.** When `GIMBAL_LOG=1` is set, print one line per event to stderr:
  `[gimbal <secs.millis since start>] <module>: <message>`.

## Drag and drop

- An outgoing drag offers a `gdk::FileList` through `gdk::ContentProvider::for_value`, with COPY |
  MOVE. Add no MIME types by hand: GTK adds `text/uri-list` and the portal types.
- **An outgoing drag never deletes or moves its source.** Moves are done by the drop side. Ignore
  `delete_data` on `drag-end`, because Firefox reports MOVE for every drop
  (`docs/research/drag-and-drop.md`).

## The look

- **No fixed sizes, colours, fonts or durations in widgets.**
  - Sizes come from the design tokens times the scale setting.
  - Colours come from the live theme.
  - The font family comes from the desktop unless the config overrides it.
  - Animation durations go through the shared motion setting (`ms(base)`).
- The sketch's greys and its Source Sans 3 are stand-ins, never values to copy.
- One shared look module holds the tokens, the generated stylesheet, the theme watcher, the opacity
  and blur, and the motion setting. Nothing else builds CSS.
- Symbolic icons from the mockup are drawn as `gtk::Svg` paintables on `gtk::Image`. Loading them
  by name through the icon theme ignores their colour slots and paints everything in the text
  colour.

## GTK lessons from Phase 1

Each of these cost a bug or a review round.

- Never unparent a widget during a drag: that cancels its gestures. Raise it with `insert_after`.
- A drag inside the window handle claims its sequence once past the drag threshold, not on press.
- Read pointer positions in surface coordinates (`gdk::Event::position`, minus the surface
  transform and the widget's bounds). Widget-relative gesture offsets are off by GTK's 25 px
  client-side shadow.
- A resize queued from inside `size_allocate` is ignored (`gtkwidget.c:3709`). Apply caps, such as
  a grid's max-columns from its width, from an idle.
- Never feed a measured or allocated size back into a minimum. A `ScrolledWindow` with policy
  `Never` passes its child's minimum up; `External` does not.
- A shrinkable `GtkPaned` child is allocated below its minimum and squeezed. To clip instead, wrap
  it in an `External` scroller.
- A `GtkRevealer`'s `hexpand` propagates even while it's collapsed.
- A press of a second mouse button cancels a single-button gesture
  (`gtkgesturesingle.c:206`). A cancelled drag must restore everything it previewed.
- Something drawn as a separate widget, like a pill above its tile, moves in the same animation
  frame as the thing it belongs to.
- Never hold a `RefCell` borrow while calling code that can re-enter it. The temporary in an
  `if let` scrutinee lives through the whole block.
- Test look changes in a floating window as well as tiled. niri's tiling forces the size and hides
  minimum-size bugs.
- Never turn accessibility off to gain speed.

## Notes file

Each brief writes `docs/notes/<brief>.md` with:
- what you built;
- what you verified and how (the commands and their output);
- the API citations;
- what needs a live session (Claude checks those);
- anything you'd change outside your files.

Tag every claim **[read]**, **[observed]** or **[inferred]**.

## Stop and ask

Stop and write the question in your notes before anything irreversible, or when what you find
changes the plan.
