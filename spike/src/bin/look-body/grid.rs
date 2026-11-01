// SPDX-License-Identifier: GPL-3.0-or-later
//! The fake folder grid: a GridView over a StringList of fake names. No filesystem.
use crate::style::{CELL_W, GAP_XL, scaled};
use gtk::SignalListItemFactory;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub fn counts(items: u32) -> (u32, u32) {
    (items - items.min(4), items.min(4))
}

pub fn item_name(i: u32) -> (String, &'static str) {
    if i < 4 {
        (format!("Folder {}", i + 1), "gimbal-folder-symbolic")
    } else {
        (format!("File {}.txt", i + 1), "text-x-generic")
    }
}

/// The `max-columns` cap for a grid viewport `view_w` wide: how many columns
/// fit at the minimum cell width, plus one. `view_w` is the grid's own
/// allocation — its CSS margins sit outside it — so nothing is subtracted
/// here; the cap sits one above what fits and therefore never binds.
pub fn max_columns(view_w: i32, scale: f64) -> i32 {
    let cell = scaled(CELL_W, scale).max(1);
    let gap = scaled(GAP_XL, scale);
    ((view_w + gap) / (cell + gap)).max(1) + 1
}

thread_local! {
    /// The scale every live grid computes its column cap from.
    static SCALE: Cell<f64> = const { Cell::new(1.0) };
    /// The scroller of every folder grid, weakly (dead tiles drop out).
    static SCROLLERS: RefCell<Vec<gtk::glib::WeakRef<gtk::ScrolledWindow>>> = const { RefCell::new(Vec::new()) };
}

fn update_cap(scroller: &gtk::ScrolledWindow) {
    let Some(grid) = scroller.child().and_downcast::<gtk::GridView>() else {
        return;
    };
    let width = scroller.hadjustment().page_size() as i32;
    let scale = SCALE.with(Cell::get);
    let cap = max_columns(width, scale).max(1) as u32;
    grid.set_max_columns(cap);
    crate::log!(
        "grid",
        "viewport {width} px at scale {scale:.2} → cap {cap}"
    );
}

/// Apply the cap outside allocation. The h-adjustment's `page-size` notify
/// fires while the ScrolledWindow is inside its own `size_allocate`, and a
/// `queue_resize` made there is silently ignored (gtk-4.22.4:gtk/gtkwidget.c:3709–3712):
/// `gtk_widget_allocate` clears resize_queued and alloc_needed unconditionally
/// after size_allocate returns (:4347–4356), so the reallocation the cap asked
/// for never happens. One idle per grid is enough: while it is pending, later
/// page-size notifications coalesce into it, and the idle reads the current
/// width and scale when it runs.
fn schedule_cap(scroller: &gtk::ScrolledWindow, pending: Rc<Cell<bool>>) {
    if pending.replace(true) {
        return;
    }
    let scroller = scroller.clone();
    glib::idle_add_local_once(move || {
        pending.set(false);
        update_cap(&scroller);
    });
}

/// Called by `apply_scale` after a scale change: every grid's cap depends on
/// the scale, and only the viewport width change would otherwise recompute it.
pub fn set_scale(scale: f64) {
    SCALE.with(|s| s.set(scale));
    SCROLLERS.with(|grids| {
        let mut grids = grids.borrow_mut();
        grids.retain(|weak| weak.upgrade().is_some());
        for weak in grids.iter() {
            if let Some(scroller) = weak.upgrade() {
                update_cap(&scroller);
            }
        }
    });
}

/// One folder view: a ScrolledWindow with a GridView of `items` fake cells.
/// `tile` selects the tile-grid margins (`.gl-grid.tile`); the single view
/// keeps the `grid-margin` insets.
pub fn folder_view(items: u32, tile: bool) -> gtk::ScrolledWindow {
    let list = gtk::StringList::new(&[]);
    (0..items).for_each(|i| list.append(&item_name(i).0));
    let factory = SignalListItemFactory::new();
    factory.connect_setup(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
        cell.add_css_class("gl-cell");
        let icon = gtk::Image::new();
        let label = gtk::Label::new(None);
        label.set_justify(gtk::Justification::Center);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_wrap(true);
        label.set_lines(2);
        cell.append(&icon);
        cell.append(&label);
        item.set_child(Some(&cell));
    });
    factory.connect_bind(move |_, obj| {
        let Some(item) = obj.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(entry) = item.item().and_downcast::<gtk::StringObject>() else {
            return;
        };
        let Some(cell) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let (_, icon) = item_name(item.position());
        if let Some(image) = cell.first_child().and_downcast::<gtk::Image>() {
            image.set_icon_name(Some(icon));
        }
        if let Some(label) = cell.last_child().and_downcast::<gtk::Label>() {
            label.set_text(&entry.string());
        }
    });
    // min-columns 1; the column cap follows the viewport width (and the scale).
    let grid = gtk::GridView::builder()
        .model(&gtk::SingleSelection::new(Some(list)))
        .factory(&factory)
        .min_columns(1)
        .max_columns(1)
        .build();
    grid.add_css_class("gl-grid");
    if tile {
        grid.add_css_class("tile");
    }
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_child(Some(&grid));
    scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    // Natural sizes must not pin the window's minimum size; the viewport decides.
    scroller.set_propagate_natural_width(false);
    scroller.set_propagate_natural_height(false);
    scroller.set_vexpand(true);
    // The viewport width (the h-adjustment's page size) drives the cap, so
    // recompute it on every width change — in an idle, never in the
    // allocation that fires the notify (see `schedule_cap`).
    let watched = scroller.clone();
    let pending = Rc::new(Cell::new(false));
    scroller
        .hadjustment()
        .connect_notify_local(Some("page-size"), move |_, _| {
            schedule_cap(&watched, pending.clone());
        });
    let weak = gtk::glib::WeakRef::new();
    weak.set(Some(&scroller));
    SCROLLERS.with(|grids| grids.borrow_mut().push(weak));
    scroller
}

pub fn status_label(items: u32) -> gtk::Label {
    let (files, folders) = counts(items);
    let label = gtk::Label::new(Some(&format!("Files: {files}, Folders: {folders}")));
    label.add_css_class("gl-status");
    label.set_hexpand(true);
    label.set_xalign(0.5);
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_split() {
        assert_eq!(counts(44), (40, 4));
        assert_eq!(counts(3), (0, 3));
        assert_eq!(counts(0), (0, 0));
    }

    #[test]
    fn column_cap() {
        // The sketch's content viewport: 11 columns fit at the 129 px pitch
        // (cell 97 + gap-xl 32), and the cap sits one above what fits.
        assert_eq!(max_columns(1544, 1.0), 13);
        assert!(max_columns(1544, 1.0) >= 11);
        // A split tile's viewport allows fewer columns (5 in the sketch).
        assert_eq!(max_columns(695, 1.0), 6);
        assert!(max_columns(695, 1.0) < max_columns(1544, 1.0));
        // At scale 0.5 roughly double the columns fit the same viewport.
        let half = max_columns(1544, 0.5);
        assert!(half >= 2 * 11, "{half} should be at least double the 11");
        // Degenerate viewport still gets a small cap, never 0.
        assert_eq!(max_columns(0, 1.0), 2);
    }
}
