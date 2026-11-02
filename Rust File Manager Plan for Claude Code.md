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
| New Rust app on GTK4 + libadwaita (gtk4-rs) | Lead candidate | GTK handles the Wayland drag protocol and gives virtualized grids, animations, accessibility and text input. The look is fully custom (see Interface design), so libadwaita is a parts box (animations, dialogs, style manager), not the look: no `AdwTabBar`, no stock header bar. Needs a spike to confirm, including whether GTK can draw Kyle's design. |
| New Rust app on iced or GPUI | Fallback | Upstream winit only recently merged Wayland drag support ([winit #4571](https://github.com/rust-windowing/winit/pull/4571)); drag-out would be our own protocol code. |
| Patch Dolphin (C++/Qt) | Least-work fallback | Already has tabs, split view, mounting, working drag and drop. Not Rust. |

Not pursued: a line-by-line port of Dolphin or Thunar. Their drag code is thin; the toolkit does the protocol work. We read their source for behavior, not to translate it.

## Interface design

Kyle drew the interface in VectorCraft (Oct 31): `~/Documents/03.) Code-Projects/Gimbal UI Sketch/Gimbal-Files.vectorcraft`, with three states on separate layers (Default, UI Split Tab, Default With Search selected). Measured values, design tokens and every decision below, in full, are in `gimbal-files-prompts/phase1/sketch-values.md` next to the repo.

- **Layout.** A header holds the menu, back, forward, up, search, tab pills, the path bar, then trash, minimize, float and close. Below it: a left sidebar (Pinned Folders, then collapsible Recent Directories, Devices with drive bars, and Remote, with the main drive at the bottom), an icon grid in the middle with a status line, and a right preview sidebar (file name and size, text with line numbers, or an image, PDF or video preview; Discard, Save and Save As).
- **Size.** Drawn at 1:1 for a 2560×1440 screen at scale 1. A setting scales the whole UI from 50% to 100% in 5% steps, live. What "100%" means is a base size in the config file.
- **Font.** The family follows the desktop's UI font, as terminals do; a config setting overrides it. The sketch's type sizes apply, times the scale.
- **Transparency.** Like Ghostty: one background-opacity setting (default 0.85; 1.0 turns it off) makes the whole window see-through, and the app asks the compositor to blur what's behind it through the `ext-background-effect-v1` Wayland protocol (niri 26.04 supports it with no niri config). Text, icons and outlines stay solid; menus and dialogs stay opaque (Kyle, Nov 1).
- **Colour.** Every colour comes from the live desktop theme (Noctalia, which changes with the wallpaper). The sketch's greys are stand-ins that only show which parts share a colour and which are lighter. Outlines use the theme's accent colour. The active tab is the lighter pill.
- **Tabs.** Pills with a close button. When they don't fit, they collapse into one accordion pill showing the tab count; hovering lists the tabs, and clicking one focuses it. Pills reorder by dragging. Ctrl-click toggles a pill, Shift-click selects a range.
- **Tab groups are the split view.** Select several tabs, right-click, Split view: their pills fade out of the header and fade in over a tiled view, 2 columns wide (2 tabs fill the full height; more tabs add rows, and the whole view scrolls down). Each tile scrolls on its own, has its own pill, and can be dragged to another position. Back, forward, the path bar and the status line follow the focused tile, which gets a focus ring. Each group appears in the header as one pill ("Group N · K tabs", renameable); clicking it opens the split view. Closing tabs down to one returns to the normal view; the group pill can close the whole group after a confirmation; several selected tabs close at once from the right-click menu or a hotkey. Dragging a tab onto a group pill comes later ("Add to group" in the menu meanwhile).
- **Search.** Clicking the icon expands it into a field that pushes the header pills right.
- **Resizing.** The path bar stretches into free header space; as the window narrows it shrinks to a minimum, then the tabs collapse into the accordion. The icon grid uses as many columns as fit (11 at 2560 px, down to 1). Both sidebars resize by dragging their border and collapse only through their own collapse icon.
- **Left sidebar.** Pinned Folders grows and shrinks with the number of pins and pushes the sections below; the whole sidebar scrolls when it overflows. Drive bars rise in contrast with fullness up to 94% and turn the theme's error colour at 95% and above.
- **Window buttons.** Trash opens the Trash and also accepts dropped files. Float toggles niri's floating mode (niri-only for now; on another desktop it becomes Maximize). Minimize is not shown under niri, which has no minimize; other desktops get it.
- **Session.** Tabs and groups come back after a restart.
- **Not in V1:** dragging a tab out to a new window or into another window. Right-click → Open in New Window covers it.

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

The prototype has: one window with tabs (the P1b skeleton uses `AdwTabView` and its tab bar as scaffolding), each tab a virtualized view of one directory, a `DropTarget` accepting `text/uri-list`, an "Open in new window" action, and a `DragSource` on items built as Phase 0 found:

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
- [ ] A folder with 100,000 files opens and scrolls smoothly in the icon grid (generate it in /tmp)
- [ ] Colors follow Noctalia: apply Noctalia's GTK template, change the wallpaper or scheme, confirm the app updates (note whether a restart is needed)
- [ ] Header look (P1i): pill tabs with the accordion overflow and hover menu, group pills and the expanding search match the sketch
- [ ] Body look (P1j): the tiled split view (fade in, per-tile and outer scrolling, focus ring), the grid dropping columns as the window narrows, draggable and collapsible sidebars, and collapsible section pills match the sketch
- [ ] Theme and scale: a stylesheet built from the design tokens takes every colour from the live theme, and the scale setting resizes the whole UI live
- [ ] Transparency (P1k): the background-opacity setting makes both look programs see-through, and the compositor blurs what's behind them

Record results in `docs/research/spike-results.md`. Dragging a tab out to a new window, or into another window, was dropped from V1 (Kyle, Oct 31), so the spike no longer tests it. P1d, which was written for that, is shelved.

P1i and P1j are separate small programs inside the spike (`spike/src/bin/`), so they don't touch the modules P1c–P1g own. They answer the second half of the toolkit question: can GTK4 draw Kyle's design without fighting libadwaita's stylesheet?

## Phase 2: architecture

**Toolkit: GTK4 + libadwaita.** The P1h acceptance run passed the deal-breakers (drag-out to Zen from any folder, 8 of 8 rows; the 100,000-file folder), and the look rows failed only on polish. Kyle approved the verdict Nov 1. Results: `docs/research/spike-results.md`. The look-polish rounds P1l (header) and P1m-r1 to r9 (body) finished Phase 1's look. Rows 7–9 passed on rerun, and **Phase 1 is done (Nov 2, 2026)**. Phase 2 builds on `spike/src/bin/look-*`, `look_shared/` and the style spec.

Start after the spike picks a toolkit. The core is UI-agnostic, so a toolkit change later costs only the UI crate.

A Cargo workspace with `fm-core`, `fm-ui`, `fm-agent` and `fm-cli`: three front doors (UI, agent socket, CLI) over one core. Hotkeys, menus, the agent socket and the CLI all call the same named commands, so agents can only do what Kyle has reviewed. Build the registry and config first, with unit tests, before any UI beyond the spike.

- **`fm-core`** (no GTK; GIO for file I/O, trash, mounts and file watching):
  - **Command registry.** Every action is a named command with arguments: open, trash, new tab, split view, close group, close selected tabs, toggle sidebar, set scale, and so on. Menus, hotkeys, the agent socket and the CLI only ever call commands.
  - **Workspace model**, plain data the UI observes: windows hold tab groups, and groups hold tabs. A group with one tab shows as a normal tab; two or more is the split view. Each tab has its path, back/forward history, selection and scroll position. Each window has one focused tab. The model is saved, so the session comes back after a restart, and agents read the same state Kyle sees.
  - **Config (TOML):** hotkeys, per-folder sort overrides, pinned folders, collapsed sections and sidebars, the scale setting, the base size that defines 100%, and the background opacity.
  - **File operations** with trash by default, dry-run and undo.
- **`fm-ui`** (GTK4, libadwaita as parts):
  - **Header:** our own tab strip (tab pills, group pills, the accordion pill and its hover menu, drag to reorder), the expanding search pill, the stretching path bar, and window buttons. The Trash button is a drop target.
  - **Workspace view:** one directory view, or the tiled split view (2 columns in an outer scroller, each tile with its own pill, scroller and focus ring, tiles rearranged by dragging). Fades and the search expansion use libadwaita's animations.
  - **Directory view:** GTK's virtualized grid with fixed-size cells, so the column count follows the width.
  - **Sidebars:** in draggable splits with collapse buttons. The left one scrolls as a whole: pinned list, collapsible section pills, drive bars. The right one holds the preview (GtkSourceView for text later).
  - **One shared look module** for the whole UI: the design tokens, the generated stylesheet, the live theme watcher, the opacity setting and the blur request. In the spike each look program copied its own `style.rs` and only the spike had the theme watcher; the app has one of each (P1h).
  - **Style engine:** the stylesheet is generated from the design-token table times the scale. Every colour is a theme colour (accent for outlines, sidebar colours for the pinned list, mixes of the window colours for active and inactive pills and the drive-bar ramp, the error colour for full drives). It is regenerated live when the theme or the scale changes. Widgets never hard-code a size or a colour.
- **`fm-agent`** (a local socket) and **`fm-cli`** (a command-line front end): both take command names and arguments and query the workspace model.
- **Platform adapters**, each behind a small interface so a non-niri, non-Noctalia desktop only swaps the adapter: niri IPC for the Float button (Maximize elsewhere; the adapter also says whether Minimize exists), UDisks2 for mounting and drive usage, the theme watcher (whatever P1f finds), and the FileTransfer portal for drags.

## Feature list

Kyle's requested features, with where each lives and any known approach. Build order: rows marked Spike first, then top to bottom.

| Feature | Where | Notes |
| --- | --- | --- |
| Drag files to other apps | UI | Spike. MIME types from Phase 0. |
| Drag files between tabs, tiles and windows | UI | Spike. Same drag source and drop target. |
| Multiple windows, open in new window | UI | Spike. Right-click a tab → Open in New Window. |
| Custom look from the sketch | UI | Spike (P1i header, P1j body). See Interface design. |
| Tabs: pills, overflow accordion, drag to reorder | UI | Spike (P1i). Our own tab strip, not `AdwTabBar`. |
| Tab groups as a tiled split view | UI + core | Spike (P1j). Replaces the old "two panes in one tab" idea. Tiles rearrange by dragging; drag a tab onto a group pill later. |
| Pull a tab out to a new window | UI | After V1 (Kyle, Oct 31). Open in New Window meanwhile. |
| Autotheme to Noctalia | System | Spike (P1f). Every colour from the live theme; outlines use the accent colour. |
| UI scale setting | UI + core | Spike (P1j). 50–100% in 5% steps, live; the base size for 100% is in the config. |
| Transparent, blurred window | UI | Spike (P1k). Background-opacity setting (default 0.85) and blur requested through `ext-background-effect-v1`, as Ghostty does. niri paints its focus ring as a solid block behind every window that draws its own decorations, which GTK 4 windows always do, so the see-through background shows that block unless niri has `draw-border-with-background false` for Gimbal (Kyle's config has it). Phase 2: look for a way to work on any niri setup without the rule. Telling niri through `xdg-decoration` that the window draws no decorations needs the window's `xdg_toplevel`, which GDK does not expose (`gdk4-wayland` 0.11.5), so it may not be possible from inside GTK. Fallback: document the one-line rule, and for GimbalOS ship it in its niri config (an installer matter). |
| Collapsible, resizable sidebars and sections | UI | Spike (P1j). |
| Restore tabs and groups after restart | Core | Saved workspace model. |
| Delete to Trash with the Delete key and the right-click menu | UI + core | P1h row 5: the spike had no way to delete. Trash by default, through the command registry. |
| Reorder pinned folders by dragging | UI + core | P1m. Same follow-and-slide feel as the tab pills; the order is saved in the config. |
| Pinned folders: click selects, double-click opens | UI | P1m-r3. The highlighted row is the current folder; only a drag reorders. |
| Split view: resize tiles and arrange columns by hand | UI + core | P1m-r3. Drag the border between columns or between stacked tiles; double-click resets. Drop a tile between tiles to move it (on a tile to swap). The default stays balanced (2 + 2, 2 + 1); a close rebalances only when it leaves the columns two apart. The layout is part of the saved workspace. |
| Close tabs and groups from the accordion list | UI | P1l. A ✕ per entry; groups ask first, as the group pill does. |
| Gimbal's own icon set from the mockup | UI | P1m. Folder, Save, Save As and Discard from Kyle's sketch, drawn as GTK symbolic icons so every part takes a theme colour (GTK 4.20+). |
| Trash button: open Trash, drop to trash | UI + core | Drop target moves to trash, never deletes. |
| Float and Minimize buttons | System | Float uses niri IPC and becomes Maximize on other desktops. Minimize is hidden under niri (no minimize there). |
| Disk usage bar under each drive | System | Free and used space per mount; refresh on mount change. Contrast rises with fullness; error colour at 95% and above. |
| Mount a drive now (right-click) | System | UDisks2 `Filesystem.Mount` over D-Bus (zbus or the `udisks2` crate); polkit may prompt for system disks. No fstab editing. |
| Path bar: paste a path to go there | UI + core | Typing `/home` navigates; a file path opens its folder and selects the file. |
| Back, forward, up, refresh | Core | History per tab. Refresh mostly unneeded with file watching. |
| Right-click menu | UI | Built from the command registry. |
| Disk Usage Analyzer | System | Launch `baobab <current dir>` if installed. |
| File search | Core | Current folder and subfolders first; indexing later if needed. |
| Text preview in right sidebar | UI | Click shows a read-only preview with line numbers; images, PDFs and videos get a thumbnail preview. |
| Edit in preview | UI + core | Alt+click on the preview makes it editable; Discard, Save and Save As buttons. Atomic save; warn if the file changed on disk; read-only for files the user can't write. |
| Per-folder sort overrides | Core | One central TOML: a global default plus listed folder exceptions. |
| Configurable hotkeys | Core | Hotkeys map to command names in the config. |
| Works well with agents | Agent | Query state, reveal paths, set sort, open tabs; trash by default, dry-run, undo. |

## Rules for agents

Put these in the repo's `CLAUDE.md`.

- **Verify, don't recall.** GTK4, libadwaita and Wayland APIs change. Check current docs (gtk-rs.org, docs.rs, GNOME developer docs) and pin crate versions; note the version you read.
- **Small changes.** One feature or fix per change, with a short description Kyle can review in minutes.
- **Never delete user files during testing.** Use a scratch directory under `/tmp`. Deletes go to trash by default.
- **An outgoing drag never deletes or moves its source.** Moves are done by the drop side. Ignore `delete_data` on `drag-end`, because Firefox reports MOVE for every drop (see the Phase 0 report).
- **No fixed sizes, colours or fonts in widgets.** Sizes come from the design tokens times the scale setting; colours come from the theme; the font family comes from the desktop unless the config overrides it. The sketch's greys and its Source Sans 3 are stand-ins, never values to copy.
- **No new dependency without a reason.** Run `cargo-deny` in CI; reject licenses incompatible with GPL-3.0.
- **License headers.** SPDX `GPL-3.0-or-later` in every source file.
- **Attribution.** Ideas from Thunar, Dolphin or Nautilus are fine. Adapted code keeps its origin and license noted in a comment.
- **GTK lessons from Phase 1** (each cost a bug or a round):
  - Cap a list or grid view's max-columns from its width, and apply the cap from an idle: a resize queued from inside `size_allocate` is ignored. Never turn accessibility off to win speed.
  - Never unparent a widget during a drag: that cancels its gestures. Raise it with `insert_after`.
  - A drag inside the window handle claims its sequence once past the drag threshold, not on press.
  - Never feed a measured or allocated width back into a minimum size; a `ScrolledWindow` with policy `Never` passes its child's minimum up to the window.
  - Never hold a `RefCell` borrow while calling code that can re-enter it. The temporary in an `if let` scrutinee lives through the whole block.
  - Test look changes in a floating window: niri's tiling forces the size and hides minimum-size bugs.
- **Tests.** Unit tests for everything in `fm-core`. UI behavior is checked by Kyle against the acceptance tests.
- **Stop and ask** before anything irreversible, or when a phase's result changes the plan.

## Open questions for Kyle

- [ ] Which compositor will this run under day to day (KWin, Niri, Hyprland, something else)? Tab tear-off and drag behavior can differ.
  - [ ] Answer: Niri for now
- [ ] Is GNOME's libadwaita look acceptable as a starting point, restyled with Noctalia colors and CSS, or does "beautiful" need a fully custom look?
  - [ ] Answer: Fully custom is preferred. Kyle drew it on Oct 31; see Interface design.
- [ ] Should the app also ship as a Flatpak? Sandboxing changes how drag and drop and file access work.
  - [ ] Answer: not right now
- [ ] Project name and repository location.
  - [ ] Answer: Name = Gimbal Files
  - [ ] location /home/kyle/Documents/03.) Code-Projects/gimbal-files/
  - [x] create a github repo for it: private, https://github.com/Kcanady21/gimbal-files (Oct 31)

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
