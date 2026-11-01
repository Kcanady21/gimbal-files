// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view (Kyle's tab groups): tiles in two columns inside a
//! gtk::Fixed, with identity (a tile's name moves with it), focus,
//! selection, close, the fade and live drag rearranging. The drag gesture
//! rides the static `Fixed` (not the moving pill: every widget's event
//! coordinates are translated into its own space at dispatch, so a gesture
//! on the moving pill would see its offsets collapse — see the notes); the
//! dragged tile follows the pointer and the displaced tiles slide to their
//! new rects over 150 ms.
use crate::style::{CONTROL_H, GAP_2XL, PILL_W, scaled};
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
}

/// Tile layout: always 2 columns, `gap-2xl` apart, inset `gap-2xl` from
/// every side of the view (the sketch's split panes are centred, so the
/// inset splits evenly left/right and top/bottom). With exactly 2 tiles
/// each fills the full inner height; with more, each row is half of it and
/// further rows scroll. An odd last tile sits in the left column.
pub fn tile_rects(n: usize, view_w: i32, view_h: i32, scale: f64) -> Vec<Rect> {
    if n == 0 || view_w <= 0 || view_h <= 0 {
        return vec![];
    }
    let m = scaled(GAP_2XL, scale);
    let inner_w = (view_w - 2 * m).max(1);
    let inner_h = (view_h - 2 * m).max(1);
    let w = ((inner_w - m) / 2).max(1);
    let rows = n.div_ceil(2);
    let h = if rows == 1 {
        inner_h
    } else {
        ((inner_h - m) / 2).max(1)
    };
    (0..n)
        .map(|i| Rect {
            x: m + (i % 2) as i32 * (w + m),
            y: m + (i / 2) as i32 * (h + m),
            w,
            h,
        })
        .collect()
}

/// The tile hit test: which tile rect holds the point.
pub fn tile_at(rects: &[Rect], x: i32, y: i32) -> Option<usize> {
    rects.iter().position(|r| r.contains(x, y))
}

/// The pill hit test: which tile's pill region holds the point. The pill is
/// `PILL_W` wide, centred above the tile's frame (the sketch), starting at
/// the tile rect's top and `control-h` tall.
pub fn pill_at(rects: &[Rect], x: i32, y: i32, scale: f64) -> Option<usize> {
    let pw = scaled(PILL_W, scale);
    let ph = scaled(CONTROL_H, scale);
    rects.iter().position(|r| {
        let px = r.x + (r.w - pw) / 2;
        x >= px.max(r.x) && x < (px + pw).min(r.x + r.w) && y >= r.y && y < r.y + ph
    })
}

/// Whether a press at (x, y) inside a `tile_w`-wide tile landed on its pill
/// (the same centred `PILL_W × control-h` region as `pill_at`, in the tile's
/// own coordinates). Used by the tile's capture-phase handler: a press on
/// the pill belongs to the pill's selection handler, which needs the
/// pre-press focus and anchor.
pub fn press_on_pill(x: f64, y: f64, tile_w: i32, scale: f64) -> bool {
    if tile_w <= 0 {
        return false;
    }
    let pw = scaled(PILL_W, scale).min(tile_w);
    let px = (tile_w - pw) / 2;
    let ph = scaled(CONTROL_H, scale);
    x >= px as f64 && x < (px + pw) as f64 && y >= 0.0 && y < ph as f64
}

/// Which tiles are focused, which is the Shift anchor, and which are
/// selected. Indexed by slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub focus: usize,
    pub anchor: usize,
    pub selected: Vec<bool>,
}

impl Selection {
    pub fn new(n: usize) -> Self {
        Self {
            focus: 0,
            anchor: 0,
            selected: vec![false; n],
        }
    }
}

/// The selection rules, pure: a plain click focuses its tile and clears the
/// selection, Ctrl toggles the clicked tile, and Shift selects the range
/// from the anchor (the last plain click, i.e. the previous focus) to the
/// clicked tile. Ctrl does not move the anchor, so Ctrl then Shift extends
/// from the last plain click, like a file manager.
pub fn selection_after(state: &Selection, slot: usize, ctrl: bool, shift: bool) -> Selection {
    let mut next = state.clone();
    if slot >= next.selected.len() {
        return next;
    }
    if ctrl {
        next.selected[slot] = !next.selected[slot];
        next.focus = slot;
    } else if shift {
        let (from, to) = (state.anchor.min(slot), state.anchor.max(slot));
        next.selected.fill(false);
        for selected in next.selected.iter_mut().take(to + 1).skip(from) {
            *selected = true;
        }
        next.focus = slot;
    } else {
        next.selected.fill(false);
        next.focus = slot;
        next.anchor = slot;
    }
    next
}

