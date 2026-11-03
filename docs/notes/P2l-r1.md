# P2l-r1: drag feedback, negotiated copy/move and viewport drops

Tags: **[read]** for source or crate facts, **[observed]** for a command or a run,
**[inferred]** for anything that needs a real pointer, a real keyboard or a live
feel.

## What I built

All changes are in `crates/fm-ui/src/`: `dnd.rs`, `folder/view.rs` and `app.rs`.
`fm-core`, `Cargo.toml`, the runner and `spike/` were not changed.

- **A real drag icon and count badge** — `folder/view.rs::setup_cell_drag_source`,
  `dnd::drag_icon`.
  - `prepare` still takes the selection's first item's `gio::Icon`, makes the
    `FileList` through `gdk::ContentProvider::for_value`, and offers COPY | MOVE.
    It now builds the badge widget once and keeps it in an
    `Rc<RefCell<Option<gtk::Widget>>>`.
  - `drag-begin` gets GTK's own icon with `gtk::DragIcon::for_drag(drag)`, sets
    the widget as its child, and calls `drag.set_hotspot(hot, hot)` with the
    icon centre. There is no `WidgetPaintable` and no manual
    `measure`/`allocate` any more: `GtkDragIcon` measures and allocates its
    child itself (`gtkdragicon.c:265-300`).
  - `drag-end` and `drag-cancel` drop the local strong reference and call
    `set_child(None)`, so the widget does not outlive the drag. `drag-end`
    still ignores `delete_data` and never removes sources.
  - The badge is still `badge_text(count)` (nothing for 0 or 1), styled only by
    `.gl-drag-badge` from the shared look. It is inset by `GAP_S` times the
    scale so the drag surface cannot clip a two- or three-digit count.
- **A negotiated copy/move decision** — `dnd.rs`, `app.rs::drop_on_folder`.
  - The cause of the real-keyboard failure is Wayland, not the earlier virtual
    keyboard: when the DnD grab takes the keyboard, GDK receives
    `wl_keyboard.leave` and clears the seat's modifier state
    (`gdkseat-wayland.c:1452-1477`). `GdkDevice::modifier_state` then reports an
    empty mask (`gdkdevice-wayland.c:453-466`), which the old
    `target_modifiers` read as “no modifier”.
  - The decision now starts from the offered mask and the compositor's one
    selected action. The target-side `GdkDrop::actions` is preferred when the
    compositor has collapsed it to one action; the local
    `GdkDrag::selected_action` is the fallback. Both come from the compositor's
    `action` event; an external FileList has no `GdkDrag`, so the target side is
    its only view.
  - The target asks the compositor to prefer the **filesystem default** on each
    motion. A selected action that differs from that known preference is the
    compositor's Ctrl/Shift override; a selected action that equals it is an
    echo of our own preference and is treated as “no explicit choice”. A change
    of preference clears the old answer before the new one is requested. This
    is the important distinction: `COPY | MOVE` is only the source's offer,
    and a nonempty selected action is not automatically a held key.
  - A positive modifier state is still used when the platform exposes one
    (Ctrl wins over Shift), but an empty state is `unknown` on Wayland, never
    “none”. The hover and drop logs print `unknown` for it.
  - The pure rule keeps the defaults: same filesystem moves, different
    filesystems copy. Ctrl forces copy, Shift forces move, Ctrl wins. The
    result is always an action the source advertised; a copy-only source can
    never become an unadvertised move. Neither action offered refuses safely.
  - `DropIntent::capture` records the offered mask, selected action, last known
    filesystem preference and modifier evidence synchronously at `drop`.
    `Process::drop_on_folder` then runs the asynchronous filesystem query and
    resolves from that captured intent. An explicit action survives a later
    filesystem result; it is never replaced by the default.
  - The worker is unchanged: conflicts, progress, Cancel and undo are still the
    P2k worker. Trash and pin targets keep their own semantics and ignore
    copy/move.
- **The whole folder viewport is a background drop target** —
  `folder/view.rs::attach_background_drop_target`.
  - The target moved from `self.grid` to the viewport-owning `self.root`
    overlay. That covers the padding around cells, the space below a short
    listing and an empty folder, in a normal tab and in every split tile.
  - `cell_rects` converts every realised grid child's border rect into root
    coordinates with `gtk::Widget::compute_bounds`, which includes the grid's
    scroll transform. The pure `dnd::background_owns` decides whether the point
    is padding/blank or a cell. No widget from another tile, the header or the
    sidebar is ever in this test.
  - A cell keeps its own destination. Over any `gl-cell`, the background mask
    bows out, so a file cell's “only folders are drop targets” refusal cannot
    become a background transfer. Each successful drop submits exactly one
    job.
  - The root gets `.gl-drop-target` (the existing accent inset outline) on a
    valid background/padding hover and loses it on leave and on drop. Refusals
    for the same folder, a folder onto itself or into a descendant still use
    the existing status-line messages. Non-file content is still ignored.

## API citations

- `gtk4-0.11.5:src/auto/drag_icon.rs:40` `DragIcon::for_drag`;
  `:56` `set_child`.
