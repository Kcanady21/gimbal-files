// SPDX-License-Identifier: GPL-3.0-or-later
//! Show a popover so a popup the compositor refused can never lock the window.
//!
//! Wayland GDK takes a seat grab while presenting the surface. If presentation
//! fails before the first configure, GTK can leave the popover visible but
//! unmapped (gtk-4.22.4 gtk/gtkpopover.c:1152). Popdown then skips the widget's
//! unmap handler, so it never hides the surface to release the seat grab.
//!
//! A popover that mapped but was never dismissed keeps the grab too. The
//! compositor's only dismissal is `xdg_popup.done`, which GDK turns into a
//! plain surface hide nothing in GTK can hook
//! (gtk-4.22.4 gdk/wayland/gdkpopup-wayland.c:473-482), and while the grab
//! lasts GDK refuses every later grabbing popup with "non-top most parent"
//! (gdkpopup-wayland.c:981). So `show` closes Gimbal's own still-open popovers
//! first: hiding the surface destroys the `xdg_popup`, and the compositor ends
//! the seat grab when the grabbing popup goes
//! (wlroots types/xdg_shell/wlr_xdg_popup.c, `reset_xdg_popup`).

use std::cell::{Cell, RefCell};
use std::time::Duration;

use gtk::glib;
use gtk::prelude::*;

use crate::look::tokens::{GAP_M, scaled};

/// Long enough for any real first configure; short enough that a refused
/// popup unlocks the window before the user gives up on it.
const MAP_DEADLINE: Duration = Duration::from_millis(750);

/// The largest height a window-anchored menu may use: the space between the
/// anchor's bottom edge and the window's bottom edge, less one `GAP_M`
/// margin, never negative. A menu that may flip above its anchor passes 0,
/// which limits it by the whole window instead. Unlike the accordion's
/// twelve-row cap there is no token ceiling: Settings is limited only by the
/// window.
pub fn menu_height_cap(scale: f64, window_height: i32, anchor_bottom: i32) -> i32 {
    let below = (window_height - anchor_bottom).max(0);
    (below - scaled(GAP_M, scale)).max(0)
}

/// Whether a capped scroller needs its scrollbar: content shorter than the
/// cap never scrolls, taller content always does. The policy is decided from
/// this measurement instead of `Automatic`, whose late scrollbar took its
/// width out of already-laid-out rows and clipped them (Kyle, Nov 8).
pub fn scrolls(content_height: i32, cap: i32) -> bool {
    content_height > cap
}

/// `menu` wrapped in a scroller that never asks for more than `cap` pixels of
/// height. Menus with wrapped labels measure far wider unwrapped than a
/// window can ever allocate, so `window_width` bounds the measurement the
/// height decision is made at.
pub fn capped_scroller(
    menu: &impl IsA<gtk::Widget>,
    cap: i32,
    window_width: i32,
) -> gtk::ScrolledWindow {
    let menu = menu.as_ref();
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_propagate_natural_height(true);
    scroller.set_propagate_natural_width(true);
    // A classic scrollbar that takes its own width: the default overlay
    // scrollbar was drawn over the rows' own widgets (Kyle, Nov 8). With the
    // natural width propagated, the popover widens to fit it.
    scroller.set_overlay_scrolling(false);
    scroller.set_child(Some(menu));

    let (_, rows_width, _, _) = menu.measure(gtk::Orientation::Horizontal, -1);
    let reference = if window_width > 0 {
        rows_width.min(window_width)
    } else {
        rows_width
    };
    let (_, rows_height, _, _) = menu.measure(gtk::Orientation::Vertical, reference);
    let scrolling = scrolls(rows_height, cap);
    scroller.set_policy(
        gtk::PolicyType::Never,
        if scrolling {
            gtk::PolicyType::Always
        } else {
            gtk::PolicyType::Never
        },
    );
    if scrolling {
        // The scrollbar's width is counted before the popover maps, never
        // taken back out of the rows.
        scroller.set_min_content_width(reference.max(1));
    }
    scroller.set_max_content_height(cap.max(1));
    scroller
}

struct OpenPopover {
    name: String,
    popover: glib::WeakRef<gtk::Popover>,
    /// `map` and `closed` are connected once per popover: the accordion's
    /// popover is shown many times over its life.
    wired: Cell<bool>,
    /// Between a `show` nobody has closed in the log yet.
    open: Cell<bool>,
}

thread_local! {
    /// Popovers shown through this module whose close is not logged yet,
    /// oldest first. A visible entry while a new popover is shown is the
    /// culprit the Gdk "non-top most parent" warning would name.
    static OPEN: RefCell<Vec<OpenPopover>> = const { RefCell::new(Vec::new()) };
    /// Close reasons claimed by [`popdown`], consumed by the `closed`
    /// handler, dropped from an idle if the popover was never shown here.
    /// Only ever compared, never dereferenced.
    static CLAIMED: RefCell<Vec<(usize, String)>> = const { RefCell::new(Vec::new()) };
}

