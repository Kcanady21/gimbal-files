// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view (Kyle's tab groups): tiles in two columns inside a
//! gtk::Fixed, with identity (a tile's name moves with it), focus,
//! selection, close, the fade and live drag rearranging. The drag gesture
//! rides the static `Fixed` (not the moving pill: every widget's event
//! coordinates are translated into its own space at dispatch, so a gesture
//! on the moving pill would see its offsets collapse — see the notes); the
//! dragged tile follows the pointer and the displaced tiles slide to their
//! new rects over 150 ms.
use crate::style::{CONTROL_H, GAP_2XL, LINE_THIN, scaled};
use adw::prelude::*;
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

/// Tile layout: always 2 columns, `gap-2xl` apart. With exactly 2 tiles each
/// fills the full visible height; with more, each row is half of it and
/// further rows scroll. An odd last tile sits in the left column.
pub fn tile_rects(n: usize, view_w: i32, view_h: i32, scale: f64) -> Vec<Rect> {
    if n == 0 || view_w <= 0 || view_h <= 0 {
        return vec![];
    }
    let gap = scaled(GAP_2XL, scale);
    let w = ((view_w - gap) / 2).max(1);
    let rows = n.div_ceil(2);
    let h = if rows == 1 {
        view_h
    } else {
        ((view_h - gap) / 2).max(1)
    };
    (0..n)
        .map(|i| Rect {
            x: (i % 2) as i32 * (w + gap),
            y: (i / 2) as i32 * (h + gap),
            w,
            h,
        })
        .collect()
}

/// The tile hit test: which tile rect holds the point.
pub fn tile_at(rects: &[Rect], x: i32, y: i32) -> Option<usize> {
    rects.iter().position(|r| r.contains(x, y))
}

/// The pill hit test: which tile's pill region holds the point. The pill
/// spans the tile's full width, starting inside its border and standing
/// `control-h` tall.
pub fn pill_at(rects: &[Rect], x: i32, y: i32, scale: f64) -> Option<usize> {
    let strip = scaled(LINE_THIN, scale) + scaled(CONTROL_H, scale);
    rects
        .iter()
        .position(|r| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + strip)
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
    selected: bool,
}

