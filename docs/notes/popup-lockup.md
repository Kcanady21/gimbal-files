# Window lockup from a refused popup (Claude, Nov 4, 2026)

## Symptom

Twice on Nov 4 (once in the Settings dropdown), Gimbal's window stopped taking any click: Float,
✕, tabs, everything. The main thread sat idle in `poll`, so the app was not hung. Only closing it
through niri and relaunching helped. [observed]

## Cause

- An autohide `GtkPopover` calls `gtk_grab_add` when it maps (gtk-4.22.4
  `gtk/gtkpopover.c:1222`). While that grab is held, all input to the window group goes to the
  popover. It is released only in `gtk_popover_unmap` (:1233). [read]
- GTK unmaps the popover when its popup surface's `mapped` property changes
  (`surface_mapped_changed`, :927–932). [read]
- A Wayland popup surface becomes mapped on its first configure (`maybe_notify_mapped`,
  `gdk/wayland/gdksurface-wayland.c:970`). [read]
- `xdg_popup.done` runs `gdk_surface_hide` (`gdk/wayland/gdkpopup-wayland.c:473`). If it arrives
  before that first configure, `mapped` goes FALSE → FALSE, `gdk_surface_queue_set_is_mapped`
  sends no notify, and the popover stays "visible" with its grab. [read]
- Gimbal sets no `autohide(false)` anywhere, so every popover grabs. Two openings are likely to be
  refused by the compositor: the Settings submenu, opened from inside the closing main menu's click,
  and the tab accordion, opened on hover with no click behind it. [inferred]

## Fix

- `crates/fm-ui/src/popup.rs`: `show()` calls `popup()`, then after 750 ms pops the popover down if
  GTK still thinks it is visible but its surface never mapped. Popping it down releases the grab.
  It logs `popup: the compositor never showed a popover…` under `GIMBAL_LOG=1`.
- All 15 `popover.popup()` sites now call `crate::popup::show` (header 7, sidebar 3, file dialogs 3,
  split 1, trash 1).
- The main menu's Settings item opens the submenu from an idle, after the click and the closing
  main menu are done.
- Gates: `scripts/check.sh` exit 0, 805 passed / 23 ignored (with uncommitted P2r + P2s). [observed]

## Not verified

- Not reproduced live. The confirming evidence is a `popup:` log line the next time it would have
  locked, with the window staying usable.
- Follow-up: the accordion's hover-open. A refused hover popup now just closes itself. If the log
  shows it firing there, give the hover path `autohide(false)` with its own leave-to-close.