/// `popover.popup()`, plus the refused-popup watchdog. The log names the
/// popover "(unnamed)"; the named form [`show_named`] is preferred.
pub fn show(popover: &impl IsA<gtk::Popover>) {
    show_named("(unnamed)", popover);
}

/// `popover.popup()` with a name for the log, plus:
/// - any Gimbal popover still open is closed first, so a second grabbing
///   popup is never stacked on a stale first one;
/// - `shown`, `mapped` and `closed` log lines naming the popover;
/// - the refused-popup watchdog.
pub fn show_named(name: &str, popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    close_stale(name, &keepers(popover));

    let mut wired = false;
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        open.retain(|entry| entry.popover.upgrade().is_some());
        let ptr = popover.as_ptr();
        if let Some(entry) = open.iter_mut().find(|entry| {
            entry
                .popover
                .upgrade()
                .is_some_and(|known| known.as_ptr() == ptr)
        }) {
            entry.open.set(true);
            wired = entry.wired.get();
            if !wired {
                wire(name, popover);
                entry.wired.set(true);
            }
        } else {
            wire(name, popover);
            wired = true;
            open.push(OpenPopover {
                name: name.to_string(),
                popover: popover.downgrade(),
                wired: Cell::new(true),
                open: Cell::new(true),
            });
        }
    });

    crate::log!("popup", "shown {name}");
    popover.popup();
    arm_watchdog(name, popover);
}

/// Gimbal's own popdown, so its close is logged as Gimbal's and never as a
/// compositor dismissal. The `closed` handler consumes the claimed reason
/// re-entrantly; an idle clears a claim nothing consumed, so it can never
/// name a later popover at the same address.
pub fn popdown(popover: &impl IsA<gtk::Popover>) {
    let popover = popover.as_ref();
    CLAIMED.with(|claimed| {
        claimed
            .borrow_mut()
            .push((popover.as_ptr() as usize, "closed by Gimbal".to_string()))
    });
    popover.popdown();
    glib::idle_add_local_once(|| {
        CLAIMED.with(|claimed| claimed.borrow_mut().clear());
    });
}

/// Wire the once-per-popover signals. `name` is captured at first show and
/// stays with the popover: the accordion shows its one popover many times
/// under the same name.
fn wire(name: &str, popover: &gtk::Popover) {
    let mapped = format!("mapped {name}");
    popover.connect_map(move |_| {
        crate::log!("popup", "{mapped}");
    });

    let closed_name = name.to_string();
    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        let ptr = weak.upgrade().map(|popover| popover.as_ptr() as usize);
        let claimed: Option<String> = ptr
            .map(|ptr| {
                CLAIMED.with(|claimed| {
                    let mut claimed = claimed.borrow_mut();
                    let mut found: Option<String> = None;
                    claimed.retain(|(candidate, reason)| {
                        if found.is_none() && *candidate == ptr {
                            found = Some(reason.clone());
                            false
                        } else {
                            true
                        }
                    });
                    found
                })
            })
            .unwrap_or(None);
        if claim_close(&weak) {
            crate::log!(
                "popup",
                "closed {closed_name} ({})",
                claimed
                    .as_deref()
                    .unwrap_or("popdown or compositor dismissal")
            );
        }
    });
}

/// Claims the right to log this popover's close, so exactly one line names
/// it: the watchdog's or the replacement's, never the `closed` handler's on
/// top of them.
fn claim_close(popover: &glib::WeakRef<gtk::Popover>) -> bool {
    let Some(target) = popover.upgrade().map(|popover| popover.as_ptr()) else {
        return false;
    };
    OPEN.with(|open| {
        let mut open = open.borrow_mut();
        match open.iter_mut().find(|entry| {
            entry
                .popover
                .upgrade()
                .is_some_and(|known| known.as_ptr() == target)
        }) {
            Some(entry) if entry.open.get() => {
                entry.open.set(false);
                true
            }
            _ => false,
        }
    })
}

/// The popovers a new popup must not close, by surface identity: itself, and
/// every popover in its anchor's widget chain. A pill menu on an accordion
/// row is a child popup of the still-open accordion — its grab is the parent
/// grab the new popup is allowed against, and closing it would unmap the
/// anchor underneath the menu.
fn keepers(popover: &gtk::Popover) -> Vec<usize> {
    let mut keep = vec![popover.as_ptr() as usize];
    let mut ancestor = popover.parent();
    while let Some(widget) = ancestor {
        keep.push(widget.as_ptr() as usize);
        ancestor = widget.parent();
    }
    keep
}

