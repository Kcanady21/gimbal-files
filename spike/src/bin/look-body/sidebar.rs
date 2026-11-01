// SPDX-License-Identifier: GPL-3.0-or-later
//! The left sidebar: title row, pinned list, section pills and drive bars.
//! (The preview sidebar lives in preview.rs; both share the helpers here.)
use crate::style::{BAR_W, MAIN_BAR_W, PIN_ROW_H, scaled};
use adw::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;
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

/// The drag in progress: the row's original slot, its grab offset, the
/// pointer's local start, the rows container's bounds in the window and the
/// window's surface transform, and whether the sequence has been claimed.
struct PinDrag {
    from: usize,
    grab: i32,
    start: (f64, f64),
    bounds: (f64, f64),
    transform: (f64, f64),
    claimed: bool,
}

/// What releasing a press on a pinned row means. A press that never passed
/// the drag threshold can only select (or open, on a double-click) — it must
/// never reorder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PinRelease {
    Select { slot: usize },
    Open { slot: usize },
    Reorder { from: usize, to: usize },
}

/// The row a pointer at `y` (in the rows' own space) is over.
pub fn pin_slot(y: f64, row_h: i32, n: usize) -> usize {
    if n == 0 || row_h <= 0 {
        return 0;
    }
    ((y as i32).div_euclid(row_h)).clamp(0, (n - 1) as i32) as usize
}

/// The decision at release, pure: a still pointer selects (and a double-click
/// opens); only `moved` — the press passed the drag threshold — can reorder.
pub fn pin_release(
    n: usize,
    row_h: i32,
    from: usize,
    pointer_y: f64,
    n_press: i32,
    moved: bool,
) -> PinRelease {
    if n == 0 || row_h <= 0 {
        return PinRelease::Select { slot: 0 };
    }
    let from = from.min(n - 1);
    if !moved {
        return if n_press >= 2 {
            PinRelease::Open { slot: from }
        } else {
            PinRelease::Select { slot: from }
        };
    }
    let to = pin_slot(pointer_y, row_h, n);
    if to == from {
        PinRelease::Select { slot: from }
    } else {
        PinRelease::Reorder { from, to }
    }
}

/// The pointer in the rows' own space: the surface position minus the window's
/// surface transform and the rows' bounds in the window. `GtkGestureDrag`
/// reports its press point already in that space (GTK itself subtracts the
/// surface transform, which is the client-side shadow — 25 px here), so a
/// still pointer yields the press point and never passes the threshold.
/// Reading the raw surface position without the transform made every click
/// look like a drag whose drop sat one row lower.
pub fn pin_local(
    surface: (f64, f64),
    surface_transform: (f64, f64),
    bounds: (f64, f64),
) -> (f64, f64) {
    (
        surface.0 - surface_transform.0 - bounds.0,
        surface.1 - surface_transform.1 - bounds.1,
    )
}

/// The pinned rows' container: a plain widget whose custom `PinnedRowLayout`
/// gives every row the list's full width at the y the drag arithmetic chose.
/// The list's minimum width is one row's own minimum, never the allocated
/// width — a `width_request` set from an allocation would feed the panel's
/// minimum back into its allocation (P1i's lesson).
mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PinnedRows {
        /// Every row's y in the container, keyed by the widget itself.
        pub positions: RefCell<Vec<(gtk::Widget, f64)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PinnedRows {
        const NAME: &'static str = "GimbalPinnedRows";
        type Type = super::PinnedRows;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PinnedRows {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().set_layout_manager(Some(PinnedRowLayout::new()));
        }
    }

    impl WidgetImpl for PinnedRows {}
}

