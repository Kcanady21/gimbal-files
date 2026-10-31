# Rust File Manager: Plan for Claude Code

Oct 30, 2026 · @Kyle Canady

## Context and goal

Build an open-source Linux file manager in Rust that is beautiful, follows the desktop's theme automatically, and works well with AI agents. Agents (Claude Code) write most of the code; Kyle reviews and approves.

- **License:** GPL-3.0-or-later. Reading and adapting ideas from GPL-2.0-or-later projects (Thunar, Dolphin) is fine; copied code keeps its attribution.
- **Platform:** Linux, Wayland only. Shell is Noctalia v5, which generates app color themes from templates.
- **Reference behavior:** Dolphin and Nautilus drag files into Zen browser correctly from any folder. Thunar and COSMIC Files work only from `~/Downloads`, the one folder Zen's Flatpak sandbox can see, because they don't use the document portal ([Phase 0 report](docs/research/drag-and-drop.md)).
- **Working style:** small, reviewable changes. Each phase ends with something Kyle can run and judge.

## Decisions so far

Cross-app drag and drop on Wayland is the deciding requirement. It is solved in GTK and Qt, and weak in the Rust-native toolkits.

| Option | Status | Why |
| --- | --- | --- |
| Tauri / Dioxus (WebKitGTK) | Ruled out | WebKitGTK performance and NVIDIA/Wayland problems; maintainers warn against it when Linux matters ([wry #1064](https://github.com/tauri-apps/wry/issues/1064)). Tauri 3 CEF runtime is alpha and adds hundreds of MB. |
| Fork COSMIC Files (iced/libcosmic) | Deprioritized | Dragging out to other apps is broken and reported since 2024 ([cosmic-files #830](https://github.com/pop-os/cosmic-files/issues/830), [cosmic-epoch #3926](https://github.com/pop-os/cosmic-epoch/issues/3926)). Confirmed on Kyle's machine. Phase 0 found the cause: `src/clipboard.rs` never offers the two portal MIME types or calls the FileTransfer portal. That's a small fix, and the open draft [cosmic-files #1586](https://github.com/pop-os/cosmic-files/pull/1586) already does it. |
| New Rust app on GTK4 + libadwaita (gtk4-rs) | Lead candidate | GTK handles the Wayland drag protocol. `AdwTabView` supports tab transfer between windows with a `create-window` signal. Needs a spike to confirm. |
| New Rust app on iced or GPUI | Fallback | Upstream winit only recently merged Wayland drag support ([winit #4571](https://github.com/rust-windowing/winit/pull/4571)); drag-out would be our own protocol code. |
| Patch Dolphin (C++/Qt) | Least-work fallback | Already has tabs, split view, mounting, working drag and drop. Not Rust. |

Not pursued: a line-by-line port of Dolphin or Thunar. Their drag code is thin; the toolkit does the protocol work. We read their source for behavior, not to translate it.

## Phase 0: learn how working file managers do it

**Done (Oct 31, 2026).** Report: [docs/research/drag-and-drop.md](docs/research/drag-and-drop.md). Source notes and the capture matrix are in `docs/research/phase0/`. Two assumptions below turned out wrong: Nautilus 50 no longer offers `x-special/gnome-icon-list`, and Dolphin's `application/x-kde-cutselection` is clipboard-only.

Goal: a short written report on what a successful drag-out puts on the wire, and why COSMIC Files fails. No app code yet.

1. **Observe the protocol.** Run Zen with Wayland protocol logging, e.g. `WAYLAND_DEBUG=1 zen 2>&1 | grep -E 'wl_data_offer|wl_data_device'`, then drag one file from Dolphin, Thunar, and COSMIC Files into an empty Zen tab. Record the MIME types each source offers, the actions, and whether the drop completes. Confirm the exact Zen launch command and that it runs native Wayland (`about:support` → Window Protocol).
2. **Read Thunar's source.** Focus on `thunar/thunar-standard-view.c` and `thunar/thunar-dnd.c`: drag targets offered, how the URI list is built, Copy vs Move vs Ask handling. Known target: `text/uri-list`.
3. **Read Dolphin's source.** Focus on `KFileItemModel::createMimeData()` in `src/kitemviews/kfileitemmodel.cpp` and how `KUrlMimeData` populates the drag. Known extras: `application/x-kde-cutselection`.
4. **Read Nautilus for comparison.** `src/nautilus-dnd.h` names the targets it offers (`x-special/gnome-icon-list`, `text/uri-list`).
5. **Diagnose COSMIC Files.** Find where it starts a drag (search for `start_dnd`, `start_drag`, `AsMimeTypes`) and list the MIME types it advertises. Compare with step 1.
6. **Write `docs/research/drag-and-drop.md`:** a table of source app × offered MIME types × result in Zen, the minimal set we must offer, and a one-paragraph diagnosis of the COSMIC failure.

Output: the report, plus a yes/no on whether COSMIC's bug looks like a small fix (e.g. a missing MIME type) or a deep one. If small, make the patch on a branch of a local COSMIC Files clone so Phase 1 can test it alongside the GTK4 prototype. (Result: small, but the patch was skipped. See Phase 1.)

## Phase 1: toolkit spike

Goal: a throwaway GTK4 + libadwaita prototype in Rust (gtk4-rs, libadwaita-rs) that proves or kills the lead candidate. Keep it under about 1,000 lines. **No patched COSMIC Files build** (Kyle decided Oct 31). COSMIC's bug is a small fix, already written upstream ([cosmic-files #1586](https://github.com/pop-os/cosmic-files/pull/1586)) and confirmed by Phase 0's captures and source reading. A local build would only repeat that check, and nothing in this GTK4 spike depends on it. If the spike fails and we fall back to iced, the fix belongs in Gimbal's own drag source: offer the portal MIME types and call the FileTransfer portal, as #1586 does.

The prototype has: one window with an `AdwTabView` and tab bar, each tab a virtualized list of one directory, a `DropTarget` accepting `text/uri-list`, an "Open in new window" action, and a `DragSource` on rows built as Phase 0 found:

- `connect_prepare` returns `gdk::ContentProvider::for_value` of a `gdk::FileList` of the selected files, with actions COPY | MOVE. Don't add MIME types by hand: GTK4 adds `text/uri-list` and both portal types (`application/vnd.portal.files`, `application/vnd.portal.filetransfer`), and offers the portal types only when the portal was reachable at startup.
- Never delete or move anything on `drag-end`. Firefox reports MOVE for every Wayland drop, and GTK4 then sets `delete_data`. Moves are done by the drop side.

Acceptance tests, run by Kyle on his own session. Run the Zen rows with the Phase 0 drag runner (`tools/dnd-capture/run.sh --auto`, with a Gimbal row added) instead of by hand:

- [ ] Drag a file from the prototype into an empty Zen tab; Zen opens it. Do this from `~/Documents`, not only from `~/Downloads`.
- [ ] After each drop into Zen, the source file still exists
- [ ] Drag several files and a folder into Zen
- [ ] Drag a symlink into Zen, and record what happens (Phase 0 left this as a gap)
- [ ] Drag a file into Dolphin and into Thunar; it copies
- [ ] Drag a file from Dolphin into the prototype; it lands
- [ ] Drag between two separate prototype instances
- [ ] "Open in new window" opens the folder in a new window
- [ ] Drag a tab out of the tab bar and drop it on the desktop; a new window opens with that tab
- [ ] Drag a tab from one window into another window's tab bar
- [ ] A folder with 100,000 files opens and scrolls smoothly (generate it in /tmp)
- [ ] Colors follow Noctalia: apply Noctalia's GTK template, change the wallpaper or scheme, confirm the app updates (note whether a restart is needed)

Record results in `docs/research/spike-results.md`. Tab tear-off may fail on native Wayland in any toolkit because the app cannot see where the pointer is released; if it fails, note it and fall back to the menu action.

## Phase 2: architecture

Start after the spike picks a toolkit. The core is UI-agnostic, so a toolkit change later costs only the UI crate.

&#91;embedded content: architecture · three front doors, one core\]

A Cargo workspace with `fm-core`, `fm-ui`, `fm-agent` and `fm-cli`. Hotkeys, menus, the agent socket and the CLI all call the same named commands, so agents can only do what Kyle has reviewed. Build the registry and config first, with unit tests, before any UI beyond the spike.

## Feature list

Kyle's requested features, with where each lives and any known approach. Build order: rows marked Spike first, then top to bottom.

| Feature | Where | Notes |
| --- | --- | --- |
| Drag files to other apps | UI | Spike. MIME types from Phase 0. |
| Drag between tabs and windows | UI | Spike. Same drag source and drop target. |
| Multiple windows, open in new window | UI | Spike. |
| Tabs, pull a tab out | UI | Spike. `AdwTabView` tab transfer; menu fallback. |
| Split tabs | UI | Two panes in one tab, each with its own path. |
| Autotheme to Noctalia | System | Use Noctalia's GTK template; check v5 App Theming docs for the file it writes. |
| Disk usage bar under each drive | System | Free and used space per mount; refresh on mount change. |
| Mount a drive now (right-click) | System | UDisks2 `Filesystem.Mount` over D-Bus (zbus or the `udisks2` crate); polkit may prompt for system disks. No fstab editing. |
| Path bar: paste a path to go there | UI + core | Typing `/home` navigates; a file path opens its folder and selects the file. |
| Back, forward, up, refresh | Core | History per tab. Refresh mostly unneeded with file watching. |
| Right-click menu | UI | Built from the command registry. |
| Disk Usage Analyzer | System | Launch `baobab <current dir>` if installed. |
| File search | Core | Current folder and subfolders first; indexing later if needed. |
| Text preview in right sidebar | UI | Click shows a read-only preview. |
| Edit in preview | UI + core | Alt+click on the preview makes it editable. Atomic save; warn if the file changed on disk; read-only for files the user can't write. |
| Per-folder sort overrides | Core | One central TOML: a global default plus listed folder exceptions. |
| Configurable hotkeys | Core | Hotkeys map to command names in the config. |
| Works well with agents | Agent | Query state, reveal paths, set sort, open tabs; trash by default, dry-run, undo. |

## Rules for agents

Put these in the repo's `CLAUDE.md`.

- **Verify, don't recall.** GTK4, libadwaita and Wayland APIs change. Check current docs (gtk-rs.org, docs.rs, GNOME developer docs) and pin crate versions; note the version you read.
- **Small changes.** One feature or fix per change, with a short description Kyle can review in minutes.
- **Never delete user files during testing.** Use a scratch directory under `/tmp`. Deletes go to trash by default.
- **An outgoing drag never deletes or moves its source.** Moves are done by the drop side. Ignore `delete_data` on `drag-end`, because Firefox reports MOVE for every drop (see the Phase 0 report).
- **No new dependency without a reason.** Run `cargo-deny` in CI; reject licenses incompatible with GPL-3.0.
- **License headers.** SPDX `GPL-3.0-or-later` in every source file.
- **Attribution.** Ideas from Thunar, Dolphin or Nautilus are fine. Adapted code keeps its origin and license noted in a comment.
- **Tests.** Unit tests for everything in `fm-core`. UI behavior is checked by Kyle against the acceptance tests.
- **Stop and ask** before anything irreversible, or when a phase's result changes the plan.

## Open questions for Kyle

- [ ] Which compositor will this run under day to day (KWin, Niri, Hyprland, something else)? Tab tear-off and drag behavior can differ.
  - [ ] Answer: Niri for now
- [ ] Is GNOME's libadwaita look acceptable as a starting point, restyled with Noctalia colors and CSS, or does "beautiful" need a fully custom look?
  - [ ] Answer: Fully custom is preferred
- [ ] Should the app also ship as a Flatpak? Sandboxing changes how drag and drop and file access work.
  - [ ] Answer: not right now
- [ ] Project name and repository location.
  - [ ] Answer: Name = Gimbal Files
  - [ ] location /home/kyle/Documents/03.) Code-Projects/gimbal-files/
  - [ ] create a github repo for it

## Sources

- [wry #1064: WebKitGTK and Linux](https://github.com/tauri-apps/wry/issues/1064)
- [cosmic-files #830: drag out to other apps](https://github.com/pop-os/cosmic-files/issues/830)
- [cosmic-epoch #3926: COSMIC Files to browser fails](https://github.com/pop-os/cosmic-epoch/issues/3926)
- [winit #4571: new drag and drop API](https://github.com/rust-windowing/winit/pull/4571)
- [libadwaita-rs TabView](https://world.pages.gitlab.gnome.org/Rust/libadwaita-rs/git/docs/libadwaita/struct.TabView.html)
- [gdk4-rs: ContentProvider and FileList](https://gtk-rs.org/gtk4-rs/stable/latest/docs/gdk4)
- [Thunar drag targets in thunar-standard-view.c](https://gitlab.xfce.org/prertik/thunar/-/commit/dfd6faa57224a9c3a697e9781aa4bd7525d9832b.patch)
- [Nautilus DnD targets](https://git.stg.centos.org/source-git/nautilus/blob/07a0047bec3dc76fe0a8fb9230b05c46cca25d1a/f/src/nautilus-dnd.h)
- [UDisks2 polkit actions](https://storaged.org/udisks/docs/udisks-polkit-actions.html)
- [Noctalia GTK and Qt theming](https://docs.noctalia.dev/noctalia/templates/official/gtk-qt/)
