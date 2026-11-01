// SPDX-License-Identifier: GPL-3.0-or-later
//! The left sidebar: title row, pinned list, section pills and drive bars.
//! (The preview sidebar lives in preview.rs; both share the helpers here.)
use crate::style::{BAR_W, MAIN_BAR_W, PIN_ROW_H, scaled};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

// Constant tables.
#[rustfmt::skip]
const DEFAULT_PINS: [&str; 7] = ["Home", "Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos"];
#[rustfmt::skip]
const BAR_FILL_CLASSES: [&str; 11] = ["bar-lv0", "bar-lv1", "bar-lv2", "bar-lv3", "bar-lv4", "bar-lv5", "bar-lv6", "bar-lv7", "bar-lv8", "bar-lv9", "bar-full"];
const BAR_LABEL_CLASSES: [&str; 3] = ["bar-label-lo", "bar-label-hi", "bar-label-err"];

pub fn pin_icon(name: &str) -> &'static str {
    match name {
        "Home" => "user-home-symbolic",
        "Documents" => "folder-documents-symbolic",
        "Downloads" => "folder-download-symbolic",
        "Music" => "folder-music-symbolic",
        "Pictures" => "folder-pictures-symbolic",
        "Videos" => "folder-videos-symbolic",
        _ => "folder-symbolic",
    }
}

pub fn rule() -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    r.add_css_class("gl-rule");
    r
}

pub fn icon_button(icon: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&gtk::Image::from_icon_name(icon)));
    b.add_css_class("gl-icon-button");
    b
}

pub fn title_row(title: &str, icon: &str, button_first: bool) -> (gtk::Box, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("gl-title");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    // The title yields to the collapse button at the panel's minimum width.
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let button = icon_button(icon);
    // The left panel's button sits at the window edge, exactly where its
    // collapsed strip's expand button is; the preview's stays at the row end.
    if button_first {
        row.append(&button);
    }
    row.append(&label);
    if !button_first {
        row.append(&button);
    }
    (row, button)
}

/// Adds `class` to `w` only for the main drive block.
fn mark(w: &impl gtk::glib::object::IsA<gtk::Widget>, main: bool, class: &str) {
    if main {
        w.add_css_class(class);
    }
}

/// A drive bar: track overlay with a proportional fill and a centred % label.
fn drive_block(name: &str, cap: &str, main: bool) -> (gtk::Box, gtk::Box, gtk::Label) {
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("gl-drive-track");
    mark(&track, main, "main");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&track));
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.add_css_class("gl-drive-fill");
    mark(&fill, main, "main");
    fill.set_halign(gtk::Align::Start);
    fill.set_valign(gtk::Align::Fill);
    overlay.add_overlay(&fill);
    let label = gtk::Label::new(None);
    label.add_css_class("gl-drive-label");
    mark(&label, main, "gl-main-label");
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::Center);
    overlay.add_overlay(&label);
    let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
    block.add_css_class("gl-drive-block");
    let name_label = gtk::Label::new(Some(name));
    name_label.add_css_class("gl-drive-name");
    name_label.set_xalign(0.5);
    name_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    block.append(&name_label);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.set_halign(gtk::Align::Center);
    row.append(&overlay);
    let cap_label = gtk::Label::new(Some(cap));
    cap_label.add_css_class("gl-drive-cap");
    cap_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    mark(&cap_label, main, "gl-main-label");
    row.append(&cap_label);
    block.append(&row);
    (block, fill, label)
}

fn set_fullness(fill: &gtk::Box, label: &gtk::Label, fullness: f64, bar_w: f64, scale: f64) {
    let f = fullness.clamp(0.0, 1.0);
    fill.set_width_request(scaled(bar_w * f, scale));
    let (fill_class, label_class) = crate::style::bar_class(f);
    for class in BAR_FILL_CLASSES {
        fill.remove_css_class(class);
    }
    fill.add_css_class(fill_class);
    for class in BAR_LABEL_CLASSES {
        label.remove_css_class(class);
    }
    label.add_css_class(label_class);
    label.set_text(&format!("{}% Full", (f * 100.0).round() as i32));
}

