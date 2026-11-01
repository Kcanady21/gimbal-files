// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view (Kyle's tab groups): tiles in two columns inside a
//! gtk::Fixed, with identity (a tile's name moves with it), focus,
//! selection, close, the fade and live drag rearranging. The drag gesture
//! rides the static `Fixed` (not the moving pill: every widget's event
//! coordinates are translated into its own space at dispatch, so a gesture
//! on the moving pill would see its offsets collapse — see the notes); the
//! dragged tile follows the pointer and the displaced tiles slide to their
//! new rects over 150 ms.
use crate::style::{CONTROL_H, GAP_2XL, GAP_L, GAP_S, PILL_W, scaled};
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

/// The two columns' tile ids, left first, top to bottom. The columns own
/// their tiles: a tile belongs to the column it is in, and a column's tiles
/// share that column's height however many there are. New tiles go to the
/// shorter column (the left one on a tie), so the first distribution is
/// 1 = 1+0, 2 = 1+1, 3 = 2+1, 4 = 2+2, 5 = 3+2, 6 = 3+3.
pub fn add_to_shorter(columns: &mut [Vec<usize>; 2], tile: usize) -> usize {
    let column = if columns[0].len() <= columns[1].len() {
        0
    } else {
        1
    };
    columns[column].push(tile);
    column
}

/// The initial distribution of `n` tiles by `add_to_shorter`.
pub fn initial_columns(n: usize) -> [Vec<usize>; 2] {
    let mut columns = [vec![], vec![]];
    for tile in 0..n {
        add_to_shorter(&mut columns, tile);
    }
    columns
}

/// Remove `tile` from its column and renumber the surviving ids, which are
/// kept contiguous by the caller's parallel vectors. A column that loses a
/// tile becomes shorter, so its remaining tiles grow.
pub fn columns_without(columns: &[Vec<usize>; 2], tile: usize) -> [Vec<usize>; 2] {
    let mut out = columns.clone();
    for column in &mut out {
        column.retain(|&id| id != tile);
        for id in column.iter_mut() {
            if *id > tile {
                *id -= 1;
            }
        }
    }
    out
}

/// The column counts, left first.
pub fn column_counts(columns: &[Vec<usize>; 2]) -> (usize, usize) {
    (columns[0].len(), columns[1].len())
}

/// The columns after dropping the tile `from` on the tile `to`. Within one
/// column this reorders it (the dragged tile takes the target's row); across
/// columns the two tiles swap slots, so each ends at the other's size. `None`
/// means the drop changes nothing.
pub fn columns_after_drop(
    columns: &[Vec<usize>; 2],
    from: usize,
    to: usize,
) -> Option<[Vec<usize>; 2]> {
    if from == to {
        return None;
    }
    let locate = |tile: usize| {
        columns.iter().enumerate().find_map(|(column, tiles)| {
            tiles
                .iter()
                .position(|&id| id == tile)
                .map(|row| (column, row))
        })
    };
    let (from_column, from_row) = locate(from)?;
    let (to_column, to_row) = locate(to)?;
    let mut out = columns.clone();
    if from_column == to_column {
        out[from_column].remove(from_row);
        out[from_column].insert(to_row.min(out[from_column].len()), from);
    } else {
        out[from_column][from_row] = to;
        out[to_column][to_row] = from;
    }
    Some(out)
}