/// Popovers still open when a new one is shown are named and closed: their
/// surfaces hold the seat grab the new popup would be refused against.
/// Popovers in `keep` stay: they are this popup itself or its anchor chain.
fn close_stale(showing: &str, keep: &[usize]) {
    let stale: Vec<(glib::WeakRef<gtk::Popover>, String)> = OPEN.with(|open| {
        let mut open = open.borrow_mut();
        open.retain(|entry| entry.popover.upgrade().is_some());
        let stale: Vec<(glib::WeakRef<gtk::Popover>, String)> = open
            .iter()
            .filter(|entry| entry.open.get())
            .filter(|entry| {
                entry.popover.upgrade().is_some_and(|popover| {
                    popover.is_visible() && !keep.contains(&(popover.as_ptr() as usize))
                })
            })
            .map(|entry| (entry.popover.clone(), entry.name.clone()))
            .collect();
        if !stale.is_empty() {
            let names: Vec<&str> = stale.iter().map(|(_, name)| name.as_str()).collect();
            crate::log!(
                "popup",
                "open before showing {showing}: {}",
                names.join(", ")
            );
        }
        for entry in open.iter_mut() {
            if entry.popover.upgrade().is_some_and(|popover| {
                entry.open.get()
                    && popover.is_visible()
                    && !keep.contains(&(popover.as_ptr() as usize))
            }) {
                entry.open.set(false);
            }
        }
        stale
    });
    // The popdown runs without the registry borrow: its `closed` handler
    // re-enters this module.
    for (popover, name) in stale {
        crate::log!("popup", "closed {name} (closed to show {showing})");
        if let Some(popover) = popover.upgrade() {
            popover.popdown();
        }
    }
}

/// The refused-popup watchdog, armed for every show: a popover whose surface
/// never mapped is closed after `MAP_DEADLINE`, because popdown alone skips
/// the unmap that hides the surface and releases the seat grab.
fn arm_watchdog(name: &str, popover: &gtk::Popover) {
    let weak = popover.downgrade();
    let name = name.to_string();
    glib::timeout_add_local_once(MAP_DEADLINE, move || {
        let Some(popover) = weak.upgrade() else {
            return;
        };
        let surface = popover.surface();
        if !popover.is_visible() || surface.as_ref().is_some_and(|surface| surface.is_mapped()) {
            return;
        }
        if claim_close(&weak) {
            crate::log!(
                "popup",
                "closed {name} (the compositor never showed it; closing it to release its grab)"
            );
        }
        // Wayland's surface hide releases the seat grab even if the surface
        // never mapped (gtk-4.22.4 gdk/wayland/gdksurface-wayland.c:1148).
        if let Some(surface) = surface {
            surface.hide();
        }
        popover.popdown();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cap_is_the_space_below_the_anchor_less_the_margin() {
        // The margin is one GAP_M (16 px) at scale 1.
        assert_eq!(menu_height_cap(1.0, 1440, 96), 1440 - 96 - 16);
        // A short window leaves its whole height minus two margins worth of
        // margin and cap: never negative.
        assert_eq!(menu_height_cap(1.0, 120, 96), 120 - 96 - 16);
        assert_eq!(menu_height_cap(1.0, 90, 96), 0);
        assert_eq!(menu_height_cap(1.0, 100, 100), 0);
        // An anchor near the bottom leaves less than the margin, so the cap
        // clamps at zero rather than asking for a negative height.
        assert_eq!(menu_height_cap(1.0, 700, 690), 0);
        assert_eq!(menu_height_cap(1.0, 700, 684), 0);
        assert_eq!(menu_height_cap(1.0, 700, 683), 1);
        // The margin scales.
        assert_eq!(
            menu_height_cap(0.85, 700, 96),
            700 - 96 - scaled(GAP_M, 0.85)
        );
        assert_eq!(menu_height_cap(2.0, 2000, 192), 2000 - 192 - 32);
    }

    #[test]
    fn the_margin_never_loses_its_pixel() {
        // scaled() never returns zero for a non-zero token, so a cap is
        // always at least one pixel less than the space below the anchor.
        assert_eq!(scaled(GAP_M, 0.01), 1);
        assert_eq!(menu_height_cap(0.01, 96, 96), 0);
        assert_eq!(menu_height_cap(0.01, 98, 96), 1);
    }

    #[test]
    fn shorter_content_never_scrolls_and_taller_content_always_does() {
        assert!(!scrolls(0, 1));
        assert!(!scrolls(403, 404));
        assert!(!scrolls(404, 404));
        assert!(scrolls(405, 404));
    }
}