- `gdk4-0.11.5:src/auto/drag.rs:117` `Drag::set_hotspot`; `:102`
  `selected_action`; `:61` `actions`.
- `gdk4-0.11.5:src/auto/drop.rs:32` `Drop::actions`; `:50` `Drop::drag`.
- `gtk4-0.11.5:src/auto/drop_target.rs:55` `current_drop`; `:82` `value`;
  `:103` `set_preload`; `:138` `connect_enter`; `:169` `connect_leave`;
  `:193` `connect_motion`; `src/drop_target.rs:52` `connect_drop`.
- `gtk4-0.11.5:src/auto/widget.rs:166` `compute_bounds`; `:1054` `pick`.
- `gtk-4.22.4:gtk/gtkdragicon.c:265-300` measure and allocate the child;
  `:404-430` `gtk_drag_icon_get_for_drag`; `:473-500` `set_child`.
- `gtk-4.22.4:gtk/gtkdroptarget.c:227-251` the drop finishes with
  `gdk_drop_get_actions`; `:365-374` default enter; `:394-430` motion and
  `gtk_drop_status`.
- `gtk-4.22.4:gdk/wayland/gdkseat-wayland.c:1452-1477`
  `keyboard_handle_leave` clears `seat->key_modifiers`;
  `gdk/wayland/gdkdevice-wayland.c:453-466`
  `gdk_wayland_device_get_modifiers` returns that state;
  `gdk/wayland/gdkseat-wayland.c:265-313` the offer's `source_actions`/`action`
  updates and emits a synthetic motion.
- `gtk-4.22.4:gdk/wayland/gdkdrop-wayland.c:286-326`
  `gdk_wayland_drop_update_actions` narrows `Drop::actions` to the selected
  action; `:122-144` `set_actions(actions, preferred)`; `:163-173` finish.
- `gtk-4.22.4:gdk/wayland/gdkdrag-wayland.c:326-338`
  `wl_data_source.action` becomes `GdkDrag::selected_action`; `:414-415` the
  source advertises its actions.
- `/usr/share/wayland/wayland.xml:671-690` (and `:836-855`): compositors may
  change the selected action on the fly in response to keyboard modifier
  changes. This is what makes the selected action useful on Wayland.
- `gtk-4.22.4:gtk/gtkwidget.c:10658-10682` `gtk_widget_compute_bounds` uses the
  widget-to-target transform (so it includes scrolling) and the border rect.

## Verified

### `scripts/check.sh`

Run from `$PROJ` with the pinned 1.95.0 toolchain on `PATH`; exit 0. The headers
and result lines:

```text
==> cargo fmt --all --check
==> cargo clippy --workspace --all-targets -- -D warnings
==> cargo test --workspace
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 175 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 6 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 146 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
==> cargo build --workspace --release
==> cargo deny check
advisories ok, bans ok, licenses ok, sources ok
all checks passed
```

`fm-ui` has 146 tests, 5 more than P2l's 141. The two old action tests were
replaced by the seven named regression tests (net +5). `fm-core` has 175, the
CLI 7, the agent 1, and the trash binary 6 parents plus 6 ignored children.
335 tests ran in all; 6 were ignored; there are no doc-tests. **[observed]**

`cargo deny` printed its pre-existing duplicate `system-deps` warning (the
same one as P2l/P2k) and still ended `all checks passed`. **[observed]**

### Regression tests added

All seven requested names are present in `dnd.rs`:

- `drop_action_defaults_by_filesystem`
- `drop_action_ctrl_copies_and_shift_moves`
- `drop_action_respects_supported_actions`
- `drop_action_external_without_local_drag`
- `drop_intent_survives_async_filesystem_query`
- `background_drop_resolves_padding_and_empty_view`
- `background_drop_preserves_cell_refusal_and_tile_destination`

`the_badge_only_shows_for_several_items` now also covers 12 and 100, and the
existing refusal/same-folder/self-or-descendant tests still pass. **[observed]**

### 10-second run

`pgrep -a gimbal-files` was empty before and after. A release build was launched
with scratch `XDG_CONFIG_HOME` and `XDG_STATE_HOME` under
`/tmp/gimbal-p2l-r1-live2`, a scratch folder with three items, `GIMBAL_LOG=1`,
and no private session bus. It was stopped by its own PID with SIGTERM after
6 s. **[observed]**

```text
pid=1001959
(gimbal-files:1001959): Gtk-CRITICAL **: 18:42:45.524: Unable to connect to the accessibility bus at 'unix:path=/run/user/1000/at-spi/bus_0,guid=cb9029fb16880c0e6c3be6de6ae793ca': Could not connect: No such file or directory
[gimbal 0.861] fm_ui: open () with 1 argument(s): ["/tmp/gimbal-p2l-r1-live2/folder"]
[gimbal 0.875] folder: first items in model: +3 (3 so far) in 14 ms
[gimbal 0.875] folder: listing complete: 3 items (3 visible) in 14 ms
[gimbal 1.862] session: saved /tmp/gimbal-p2l-r1-live2/state/gimbal-files/session.toml
```