/// The rect of every slot, left column first. Two columns `gap-l` apart,
/// inset `gap-2xl` from the view's sides and bottom but flush with its top,
/// so the first row of pills has its top edge level with the side panels. A
/// slot is the tile's pill plus its frame: the pill is `control-h` tall and
/// sits `gap-s` above the frame. Each column's slots share the visible height
/// equally (`inner_h`), so a column with fewer tiles gets taller ones,
/// whichever column that is. Beyond 4 tiles a slot is never shorter than half
/// the visible height, so the view scrolls instead of shrinking tiles further.
pub fn column_rects(counts: [usize; 2], view_w: i32, view_h: i32, scale: f64) -> [Vec<Rect>; 2] {
    if counts == [0, 0] || view_w <= 0 || view_h <= 0 {
        return [vec![], vec![]];
    }
    let inset = scaled(GAP_2XL, scale);
    let gap = scaled(GAP_L, scale);
    let pill = scaled(CONTROL_H, scale);
    let pill_gap = scaled(GAP_S, scale);
    let inner_w = (view_w - 2 * inset).max(1);
    let inner_h = (view_h - inset).max(1);
    let w = ((inner_w - gap) / 2).max(1);
    // A slot is pill + gap-s + frame; half the visible height is the floor
    // beyond four tiles (the height a two-tile column would have).
    let min_slot = if counts[0] + counts[1] > 4 {
        ((inner_h - gap) / 2).max(1)
    } else {
        0
    };
    let slot_h = |count: usize| -> i32 {
        if count == 0 {
            return 0;
        }
        let shared = (inner_h - (count as i32 - 1) * gap) / count as i32;
        shared.max(min_slot).max(pill + pill_gap + 1)
    };
    [0, 1].map(|column| {
        let h = slot_h(counts[column]);
        (0..counts[column])
            .map(|row| Rect {
                x: inset + column as i32 * (w + gap),
                y: row as i32 * (h + gap),
                w,
                h,
            })
            .collect()
    })
}

/// The rect of every tile, indexed by tile id, from its column and row.
pub fn rects_by_tile(columns: &[Vec<usize>; 2], view_w: i32, view_h: i32, scale: f64) -> Vec<Rect> {
    let counts = [columns[0].len(), columns[1].len()];
    if counts == [0, 0] || view_w <= 0 || view_h <= 0 {
        return vec![];
    }
    let slots = column_rects(counts, view_w, view_h, scale);
    let mut out = vec![
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        counts[0] + counts[1]
    ];
    for column in 0..2 {
        for (row, &tile) in columns[column].iter().enumerate() {
            if tile < out.len() {
                out[tile] = slots[column][row];
            }
        }
    }
    out
}