fn dir_row(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.add_css_class("gl-dir-row");
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.5);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    row.append(&label);
    row
}

/// The row order after moving the pin at `from` to `to`: one remove/insert,
/// the same arithmetic the tile drag uses, so a dragged row keeps its name.
pub fn pin_order(n: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).filter(|&i| i != from).collect();
    if from < n && to <= order.len() {
        order.insert(to, from);
    }
    order
}

/// The live pinned-row layout: which slot the pointer is over and, indexed by
/// the row's original slot, the y every row sits at while the row at `from`
/// follows the pointer (whose grab offset inside the row is `grab`). Rows are
/// one fixed pitch tall; a pointer above or below the list still targets the
/// first or last row, so a drag out of the panel is forgiving. Every other
/// row sits at its post-move slot.
pub fn pin_drag_layout(
    n: usize,
    row_h: i32,
    from: usize,
    pointer_y: i32,
    grab: i32,
) -> (Option<usize>, Vec<i32>) {
    if n == 0 || from >= n || row_h <= 0 {
        return (None, vec![]);
    }
    let target = (pointer_y.div_euclid(row_h)).clamp(0, (n - 1) as i32) as usize;
    let order = pin_order(n, from, target);
    let mut y = vec![0; n];
    for (slot, &row) in order.iter().enumerate() {
        y[row] = slot as i32 * row_h;
    }
    y[from] = pointer_y - grab;
    (Some(target), y)
}

/// The drag in progress: the row's original slot, its grab offset and start
/// point, the child order while the dragged row is raised to the top, and
/// whether the sequence has been claimed after the threshold.
struct PinDrag {
    from: usize,
    grab: i32,
    start: (f64, f64),
    order: Vec<usize>,
    claimed: bool,
}

/// The pinned list: `gl-pinned` box, one row per pin, draggable to reorder.
/// The rows stay in a vertical Box and are moved with top margins (never
/// unparented), so the dragged row can follow the pointer while the others
/// slide to their post-move slots.
pub struct PinnedList {
    pub root: gtk::Box,
    rows: gtk::Box,
    names: RefCell<Vec<String>>,
    /// The row widgets in logical slot order (the child order at rest).
    widgets: RefCell<Vec<gtk::Box>>,
    selected: Cell<usize>,
    scale: Cell<f64>,
    drag: RefCell<Option<PinDrag>>,
    animations: RefCell<Vec<adw::Animation>>,
}