struct State {
    tiles: Vec<Tile>,       // indexed by slot
    widgets: Vec<gtk::Box>, // indexed by slot
    pills: Vec<gtk::Box>,
    rects: Vec<Rect>,
    focused: usize,
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

/// One slot's shared generation counter: bumping it supersedes the slides
/// of that slot (their ticks compare against the cell's current value).
type SlideGen = Rc<Cell<u64>>;

/// One slot's shared current position, so a running slide survives a
/// reorder: the cell moves with the tile.
type PosCell = Rc<Cell<(f64, f64)>>;

struct Inner {
    outer: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    state: RefCell<State>,
    items: u32,
    fade: RefCell<Option<adw::Animation>>,
    on_exit: Rc<dyn Fn()>,
    /// Self handle, set right after construction (an intentional cycle: the
    /// view lives as long as the window).
    handle: RefCell<Option<SplitView>>,
    /// Per-slot current position (see `PosCell`).
    pos: RefCell<Vec<PosCell>>,
    /// The slide animation per slot and its generation. Replacing a slide
    /// bumps the generation and superseded ticks no-op — a playing
    /// animation keeps itself alive, so dropping the handle isn't enough
    /// (libadwaita adw-animation.c:248).
    slides: RefCell<Vec<Option<adw::Animation>>>,
    slide_gen: RefCell<Vec<SlideGen>>,
    drag: RefCell<Option<Drag>>,
}

/// Cloneable handle; the widgets and model live in the Rc.
#[derive(Clone)]
pub struct SplitView {
    inner: Rc<Inner>,
}

fn opacity_target(outer: &gtk::ScrolledWindow) -> adw::CallbackAnimationTarget {
    let outer = outer.clone();
    adw::CallbackAnimationTarget::new(move |v| outer.set_opacity(v))
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
        outer.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        outer.set_propagate_natural_width(false);
        outer.set_propagate_natural_height(false);
        let this = Self {
            inner: Rc::new(Inner {
                outer,
                fixed,
                items,
                state: RefCell::new(State {
                    tiles: vec![],
                    widgets: vec![],
                    pills: vec![],
                    rects: vec![],
                    focused: 0,
                    scale: 1.0,
                    view: [0, 0],
                }),
                fade: RefCell::new(None),
                on_exit,
                handle: RefCell::new(None),
                pos: RefCell::new(vec![]),
                slides: RefCell::new(vec![]),
                slide_gen: RefCell::new(vec![]),
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

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.outer
    }

    pub fn set_count(&self, n: usize) {
        {
            let mut st = self.inner.state.borrow_mut();
            st.tiles = (1..=n)
                .map(|i| Tile {
                    id: i,
                    name: format!("Tab {i}"),
                    selected: false,
                })
                .collect();
            st.focused = 0;
        }
        self.inner.rebuild();
    }

    pub fn set_scale(&self, scale: f64) {
        self.inner.state.borrow_mut().scale = scale;
        self.inner.relayout();
    }

    pub fn fade_in(&self) {
        let outer = self.inner.outer.clone();
        let target = opacity_target(&outer);
        self.play(adw::TimedAnimation::new(&outer, 0.0, 1.0, 250, target));
    }

    /// Fade the tiles out, then run `done` (the caller switches the stack).
    pub fn fade_out(&self, done: Box<dyn Fn()>) {
        let outer = self.inner.outer.clone();
        let target = opacity_target(&outer);
        let anim = adw::TimedAnimation::new(&outer, 1.0, 0.0, 250, target);
        let weak = Rc::downgrade(&self.inner);
        anim.connect_done(move |_| {
            if let Some(inner) = weak.upgrade() {
                done();
                inner.outer.set_opacity(1.0);
                inner.fade.borrow_mut().take();
            }
        });
        self.play(anim);
    }

    fn play(&self, anim: adw::TimedAnimation) {
        self.inner.fade.replace(Some(anim.upcast()));
        self.inner.fade.borrow().as_ref().unwrap().play();
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
        *self.drag.borrow_mut() = None;
        let items = self.items;
        let tiles: Vec<(usize, String)> = st.tiles.iter().map(|t| (t.id, t.name.clone())).collect();
        let n = tiles.len();
        let (mut pos, mut slides, mut generation) = (
            Vec::with_capacity(n),
            Vec::with_capacity(n),
            Vec::with_capacity(n),
        );
        for (id, name) in &tiles {
            let (tile, pill, close) = Self::tile_widget(name, items);
            Self::attach_handlers(&view, *id, &tile, &pill, &close);
            st.widgets.push(tile.clone());
            st.pills.push(pill);
            self.fixed.put(&tile, 0.0, 0.0);
            pos.push(Rc::new(Cell::new((0.0, 0.0))));
            slides.push(None);
            generation.push(Rc::new(Cell::new(0)));
        }
        let focus = st.focused;
        drop(st);
        *self.pos.borrow_mut() = pos;
        *self.slides.borrow_mut() = slides;
        *self.slide_gen.borrow_mut() = generation;
        self.relayout();
        self.refresh_classes(focus);
    }

    fn relayout(&self) {
        let mut st = self.state.borrow_mut();
        let inset = 2 * scaled(GAP_2XL, st.scale);
        st.rects = tile_rects(
            st.tiles.len(),
            (st.view[0] - inset).max(1),
            (st.view[1] - inset).max(1),
            st.scale,
        );
        // Direct moves from here on: any running slide is superseded.
        for slot in 0..st.widgets.len() {
            self.bump_gen(slot);
            self.slides.borrow_mut()[slot] = None;
        }
        for (slot, (widget, r)) in st.widgets.iter().zip(&st.rects).enumerate() {
            self.fixed.move_(widget, r.x as f64, r.y as f64);
            widget.set_size_request(r.w, r.h);
            self.pos.borrow_mut()[slot].set((r.x as f64, r.y as f64));
        }
    }

    fn refresh_classes(&self, focus: usize) {
        let st = self.state.borrow();
        for (i, (pill, tile)) in st.pills.iter().zip(&st.widgets).enumerate() {
            let selected = st.tiles[i].selected;
            for class in ["active", "selected"] {
                pill.remove_css_class(class);
            }
            if i == focus && !selected {
                pill.add_css_class("active");
            }
            if selected {
                pill.add_css_class("selected");
            }
            tile.remove_css_class("focused");
            if i == focus {
                tile.add_css_class("focused");
            }
        }
    }

    fn tile_widget(name: &str, items: u32) -> (gtk::Box, gtk::Box, gtk::Button) {
        let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
        tile.add_css_class("gl-tile");
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill.add_css_class("gl-pill");
        pill.add_css_class("tab");
        let label = gtk::Label::new(Some(name));
        label.set_hexpand(true);
        pill.append(&label);
        let close = gtk::Button::new();
        close.add_css_class("gl-pill-close");
        close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
        pill.append(&close);
        tile.append(&pill);
        tile.append(&crate::grid::folder_view(items));
        (tile, pill, close)
    }

    fn attach_handlers(
        view: &SplitView,
        id: usize,
        tile: &gtk::Box,
        pill: &gtk::Box,
        close: &gtk::Button,
    ) {
        // Clicking anywhere in a tile focuses it (capture: wins over the grid).
        let click = gtk::GestureClick::new();
        click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let click_view = view.clone();
        click.connect_pressed(move |_, _, _, _| click_view.inner.focus(id));
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
        self.pos.borrow_mut()[slot].set((x, y));
        self.fixed.move_(widget, x, y);
    }

    fn bump_gen(&self, slot: usize) {
        let cell = self.slide_gen.borrow()[slot].clone();
        cell.set(cell.get() + 1);
    }

    /// Slide tile `slot` to `end` over 150 ms, from wherever it is now.
    fn slide_to(&self, slot: usize, widget: &gtk::Box, end: Rect) {
        let start = self.pos.borrow()[slot].get();
        let (ex, ey) = (end.x as f64, end.y as f64);
        if (start.0 - ex).abs() < 0.5 && (start.1 - ey).abs() < 0.5 {
            self.set_pos(slot, ex, ey, widget);
            return;
        }
        self.bump_gen(slot);
        let gen_cell = self.slide_gen.borrow()[slot].clone();
        let generation = gen_cell.get();
        let cell = self.pos.borrow()[slot].clone();
        let fixed = self.fixed.clone();
        let w = widget.clone();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if gen_cell.get() != generation {
                return;
            }
            let p = (start.0 + (ex - start.0) * v, start.1 + (ey - start.1) * v);
            cell.set(p);
            fixed.move_(&w, p.0, p.1);
        });
        let anim = adw::TimedAnimation::new(widget, 0.0, 1.0, 150, target);
        self.slides.borrow_mut()[slot] = Some(anim.clone().upcast());
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
        self.slides.borrow_mut()[slot] = None;
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

    /// Reorder the model and the widget vectors — the widgets themselves
    /// stay (only their slots change) — then settle the dragged tile into
    /// its slot; the displaced ones are already sliding there.
    fn move_tile(&self, from: usize, to: usize) {
        let n = self.state.borrow().tiles.len();
        if from == to || from >= n || to >= n {
            return;
        }
        {
            let mut st = self.state.borrow_mut();
            let tile = st.tiles.remove(from);
            st.tiles.insert(to, tile);
            let widget = st.widgets.remove(from);
            st.widgets.insert(to, widget);
            let pill = st.pills.remove(from);
            st.pills.insert(to, pill);
            st.focused = to;
        }
        {
            let mut pos = self.pos.borrow_mut();
            let cell = pos.remove(from);
            pos.insert(to, cell);
        }
        let (widget, rect) = {
            let st = self.state.borrow();
            (st.widgets[to].clone(), st.rects[to])
        };
        self.refresh_classes(to);
        self.slide_to(to, &widget, rect);
        let name = self.state.borrow().tiles[to].name.clone();
        crate::log!("split", "'{name}' moved from slot {from} to {to}");
    }

    fn focus(&self, id: usize) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        {
            let mut st = self.state.borrow_mut();
            st.focused = slot;
            for t in &mut st.tiles {
                t.selected = false;
            }
        }
        self.refresh_classes(slot);
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
        let prev_focus = self.state.borrow().focused;
        {
            let mut st = self.state.borrow_mut();
            st.focused = slot;
            if ctrl {
                if let Some(t) = st.tiles.get_mut(slot) {
                    t.selected = !t.selected;
                }
            } else if shift {
                let (from, to) = (prev_focus.min(slot), prev_focus.max(slot));
                for t in st.tiles.iter_mut().take(to + 1).skip(from) {
                    t.selected = true;
                }
            } else {
                for t in &mut st.tiles {
                    t.selected = false;
                }
            }
        }
        self.refresh_classes(slot);
    }

    fn popup_menu(&self, id: usize) {
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
        pop.set_parent(&self.outer);
        pop.popup();
    }

    fn close_selected_or(&self, id: usize) {
        let Some(slot) = self.slot_of(id) else {
            return;
        };
        let remaining = {
            let mut st = self.state.borrow_mut();
            let mut targets: Vec<usize> = (0..st.tiles.len())
                .filter(|&i| st.tiles[i].selected)
                .collect();
            if targets.is_empty() {
                targets.push(slot);
            }
            for i in targets.into_iter().rev() {
                if i < st.tiles.len() && st.tiles.len() > 1 {
                    st.tiles.remove(i);
                }
            }
            if st.focused >= st.tiles.len() {
                st.focused = st.tiles.len() - 1;
            }
            st.tiles.len()
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
        assert_eq!(rects[0].h, 800);
        assert_eq!(rects[1].x, 520); // (1000 - 40) / 2 + 40
        assert!(tile_rects(0, 1000, 800, 1.0).is_empty());
        assert!(tile_rects(2, 0, 800, 1.0).is_empty()); // degenerate views
    }

    #[test]
    fn four_tiles_fill_two_rows() {
        let rects = tile_rects(4, 1000, 800, 1.0);
        assert_eq!(rects.len(), 4);
        assert_eq!(rects[0].h, 380); // (800 - 40) / 2
        assert_eq!(rects[2].y + rects[2].h, 800); // rows exactly fill the view
    }

    #[test]
    fn three_tiles_odd_left() {
        let rects = tile_rects(3, 1000, 800, 1.0);
        // The odd last tile sits in the left column of the second row.
        assert_eq!(rects[2].x, 0);
        assert_eq!(rects[2].y, rects[0].h + 40);
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
        let strip = scaled(LINE_THIN, 1.0) + scaled(CONTROL_H, 1.0);
        // The top strip of each tile is its pill; below it is the tile body.
        assert_eq!(pill_at(&rects, 100, strip - 1, 1.0), Some(0));
        assert_eq!(pill_at(&rects, 100, strip, 1.0), None);
        assert_eq!(pill_at(&rects, 600, 10, 1.0), Some(1));
        assert_eq!(pill_at(&rects, 519, 10, 1.0), None); // the gutter
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
