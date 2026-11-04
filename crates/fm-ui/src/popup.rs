// SPDX-License-Identifier: GPL-3.0-or-later
//! Show a popover so a popup the compositor refused can never lock the window.
//!
//! An autohide popover takes a window-group grab when it maps
//! (gtk-4.22.4 gtk/gtkpopover.c:1222) and drops it only on unmap, which GTK
//! triggers from the popup surface's `mapped` notify (gtkpopover.c:927–932).
//! A Wayland popup becomes mapped on its first configure
//! (gdk/wayland/gdksurface-wayland.c:970 `maybe_notify_mapped`). When the
//! compositor sends `xdg_popup.done` before that configure, `gdk_surface_hide`
//! moves `mapped` from FALSE to FALSE (gdk/gdksurface.c
//! `gdk_surface_queue_set_is_mapped`), no notify fires, and the popover stays
//! "visible" with its grab: every click in the window goes to an invisible
//! widget. The watchdog below pops such a popover down, which releases the grab.

use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

/// Long enough for any real first configure; short enough that a refused
/// popup unlocks the window before the user gives up on it.
const MAP_DEADLINE: Duration = Duration::from_millis(750);

/// `popover.popup()`, plus the refused-popup watchdog.
pub fn show(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    popover.popup();
    let weak = popover.downgrade();
    glib::timeout_add_local_once(MAP_DEADLINE, move || {
        let Some(popover) = weak.upgrade() else {
            return;
        };
        let surface_mapped = popover.surface().is_some_and(|surface| surface.is_mapped());
        if popover.is_visible() && !surface_mapped {
            crate::log!(
                "popup",
                "the compositor never showed a popover; closing it to release its grab"
            );
            popover.popdown();
        }
    });
}