/// A selection index after tile `removed` is deleted from a vector of `length`
/// entries: earlier indices stay, the removed one moves to its neighbour, and
/// later ones shift down.
fn adjusted(index: usize, removed: usize, length: usize) -> usize {
    if index > removed {
        index - 1
    } else if index == removed {
        removed.min(length.saturating_sub(1))
    } else {
        index
    }
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

/// The live drag layout: the tile the pointer (in fixed coordinates) is over,
/// and, indexed by tile id, the rect every tile sits at while the pointer
/// drags the tile at `from` (whose grab offset inside the tile is `grab`).
/// The dragged tile follows the pointer at its old size; the others already
/// sit at their post-drop rects, including the size of the slot they land in.
/// Over the gutter or outside the view nothing is displaced.
pub fn drag_layout(
    columns: &[Vec<usize>; 2],
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
    let mut target = tile_at(rects, px, py).filter(|&to| to != from);
    if let Some(to) = target {
        let Some(next) = columns_after_drop(columns, from, to) else {
            target = None;
            return (target, out);
        };
        // The slots themselves do not change when a drop swaps or reorders
        // tiles, so the current rects already hold every slot's rect.
        let mut slots: [Vec<Rect>; 2] = [vec![], vec![]];
        for column in 0..2 {
            slots[column] = columns[column].iter().map(|&tile| rects[tile]).collect();
        }
        for column in 0..2 {
            for (row, &tile) in next[column].iter().enumerate() {
                out[tile] = slots[column][row];
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

/// One tile's identity: its tab name rides along through every move. The uid
/// never changes, so a handler captured at construction survives a close
/// renumbering the tile vectors.
struct Tile {
    uid: usize,
    name: String,
}

struct State {
    tiles: Vec<Tile>,         // indexed by tile id
    columns: [Vec<usize>; 2], // column -> tile ids, top to bottom
    widgets: Vec<gtk::Box>,   // indexed by tile id: the outer pill+frame box
    frames: Vec<gtk::Box>,    // indexed by tile id: the framed pane inside it
    pills: Vec<gtk::Box>,
    rects: Vec<Rect>, // indexed by tile id
    selection: Selection,
    scale: f64,
    view: [i32; 2],
    next_uid: usize,
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

/// Everything one tile owns visually: its current position and size, its
/// running slide and that slide's generation. One struct, moved together with
/// the tile — the r2 fast-drop bug left the generations in the old order, so
/// a settle superseded a displaced tile's still-running slide and stranded it
/// mid-way.
struct Slot {
    pos: Rc<Cell<(f64, f64)>>,
    size: Rc<Cell<(i32, i32)>>,
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
            inner.relayout(false);
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
                    columns: [vec![], vec![]],
                    widgets: vec![],
                    frames: vec![],
                    pills: vec![],
                    rects: vec![],
                    selection: Selection::new(0),
                    scale: 1.0,
                    view: [0, 0],
                    next_uid: 0,
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
        let current = self.inner.state.borrow().tiles.len();
        if current == 0 {
            self.inner.build(n);
        } else if n > current {
            for _ in current..n {
                self.inner.add_tile();
            }
        } else if n < current {
            while self.inner.state.borrow().tiles.len() > n.max(1) {
                let index = self.inner.last_closable_index();
                self.inner.remove_tile(index);
            }
        }
        self.inner.relayout(true);
        self.inner.refresh_classes();
    }

    pub fn set_scale(&self, scale: f64) {
        self.inner.state.borrow_mut().scale = scale;
        self.inner.relayout(false);
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
    fn slot_of(&self, uid: usize) -> Option<usize> {
        self.state.borrow().tiles.iter().position(|t| t.uid == uid)
    }

    /// Remove every tile and build `n` fresh ones (the first build, before the
    /// view has widgets). Later count changes add or close one tile at a time
    /// so the survivors can animate.
    fn build(&self, n: usize) {
        let view = self.handle.borrow().as_ref().unwrap().clone();
        {
            let mut st = self.state.borrow_mut();
            for widget in st.widgets.drain(..) {
                self.fixed.remove(&widget);
            }
            st.tiles.clear();
            st.columns = [vec![], vec![]];
            st.pills.clear();
            st.frames.clear();
            st.rects.clear();
            st.selection = Selection::new(0);
            st.next_uid = 0;
            *self.drag.borrow_mut() = None;
        }
        *self.slots.borrow_mut() = vec![];
        for _ in 0..n {
            self.add_one(&view, false);
        }
        // The pure initial distribution is the model of record.
        self.state.borrow_mut().columns = initial_columns(n);
        self.relayout(false);
        self.refresh_classes();
    }

    /// Add one tile to the shorter column, keeping every existing tile.
    fn add_tile(&self) {
        let view = self.handle.borrow().as_ref().unwrap().clone();
        self.add_one(&view, true);
        self.relayout(true);
        self.refresh_classes();
    }

    fn add_one(&self, view: &SplitView, animate: bool) {
        let items = self.items;
        let (index, name, tile) = {
            let mut st = self.state.borrow_mut();
            let index = st.tiles.len();
            let uid = st.next_uid;
            st.next_uid += 1;
            let name = format!("Tab {}", uid + 1);
            let (tile, frame, pill, close) = Self::tile_widget(&name, items);
            Self::attach_handlers(view, uid, &tile, &pill, &close);
            st.tiles.push(Tile {
                uid,
                name: name.clone(),
            });
            add_to_shorter(&mut st.columns, index);
            st.widgets.push(tile.clone());
            st.frames.push(frame);
            st.pills.push(pill);
            st.selection.selected.push(false);
            (index, name, tile)
        };
        let rect = {
            let st = self.state.borrow();
            rects_by_tile(&st.columns, st.view[0], st.view[1], st.scale)
                .get(index)
                .copied()
                .unwrap_or(Rect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                })
        };
        self.fixed.put(&tile, rect.x as f64, rect.y as f64);
        tile.set_size_request(rect.w, rect.h);
        self.slots.borrow_mut().push(Slot {
            pos: Rc::new(Cell::new((rect.x as f64, rect.y as f64))),
            size: Rc::new(Cell::new((rect.w, rect.h))),
            slide: None,
            generation: Rc::new(Cell::new(0)),
        });
        if animate {
            crate::log!("split", "added '{name}' at slot {index}");
        }
    }

    /// The last tile a count reduction should close: from the longer column,
    /// so the count stays balanced.
    fn last_closable_index(&self) -> usize {
        let st = self.state.borrow();
        let (left, right) = column_counts(&st.columns);
        let column = if right > left { 1 } else { 0 };
        st.columns[column]
            .last()
            .copied()
            .unwrap_or_else(|| st.tiles.len().saturating_sub(1))
    }

    /// Close one tile in place: its widget leaves the Fixed (there is no live
    /// gesture on it), the survivors keep their widgets and animate to the
    /// column's new rects.
    fn remove_tile(&self, index: usize) {
        let (widget, name) = {
            let st = self.state.borrow();
            let Some(widget) = st.widgets.get(index).cloned() else {
                return;
            };
            (widget, st.tiles[index].name.clone())
        };
        self.bump_gen(index);
        self.fixed.remove(&widget);
        {
            let mut st = self.state.borrow_mut();
            st.columns = columns_without(&st.columns, index);
            st.tiles.remove(index);
            st.widgets.remove(index);
            st.frames.remove(index);
            st.pills.remove(index);
            if index < st.selection.selected.len() {
                st.selection.selected.remove(index);
            }
            let length = st.selection.selected.len();
            st.selection.focus = adjusted(st.selection.focus, index, length);
            st.selection.anchor = adjusted(st.selection.anchor, index, length);
            st.rects = rects_by_tile(&st.columns, st.view[0], st.view[1], st.scale);
        }
        if index < self.slots.borrow().len() {
            self.slots.borrow_mut().remove(index);
        }
        crate::log!("split", "closed '{name}' from slot {index}");
    }

    /// Recompute every tile's rect and move it there: immediately for a
    /// viewport/scale change, over 200 ms for a close, an add or a drop.
    fn relayout(&self, animate: bool) {
        let (rects, widgets) = {
            let mut st = self.state.borrow_mut();
            st.rects = rects_by_tile(&st.columns, st.view[0], st.view[1], st.scale);
            (st.rects.clone(), st.widgets.clone())
        };
        for (slot, widget) in widgets.iter().enumerate() {
            if let Some(&rect) = rects.get(slot) {
                if animate {
                    self.slide_to(slot, widget, rect);
                } else {
                    self.set_rect(slot, widget, rect);
                }
            }
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
        // A Box child fills its cross axis by default: without these the ring
        // stretched to the pill's 44 px content height and drew as an oval.
        close.set_halign(gtk::Align::Center);
        close.set_valign(gtk::Align::Center);
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

    /// Move and size tile `slot` immediately.
    fn set_visual(&self, slot: usize, widget: &gtk::Box, x: f64, y: f64, w: i32, h: i32) {
        if let Some(state) = self.slots.borrow().get(slot) {
            state.pos.set((x, y));
            state.size.set((w, h));
        }
        self.fixed.move_(widget, x, y);
        widget.set_size_request(w, h);
    }

    /// Bump slot `slot`'s generation: its running slide's ticks no-op.
    fn bump_gen(&self, slot: usize) {
        let generation = self.slots.borrow()[slot].generation.clone();
        generation.set(generation.get() + 1);
    }

    fn set_rect(&self, slot: usize, widget: &gtk::Box, rect: Rect) {
        self.bump_gen(slot);
        self.slots.borrow_mut()[slot].slide = None;
        self.set_visual(slot, widget, rect.x as f64, rect.y as f64, rect.w, rect.h);
    }

    /// Slide tile `slot` to `end` over 200 ms, from wherever it is now; the
    /// size travels with the position, so a close's growth is smooth too.
    fn slide_to(&self, slot: usize, widget: &gtk::Box, end: Rect) {
        let (start, size) = {
            let slots = self.slots.borrow();
            (slots[slot].pos.get(), slots[slot].size.get())
        };
        let (ex, ey) = (end.x as f64, end.y as f64);
        if (start.0 - ex).abs() < 0.5 && (start.1 - ey).abs() < 0.5 && size == (end.w, end.h) {
            self.set_rect(slot, widget, end);
            return;
        }
        let generation_cell = self.slots.borrow()[slot].generation.clone();
        let generation = generation_cell.get() + 1;
        generation_cell.set(generation);
        let (cell, size_cell) = {
            let slots = self.slots.borrow();
            (slots[slot].pos.clone(), slots[slot].size.clone())
        };
        let fixed = self.fixed.clone();
        let w = widget.clone();
        let (start_w, start_h) = size;
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if generation_cell.get() != generation {
                return;
            }
            let x = start.0 + (ex - start.0) * v;
            let y = start.1 + (ey - start.1) * v;
            let width = (start_w as f64 + (end.w - start_w) as f64 * v).round() as i32;
            let height = (start_h as f64 + (end.h - start_h) as f64 * v).round() as i32;
            cell.set((x, y));
            size_cell.set((width.max(1), height.max(1)));
            fixed.move_(&w, x, y);
            w.set_size_request(width.max(1), height.max(1));
        });
        let anim = adw::TimedAnimation::new(widget, 0.0, 1.0, 200, target);
        self.slots.borrow_mut()[slot].slide = Some(anim.clone().upcast());
        anim.play();
    }

    /// A press on a pill began a drag: remember the tile and grab offset and
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
        if self.fixed.last_child().as_ref() != Some(widget.upcast_ref()) {
            let last = self.fixed.last_child();
            widget.insert_after(&self.fixed, last.as_ref());
        }
        crate::log!("split", "dragging '{name}' from slot {slot}");
    }

    /// The dragged tile follows the pointer at its old size; tiles it
    /// displaces slide to their post-drop rects over 200 ms, size included.
    fn update_drag(&self, dx: f64, dy: f64) {
        let Some(mut drag) = self.drag.borrow().clone() else {
            return;
        };
        let (columns, rects, widgets) = {
            let st = self.state.borrow();
            (st.columns.clone(), st.rects.clone(), st.widgets.clone())
        };
        let Some(&rect) = rects.get(drag.from) else {
            return;
        };
        let px = rect.x + drag.grab.0 + dx as i32;
        let py = rect.y + drag.grab.1 + dy as i32;
        let (_target, layout) = drag_layout(&columns, &rects, drag.from, px, py, drag.grab);
        let size = (rect.w, rect.h);
        for (slot, r) in layout.iter().enumerate() {
            if slot == drag.from {
                self.set_visual(slot, &widgets[slot], r.x as f64, r.y as f64, size.0, size.1);
            } else if *r != drag.targets[slot] {
                self.slide_to(slot, &widgets[slot], *r);
            }
        }
        drag.targets = layout;
        *self.drag.borrow_mut() = Some(drag);
    }

    /// Settle: commit the drop if the pointer ended on another tile (every
    /// tile, the dragged one included, then slides to its new slot and size),
    /// else slide the tile back into its own slot.
    fn end_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let (columns, rects, widgets) = {
            let st = self.state.borrow();
            (st.columns.clone(), st.rects.clone(), st.widgets.clone())
        };
        let Some(&rect) = rects.get(drag.from) else {
            return;
        };
        let px = rect.x + drag.grab.0 + dx as i32;
        let py = rect.y + drag.grab.1 + dy as i32;
        let target = tile_at(&rects, px, py).filter(|&to| to != drag.from);
        match target.and_then(|to| columns_after_drop(&columns, drag.from, to)) {
            Some(next) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.columns = next;
                    st.selection.focus = drag.from;
                    st.selection.anchor = drag.from;
                    st.selection.selected.fill(false);
                }
                self.refresh_classes();
                self.relayout(true);
                let name = self.state.borrow().tiles[drag.from].name.clone();
                crate::log!("split", "'{name}' dropped on tile {target:?}");
            }
            None => self.slide_to(drag.from, &widgets[drag.from], rects[drag.from]),
        }
        crate::log!("split", "drag ended on tile {target:?}");
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
        let mut targets: Vec<usize> = {
            let st = self.state.borrow();
            (0..st.tiles.len())
                .filter(|&i| st.selection.selected.get(i).copied().unwrap_or(false))
                .collect()
        };
        if targets.is_empty() {
            targets.push(slot);
        }
        targets.sort_unstable();
        targets.dedup();
        for index in targets.into_iter().rev() {
            if self.state.borrow().tiles.len() > 1 {
                self.remove_tile(index);
            }
        }
        let remaining = self.state.borrow().tiles.len();
        if remaining <= 1 {
            (self.on_exit)();
        } else {
            let focus = self.state.borrow().selection.focus.min(remaining - 1);
            {
                let mut st = self.state.borrow_mut();
                st.selection = Selection::new(remaining);
                st.selection.focus = focus;
                st.selection.anchor = focus;
            }
            self.refresh_classes();
            self.relayout(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rects(n: usize) -> Vec<Rect> {
        rects_by_tile(&initial_columns(n), 1000, 800, 1.0)
    }

    fn rects_for(columns: &[Vec<usize>; 2]) -> Vec<Rect> {
        rects_by_tile(columns, 1000, 800, 1.0)
    }

    #[test]
    fn two_tiles_fill_the_height() {
        let rects = rects(2);
        assert_eq!(rects.len(), 2);
        assert_eq!(rects[0].y, 0); // the pill tops sit level with the panels
        assert_eq!(rects[0].h, 760); // 800 − gap-2xl bottom inset
        assert_eq!(rects[0].x, 40); // the side inset splits evenly
        assert_eq!(rects[1].x, 512); // 40 + 448 + gap-l 24
        assert_eq!(rects[1].h, 760);
        assert!(rects_by_tile(&initial_columns(0), 1000, 800, 1.0).is_empty());
        assert!(rects_by_tile(&initial_columns(2), 0, 800, 1.0).is_empty());
    }

    #[test]
    fn initial_columns_fill_the_shorter_column() {
        assert_eq!(initial_columns(1), [vec![0], vec![]]);
        assert_eq!(initial_columns(2), [vec![0], vec![1]]);
        assert_eq!(initial_columns(3), [vec![0, 2], vec![1]]);
        assert_eq!(initial_columns(4), [vec![0, 2], vec![1, 3]]);
        assert_eq!(initial_columns(5), [vec![0, 2, 4], vec![1, 3]]);
        assert_eq!(initial_columns(6), [vec![0, 2, 4], vec![1, 3, 5]]);
        // New tiles join the shorter column, the left one on a tie.
        let mut columns = [vec![0], vec![1, 2]];
        assert_eq!(add_to_shorter(&mut columns, 3), 0);
        assert_eq!(columns, [vec![0, 3], vec![1, 2]]);
        let mut columns = [vec![0, 1], vec![2]];
        assert_eq!(add_to_shorter(&mut columns, 3), 1);
        assert_eq!(columns, [vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn uneven_columns_make_their_tiles_taller() {
        let three = rects(3);
        // 3 tiles: two stacked left, one full-height right.
        assert_eq!(three[0].h, 368); // (760 − gap-l) / 2
        assert_eq!(three[1].h, 760); // the right column's only tile
        assert_eq!(three[2].x, 40);
        assert_eq!(three[2].y, 392); // 368 + gap-l
        assert_eq!(three[2].y + three[2].h, 760); // the left column fills down
        // 4 tiles: equal 2 + 2 columns.
        let four = rects(4);
        assert_eq!((four[0].h, four[1].h), (368, 368));
        assert_eq!((four[2].y, four[3].y), (392, 392));
        // 5 tiles: 3 left, 2 right; the right column still fits, the left
        // scrolls at the half-visible-height floor.
        let five = rects(5);
        assert_eq!((five[0].h, five[1].h), (368, 368));
        assert_eq!(five[3].y + five[3].h, 760); // right column ends at the inset
        assert_eq!(five[4].x, 40);
        assert!(five[4].y + five[4].h > 800); // the third left tile overflows
        // 6 tiles: 3 + 3, both columns scroll.
        let six = rects(6);
        assert_eq!((six[4].h, six[5].h), (368, 368));
        assert!(six[4].y + six[4].h > 800);
        assert!(six[5].y + six[5].h > 800);
        // The pill + gap-s leave the frame this much of each slot.
        let frame = |r: Rect| r.h - scaled(CONTROL_H, 1.0) - scaled(GAP_S, 1.0);
        assert_eq!(frame(three[0]), 310);
        assert_eq!(frame(three[1]), 702);
    }

    #[test]
    fn closing_from_either_column_leaves_the_other_taller() {
        // Four tiles: close a left tile; the left column's lone tile is full
        // height, whichever column it is.
        let four = initial_columns(4);
        let left_closed = columns_without(&four, 2);
        assert_eq!(column_counts(&left_closed), (1, 2));
        let r = rects_for(&left_closed);
        assert_eq!((r[0].h, r[0].x), (760, 40)); // the left survivor
        assert_eq!((r[1].h, r[1].x), (368, 512)); // the right pair stays short
        assert_eq!(r[2].h, 368);
        // Close a right tile instead; the right survivor is now full height.
        let right_closed = columns_without(&four, 3);
        assert_eq!(column_counts(&right_closed), (2, 1));
        let r = rects_for(&right_closed);
        assert_eq!((r[1].h, r[1].x), (760, 512));
        assert_eq!((r[0].h, r[2].h), (368, 368));
        // Close a middle tile from 5: the left column drops to 2 and grows.
        let five = initial_columns(5);
        let middle = columns_without(&five, 2);
        assert_eq!(column_counts(&middle), (2, 2));
        let r = rects_for(&middle);
        assert_eq!((r[0].h, r[1].h), (368, 368));
        // Every surviving id was renumbered.
        let mut all: Vec<usize> = middle.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, [0, 1, 2, 3]);
    }

    #[test]
    fn dropping_swaps_across_columns_and_reorders_within_one() {
        let four = initial_columns(4);
        // Tile 0 (left, row 0) on tile 1 (right, row 0): the two swap slots.
        let swapped = columns_after_drop(&four, 0, 1).unwrap();
        assert_eq!(swapped, [vec![1, 2], vec![0, 3]]);
        let r = rects_for(&swapped);
        assert_eq!((r[0].x, r[0].h), (512, 368)); // tile 0 now on the right
        assert_eq!((r[1].x, r[1].h), (40, 368)); // tile 1 now on the left
        // Within the left column: tile 0 on tile 2 moves to the bottom.
        let reordered = columns_after_drop(&four, 0, 2).unwrap();
        assert_eq!(reordered, [vec![2, 0], vec![1, 3]]);
        assert_eq!(rects_for(&reordered)[0].y, 392);
        // Within the right column, backwards: tile 3 on tile 1.
        let back = columns_after_drop(&four, 3, 1).unwrap();
        assert_eq!(back, [vec![0, 2], vec![3, 1]]);
        // Dropping on itself changes nothing.
        assert_eq!(columns_after_drop(&four, 2, 2), None);
    }

    #[test]
    fn a_dragged_tile_ends_at_the_slot_sizes() {
        // Three tiles: tile 1 is the full-height right tile, tile 0 a short
        // left one. After swapping, each takes the other's height.
        let columns = initial_columns(3);
        let before = rects_for(&columns);
        assert_eq!((before[0].h, before[1].h), (368, 760));
        let after = rects_for(&columns_after_drop(&columns, 1, 0).unwrap());
        assert_eq!((after[0].h, after[1].h), (760, 368));
    }

    #[test]
    fn rects_never_overlap() {
        let overlap = |a: Rect, b: Rect| {
            a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
        };
        let mut cases = vec![];
        for n in 2..=6 {
            cases.push(initial_columns(n));
        }
        for n in 3..=6 {
            let columns = initial_columns(n);
            for from in 0..n {
                for to in 0..n {
                    if let Some(next) = columns_after_drop(&columns, from, to) {
                        cases.push(next);
                    }
                }
            }
            for tile in 0..n {
                cases.push(columns_without(&columns, tile));
            }
        }
        for columns in cases {
            let rects = rects_for(&columns);
            for i in 0..rects.len() {
                for j in i + 1..rects.len() {
                    assert!(
                        !overlap(rects[i], rects[j]),
                        "tiles {i} and {j} overlap: {:?} {:?}",
                        rects[i],
                        rects[j]
                    );
                }
            }
        }
    }

    #[test]
    fn five_tiles_never_drop_below_half_the_visible_height() {
        for n in 5..=6 {
            for r in rects(n) {
                // Half of the 800 px viewport, less the half-gap slack the
                // two-tile column uses.
                assert!(r.h >= 368, "n={n}: slot {} is too short", r.h);
            }
        }
    }

    #[test]
    fn hit_test() {
        let rects = rects(4);
        // Inside each quadrant of the 2×2 arrangement.
        assert_eq!(tile_at(&rects, 100, 100), Some(0));
        assert_eq!(tile_at(&rects, 600, 100), Some(1));
        assert_eq!(tile_at(&rects, 100, 500), Some(2));
        assert_eq!(tile_at(&rects, 600, 500), Some(3));
        // The gap-l gutter between the columns and rows holds no tile.
        assert_eq!(tile_at(&rects, 500, 100), None);
        assert_eq!(tile_at(&rects, 100, 380), None);
        // Outside the view entirely.
        assert_eq!(tile_at(&rects, -5, 100), None);
        assert_eq!(tile_at(&rects, 1500, 1500), None);
    }

    #[test]
    fn pill_hit_test() {
        let rects = rects(4);
        let r = rects[0];
        let px = r.x + (r.w - scaled(PILL_W, 1.0)) / 2; // centred pill
        let ph = scaled(CONTROL_H, 1.0);
        assert_eq!(pill_at(&rects, px + 10, r.y + 10, 1.0), Some(0));
        // Left of the centred pill is the tile body, not the pill.
        assert_eq!(pill_at(&rects, r.x + 10, r.y + 10, 1.0), None);
        // Below the pill strip too.
        assert_eq!(pill_at(&rects, px + 10, r.y + ph, 1.0), None);
        // The gutter between the columns holds no pill.
        assert_eq!(pill_at(&rects, 500, r.y + 10, 1.0), None);
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

    #[test]
    fn selection_indices_follow_a_close() {
        assert_eq!(adjusted(0, 1, 3), 0);
        assert_eq!(adjusted(1, 1, 3), 1);
        assert_eq!(adjusted(3, 1, 3), 2);
        assert_eq!(adjusted(2, 2, 2), 1);
        assert_eq!(adjusted(0, 0, 0), 0);
    }

    #[test]
    fn drag_moves_forward_across_the_gutter() {
        for n in [2usize, 3, 6] {
            let columns = initial_columns(n);
            let rects = rects_for(&columns);
            let r1 = rects[1];
            let (target, layout) = drag_layout(&columns, &rects, 0, r1.x + 10, r1.y + 10, (0, 0));
            assert_eq!(target, Some(1), "n={n}");
            // The dragged tile follows the pointer at its old size…
            assert_eq!(
                layout[0],
                Rect {
                    x: r1.x + 10,
                    y: r1.y + 10,
                    ..rects[0]
                },
                "n={n}"
            );
            // …and every other tile sits at its post-drop rect.
            let next = columns_after_drop(&columns, 0, 1).unwrap();
            let final_rects = rects_for(&next);
            for tile in 1..n {
                assert_eq!(layout[tile], final_rects[tile], "n={n} tile={tile}");
            }
        }
    }

    #[test]
    fn drag_moves_backward() {
        for n in [2usize, 3, 6] {
            let columns = initial_columns(n);
            let rects = rects_for(&columns);
            let last = n - 1;
            let r0 = rects[0];
            let (target, layout) =
                drag_layout(&columns, &rects, last, r0.x + 10, r0.y + 10, (0, 0));
            assert_eq!(target, Some(0), "n={n}");
            let next = columns_after_drop(&columns, last, 0).unwrap();
            let final_rects = rects_for(&next);
            for tile in 0..last {
                assert_eq!(layout[tile], final_rects[tile], "n={n} tile={tile}");
            }
            assert_eq!(layout[last].x, r0.x + 10, "n={n}");
        }
    }

    #[test]
    fn drag_over_gutter_and_outside_moves_nothing() {
        for n in [2usize, 3, 6] {
            let columns = initial_columns(n);
            let rects = rects_for(&columns);
            let gx = (rects[0].x + rects[0].w + rects[1].x) / 2;
            let (target, layout) = drag_layout(&columns, &rects, 0, gx, rects[0].y + 10, (0, 0));
            assert_eq!(target, None, "n={n}");
            for tile in 1..n {
                assert_eq!(layout[tile], rects[tile], "n={n}");
            }
            // Outside the view entirely: still nothing displaced.
            let (target, layout) = drag_layout(&columns, &rects, 0, -5, rects[0].y + 10, (0, 0));
            assert_eq!(target, None, "n={n}");
            for tile in 1..n {
                assert_eq!(layout[tile], rects[tile], "n={n}");
            }
            assert!(
                layout[0].x < 0,
                "n={n}: the dragged tile follows the pointer"
            );
        }
    }

    #[test]
    fn drag_respects_the_grab_offset() {
        let columns = initial_columns(3);
        let rects = rects_for(&columns);
        let (target, layout) = drag_layout(&columns, &rects, 0, 600, 300, (30, 20));
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