impl PinnedList {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-pinned");
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&rows);
        let this = Rc::new(Self {
            root,
            rows,
            names: RefCell::new(DEFAULT_PINS.iter().map(|name| name.to_string()).collect()),
            widgets: RefCell::new(vec![]),
            selected: Cell::new(0),
            scale: Cell::new(1.0),
            drag: RefCell::new(None),
            animations: RefCell::new(vec![]),
        });
        let weak = Rc::downgrade(&this);
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(move |_, x, y| {
            if let Some(this) = weak.upgrade() {
                this.begin_drag(x, y);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_drag_update(move |g, dx, dy| {
            if let Some(this) = weak.upgrade() {
                this.update_drag(g, dx, dy);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_drag_end(move |_, dx, dy| {
            if let Some(this) = weak.upgrade() {
                this.end_drag(dx, dy);
            }
        });
        let weak = Rc::downgrade(&this);
        drag.connect_cancel(move |_, _| {
            if let Some(this) = weak.upgrade() {
                this.cancel_drag();
            }
        });
        this.rows.add_controller(drag);
        let weak = Rc::downgrade(&this);
        let click = gtk::GestureClick::new();
        click.set_button(1);
        click.connect_released(move |_, _, _, y| {
            if let Some(this) = weak.upgrade() {
                this.clicked(y);
            }
        });
        this.rows.add_controller(click);
        this.rebuild();
        this
    }

    fn row_h(&self) -> i32 {
        scaled(PIN_ROW_H, self.scale.get())
    }

    fn names_len(&self) -> usize {
        self.names.borrow().len()
    }

    pub fn add(&self, name: String) {
        self.names.borrow_mut().push(name);
        self.rebuild();
    }

    pub fn remove_last(&self) {
        if self.names_len() > 1 {
            self.names.borrow_mut().pop();
            self.selected
                .set(self.selected.get().min(self.names_len() - 1));
            self.rebuild();
        }
    }

    pub fn set_count(&self, n: usize) {
        while self.names_len() < n {
            self.add(format!("Pinned {}", self.names_len() + 1));
        }
        while self.names_len() > n.max(1) && self.names_len() > 1 {
            self.remove_last();
        }
        self.rebuild();
    }

    pub fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
    }

    fn rebuild(&self) {
        self.animations.borrow_mut().clear();
        *self.drag.borrow_mut() = None;
        while let Some(child) = self.rows.first_child() {
            self.rows.remove(&child);
        }
        let names = self.names.borrow().clone();
        let mut widgets = Vec::with_capacity(names.len());
        for (i, name) in names.iter().enumerate() {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("gl-pinned-row");
            if i == self.selected.get() {
                row.add_css_class("sel");
            }
            row.append(&gtk::Image::from_icon_name(pin_icon(name)));
            let label = gtk::Label::new(Some(name));
            label.set_hexpand(true);
            label.set_xalign(0.0);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&label);
            self.rows.append(&row);
            widgets.push(row);
        }
        *self.widgets.borrow_mut() = widgets;
    }

    fn clicked(&self, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || y < 0.0 {
            return;
        }
        let slot = (y as i32 / row_h) as usize;
        if slot < n {
            self.selected.set(slot);
            let widgets = self.widgets.borrow();
            for (i, row) in widgets.iter().enumerate() {
                row.remove_css_class("sel");
                if i == slot {
                    row.add_css_class("sel");
                }
            }
            let name = self.names.borrow()[slot].clone();
            crate::log!("pins", "selected '{name}' at slot {slot}");
        }
    }

    fn begin_drag(&self, x: f64, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || row_h <= 0 || y < 0.0 {
            return;
        }
        let from = (y as i32 / row_h) as usize;
        if from >= n {
            return;
        }
        // Raise the dragged row to the end of the child order (draws above
        // the others); this only reorders, it never unparents.
        let mut order: Vec<usize> = (0..n).collect();
        order.retain(|&row| row != from);
        order.push(from);
        let widgets = self.widgets.borrow();
        let mut sibling: Option<gtk::Widget> = None;
        for &row in &order {
            let widget = widgets[row].clone();
            self.rows.reorder_child_after(&widget, sibling.as_ref());
            sibling = Some(widget.upcast());
        }
        drop(widgets);
        let grab = y as i32 - from as i32 * row_h;
        // Raising the row to the end must not move anything before the first
        // update: lay the rows out at their current slots with the dragged row
        // still under the pointer.
        let (_, y_now) = pin_drag_layout(n, row_h, from, y as i32, grab);
        self.layout_drag(&order, &y_now, from, row_h);
        *self.drag.borrow_mut() = Some(PinDrag {
            from,
            grab,
            start: (x, y),
            order,
            claimed: false,
        });
        crate::log!(
            "pins",
            "dragging '{}' from slot {from}",
            self.names.borrow()[from]
        );
    }

    fn update_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(mut drag) = self.drag.borrow_mut().take() else {
            return;
        };
        if !drag.claimed
            && self.rows.drag_check_threshold(
                drag.start.0.round() as i32,
                drag.start.1.round() as i32,
                (drag.start.0 + dx).round() as i32,
                (drag.start.1 + dy).round() as i32,
            )
            && gesture.set_state(gtk::EventSequenceState::Claimed)
        {
            drag.claimed = true;
            crate::log!("pins", "claimed the sequence after the drag threshold");
        }
        let row_h = self.row_h();
        let pointer_y = (drag.start.1 + dy).round() as i32;
        let (target, y) = pin_drag_layout(self.names_len(), row_h, drag.from, pointer_y, drag.grab);
        self.layout_drag(&drag.order, &y, drag.from, row_h);
        crate::log!("pins", "drag over slot {target:?}");
        *self.drag.borrow_mut() = Some(drag);
    }

    /// Put every child in `order` at its desired y using Box top margins. The
    /// margin arithmetic is the inverse of the Box's cumulative layout; the
    /// dragged row moves directly, the other rows slide over 150 ms.
    fn layout_drag(&self, order: &[usize], y: &[i32], dragged: usize, row_h: i32) {
        let widgets = self.widgets.borrow();
        let mut accumulated = 0;
        for (i, &row) in order.iter().enumerate() {
            let Some(&desired) = y.get(row) else {
                continue;
            };
            let margin = desired - i as i32 * row_h - accumulated;
            accumulated += margin;
            let widget = &widgets[row];
            if row == dragged {
                widget.set_margin_top(margin);
            } else if widget.margin_top() != margin {
                self.slide_margin(widget, margin);
            }
        }
    }

    fn slide_margin(&self, widget: &gtk::Box, to: i32) {
        let start = widget.margin_top();
        if start == to {
            widget.set_margin_top(to);
            return;
        }
        let w = widget.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            let margin = (start as f64 + (to - start) as f64 * v).round() as i32;
            w.set_margin_top(margin);
        });
        let anim = adw::TimedAnimation::new(widget, 0.0, 1.0, 150, target);
        self.animations.borrow_mut().push(anim.clone().upcast());
        anim.play();
    }

    fn end_drag(&self, _dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let row_h = self.row_h();
        let pointer_y = (drag.start.1 + dy).round() as i32;
        let (target, _) = pin_drag_layout(self.names_len(), row_h, drag.from, pointer_y, drag.grab);
        let Some(to) = target else {
            self.settle();
            return;
        };
        if to != drag.from {
            let name = self.names.borrow()[drag.from].clone();
            {
                let mut names = self.names.borrow_mut();
                let name = names.remove(drag.from);
                names.insert(to, name);
            }
            {
                let mut widgets = self.widgets.borrow_mut();
                let row = widgets.remove(drag.from);
                widgets.insert(to, row);
            }
            self.selected.set(to);
            crate::log!("pins", "moved '{name}' from slot {} to {to}", drag.from);
        }
        // The drag has ended; putting the rows back in slot order and zeroing
        // their margins only reorders, it never unparents.
        self.settle();
    }

    fn cancel_drag(&self) {
        self.drag.borrow_mut().take();
        self.settle();
    }

    /// Put the rows back in logical slot order with zero margins and refresh
    /// the selection class. Reordering the existing children (rather than
    /// rebuilding) keeps the drag path free of unparenting.
    fn settle(&self) {
        self.animations.borrow_mut().clear();
        let widgets = self.widgets.borrow();
        let mut sibling: Option<gtk::Widget> = None;
        for (i, row) in widgets.iter().enumerate() {
            self.rows.reorder_child_after(row, sibling.as_ref());
            row.set_margin_top(0);
            row.remove_css_class("sel");
            if i == self.selected.get() {
                row.add_css_class("sel");
            }
            sibling = Some(row.clone().upcast());
        }
    }
}