The only critical is the known at-spi bus warning from an isolated launch that
does not use `dbus-run-session`; it is not from these changes **[inferred]**.
The scratch folders were removed afterwards. **[observed]**

## Not verified live (Kyle checks these)

Unit tests alone do not prove a visible icon, real key negotiation or GTK
picking. The following need the pointer and keyboard:

- One file: the item's icon follows the pointer. Three files: the badge shows a
  readable `3` without clipping. A double-digit selection (12) also fits.
- Ctrl within one drive copies; Shift across drives moves; no modifier keeps
  move on one filesystem and copy across. Change Ctrl/Shift mid-drag.
- Padding around a cell, the blank area below a short listing, an empty folder
  and every split tile show the accent outline and accept the drop.
- A file cell still refuses with “only folders are drop targets”; a folder cell
  keeps its own destination.
- Dolphin both ways, a pinned row, pin-by-drop, cancel during a drag, and undo
  of a test transfer. Check the original and destination contents, not only the
  cursor or the status text.
- The drag icon surface and badge under niri/Wayland. The hover/drop logs should
  show `offered`, `selected`, `target preference`, `filesystem`, `modifiers`
  and the final `Copy`/`Move`.
- One edge case: a subfolder that is a different mount from its parent. The
  background target and the cell target can ask for different filesystem
  defaults; the last target to set the compositor preference wins. The drop
  still uses that target's own captured intent, but the hover cursor should be
  checked there. **[inferred]**

## Anything outside my files

Nothing needs changing for this brief. There was no `fm-core`, `Cargo.toml`,
runner or `spike/` change. The P2l note's runner workaround (autodrag has no
`app` entry) is unchanged and still outside this brief.

## Review and live check (ChatGPT, Nov 2, 2026): fails

**[observed]** `scripts/check.sh`: exit 0, fmt/clippy/tests/release/deny pass.
335 tests passed; 6 ignored; 146 fm-ui tests. Full reviewer output:
`/tmp/gimbal-p2l-r1-review-check.log`. The existing duplicate system-deps warning remains.

**[read] Safety and scope:** Drag-end still ignores delete_data; the new icon uses
GTK's actual DragIcon. Transfers still use the file-ops worker. The inspected code
adds no dependency or core change. Full changed-file scope/diff is not verified:
project rules prohibit agent git commands; Kyle was asked for status and diff stat.
No app launch or input injection was performed by this reviewer.

**[read] Blocker 1 (P1): unresolved hover intent can become an unintended move.**
`dnd.rs:615-619` only stores the asynchronous filesystem result; it does not refresh
negotiation. `dnd.rs:824-827` supplies a requested preference only if the last motion
already had a filesystem result. Sequence: enter with provisional Copy; hold Ctrl;
receive negotiated Copy; release before a post-query motion. Captured evidence is
both offered, selected Copy, requested None, modifiers unknown. `explicit_action`
returns None; a subsequent same-filesystem result in `Process::drop_on_folder`
resolves to Move. Thus the actual job can move although the compositor selected
Copy. The analogous cross-drive Shift sequence can copy instead of moving. The
current pure tests only cover already-established preferences and miss this event
sequence. Fix negotiation timing and freeze the final agreed operation, rather than
reinterpreting an ambiguous action after accepting the drop.

**[read] Blocker 2 (P2): external selected action becomes a frozen capability mask.**
`dnd.rs:413-427` uses `Drop::actions()` as the offered mask when no local Drag exists;
enter saves that mask for the whole hover. In pinned GDK Wayland source,
`gdkdrop-wayland.c:286-303` this property is the selected action once selected, and
`gdkseat-wayland.c:402` applies pending_action before GTK enter. An external source
that supports COPY|MOVE can therefore be cached as COPY-only or MOVE-only. Later
modifier changes are filtered against that narrowed cache. For example, cached
COPY causes a later Shift-selected Move to be rejected and the final job to copy.
Do not claim a full source capability mask from an already-negotiated singleton.
The current external test supplies a broad mask manually and misses this path.

**[read] Nested-target concern checked:** GTK's `gtkdrop.c:96-115` accepts only the
first status per event. The parent's unconditional preference alone does not prove
that it overrides a child. No separate blocker is claimed for that suspicion.

**Live:** not checked by ChatGPT. Icon visibility/badge clipping, actual keyboard
negotiation, padding hit coverage and interoperability still need Kyle's check.

**Still open:** both blockers are assigned to P2l-r2. Changed-file scope remains
pending Kyle's git output. After code review passes, Kyle should check: single icon
and three-item badge; Ctrl on one drive; Shift across drives; quick drops and key
changes while stationary; defaults; padding/empty folder/split tiles; file-cell
refusal/folder destination; Dolphin both ways; pinned row/pin-by-drop; cancel/undo.

**Scope update (Kyle reports, Nov 2):** Kyle supplied `git status --short` and
`git diff --stat`. Modified code is limited to `app.rs`, `dnd.rs` and
`folder/view.rs`; `docs/notes/P2l-r1.md` is untracked. The other tracked note change,
`docs/notes/P2l.md`, is ChatGPT's earlier live-report addition. Reported scope matches
the implementation notes. Full baseline diff was not supplied. The two blockers remain.