/// Moves the element at `from` to index `to` in place — the one helper every
/// per-slot vector's reorder goes through, so per-slot state always moves
/// with its tile (the r2 fast-drop bug was one vector left behind).
fn slot_move<T>(slots: &mut Vec<T>, from: usize, to: usize) {
    let slot = slots.remove(from);
    slots.insert(to, slot);
}

/// Slot every tile moves to when the tile at `from` is re-inserted at `to`.
fn reorder_map(n: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..n).filter(|&i| i != from).collect();
    order.insert(to, from);
    let mut map = vec![0; n];
    for (slot, &tile) in order.iter().enumerate() {
        map[tile] = slot;
    }
    map
}

/// The live drag layout: the slot the pointer (in fixed coordinates) is
/// over, and, indexed by current slot, the rect every tile sits at while the
/// pointer drags the tile at `from` (whose grab offset inside the tile is
/// `grab`). The dragged tile follows the pointer; once the pointer is over
/// another tile, the others already sit at their post-move rects. Over the
/// gutter or outside the view nothing is displaced.
pub fn drag_layout(
    rects: &[Rect],
    from: usize,
    px: i32,
    py: i32,
    grab: (i32, i32),
) -> (Option<usize>, Vec<Rect>) {
    let mut out = rects.to_vec();
    if from >= rects.len() {
        return (None, out);
    }
    let target = tile_at(rects, px, py);
    if let Some(to) = target.filter(|&to| to != from) {
        let map = reorder_map(rects.len(), from, to);
        for (slot, rect) in out.iter_mut().enumerate() {
            if slot != from {
                *rect = rects[map[slot]];
            }
        }
    }
    let r = rects[from];
    out[from] = Rect {
        x: px - grab.0,
        y: py - grab.1,
        w: r.w,
        h: r.h,
    };
    (target, out)
}

/// One tile's identity: its tab name rides along through every move.
struct Tile {
    id: usize,
    name: String,
}

struct State {
    tiles: Vec<Tile>,       // indexed by slot
    widgets: Vec<gtk::Box>, // indexed by slot: the outer pill+frame box
    frames: Vec<gtk::Box>,  // indexed by slot: the framed pane inside it
    pills: Vec<gtk::Box>,
    rects: Vec<Rect>,
    selection: Selection,
    scale: f64,
    view: [i32; 2],
}

/// The drag in progress: the slot being dragged, the grab offset (press
/// point minus the tile's origin) and the rects the tiles were last told to
/// sit at (so a slide is only (re)started when a tile's target changes).
#[derive(Clone)]
struct Drag {
    from: usize,
    grab: (i32, i32),
    targets: Vec<Rect>,
}

/// Everything one slot owns: its current position, its running slide and
/// that slide's generation. One struct, moved together on a reorder — the
/// r2 fast-drop bug left the generations in the old order, so a settle
/// superseded a displaced tile's still-running slide and stranded it
/// mid-way.
struct Slot {
    pos: Rc<Cell<(f64, f64)>>,
    slide: Option<adw::Animation>,
    generation: Rc<Cell<u64>>,
}

struct Inner {
    outer: gtk::ScrolledWindow,
    /// The crossfade around `outer` (the P1i fade, now a `GtkRevealer`
    /// instead of a widget fade; see P1k).
    revealer: gtk::Revealer,
    fixed: gtk::Fixed,
    state: RefCell<State>,
    items: u32,
    on_exit: Rc<dyn Fn()>,
    /// Self handle, set right after construction (an intentional cycle: the
    /// view lives as long as the window).
    handle: RefCell<Option<SplitView>>,
    /// Per-slot state, indexed by slot (see `Slot`).
    slots: RefCell<Vec<Slot>>,
    drag: RefCell<Option<Drag>>,
}

/// Cloneable handle; the widgets and model live in the Rc.
#[derive(Clone)]
pub struct SplitView {
    inner: Rc<Inner>,
}

/// The viewport size (the adjustment's page size) drives tile_rects; relayout
/// whenever it changes (i.e. on every allocation change).
fn watch_viewport(inner: &Rc<Inner>, axis: usize, adj: &gtk::Adjustment) {
    let weak = Rc::downgrade(inner);
    adj.connect_notify_local(Some("page-size"), move |adj, _| {
        if let Some(inner) = weak.upgrade() {
            inner.state.borrow_mut().view[axis] = adj.page_size() as i32;
            inner.relayout();
        }
    });
}

