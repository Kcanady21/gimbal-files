// SPDX-License-Identifier: GPL-3.0-or-later
//! Show a popover so a popup the compositor refused can never lock the window.
//!
//! Wayland GDK takes a seat grab while presenting the surface. If presentation
//! fails before the first configure, GTK can leave the popover visible but
//! unmapped (gtk-4.22.4 gtk/gtkpopover.c:1152). Popdown then skips the widget's
//! unmap handler, so it never hides the surface to release the seat grab.

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
        let surface = popover.surface();
        if !popover.is_visible() || surface.as_ref().is_some_and(|surface| surface.is_mapped()) {
            return;
        }
        crate::log!(
            "popup",
            "the compositor never showed a popover; closing it to release its grab"
        );
        // Wayland's surface hide releases the seat grab even if the surface
        // never mapped (gtk-4.22.4 gdk/wayland/gdksurface-wayland.c:1148).
        if let Some(surface) = surface {
            surface.hide();
        }
        popover.popdown();
    });
}