pub struct LeftPanel {
    pub root: gtk::Box,
    pub collapse_button: gtk::Button,
    pinned: Rc<PinnedList>,
    devices_content: gtk::Box,
    drives: Vec<(gtk::Box, gtk::Label, f64)>,
    main: Option<(gtk::Box, gtk::Label)>,
    scale: f64,
}

impl LeftPanel {
    pub fn new(drives: &[f64]) -> Self {
        let mut this = Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 0),
            collapse_button: gtk::Button::new(),
            pinned: PinnedList::new(),
            devices_content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            drives: vec![],
            main: None,
            scale: 1.0,
        };
        this.root.add_css_class("gl-panel");
        let (row, button) = title_row("Pinned Folders", "sidebar-show-symbolic", true);
        this.collapse_button = button;
        this.root.append(&row);
        this.root.append(&rule());
        // The whole panel scrolls when the pins outgrow it; the main drive
        // block stays pinned below the scroller.
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&this.pinned.root);
        body.append(&rule());
        // Three collapsible sections: the pill toggles a SlideDown revealer.
        let recent = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for _ in 0..5 {
            recent.append(&dir_row("Directory Name"));
        }
        let no_remotes = dir_row("No remotes");
        let sections: [(&gtk::Box, &str); 3] = [
            (&recent, "Recent Directories"),
            (&this.devices_content, "Devices"),
            (&no_remotes, "Remote"),
        ];
        for (content, name) in sections {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
            section.add_css_class("gl-section");
            let pill = gtk::Button::new();
            pill.add_css_class("gl-section-pill");
            let pill_label = gtk::Label::new(Some(name));
            pill_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            pill.set_child(Some(&pill_label));
            let revealer = gtk::Revealer::new();
            revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
            revealer.set_transition_duration(200);
            revealer.set_reveal_child(true);
            revealer.set_child(Some(content));
            section.append(&pill);
            section.append(&revealer);
            body.append(&section);
            let rev = revealer.clone();
            pill.connect_clicked(move |_| rev.set_reveal_child(!rev.reveals_child()));
        }
        scroll.set_child(Some(&body));
        this.root.append(&scroll);
        this.root.append(&rule());
        let (block, fill, label) = drive_block("Main Drive Name", "1 TB", true);
        this.main = Some((fill, label));
        this.root.append(&block);
        for f in drives {
            let (block, fill, label) = drive_block("Drive Name", "512 GB", false);
            this.devices_content.append(&block);
            this.drives.push((fill, label, *f));
        }
        this.set_scale(1.0);
        this
    }

    pub fn add_pin(&mut self) {
        self.pinned
            .add(format!("Pinned {}", self.pinned.names_len() + 1));
    }

    pub fn remove_pin(&mut self) {
        self.pinned.remove_last();
    }

    pub fn set_pin_count(&mut self, n: usize) {
        self.pinned.set_count(n);
    }

    pub fn set_drive(&mut self, index: usize, fullness: f64) {
        if let Some(entry) = self.drives.get_mut(index) {
            entry.2 = fullness.clamp(0.0, 1.0);
            set_fullness(&entry.0, &entry.1, entry.2, BAR_W, self.scale);
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.pinned.set_scale(scale);
        for (fill, label, f) in &self.drives {
            set_fullness(fill, label, *f, BAR_W, scale);
        }
        if let Some((fill, label)) = &self.main {
            set_fullness(fill, label, 0.75, MAIN_BAR_W, scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_order_moves_one_row() {
        assert_eq!(pin_order(5, 0, 2), [1, 2, 0, 3, 4]);
        assert_eq!(pin_order(5, 4, 0), [4, 0, 1, 2, 3]);
        assert_eq!(pin_order(1, 0, 0), [0]);
        assert_eq!(pin_order(3, 1, 2), [0, 2, 1]);
        // A same-slot drop changes nothing.
        for n in 1..=6 {
            assert_eq!(pin_order(n, n - 1, n - 1), (0..n).collect::<Vec<_>>());
        }
    }

    #[test]
    fn pinned_drag_follows_the_pointer_and_slides_the_others() {
        let h = 64;
        // Drag row 0 over slot 2 with a 10 px grab: it rides the pointer while
        // rows 1 and 2 move up one slot and rows 3 and 4 stay put.
        let (target, y) = pin_drag_layout(5, h, 0, 2 * h + 10, 10);
        assert_eq!(target, Some(2));
        assert_eq!(y[0], 2 * h); // pointer_y − grab
        assert_eq!((y[1], y[2], y[3], y[4]), (0, 64, 192, 256));
        // Above the list clamps to the first slot; the dragged row may leave
        // the panel and still follows the pointer.
        let (target, y) = pin_drag_layout(4, h, 2, -500, 0);
        assert_eq!(target, Some(0));
        assert_eq!((y[0], y[1], y[2], y[3]), (64, 128, -500, 192));
        // Below the list clamps to the last slot.
        let (target, _) = pin_drag_layout(4, h, 1, 99_999, 0);
        assert_eq!(target, Some(3));
        // Degenerate inputs.
        assert_eq!(pin_drag_layout(0, h, 0, 0, 0), (None, vec![]));
        assert_eq!(pin_drag_layout(2, 0, 0, 0, 0), (None, vec![]));
        assert_eq!(pin_drag_layout(2, h, 5, 0, 0), (None, vec![]));
    }
}