impl SplitView {
    pub fn new(items: u32, on_exit: Rc<dyn Fn()>) -> Self {
        let fixed = gtk::Fixed::new();
        let outer = gtk::ScrolledWindow::new();
        outer.set_child(Some(&fixed));
        // EXTERNAL horizontally: the tiles always fit by construction and no
        // scrollbar is drawn, and — the real reason — a NEVER policy would
        // make the split view's minimum width the tiles' full extent
        // (gtkscrolledwindow.c:1897), which clamps the sidebars and squeezes
        // the content as the window narrows.
        outer.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        outer.set_propagate_natural_width(false);
        outer.set_propagate_natural_height(false);
        // Crossfade for the P1i fade.
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        revealer.set_transition_duration(250);
        revealer.set_child(Some(&outer));
        revealer.set_reveal_child(false);
        let this = Self {
            inner: Rc::new(Inner {
                outer,
                revealer,
                fixed,
                items,
                state: RefCell::new(State {
                    tiles: vec![],
                    widgets: vec![],
                    frames: vec![],
                    pills: vec![],
                    rects: vec![],
                    selection: Selection::new(0),
                    scale: 1.0,
                    view: [0, 0],
                }),
                on_exit,
                handle: RefCell::new(None),
                slots: RefCell::new(vec![]),
                drag: RefCell::new(None),
            }),
        };
        this.inner.handle.replace(Some(this.clone()));
        watch_viewport(&this.inner, 1, &this.inner.outer.vadjustment());
        watch_viewport(&this.inner, 0, &this.inner.outer.hadjustment());
        // The tile drag: one gesture on the Fixed. It never claims the
        // sequence (gtkgesturedrag.c claims nothing), so plain clicks on the
        // pill and grid selection keep working; drags that don't start on a
        // pill are dropped in begin_drag.
        let drag = gtk::GestureDrag::new();
        let begin_view = this.clone();
        drag.connect_drag_begin(move |_, sx, sy| begin_view.inner.begin_drag(sx, sy));
        let update_view = this.clone();
        drag.connect_drag_update(move |_, dx, dy| update_view.inner.update_drag(dx, dy));
        let end_view = this.clone();
        drag.connect_drag_end(move |_, dx, dy| end_view.inner.end_drag(dx, dy));
        this.inner.fixed.add_controller(drag);
        this
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.inner.revealer
    }

    pub fn set_count(&self, n: usize) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.tiles = (1..=n)
                .map(|i| Tile {
                    id: i,
                    name: format!("Tab {i}"),
                })
                .collect();
            st.selection = Selection::new(n);
        }
        self.inner.rebuild();
    }

    pub fn set_scale(&self, scale: f64) {
        self.inner.state.borrow_mut().scale = scale;
        self.inner.relayout();
    }

    pub fn fade_in(&self) {
        self.inner.revealer.set_reveal_child(true);
    }

    /// Fade the tiles out, then run `done` (the caller switches the stack).
    pub fn fade_out(&self, done: Box<dyn Fn()>) {
        self.inner.revealer.set_reveal_child(false);
        let weak = Rc::downgrade(&self.inner);
        glib::timeout_add_local_once(std::time::Duration::from_millis(250), move || {
            // A new fade-in during the transition cancels the switch.
            if let Some(inner) = weak.upgrade()
                && !inner.revealer.reveals_child()
            {
                done();
            }
        });
    }
}

impl Inner {
    fn slot_of(&self, id: usize) -> Option<usize> {
        self.state.borrow().tiles.iter().position(|t| t.id == id)
    }

    fn rebuild(&self) {
        let view = self.handle.borrow().as_ref().unwrap().clone();
        let mut st = self.state.borrow_mut();
        for widget in st.widgets.drain(..) {
            self.fixed.remove(&widget);
        }
        st.pills.clear();
        st.frames.clear();
        *self.drag.borrow_mut() = None;
        let items = self.items;
        let tiles: Vec<(usize, String)> = st.tiles.iter().map(|t| (t.id, t.name.clone())).collect();
        let mut slots = Vec::with_capacity(tiles.len());
        for (id, name) in &tiles {
            let (tile, frame, pill, close) = Self::tile_widget(name, items);
            Self::attach_handlers(&view, *id, &tile, &pill, &close);
            st.widgets.push(tile.clone());
            st.frames.push(frame);
            st.pills.push(pill);
            self.fixed.put(&tile, 0.0, 0.0);
            slots.push(Slot {
                pos: Rc::new(Cell::new((0.0, 0.0))),
                slide: None,
                generation: Rc::new(Cell::new(0)),
            });
        }
        drop(st);
        *self.slots.borrow_mut() = slots;
        self.relayout();
        self.refresh_classes();
    }