glib::wrapper! {
    pub struct PinnedRows(ObjectSubclass<imp::PinnedRows>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PinnedRows {
    fn new() -> Self {
        glib::Object::new::<Self>()
    }

    fn append_row(&self, row: &gtk::Box, y: f64) {
        row.insert_after(self, self.last_child().as_ref());
        self.set_y(row, y);
    }

    fn set_y(&self, row: &gtk::Box, y: f64) {
        let child: gtk::Widget = row.clone().upcast();
        {
            let mut positions = self.imp().positions.borrow_mut();
            match positions.iter_mut().find(|(widget, _)| *widget == child) {
                Some((_, slot)) => *slot = y,
                None => positions.push((child, y)),
            }
        }
        self.queue_allocate();
    }

    fn y_of(&self, row: &gtk::Box) -> f64 {
        let child: gtk::Widget = row.clone().upcast();
        self.y_of_widget(&child)
    }

    fn y_of_widget(&self, child: &gtk::Widget) -> f64 {
        self.imp()
            .positions
            .borrow()
            .iter()
            .find(|(widget, _)| widget == child)
            .map(|(_, y)| *y)
            .unwrap_or(0.0)
    }

    fn forget(&self, child: &gtk::Widget) {
        self.imp()
            .positions
            .borrow_mut()
            .retain(|(widget, _)| widget != child);
    }

    fn clear_rows(&self) {
        while let Some(child) = self.first_child() {
            self.forget(&child);
            child.unparent();
        }
        self.imp().positions.borrow_mut().clear();
        self.queue_allocate();
    }
}

/// The layout manager behind `PinnedRows`: x is always 0 and the width is the
/// list's full allocation, so labels only ellipsize when the panel itself is
/// too narrow. The y and height come from the row's slot arithmetic. Both
/// orientations report the widest child as the minimum; the height request is
/// the list's one (rows × row height).
mod layout_imp {
    use super::*;

    #[derive(Default)]
    pub struct PinnedRowLayout;

    #[glib::object_subclass]
    impl ObjectSubclass for PinnedRowLayout {
        const NAME: &'static str = "GimbalPinnedRowLayout";
        type Type = super::PinnedRowLayout;
        type ParentType = gtk::LayoutManager;
    }

    impl ObjectImpl for PinnedRowLayout {}

    impl LayoutManagerImpl for PinnedRowLayout {
        fn request_mode(&self, _widget: &gtk::Widget) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::ConstantSize
        }

        fn measure(
            &self,
            widget: &gtk::Widget,
            orientation: gtk::Orientation,
            for_size: i32,
        ) -> (i32, i32, i32, i32) {
            let rows = widget
                .downcast_ref::<PinnedRows>()
                .expect("PinnedRowLayout is only set on PinnedRows");
            let mut minimum = 0;
            let mut natural = 0;
            let mut child = widget.first_child();
            while let Some(widget) = child {
                let (child_min, child_nat, _, _) = widget.measure(orientation, for_size);
                if orientation == gtk::Orientation::Vertical {
                    // The absolute slots make the height the bottom extent.
                    let y = rows.y_of_widget(&widget).round() as i32;
                    minimum = minimum.max(y + child_min);
                    natural = natural.max(y + child_nat);
                } else {
                    // One row's width, not their sum: the list never asks the
                    // panel to grow with its own allocation.
                    minimum = minimum.max(child_min);
                    natural = natural.max(child_nat);
                }
                child = widget.next_sibling();
            }
            (minimum, natural, -1, -1)
        }

        fn allocate(&self, widget: &gtk::Widget, width: i32, _height: i32, _baseline: i32) {
            let rows = widget
                .downcast_ref::<PinnedRows>()
                .expect("PinnedRowLayout is only set on PinnedRows");
            let mut child = widget.first_child();
            while let Some(widget) = child {
                let y = rows.y_of_widget(&widget);
                let (_, natural, _, _) = widget.measure(gtk::Orientation::Vertical, width);
                let height = natural.max(1);
                let transform =
                    gtk::gsk::Transform::new().translate(&gtk::graphene::Point::new(0.0, y as f32));
                widget.allocate(width, height, -1, Some(transform));
                child = widget.next_sibling();
            }
        }
    }
}

glib::wrapper! {
    pub struct PinnedRowLayout(ObjectSubclass<layout_imp::PinnedRowLayout>) @extends gtk::LayoutManager;
}

impl PinnedRowLayout {
    fn new() -> Self {
        glib::Object::new::<Self>()
    }
}

/// The pinned list: one `PinnedRows` container with every row at an absolute
/// position, the way the tab pills and the tiles work. A drag raises the row
/// with `insert_after` (never unparents it), follows the pointer read from
/// surface coordinates, and slides the other rows out of the way. Plain clicks
/// still select.
pub struct PinnedList {
    pub root: gtk::Box,
    rows: PinnedRows,
    names: RefCell<Vec<String>>,
    /// The row widgets in logical slot order (the child order at rest).
    widgets: RefCell<Vec<gtk::Box>>,
    /// One slide generation per row, moved with its widget on a reorder.
    gens: RefCell<Vec<Rc<Cell<u64>>>>,
    selected: Cell<usize>,
    scale: Cell<f64>,
    drag: RefCell<Option<PinDrag>>,
    animations: RefCell<Vec<adw::Animation>>,
    /// The click gesture; a claimed drag denies it for the sequence.
    click: gtk::GestureClick,
}

impl PinnedList {
    pub fn new() -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-pinned");
        let rows = PinnedRows::new();
        rows.set_hexpand(true);
        rows.set_valign(gtk::Align::Start);
        root.append(&rows);
        let click = gtk::GestureClick::new();
        let this = Rc::new(Self {
            root,
            rows,
            names: RefCell::new(DEFAULT_PINS.iter().map(|name| name.to_string()).collect()),
            widgets: RefCell::new(vec![]),
            gens: RefCell::new(vec![]),
            selected: Cell::new(0),
            scale: Cell::new(1.0),
            drag: RefCell::new(None),
            animations: RefCell::new(vec![]),
            click: click.clone(),
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
        drag.connect_drag_end(move |g, dx, dy| {
            if let Some(this) = weak.upgrade() {
                this.end_drag(g, dx, dy);
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
        let click = this.click.clone();
        click.set_button(1);
        click.connect_released(move |_, n_press, _, y| {
            if let Some(this) = weak.upgrade() {
                this.clicked(n_press, y);
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
        let row_h = self.row_h();
        self.animations.borrow_mut().clear();
        let widgets = self.widgets.borrow();
        for (i, row) in widgets.iter().enumerate() {
            self.bump_gen(i);
            self.rows.set_y(row, (i as i32 * row_h) as f64);
        }
        self.rows
            .set_size_request(-1, self.names_len() as i32 * row_h);
    }

    fn rebuild(&self) {
        self.animations.borrow_mut().clear();
        *self.drag.borrow_mut() = None;
        self.rows.clear_rows();
        let names = self.names.borrow().clone();
        let row_h = self.row_h();
        let mut widgets = Vec::with_capacity(names.len());
        let mut gens = Vec::with_capacity(names.len());
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
            self.rows.append_row(&row, (i as i32 * row_h) as f64);
            widgets.push(row);
            gens.push(Rc::new(Cell::new(0)));
        }
        *self.widgets.borrow_mut() = widgets;
        *self.gens.borrow_mut() = gens;
        self.rows.set_size_request(-1, names.len() as i32 * row_h);
    }

    fn bump_gen(&self, row: usize) {
        if let Some(generation) = self.gens.borrow().get(row) {
            generation.set(generation.get() + 1);
        }
    }

    fn clicked(&self, n_press: i32, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || y < 0.0 {
            return;
        }
        let slot = pin_slot(y, row_h, n);
        match pin_release(n, row_h, slot, y, n_press, false) {
            PinRelease::Open { slot } => {
                let name = self.names.borrow()[slot].clone();
                crate::log!("pins", "open {name}");
            }
            _ => self.select(slot),
        }
    }

    /// Highlight `slot` as the current folder.
    fn select(&self, slot: usize) {
        let n = self.names_len();
        if n == 0 {
            return;
        }
        let slot = slot.min(n - 1);
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

    fn begin_drag(&self, x: f64, y: f64) {
        let n = self.names_len();
        let row_h = self.row_h();
        if n == 0 || row_h <= 0 || y < 0.0 {
            return;
        }
        let from = pin_slot(y, row_h, n);
        // The rows' origin in *surface* coordinates: their bounds in the
        // window plus the window's surface transform (the client-side shadow,
        // which GTK subtracts for the gesture's own widget coordinates —
        // gtk-4.22.4:gtkwidget.c:4958-4966). Without the transform the two
        // sides of the threshold comparison sat 25 px apart, so every click
        // claimed and dropped one row lower; see `pin_local`.
        let transform = self
            .rows
            .native()
            .map(|native| native.surface_transform())
            .unwrap_or((0.0, 0.0));
        let bounds = self
            .rows
            .root()
            .and_then(|root| self.rows.compute_bounds(&root))
            .map(|bounds| (bounds.x() as f64, bounds.y() as f64))
            .unwrap_or((0.0, 0.0));
        let grab = y - from as f64 * row_h as f64;
        // Nothing moves on a press: the raise and every layout change wait
        // for the threshold in `update_drag`. A plain click therefore only
        // selects.
        *self.drag.borrow_mut() = Some(PinDrag {
            from,
            grab: grab.round() as i32,
            start: (x, y),
            bounds,
            transform,
            claimed: false,
        });
    }

    fn update_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(mut drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let local = match gesture.current_event().and_then(|event| event.position()) {
            Some((sx, sy)) => pin_local((sx, sy), drag.transform, drag.bounds),
            None => (drag.start.0 + dx, drag.start.1 + dy),
        };
        // Everything here is in the rows' own space, so the press point and
        // the pointer agree and a still click is never a drag.
        let moved = self.rows.drag_check_threshold(
            0,
            0,
            (local.0 - drag.start.0).round() as i32,
            (local.1 - drag.start.1).round() as i32,
        );
        if !drag.claimed && moved && gesture.set_state(gtk::EventSequenceState::Claimed) {
            drag.claimed = true;
            // The drag owns this sequence: the click gesture must not also
            // select a row when the pointer is released.
            self.click.set_state(gtk::EventSequenceState::Denied);
            // Raise the dragged row to the end of the child order (draws above
            // the others). Reordering only; it never unparents.
            let widgets = self.widgets.borrow();
            let row = widgets[drag.from].clone();
            drop(widgets);
            if self.rows.last_child().as_ref() != Some(row.clone().upcast_ref()) {
                let last = self.rows.last_child();
                row.insert_after(&self.rows, last.as_ref());
            }
            self.bump_gen(drag.from);
            crate::log!(
                "pins",
                "dragging '{}' from slot {}",
                self.names.borrow()[drag.from],
                drag.from
            );
            crate::log!("pins", "claimed the sequence after the drag threshold");
        }
        if !drag.claimed {
            // Below the threshold this is still a click, not a reorder.
            *self.drag.borrow_mut() = Some(drag);
            return;
        }
        let n = self.names_len();
        let row_h = self.row_h();
        let (target, y) = pin_drag_layout(n, row_h, drag.from, local.1.round() as i32, drag.grab);
        self.layout_drag(drag.from, &y, row_h);
        crate::log!("pins", "drag over slot {target:?}");
        *self.drag.borrow_mut() = Some(drag);
    }

    /// Put every row at its desired absolute y. The dragged row follows
    /// directly; every other row slides over 150 ms.
    fn layout_drag(&self, dragged: usize, y: &[i32], row_h: i32) {
        let widgets = self.widgets.borrow();
        for (i, row) in widgets.iter().enumerate() {
            let desired = y.get(i).copied().unwrap_or(i as i32 * row_h);
            if i == dragged {
                self.rows.set_y(row, desired as f64);
            } else {
                let current = self.rows.y_of(row).round() as i32;
                if current != desired {
                    self.slide_y(i, row, desired);
                }
            }
        }
    }

    fn slide_y(&self, row: usize, widget: &gtk::Box, to: i32) {
        let start = self.rows.y_of(widget);
        if (start - to as f64).abs() < 0.5 {
            self.rows.set_y(widget, to as f64);
            return;
        }
        let generation_cell = self.gens.borrow()[row].clone();
        let generation = generation_cell.get() + 1;
        generation_cell.set(generation);
        let rows = self.rows.clone();
        let w = widget.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if generation_cell.get() != generation {
                return;
            }
            let y = start + (to as f64 - start) * v;
            rows.set_y(&w, y);
        });
        let anim = adw::TimedAnimation::new(widget, 0.0, 1.0, 150, target);
        self.animations.borrow_mut().push(anim.clone().upcast());
        anim.play();
    }

    fn end_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let local = match gesture.current_event().and_then(|event| event.position()) {
            Some((sx, sy)) => pin_local((sx, sy), drag.transform, drag.bounds),
            None => (drag.start.0 + dx, drag.start.1 + dy),
        };
        let n = self.names_len();
        let row_h = self.row_h();
        if let PinRelease::Reorder { from, to } =
            pin_release(n, row_h, drag.from, local.1, 1, drag.claimed)
        {
            let name = self.names.borrow()[from].clone();
            self.names.borrow_mut().remove(from);
            self.names.borrow_mut().insert(to, name.clone());
            let row = {
                let mut widgets = self.widgets.borrow_mut();
                widgets.remove(from)
            };
            self.widgets.borrow_mut().insert(to, row);
            let generation = {
                let mut gens = self.gens.borrow_mut();
                gens.remove(from)
            };
            self.gens.borrow_mut().insert(to, generation);
            self.selected.set(to);
            crate::log!("pins", "moved '{name}' from slot {from} to {to}");
        }
        self.settle();
    }

    fn cancel_drag(&self) {
        self.drag.borrow_mut().take();
        self.settle();
    }

    /// Put the rows back in logical slot order at their slot y and refresh the
    /// selection class. Reordering the existing children (rather than
    /// rebuilding) keeps the drag path free of unparenting.
    fn settle(&self) {
        self.animations.borrow_mut().clear();
        let row_h = self.row_h();
        let widgets = self.widgets.borrow();
        let mut sibling: Option<gtk::Widget> = None;
        for (i, row) in widgets.iter().enumerate() {
            row.insert_after(&self.rows, sibling.as_ref());
            row.remove_css_class("sel");
            if i == self.selected.get() {
                row.add_css_class("sel");
            }
            self.bump_gen(i);
            self.slide_y(i, row, i as i32 * row_h);
            sibling = Some(row.clone().upcast());
        }
        self.rows
            .set_size_request(-1, self.names_len() as i32 * row_h);
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
            content.add_css_class("gl-section-content");
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
    fn every_pin_reorder_moves_exactly_one_row() {
        for n in 1..=7 {
            for from in 0..n {
                for to in 0..n {
                    let order = pin_order(n, from, to);
                    // A permutation: every row appears once and only once.
                    let mut sorted = order.clone();
                    sorted.sort_unstable();
                    assert_eq!(sorted, (0..n).collect::<Vec<_>>(), "n={n} {from}->{to}");
                    // The dragged row lands at `to`; everything else keeps
                    // its relative order.
                    assert_eq!(order[to], from, "n={n} {from}->{to}");
                    let others: Vec<usize> = order.iter().copied().filter(|&r| r != from).collect();
                    let expected: Vec<usize> = (0..n).filter(|&r| r != from).collect();
                    assert_eq!(others, expected, "n={n} {from}->{to}");
                }
            }
        }
    }

    #[test]
    fn pinned_drag_layout_uses_original_row_indices() {
        // Every row's returned y is its post-move slot, except the dragged
        // row, which rides the pointer.
        let h = 64;
        for from in 0..5usize {
            for to in 0..5usize {
                let (target, y) = pin_drag_layout(5, h, from, to as i32 * h + 10, 10);
                assert_eq!(target, Some(to));
                assert_eq!(y[from], to as i32 * h);
                let order = pin_order(5, from, to);
                for (slot, &row) in order.iter().enumerate() {
                    if row != from {
                        assert_eq!(y[row], slot as i32 * h, "from={from} to={to} row={row}");
                    }
                }
            }
        }
    }

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

    #[test]
    fn a_click_never_reorders() {
        // The press point and the pointer are the same for a still click, at
        // every row and every offset inside it: never a reorder.
        let h = 64;
        let surface_transform = (25.0, 25.0); // the window's CSD shadow
        let bounds = (27.0, 220.0);
        for from in 0..7usize {
            for offset in [0.0, 1.0, 31.5, 63.0] {
                let press = (
                    bounds.0 + 10.0 + surface_transform.0,
                    bounds.1 + from as f64 * 64.0 + offset + surface_transform.1,
                );
                let start = (
                    press.0 - surface_transform.0 - bounds.0,
                    press.1 - surface_transform.1 - bounds.1,
                );
                assert_eq!(pin_slot(start.1, h, 7), from, "from={from} offset={offset}");
                // The release is at the same surface point.
                let local = pin_local(press, surface_transform, bounds);
                let moved = (local.0 - start.0).abs() > 0.5 || (local.1 - start.1).abs() > 0.5;
                assert!(!moved, "still click claimed, from={from} offset={offset}");
                assert_eq!(
                    pin_release(7, h, from, local.1, 1, moved),
                    PinRelease::Select { slot: from },
                    "from={from} offset={offset}"
                );
            }
        }
        // Without the surface transform the comparison sat 25 px apart: that
        // is the bug this test guards.
        let stale_origin = (bounds.0, bounds.1 + surface_transform.1 * 2.0);
        let wrong = pin_local(
            (bounds.0 + 10.0, bounds.1 + 6.0 * 64.0 + 40.0),
            (0.0, 0.0),
            stale_origin,
        );
        let start = (10.0, 6.0 * 64.0 + 40.0);
        assert!(
            (wrong.1 - start.1).abs() > 8.0,
            "the un-transformed read was a drag"
        );
    }

    #[test]
    fn only_a_past_threshold_press_reorders() {
        let h = 64;
        // A real drag down two rows reorders.
        assert_eq!(
            pin_release(7, h, 0, 2.0 * h as f64 + 10.0, 1, true),
            PinRelease::Reorder { from: 0, to: 2 }
        );
        // A drag that ends where it started changes nothing.
        assert_eq!(
            pin_release(7, h, 3, 3.0 * h as f64 + 10.0, 1, true),
            PinRelease::Select { slot: 3 }
        );
        // A double-click opens, and still never reorders.
        assert_eq!(
            pin_release(7, h, 4, 4.0 * h as f64 + 10.0, 2, false),
            PinRelease::Open { slot: 4 }
        );
        assert_eq!(
            pin_release(0, h, 0, 0.0, 1, true),
            PinRelease::Select { slot: 0 }
        );
        assert_eq!(
            pin_release(3, 0, 0, 0.0, 1, true),
            PinRelease::Select { slot: 0 }
        );
    }
}