    fn relayout(&self) {
        let mut st = self.state.borrow_mut();
        st.rects = tile_rects(st.tiles.len(), st.view[0], st.view[1], st.scale);
        // Direct moves from here on: any running slide is superseded.
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            slot.generation.set(slot.generation.get() + 1);
            slot.slide = None;
        }
        for (slot, (widget, r)) in st.widgets.iter().zip(&st.rects).enumerate() {
            self.fixed.move_(widget, r.x as f64, r.y as f64);
            widget.set_size_request(r.w, r.h);
            slots[slot].pos.set((r.x as f64, r.y as f64));
        }
    }

    fn refresh_classes(&self) {
        let st = self.state.borrow();
        let focus = st.selection.focus;
        for (i, (pill, frame)) in st.pills.iter().zip(&st.frames).enumerate() {
            let selected = st.selection.selected.get(i).copied().unwrap_or(false);
            for class in ["active", "selected"] {
                pill.remove_css_class(class);
            }
            if i == focus && !selected {
                pill.add_css_class("active");
            }
            if selected {
                pill.add_css_class("selected");
            }
            frame.remove_css_class("focused");
            if i == focus {
                frame.add_css_class("focused");
            }
        }
    }

    /// One tile: the tab pill sits short and centred *above* the framed pane
    /// (the sketch), not across its top edge.
    fn tile_widget(name: &str, items: u32) -> (gtk::Box, gtk::Box, gtk::Box, gtk::Button) {
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill.add_css_class("gl-pill");
        pill.add_css_class("tab");
        pill.set_halign(gtk::Align::Center);
        let label = gtk::Label::new(Some(name));
        label.set_hexpand(true);
        pill.append(&label);
        let close = gtk::Button::new();
        close.add_css_class("gl-pill-close");
        close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
        pill.append(&close);
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("gl-tile");
        frame.set_vexpand(true);
        frame.append(&crate::grid::folder_view(items, true));
        outer.append(&pill);
        outer.append(&frame);
        (outer, frame, pill, close)
    }

    fn attach_handlers(
        view: &SplitView,
        id: usize,
        tile: &gtk::Box,
        pill: &gtk::Box,
        close: &gtk::Button,
    ) {
        // Clicking anywhere in a tile focuses it (capture: wins over the grid)
        // — except on the pill, where the pill's own handler decides with the
        // modifiers: a capture-phase focus here would clear the selection and
        // move the Shift anchor before the pill reads it (live-check 1).
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let click_view = view.clone();
        let click_tile = tile.clone();
        click.connect_pressed(move |_, _, x, y| {
            let scale = click_view.inner.state.borrow().scale;
            if !press_on_pill(x, y, click_tile.width(), scale) {
                click_view.inner.focus(id);
            }
        });
        tile.add_controller(click);
        // Pill: plain/Ctrl/Shift click focuses or selects; right-click opens
        // the menu. Dragging is handled by the Fixed's gesture.
        let pill_click = gtk::GestureClick::new();
        pill_click.set_button(0);
        let pill_view = view.clone();
        pill_click.connect_pressed(move |g, _, _, _| match g.current_button() {
            3 => pill_view.inner.popup_menu(id),
            1 => pill_view.inner.pill_pressed(g, id),
            _ => {}
        });
        pill.add_controller(pill_click);
        let close_view = view.clone();
        close.connect_clicked(move |_| close_view.inner.close_selected_or(id));
    }

    fn set_pos(&self, slot: usize, x: f64, y: f64, widget: &gtk::Box) {
        self.slots.borrow()[slot].pos.set((x, y));
        self.fixed.move_(widget, x, y);
    }

    /// Bump slot `slot`'s generation: its running slide's ticks no-op.
    fn bump_gen(&self, slot: usize) {
        let generation = self.slots.borrow()[slot].generation.clone();
        generation.set(generation.get() + 1);
    }

    /// Slide tile `slot` to `end` over 150 ms, from wherever it is now.
    fn slide_to(&self, slot: usize, widget: &gtk::Box, end: Rect) {
        let start = self.slots.borrow()[slot].pos.get();
        let (ex, ey) = (end.x as f64, end.y as f64);
        if (start.0 - ex).abs() < 0.5 && (start.1 - ey).abs() < 0.5 {
            self.set_pos(slot, ex, ey, widget);
            return;
        }
        let generation_cell = self.slots.borrow()[slot].generation.clone();
        let generation = generation_cell.get() + 1;
        generation_cell.set(generation);
        let cell = self.slots.borrow()[slot].pos.clone();
        let fixed = self.fixed.clone();
        let w = widget.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if generation_cell.get() != generation {
                return;
            }
            let p = (start.0 + (ex - start.0) * v, start.1 + (ey - start.1) * v);
            cell.set(p);
            fixed.move_(&w, p.0, p.1);
        });
        let anim = adw::TimedAnimation::new(widget, 0.0, 1.0, 150, target);
        self.slots.borrow_mut()[slot].slide = Some(anim.clone().upcast());
        anim.play();
    }

    /// A press on a pill began a drag: remember the slot and grab offset and
    /// raise the tile above the others. Reordering keeps the widget parented
    /// (gtk_widget_insert_after reorders in place, gtkwidget.c:12484), so
    /// nothing in the drag path unparents a widget with a live gesture.
    fn begin_drag(&self, sx: f64, sy: f64) {
        let st = self.state.borrow();
        let Some(slot) = pill_at(&st.rects, sx as i32, sy as i32, st.scale) else {
            return;
        };
        let Some(rect) = st.rects.get(slot).copied() else {
            return;
        };
        let Some(widget) = st.widgets.get(slot).cloned() else {
            return;
        };
        let name = st.tiles[slot].name.clone();
        let rects = st.rects.clone();
        drop(st);
        *self.drag.borrow_mut() = Some(Drag {
            from: slot,
            grab: (sx as i32 - rect.x, sy as i32 - rect.y),
            targets: rects,
        });
        // A slide of this slot still running is superseded by the pointer.
        self.bump_gen(slot);
        self.slots.borrow_mut()[slot].slide = None;
        let last = self.fixed.last_child();
        widget.insert_after(&self.fixed, last.as_ref());
        crate::log!("split", "dragging '{name}' from slot {slot}");
    }

    /// The dragged tile follows the pointer; tiles it displaces slide to
    /// their new rects over 150 ms.
    fn update_drag(&self, dx: f64, dy: f64) {
        let Some(mut drag) = self.drag.borrow().clone() else {
            return;
        };
        let (rects, widgets) = {
            let st = self.state.borrow();
            (st.rects.clone(), st.widgets.clone())
        };
        let Some(&rect) = rects.get(drag.from) else {
            return;
        };
        let px = rect.x + drag.grab.0 + dx as i32;
        let py = rect.y + drag.grab.1 + dy as i32;
        let (_target, layout) = drag_layout(&rects, drag.from, px, py, drag.grab);
        for (slot, r) in layout.iter().enumerate() {
            if slot == drag.from {
                self.set_pos(slot, r.x as f64, r.y as f64, &widgets[slot]);
            } else if *r != drag.targets[slot] {
                self.slide_to(slot, &widgets[slot], *r);
            }
        }
        drag.targets = layout;
        *self.drag.borrow_mut() = Some(drag);
    }

    /// Settle: commit the move if the pointer ended on another tile (the
    /// displaced tiles re-slide if they lag behind), else slide the tile
    /// back into its own slot.
    fn end_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let (rects, widgets) = {
            let st = self.state.borrow();
            (st.rects.clone(), st.widgets.clone())
        };
        let Some(&rect) = rects.get(drag.from) else {
            return;
        };
        let px = rect.x + drag.grab.0 + dx as i32;
        let py = rect.y + drag.grab.1 + dy as i32;
        let (target, layout) = drag_layout(&rects, drag.from, px, py, drag.grab);
        for (slot, r) in layout.iter().enumerate() {
            if slot != drag.from && *r != drag.targets[slot] {
                self.slide_to(slot, &widgets[slot], *r);
            }
        }
        match target {
            Some(to) if to != drag.from => self.move_tile(drag.from, to),
            _ => self.slide_to(drag.from, &widgets[drag.from], rects[drag.from]),
        }
        crate::log!("split", "drag ended on slot {target:?}");
    }

    /// Reorder the model, the widget vectors and the per-slot state — one
    /// `slot_move` each, so nothing is left in the old order (the r2
    /// fast-drop bug) — then settle the dragged tile into its slot; the
    /// displaced ones are already sliding there.
    fn move_tile(&self, from: usize, to: usize) {
        let n = self.state.borrow().tiles.len();
        if from == to || from >= n || to >= n {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            slot_move(&mut st.tiles, from, to);
            slot_move(&mut st.widgets, from, to);
            slot_move(&mut st.frames, from, to);
            slot_move(&mut st.pills, from, to);
            slot_move(&mut st.selection.selected, from, to);
            st.selection.focus = to;
            st.selection.anchor = to;
        }
        slot_move(&mut self.slots.borrow_mut(), from, to);
        let (widget, rect) = {
            let st = self.state.borrow();
            (st.widgets[to].clone(), st.rects[to])
        };
        self.refresh_classes();
        self.slide_to(to, &widget, rect);
        let name = self.state.borrow().tiles[to].name.clone();
        crate::log!("split", "'{name}' moved from slot {from} to {to}");
    }

    /// A plain press anywhere in a tile: focus it and clear the selection.
    fn focus(&self, id: usize) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        self.apply_selection(slot, false, false);
    }

    fn pill_pressed(&self, g: &gtk::GestureClick, id: usize) {
        let state = g.current_event_state();
        let ctrl = state.contains(gtk::gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gtk::gdk::ModifierType::SHIFT_MASK);
        self.pill_click(id, ctrl, shift);
    }

    fn pill_click(&self, id: usize, ctrl: bool, shift: bool) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        self.apply_selection(slot, ctrl, shift);
    }

    /// Run the pure selection rules from `selection_after` and repaint.
    fn apply_selection(&self, slot: usize, ctrl: bool, shift: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.selection = selection_after(&st.selection, slot, ctrl, shift);
        }
        self.refresh_classes();
    }

    /// The right-click menu: parented to the clicked pill, so it opens at the
    /// pill the press landed on (live-check 2).
    fn popup_menu(&self, id: usize) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        let Some(pill) = self.state.borrow().pills.get(slot).cloned() else {
            return;
        };
        let pop = gtk::Popover::new();
        let button = gtk::Button::with_label("Close");
        let view = self.handle.borrow().as_ref().unwrap().clone();
        let (pop_click, pop_gone) = (pop.clone(), pop.clone());
        button.connect_clicked(move |_| {
            pop_click.popdown();
            view.inner.close_selected_or(id);
        });
        pop.set_child(Some(&button));
        pop.connect_closed(move |_| pop_gone.unparent());
        pop.set_parent(&pill);
        pop.popup();
    }

    fn close_selected_or(&self, id: usize) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        let remaining = {
            let mut st = self.state.borrow_mut();
            let mut targets: Vec<usize> = (0..st.tiles.len())
                .filter(|&i| st.selection.selected.get(i).copied().unwrap_or(false))
                .collect();
            if targets.is_empty() {
                targets.push(slot);
            }
            for i in targets.into_iter().rev() {
                if i < st.tiles.len() && st.tiles.len() > 1 {
                    st.tiles.remove(i);
                }
            }
            let remaining = st.tiles.len();
            let focus = st.selection.focus.min(remaining - 1);
            st.selection = Selection::new(remaining);
            st.selection.focus = focus;
            st.selection.anchor = focus;
            remaining
        };
        if remaining == 1 {
            (self.on_exit)();
        } else {
            self.rebuild();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_tiles_fill_the_height() {
        let rects = tile_rects(2, 1000, 800, 1.0);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].h, 720); // 800 − 2 × 40 inset
        assert_eq!(rects[0].x, 40); // the inset splits evenly
        assert_eq!(rects[1].x, 520); // 40 + (920 − 40) / 2 + 40
        assert!(tile_rects(0, 1000, 800, 1.0).is_empty());
        assert!(tile_rects(2, 0, 800, 1.0).is_empty()); // degenerate views
    }

    #[test]
    fn four_tiles_fill_two_rows() {
        let rects = tile_rects(4, 1000, 800, 1.0);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[0].h, 340); // (800 − 2 × 40 − 40) / 2
        assert_eq!(rects[2].y + rects[2].h, 760); // rows fill down to the inset
    }

    #[test]
    fn three_tiles_odd_left() {
        let rects = tile_rects(3, 1000, 800, 1.0);
        // The odd last tile sits in the left column of the second row.
        assert_eq!(rects[2].x, 40);
        assert_eq!(rects[2].y, rects[0].h + 80);
    }

    #[test]
    fn six_tiles_scroll() {
        let rects = tile_rects(6, 1000, 800, 1.0);
        assert_eq!(rects.len(), 6);
        assert!(rects[4].y + rects[4].h > 800); // last row overflows
        assert_eq!(rects[5].x, 520);
    }

    #[test]
    fn hit_test() {
        let rects = tile_rects(4, 1000, 800, 1.0);
        // Inside each quadrant of the 2×2 arrangement.
        assert_eq!(tile_at(&rects, 100, 100), Some(0));
        assert_eq!(tile_at(&rects, 600, 100), Some(1));
        assert_eq!(tile_at(&rects, 100, 500), Some(2));
        assert_eq!(tile_at(&rects, 600, 500), Some(3));
        // The gap-2xl gutter between the columns and rows holds no tile.
        assert_eq!(tile_at(&rects, 519, 100), None);
        assert_eq!(tile_at(&rects, 100, 419), None);
        // Outside the view entirely.
        assert_eq!(tile_at(&rects, -5, 100), None);
        assert_eq!(tile_at(&rects, 1500, 1500), None);
    }

    #[test]
    fn pill_hit_test() {
        let rects = tile_rects(4, 1000, 800, 1.0);
        let r = rects[0];
        let px = r.x + (r.w - scaled(PILL_W, 1.0)) / 2; // centred pill
        let ph = scaled(CONTROL_H, 1.0);
        assert_eq!(pill_at(&rects, px + 10, r.y + 10, 1.0), Some(0));
        // Left of the centred pill is the tile body, not the pill.
        assert_eq!(pill_at(&rects, r.x + 10, r.y + 10, 1.0), None);
        // Below the pill strip too.
        assert_eq!(pill_at(&rects, px + 10, r.y + ph, 1.0), None);
        // The gutter between the columns holds no pill.
        assert_eq!(pill_at(&rects, 519, r.y + 10, 1.0), None);
        assert_eq!(
            pill_at(&rects, rects[1].x + (rects[1].w - 270) / 2, r.y + 10, 1.0),
            Some(1)
        );
    }

    #[test]
    fn plain_click_clears_and_focuses() {
        let mut state = Selection::new(4);
        state.selected = vec![true, true, true, true];
        state.focus = 3;
        state.anchor = 1;
        let next = selection_after(&state, 2, false, false);
        assert_eq!(next.focus, 2);
        assert_eq!(next.anchor, 2);
        assert_eq!(next.selected, [false, false, false, false]);
    }

    #[test]
    fn ctrl_click_adds_and_removes() {
        let mut state = Selection::new(4);
        state.focus = 0;
        state.anchor = 0;
        let added = selection_after(&state, 2, true, false);
        assert_eq!(added.selected, [false, false, true, false]);
        assert_eq!((added.focus, added.anchor), (2, 0));
        let second = selection_after(&added, 3, true, false);
        assert_eq!(second.selected, [false, false, true, true]);
        let removed = selection_after(&second, 2, true, false);
        assert_eq!(removed.selected, [false, false, false, true]);
        let empty = selection_after(&removed, 3, true, false);
        assert_eq!(empty.selected, [false, false, false, false]);
        // Ctrl never moves the Shift anchor.
        assert_eq!(empty.anchor, 0);
    }

    #[test]
    fn shift_click_selects_the_range_from_the_previous_focus() {
        let mut state = Selection::new(5);
        // A plain click on slot 0 is the previous focus and the anchor.
        state = selection_after(&state, 0, false, false);
        let range = selection_after(&state, 3, false, true);
        assert_eq!(range.selected, [true, true, true, true, false]);
        assert_eq!(range.focus, 3);
        assert_eq!(range.anchor, 0);
        // Backwards from the anchor works too, and replaces the selection.
        let back = selection_after(&range, 1, false, true);
        assert_eq!(back.selected, [true, true, false, false, false]);
        assert_eq!(back.focus, 1);
    }

    #[test]
    fn tile_press_on_the_pill_is_not_a_plain_focus() {
        // Pill 270 wide, centred in a 500 px tile at scale 1.
        assert!(press_on_pill(200.0, 10.0, 500, 1.0));
        assert!(!press_on_pill(100.0, 10.0, 500, 1.0)); // left of it
        assert!(!press_on_pill(200.0, 60.0, 500, 1.0)); // below it
        // At 0.5 the pill is 135 wide and the region halves.
        assert!(press_on_pill(100.0, 10.0, 250, 0.5));
        assert!(!press_on_pill(50.0, 10.0, 250, 0.5));
        // A tile narrower than the pill uses the whole tile width.
        assert!(press_on_pill(50.0, 10.0, 100, 1.0));
        assert!(!press_on_pill(-1.0, 10.0, 100, 1.0));
    }

    /// A test double of `Slot`: whatever a slot owns must follow its tile.
    #[derive(Debug, PartialEq)]
    struct TestSlot {
        tile: usize,
        generation: u64,
        pos: (f64, f64),
    }

    #[test]
    fn reorder_moves_every_per_slot_field_with_the_tile() {
        let mut slots: Vec<TestSlot> = (0..4)
            .map(|i| TestSlot {
                tile: i,
                generation: 10 * i as u64,
                pos: (i as f64, -(i as f64)),
            })
            .collect();
        // Tab 1 dragged onto Tab 4: its whole slot state moves to the end.
        slot_move(&mut slots, 0, 3);
        assert_eq!(
            slots[3],
            TestSlot {
                tile: 0,
                generation: 0,
                pos: (0.0, 0.0)
            }
        );
        assert_eq!(slots[0].tile, 1);
        assert_eq!(slots[2].tile, 3);
        // And a second move keeps every field with its tile.
        slot_move(&mut slots, 2, 1);
        let order = [1usize, 3, 2, 0];
        for (slot, s) in slots.iter().enumerate() {
            assert_eq!(s.tile, order[slot], "slot {slot}");
            assert_eq!(s.generation, 10 * order[slot] as u64, "slot {slot}");
            assert_eq!(s.pos, (order[slot] as f64, -(order[slot] as f64)));
        }
    }

    /// Slot of every tile after the tile at `from` is re-inserted at `to`.
    fn moved(n: usize, from: usize, to: usize) -> Vec<usize> {
        let mut v: Vec<usize> = (0..n).collect();
        let t = v.remove(from);
        v.insert(to, t);
        v
    }

    #[test]
    fn drag_moves_forward() {
        for n in [2usize, 3, 6] {
            let rects = tile_rects(n, 1000, 800, 1.0);
            let r1 = rects[1];
            let (target, layout) = drag_layout(&rects, 0, r1.x + 10, r1.y + 10, (0, 0));
            assert_eq!(target, Some(1), "n={n}");
            // The dragged tile follows the pointer…
            assert_eq!(
                layout[0],
                Rect {
                    x: r1.x + 10,
                    y: r1.y + 10,
                    ..rects[0]
                },
                "n={n}"
            );
            // …and every other tile sits where the move puts it.
            let order = moved(n, 0, 1);
            for (slot, r) in layout.iter().enumerate().skip(1) {
                let p = order.iter().position(|&t| t == slot).unwrap();
                assert_eq!(*r, rects[p], "n={n} slot={slot}");
            }
        }
    }

    #[test]
    fn drag_moves_backward() {
        for n in [2usize, 3, 6] {
            let rects = tile_rects(n, 1000, 800, 1.0);
            let last = n - 1;
            let r0 = rects[0];
            let (target, layout) = drag_layout(&rects, last, r0.x + 10, r0.y + 10, (0, 0));
            assert_eq!(target, Some(0), "n={n}");
            let order = moved(n, last, 0);
            for (slot, r) in layout.iter().enumerate().take(last) {
                let p = order.iter().position(|&t| t == slot).unwrap();
                assert_eq!(*r, rects[p], "n={n} slot={slot}");
            }
            assert_eq!(layout[last].x, r0.x + 10, "n={n}");
        }
    }

    #[test]
    fn drag_over_gutter_and_outside_moves_nothing() {
        for n in [2usize, 3, 6] {
            let rects = tile_rects(n, 1000, 800, 1.0);
            // The gap-2xl gutter between the columns holds no tile.
            let gx = (rects[0].x + rects[0].w + rects[1].x) / 2;
            let (target, layout) = drag_layout(&rects, 0, gx, rects[0].y + 10, (0, 0));
            assert_eq!(target, None, "n={n}");
            for slot in 1..n {
                assert_eq!(layout[slot], rects[slot], "n={n}");
            }
            // Outside the view entirely: still nothing displaced.
            let (target, layout) = drag_layout(&rects, 0, -5, rects[0].y + 10, (0, 0));
            assert_eq!(target, None, "n={n}");
            for slot in 1..n {
                assert_eq!(layout[slot], rects[slot], "n={n}");
            }
            assert!(
                layout[0].x < 0,
                "n={n}: the dragged tile follows the pointer"
            );
        }
    }

    #[test]
    fn drag_respects_the_grab_offset() {
        let rects = tile_rects(3, 1000, 800, 1.0);
        let (target, layout) = drag_layout(&rects, 0, 600, 300, (30, 20));
        assert_eq!(target, Some(1));
        // The tile rides grab-sized below-left of the pointer.
        assert_eq!(
            layout[0],
            Rect {
                x: 570,
                y: 280,
                ..rects[0]
            }
        );
    }
}
