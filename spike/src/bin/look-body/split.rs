// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view (Kyle's tab groups): tiles in two columns inside a
//! gtk::Fixed, with identity (a tile's name moves with it), focus,
//! selection, close, the fade, live drag rearranging, gap drops with an
//! accent line, and draggable borders.
//!
//! The model is three pure things this file's tests exercise:
//!
//! * **columns** — which tile ids each column holds, top to bottom;
//! * **shares** — each column's tile heights as fractions of its available
//!   height, and the left column's width as a fraction of the width left
//!   after the gutter. Storing fractions (never pixels) keeps a window
//!   resize or a scale change proportional;
//! * **layout** — the slot rects those shares produce.
//!
//! The drag gesture rides the static `Fixed` (never a moving pill: every
//! widget's event coordinates are translated into its own space at
//! dispatch, so a gesture on the moving pill would see its offsets
//! collapse). One gesture decides at the press whether it will move a tile
//! (press on a pill) or a border (press on a gutter), so the two can never
//! fight; nothing is claimed or moved before the drag threshold.
use crate::style::{CELL_H, CELL_W, CONTROL_H, GAP_2XL, GAP_L, GAP_S, LINE_THIN, PILL_W, scaled};
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

/// The split view's margins and the space the two columns share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// `gap-2xl` from the sides and the bottom; the top is flush.
    pub inset: i32,
    /// `gap-l` between the columns and between stacked tiles.
    pub gutter: i32,
    pub inner_w: i32,
    pub inner_h: i32,
}

pub fn frame(view_w: i32, view_h: i32, scale: f64) -> Frame {
    let inset = scaled(GAP_2XL, scale);
    Frame {
        inset,
        gutter: scaled(GAP_L, scale),
        inner_w: (view_w - 2 * inset).max(1),
        inner_h: (view_h - inset).max(1),
    }
}

/// The viewport size and scale the layout is computed for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub w: i32,
    pub h: i32,
    pub scale: f64,
}

/// Everything a layout needs: the columns, their height shares, the column
/// width share and the viewport.
#[derive(Clone, Copy)]
pub struct Board<'a> {
    pub columns: &'a [Vec<usize>; 2],
    pub heights: &'a [Vec<f64>; 2],
    pub width_share: f64,
    pub view: Viewport,
}

impl Board<'_> {
    /// The committed slot layout of these columns and shares.
    pub fn slots(&self) -> [Vec<Rect>; 2] {
        layout(
            self.columns,
            self.heights,
            self.width_share,
            self.view.w,
            self.view.h,
            self.view.scale,
        )
    }

    /// The rect of every tile, indexed by tile id.
    pub fn rects(&self) -> Vec<Rect> {
        rects_by_tile(self.columns, &self.slots())
    }
}

/// The narrowest a column may be dragged: one grid column wide. The tab pill
/// is `PILL_W` wide and GTK will not shrink it (a Box's minimum includes its
/// children), so that own minimum is the binding floor — a narrower column
/// would draw the pill over the gutter.
pub fn min_column_width(scale: f64) -> i32 {
    scaled(CELL_W, scale).max(scaled(PILL_W, scale))
}

/// The shortest a tile's slot may be dragged: its pill, the pill's gap and
/// one grid row of content.
pub fn min_slot_height(scale: f64) -> i32 {
    scaled(CONTROL_H + GAP_S + CELL_H, scale)
}

/// The half-visible-height floor that applies beyond four tiles, else 0.
pub fn half_floor(total: usize, f: &Frame) -> i32 {
    if total > 4 {
        ((f.inner_h - f.gutter) / 2).max(1)
    } else {
        0
    }
}

/// The minimum a border drag keeps on each side.
pub fn slot_minimum(total: usize, f: &Frame, scale: f64) -> i32 {
    min_slot_height(scale).max(half_floor(total, f))
}

/// Equal shares for `n` slots; what a reset (double-click, add, close) sets.
pub fn equal_shares(n: usize) -> Vec<f64> {
    if n == 0 {
        vec![]
    } else {
        vec![1.0 / n as f64; n]
    }
}

/// (left, right) column widths for a view width and the left column's share
/// of the width left after the gutter. A column with no tiles gives the
/// width to the other one; when the minimums cannot both fit, the split
/// falls back to equal halves.
pub fn column_widths(f: &Frame, share: f64, counts: [usize; 2], scale: f64) -> (i32, i32) {
    let avail = (f.inner_w - f.gutter).max(1);
    match (counts[0] == 0, counts[1] == 0) {
        (true, true) => (0, 0),
        (true, false) => (0, avail),
        (false, true) => (avail, 0),
        (false, false) => {
            let min = min_column_width(scale);
            if avail < 2 * min {
                let left = (avail / 2).max(1);
                return (left, (avail - left).max(1));
            }
            let left =
                ((avail as f64 * share.clamp(0.0, 1.0)).round() as i32).clamp(min, avail - min);
            (left, avail - left)
        }
    }
}

/// The left column's new share after the column border is dragged `dx` px.
pub fn width_share_after_drag(share: f64, avail: i32, dx: i32, min_w: i32) -> f64 {
    if avail <= 0 || avail < 2 * min_w {
        return share.clamp(0.0, 1.0);
    }
    let left = ((avail as f64 * share).round() as i32 + dx).clamp(min_w, avail - min_w);
    left as f64 / avail as f64
}

/// The new height shares for column slots that sum to `avail_h` px after
/// dragging the border at `border` (1..n, between slots border−1 and border)
/// by `dy` px. Both neighbours keep `min_h`; a pair that is already tighter
/// than two minimums cannot be resized and is returned unchanged.
pub fn heights_after_drag(
    shares: &[f64],
    avail_h: i32,
    border: usize,
    dy: i32,
    min_h: i32,
) -> Vec<f64> {
    let n = shares.len();
    if n < 2 || border == 0 || border >= n || avail_h <= 0 {
        return shares.to_vec();
    }
    let mut px: Vec<i32> = shares
        .iter()
        .map(|s| ((avail_h as f64 * s).round() as i32).max(1))
        .collect();
    let pair = px[border - 1] + px[border];
    let min_h = min_h.max(1);
    if pair < 2 * min_h {
        return shares.to_vec();
    }
    let first = (px[border - 1] + dy).clamp(min_h, pair - min_h);
    px[border - 1] = first;
    px[border] = pair - first;
    px.iter().map(|p| *p as f64 / avail_h as f64).collect()
}

/// The slot rects of both columns, left first, top to bottom. A slot is the
/// tile's pill plus its frame: the pill is `control-h` tall and sits `gap-s`
/// above the frame. Each column's slots share its available height by the
/// stored shares, so a column with fewer tiles gets taller ones, whichever
/// column it is. Beyond four tiles every slot keeps at least half the visible
/// height and the view scrolls.
pub fn layout(
    columns: &[Vec<usize>; 2],
    heights: &[Vec<f64>; 2],
    width_share: f64,
    view_w: i32,
    view_h: i32,
    scale: f64,
) -> [Vec<Rect>; 2] {
    let counts = [columns[0].len(), columns[1].len()];
    let total = counts[0] + counts[1];
    if total == 0 || view_w <= 0 || view_h <= 0 {
        return [vec![], vec![]];
    }
    let f = frame(view_w, view_h, scale);
    let (left_w, right_w) = column_widths(&f, width_share, counts, scale);
    let gap = f.gutter;
    let half = half_floor(total, &f);
    let pill = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
    [0, 1].map(|column| {
        let n = counts[column];
        if n == 0 {
            return vec![];
        }
        let (w, x) = if column == 0 {
            (left_w, f.inset)
        } else {
            (right_w, f.inset + left_w + gap)
        };
        let avail = (f.inner_h - (n as i32 - 1) * gap).max(1);
        let mut y = 0;
        let mut out = Vec::with_capacity(n);
        for row in 0..n {
            let share = heights[column].get(row).copied().unwrap_or(1.0 / n as f64);
            let h = ((avail as f64 * share.clamp(0.0, 1.0)).round() as i32)
                .max(1)
                .max(pill + 1)
                .max(half);
            out.push(Rect { x, y, w, h });
            y += h + gap;
        }
        out
    })
}

/// The rect of every tile, indexed by tile id, from its column and row.
pub fn rects_by_tile(columns: &[Vec<usize>; 2], slots: &[Vec<Rect>; 2]) -> Vec<Rect> {
    let total = columns[0].len() + columns[1].len();
    let mut out = vec![
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        total
    ];
    for (column, tiles) in columns.iter().enumerate() {
        for (row, &tile) in tiles.iter().enumerate() {
            // Before the first allocation, or when the view is empty, there is
            // no slot to read; the tile keeps the fallback rect.
            if let (Some(slot), true) =
                (slots.get(column).and_then(|s| s.get(row)), tile < out.len())
            {
                out[tile] = *slot;
            }
        }
    }
    out
}

/// The per-column slot rects of a committed layout, from rects by tile id.
pub fn slots_of(columns: &[Vec<usize>; 2], rects: &[Rect]) -> [Vec<Rect>; 2] {
    [0, 1].map(|column| {
        columns[column]
            .iter()
            .filter_map(|&tile| rects.get(tile).copied())
            .collect()
    })
}

/// New tiles go to the shorter column (the left one on a tie), so the first
/// distribution is 1 = 1+0, 2 = 1+1, 3 = 2+1, 4 = 2+2, 5 = 3+2, 6 = 3+3.
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

/// The column counts, left first.
pub fn column_counts(columns: &[Vec<usize>; 2]) -> (usize, usize) {
    (columns[0].len(), columns[1].len())
}

/// Remove `tile` from its column, renumber the surviving ids (which are kept
/// contiguous by the caller's parallel vectors), keep both columns populated,
/// and rebalance. Three rules, in order:
///
/// 1. if the close empties a column while the other still has two or more
///    tiles, the other column's last tile moves across;
/// 2. if the columns end two or more tiles apart, the longer column's bottom
///    tile moves across, repeatedly, until they differ by at most one — so
///    3 + 3 closed twice from one column becomes 2 + 2, not 1 + 3;
/// 3. a layout made by hand is left alone otherwise: 3 + 1 stays 3 + 1
///    through a close that only leaves one tile between the columns.
pub fn columns_without(columns: &[Vec<usize>; 2], tile: usize) -> [Vec<usize>; 2] {
    let mut out = columns.clone();
    for column in &mut out {
        column.retain(|&id| id != tile);
    }
    // The empty-column rule: only move a tile when the donor keeps at least
    // one (2 → 1 has no move).
    for empty in 0..2 {
        let other = 1 - empty;
        if out[empty].is_empty() && out[other].len() >= 2 {
            let moved = out[other].pop().expect("the other column has two or more");
            out[empty].push(moved);
            break;
        }
    }
    // Rebalance until the columns are within one tile of each other.
    loop {
        let (left, right) = (out[0].len(), out[1].len());
        if left.abs_diff(right) < 2 {
            break;
        }
        let long = if left > right { 0 } else { 1 };
        let short = 1 - long;
        if out[long].len() < 2 {
            break;
        }
        let moved = out[long].pop().expect("the longer column has two or more");
        out[short].push(moved);
    }
    for column in &mut out {
        for id in column.iter_mut() {
            if *id > tile {
                *id -= 1;
            }
        }
    }
    out
}

/// Where a tile drag would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drop {
    /// On a tile: across columns the two swap slots, within a column it
    /// reorders (the dragged tile takes the target's row).
    Swap(usize),
    /// Into a gap: the tile becomes row `row` of `column`, so that column
    /// gets one longer and the other one shorter.
    Insert { column: usize, row: usize },
}

/// The columns after dropping the tile `from` on the tile `to`.
pub fn columns_after_swap(
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

/// The columns after inserting the tile `from` at `row` (0..=len) of
/// `column`. Refused (`None`) when the move would leave a column empty while
/// two or more tiles exist.
pub fn columns_after_insert(
    columns: &[Vec<usize>; 2],
    from: usize,
    column: usize,
    row: usize,
) -> Option<[Vec<usize>; 2]> {
    if column > 1 {
        return None;
    }
    let total: usize = columns[0].len() + columns[1].len();
    let from_column = (0..2).find(|&c| columns[c].contains(&from))?;
    let pos = columns[from_column].iter().position(|&id| id == from)?;
    if from_column != column && columns[from_column].len() == 1 && total >= 2 {
        return None;
    }
    let mut out = columns.clone();
    out[from_column].remove(pos);
    // A same-column move above the gap shifts that gap up by one; the row is
    // then clamped to the shrunken list (a gap below the last tile becomes
    // the end).
    let mut row = row;
    if from_column == column && pos < row {
        row -= 1;
    }
    let row = row.min(out[column].len());
    out[column].insert(row, from);
    if out[from_column].is_empty() && total >= 2 {
        return None;
    }
    if out == *columns {
        return None;
    }
    Some(out)
}

/// The columns after `drop`; `None` means the drop is refused and the tile
/// slides back.
pub fn apply_drop(columns: &[Vec<usize>; 2], from: usize, drop: Drop) -> Option<[Vec<usize>; 2]> {
    match drop {
        Drop::Swap(to) => columns_after_swap(columns, from, to),
        Drop::Insert { column, row } => columns_after_insert(columns, from, column, row),
    }
}

/// The drop under the pointer. The gap band wins over the tile under it: a
/// gap's zone is the gutter plus a fifth of the adjacent slot's height on
/// each side. A column's own x range selects it. Dropping on a tile of the
/// other column is a swap; the dragged tile's own column has no swap.
pub fn drop_at(
    columns: &[Vec<usize>; 2],
    rects: &[Rect],
    from: usize,
    px: i32,
    py: i32,
    scale: f64,
) -> Option<Drop> {
    let slots = slots_of(columns, rects);
    let gap = scaled(GAP_L, scale);
    for (column, s) in slots.iter().enumerate() {
        let n = s.len();
        if n == 0 {
            continue;
        }
        let (x, w) = (s[0].x, s[0].w);
        if px < x || px >= x + w {
            continue;
        }
        for row in 1..=n {
            let (centre, band) = if row < n {
                (s[row].y - gap / 2, s[row - 1].h / 5)
            } else {
                let last = s[n - 1];
                (last.y + last.h, last.h / 5)
            };
            let band = band.max(1);
            if py >= centre - band && py <= centre + band {
                return Some(Drop::Insert { column, row });
            }
        }
    }
    // A tile: across columns the two swap slots, within one the column
    // reorders. Skipped when it is the dragged tile itself.
    for (column, tiles) in columns.iter().enumerate() {
        for (row, &tile) in tiles.iter().enumerate() {
            if tile != from && slots[column][row].contains(px, py) {
                return Some(Drop::Swap(tile));
            }
        }
    }
    None
}

/// The drop line's rect: across the target column at the gap the tile will
/// land in. `row == n` is the gap below the column's last tile.
pub fn drop_line(slots: &[Vec<Rect>; 2], drop: Drop, scale: f64) -> Option<Rect> {
    let Drop::Insert { column, row } = drop else {
        return None;
    };
    let s = slots.get(column)?;
    let n = s.len();
    if n == 0 {
        return None;
    }
    let gap = scaled(GAP_L, scale);
    let thin = scaled(LINE_THIN, scale).max(1);
    let y = if row < n {
        s[row].y - gap / 2
    } else {
        s[n - 1].y + s[n - 1].h + gap / 2
    };
    Some(Rect {
        x: s[0].x,
        y: y - thin / 2,
        w: s[0].w,
        h: thin,
    })
}

/// The new heights after the columns change: a column that gained or lost a
/// tile goes back to equal shares; the others keep theirs. A swap or a
/// reorder changes no length, so each tile keeps the slot height it lands in.
pub fn heights_after_change(
    old_columns: &[Vec<usize>; 2],
    old_heights: &[Vec<f64>; 2],
    new_columns: &[Vec<usize>; 2],
) -> [Vec<f64>; 2] {
    [0, 1].map(|column| {
        if old_columns[column].len() == new_columns[column].len()
            && old_heights[column].len() == old_columns[column].len()
        {
            old_heights[column].clone()
        } else {
            equal_shares(new_columns[column].len())
        }
    })
}

/// A border under the pointer: the gutter between the columns, or the gutter
/// between two stacked tiles of one column.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    Columns,
    Heights { column: usize, row: usize },
}

pub fn border_at(slots: &[Vec<Rect>; 2], x: i32, y: i32) -> Option<Border> {
    let (left, right) = (&slots[0], &slots[1]);
    if !left.is_empty() && !right.is_empty() {
        let left_end = left[0].x + left[0].w;
        let right_start = right[0].x;
        let bottom = left
            .iter()
            .chain(right.iter())
            .map(|r| r.y + r.h)
            .max()
            .unwrap_or(0);
        if x >= left_end && x <= right_start && y >= 0 && y <= bottom {
            return Some(Border::Columns);
        }
    }
    for (column, s) in slots.iter().enumerate() {
        if s.len() < 2 {
            continue;
        }
        let (x0, w) = (s[0].x, s[0].w);
        if x < x0 || x >= x0 + w {
            continue;
        }
        for row in 1..s.len() {
            let top = s[row - 1].y + s[row - 1].h;
            let bottom = s[row].y;
            if y >= top && y < bottom {
                return Some(Border::Heights { column, row });
            }
        }
    }
    None
}

/// The live drag layout: the drop under the pointer and, indexed by tile id,
/// the rect every tile sits at while the tile `from` (whose grab offset inside
/// the tile is `grab`) follows the pointer at its old size. The other tiles
/// already sit at their post-drop rects, including the sizes of an insert's
/// new (shorter) slots.
pub fn drag_layout(
    board: &Board,
    rects: &[Rect],
    from: usize,
    px: i32,
    py: i32,
    grab: (i32, i32),
) -> (Option<Drop>, Vec<Rect>) {
    let mut out = rects.to_vec();
    let Some(&rect) = rects.get(from) else {
        return (None, out);
    };
    let mut target = drop_at(board.columns, rects, from, px, py, board.view.scale);
    if let Some(drop) = target {
        match apply_drop(board.columns, from, drop) {
            Some(next) => {
                let next_heights = heights_after_change(board.columns, board.heights, &next);
                let slots = layout(
                    &next,
                    &next_heights,
                    board.width_share,
                    board.view.w,
                    board.view.h,
                    board.view.scale,
                );
                for (column, tiles) in next.iter().enumerate() {
                    for (row, &tile) in tiles.iter().enumerate() {
                        if let (Some(slot), true) = (slots[column].get(row), tile < out.len()) {
                            out[tile] = *slot;
                        }
                    }
                }
            }
            None => target = None,
        }
    }
    out[from] = Rect {
        x: px - grab.0,
        y: py - grab.1,
        w: rect.w,
        h: rect.h,
    };
    (target, out)
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

/// The pill hit test: which tile's pill region holds the point. The pill is
/// `PILL_W` wide, centred above the tile's frame, starting at the tile rect's
/// top and `control-h` tall.
pub fn pill_at(rects: &[Rect], x: i32, y: i32, scale: f64) -> Option<usize> {
    let pw = scaled(PILL_W, scale);
    let ph = scaled(CONTROL_H, scale);
    rects.iter().position(|r| {
        let px = r.x + (r.w - pw) / 2;
        x >= px.max(r.x) && x < (px + pw).min(r.x + r.w) && y >= r.y && y < r.y + ph
    })
}

/// Whether a press at (x, y) inside a `tile_w`-wide tile landed on its pill.
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
/// selection, Ctrl toggles, and Shift selects the range from the anchor (the
/// last plain click) to the clicked tile. Ctrl does not move the anchor.
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

/// One tile's identity: its tab name rides along through every move.
struct Tile {
    uid: usize,
    name: String,
}

struct State {
    tiles: Vec<Tile>,         // indexed by tile id
    columns: [Vec<usize>; 2], // column -> tile ids, top to bottom
    /// Each column's slot heights as shares of its available height.
    heights: [Vec<f64>; 2],
    /// The left column's share of the width left after the gutter.
    width_share: f64,
    widgets: Vec<gtk::Box>, // indexed by tile id: the outer pill+frame box
    frames: Vec<gtk::Box>,
    pills: Vec<gtk::Box>,
    rects: Vec<Rect>, // indexed by tile id
    selection: Selection,
    scale: f64,
    view: [i32; 2],
    next_uid: usize,
}

impl State {
    fn board(&self) -> Board<'_> {
        Board {
            columns: &self.columns,
            heights: &self.heights,
            width_share: self.width_share,
            view: Viewport {
                w: self.view[0],
                h: self.view[1],
                scale: self.scale,
            },
        }
    }

    fn slots(&self) -> [Vec<Rect>; 2] {
        slots_of(&self.columns, &self.rects)
    }

    /// The rect of every tile, indexed by tile id.
    fn rects(&self) -> Vec<Rect> {
        self.board().rects()
    }
}

/// What a press on the split view started.
#[derive(Clone)]
enum Drag {
    /// A pill press: the tile follows the pointer.
    Tile {
        from: usize,
        grab: (i32, i32),
        targets: Vec<Rect>,
    },
    /// The column border: the width share follows the pointer.
    Width { share: f64, avail: i32 },
    /// A border between two stacked tiles: the height shares follow.
    Height {
        column: usize,
        border: usize,
        shares: Vec<f64>,
        avail: i32,
    },
}

/// Everything one tile owns visually: its current position and size, its
/// running slide and that slide's generation.
struct Slot {
    pos: Rc<Cell<(f64, f64)>>,
    size: Rc<Cell<(i32, i32)>>,
    slide: Option<adw::Animation>,
    generation: Rc<Cell<u64>>,
}

/// The resize cursor currently set on the view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Cursor {
    None,
    Width,
    Height,
}

impl Cursor {
    fn name(self) -> Option<&'static str> {
        match self {
            Cursor::None => None,
            Cursor::Width => Some("ew-resize"),
            Cursor::Height => Some("ns-resize"),
        }
    }
}

struct Inner {
    outer: gtk::ScrolledWindow,
    revealer: gtk::Revealer,
    fixed: gtk::Fixed,
    /// The accent line that shows a gap drop while dragging.
    line: gtk::Box,
    state: RefCell<State>,
    items: u32,
    on_exit: Rc<dyn Fn()>,
    handle: RefCell<Option<SplitView>>,
    slots: RefCell<Vec<Slot>>,
    drag: RefCell<Option<Drag>>,
    cursor: Cell<Cursor>,
}

/// Cloneable handle; the widgets and model live in the Rc.
#[derive(Clone)]
pub struct SplitView {
    inner: Rc<Inner>,
}

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
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        line.add_css_class("gl-drop-line");
        line.set_visible(false);
        fixed.put(&line, 0.0, 0.0);
        line.set_size_request(1, scaled(LINE_THIN, 1.0).max(1));
        let outer = gtk::ScrolledWindow::new();
        outer.set_child(Some(&fixed));
        // EXTERNAL horizontally: the tiles always fit by construction and no
        // scrollbar is drawn, and a NEVER policy would make the split view's
        // minimum width the tiles' full extent, squeezing the content.
        outer.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        outer.set_propagate_natural_width(false);
        outer.set_propagate_natural_height(false);
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
                line,
                items,
                state: RefCell::new(State {
                    tiles: vec![],
                    columns: [vec![], vec![]],
                    heights: [vec![], vec![]],
                    width_share: 0.5,
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
                cursor: Cell::new(Cursor::None),
            }),
        };
        this.inner.handle.replace(Some(this.clone()));
        watch_viewport(&this.inner, 1, &this.inner.outer.vadjustment());
        watch_viewport(&this.inner, 0, &this.inner.outer.hadjustment());
        // One drag gesture for tiles and borders: the press decides which.
        // It never claims (gtkgesturedrag.c claims nothing), so clicks keep
        // working; nothing moves before the threshold.
        let drag = gtk::GestureDrag::new();
        let begin_view = this.clone();
        drag.connect_drag_begin(move |_, sx, sy| begin_view.inner.begin_drag(sx, sy));
        let update_view = this.clone();
        drag.connect_drag_update(move |_, dx, dy| update_view.inner.update_drag(dx, dy));
        let end_view = this.clone();
        drag.connect_drag_end(move |_, dx, dy| end_view.inner.end_drag(dx, dy));
        let cancel_view = this.clone();
        drag.connect_cancel(move |_, _| cancel_view.inner.cancel_drag());
        this.inner.fixed.add_controller(drag);
        // Double-click on a border resets it to equal shares.
        let reset = gtk::GestureClick::new();
        let reset_view = this.clone();
        reset.connect_pressed(move |_, n_press, x, y| {
            if n_press >= 2 {
                reset_view.inner.reset_border(x, y);
            }
        });
        this.inner.fixed.add_controller(reset);
        // The hover cursor: a border shows ew-resize / ns-resize.
        let motion = gtk::EventControllerMotion::new();
        let hover_view = this.clone();
        motion.connect_motion(move |_, x, y| hover_view.inner.hover(x, y));
        let leave_view = this.clone();
        motion.connect_leave(move |_| leave_view.inner.set_cursor(Cursor::None));
        this.inner.fixed.add_controller(motion);
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

    /// Remove every tile and build `n` fresh ones (the first build).
    fn build(&self, n: usize) {
        let view = self.handle.borrow().as_ref().unwrap().clone();
        {
            let mut st = self.state.borrow_mut();
            for widget in st.widgets.drain(..) {
                self.fixed.remove(&widget);
            }
            st.tiles.clear();
            st.columns = [vec![], vec![]];
            st.heights = [vec![], vec![]];
            st.width_share = 0.5;
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
        self.state.borrow_mut().columns = initial_columns(n);
        {
            let mut st = self.state.borrow_mut();
            st.heights = [
                equal_shares(st.columns[0].len()),
                equal_shares(st.columns[1].len()),
            ];
        }
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
            let column = add_to_shorter(&mut st.columns, index);
            // A column that gained a tile goes back to equal shares.
            st.heights[column] = equal_shares(st.columns[column].len());
            st.widgets.push(tile.clone());
            st.frames.push(frame);
            st.pills.push(pill);
            st.selection.selected.push(false);
            (index, name, tile)
        };
        let rect = {
            let st = self.state.borrow();
            st.rects().get(index).copied().unwrap_or(Rect {
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

    /// The last tile a count reduction should close: from the longer column.
    fn last_closable_index(&self) -> usize {
        let st = self.state.borrow();
        let (left, right) = column_counts(&st.columns);
        let column = if right > left { 1 } else { 0 };
        st.columns[column]
            .last()
            .copied()
            .unwrap_or_else(|| st.tiles.len().saturating_sub(1))
    }

    /// Close one tile in place: its widget leaves the Fixed, the survivors keep
    /// their widgets and animate to the new rects. The columns rebalance and
    /// a column that changed size returns to equal shares.
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
            let old_columns = st.columns.clone();
            let old_heights = st.heights.clone();
            let next = columns_without(&old_columns, index);
            st.heights = heights_after_change(&old_columns, &old_heights, &next);
            st.columns = next;
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
            // The column border keeps its position unless a column empties.
            if st.columns[0].is_empty() || st.columns[1].is_empty() {
                st.width_share = 0.5;
            }
            let rects = st.rects();
            st.rects = rects;
        }
        if index < self.slots.borrow().len() {
            self.slots.borrow_mut().remove(index);
        }
        crate::log!("split", "closed '{name}' from slot {index}");
    }

    /// Recompute every tile's rect and move it there: immediately for a
    /// viewport/scale/border change, over 200 ms for a close, an add or a drop.
    fn relayout(&self, animate: bool) {
        let (rects, widgets) = {
            let mut st = self.state.borrow_mut();
            let rects = st.rects();
            st.rects = rects;
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

    /// One tile: the tab pill sits short and centred *above* the framed pane.
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

    fn set_visual(&self, slot: usize, widget: &gtk::Box, x: f64, y: f64, w: i32, h: i32) {
        if let Some(state) = self.slots.borrow().get(slot) {
            state.pos.set((x, y));
            state.size.set((w, h));
        }
        self.fixed.move_(widget, x, y);
        widget.set_size_request(w, h);
    }

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

    /// A press anywhere in the view: a pill starts a tile drag, a border
    /// starts a resize. Nothing moves until the pointer passes the threshold.
    fn begin_drag(&self, sx: f64, sy: f64) {
        let (rects, columns, slots, scale, view, width_share, heights) = {
            let st = self.state.borrow();
            (
                st.rects.clone(),
                st.columns.clone(),
                st.slots(),
                st.scale,
                st.view,
                st.width_share,
                st.heights.clone(),
            )
        };
        if let Some(slot) = pill_at(&rects, sx as i32, sy as i32, scale) {
            let Some(rect) = rects.get(slot).copied() else {
                return;
            };
            let Some(widget) = self.state.borrow().widgets.get(slot).cloned() else {
                return;
            };
            let name = self.state.borrow().tiles[slot].name.clone();
            *self.drag.borrow_mut() = Some(Drag::Tile {
                from: slot,
                grab: (sx as i32 - rect.x, sy as i32 - rect.y),
                targets: rects,
            });
            self.bump_gen(slot);
            self.slots.borrow_mut()[slot].slide = None;
            if self.fixed.last_child().as_ref() != Some(widget.upcast_ref()) {
                let last = self.fixed.last_child();
                widget.insert_after(&self.fixed, last.as_ref());
            }
            crate::log!("split", "dragging '{name}' from slot {slot}");
            return;
        }
        let Some(border) = border_at(&slots, sx as i32, sy as i32) else {
            return;
        };
        let f = frame(view[0], view[1], scale);
        match border {
            Border::Columns => {
                let avail = (f.inner_w - f.gutter).max(1);
                *self.drag.borrow_mut() = Some(Drag::Width {
                    share: width_share,
                    avail,
                });
                crate::log!("split", "resizing the column border");
            }
            Border::Heights { column, row } => {
                let n = columns[column].len();
                let avail = (f.inner_h - (n as i32 - 1) * f.gutter).max(1);
                *self.drag.borrow_mut() = Some(Drag::Height {
                    column,
                    border: row,
                    shares: heights[column].clone(),
                    avail,
                });
                crate::log!("split", "resizing column {column} at border {row}");
            }
        }
    }

    fn update_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow().clone() else {
            return;
        };
        match drag {
            Drag::Tile {
                from,
                grab,
                targets,
            } => {
                let (columns, heights, width_share, rects, widgets, view, scale) = {
                    let st = self.state.borrow();
                    (
                        st.columns.clone(),
                        st.heights.clone(),
                        st.width_share,
                        st.rects.clone(),
                        st.widgets.clone(),
                        st.view,
                        st.scale,
                    )
                };
                let Some(&rect) = rects.get(from) else {
                    return;
                };
                let px = rect.x + grab.0 + dx as i32;
                let py = rect.y + grab.1 + dy as i32;
                let board = Board {
                    columns: &columns,
                    heights: &heights,
                    width_share,
                    view: Viewport {
                        w: view[0],
                        h: view[1],
                        scale,
                    },
                };
                let (drop, layout) = drag_layout(&board, &rects, from, px, py, grab);
                let size = (rect.w, rect.h);
                for (slot, r) in layout.iter().enumerate() {
                    if slot == from {
                        self.set_visual(
                            slot,
                            &widgets[slot],
                            r.x as f64,
                            r.y as f64,
                            size.0,
                            size.1,
                        );
                    } else if *r != targets[slot] {
                        self.slide_to(slot, &widgets[slot], *r);
                    }
                }
                self.show_drop_line(drop);
                *self.drag.borrow_mut() = Some(Drag::Tile {
                    from,
                    grab,
                    targets: layout,
                });
            }
            Drag::Width { share, avail } => {
                let scale = self.state.borrow().scale;
                let next = width_share_after_drag(share, avail, dx as i32, min_column_width(scale));
                self.state.borrow_mut().width_share = next;
                self.relayout(false);
            }
            Drag::Height {
                column,
                border,
                shares,
                avail,
            } => {
                let (scale, view) = {
                    let st = self.state.borrow();
                    (st.scale, st.view)
                };
                let f = frame(view[0], view[1], scale);
                let total = {
                    let st = self.state.borrow();
                    st.columns[0].len() + st.columns[1].len()
                };
                let min_h = slot_minimum(total, &f, scale);
                let next = heights_after_drag(&shares, avail, border, dy as i32, min_h);
                self.state.borrow_mut().heights[column] = next;
                self.relayout(false);
            }
        }
    }

    /// Settle: a tile drop commits, otherwise everything slides back. Border
    /// drags have already written their shares.
    fn end_drag(&self, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let Drag::Tile { from, grab, .. } = drag else {
            return;
        };
        let (columns, heights, rects, widgets, scale) = {
            let st = self.state.borrow();
            (
                st.columns.clone(),
                st.heights.clone(),
                st.rects.clone(),
                st.widgets.clone(),
                st.scale,
            )
        };
        let Some(&rect) = rects.get(from) else {
            return;
        };
        let px = rect.x + grab.0 + dx as i32;
        let py = rect.y + grab.1 + dy as i32;
        let drop = drop_at(&columns, &rects, from, px, py, scale);
        let landed = drop.and_then(|d| apply_drop(&columns, from, d).map(|next| (d, next)));
        match landed {
            Some((drop, next)) => {
                let next_heights = heights_after_change(&columns, &heights, &next);
                {
                    let mut st = self.state.borrow_mut();
                    st.columns = next;
                    st.heights = next_heights;
                    if st.columns[0].is_empty() || st.columns[1].is_empty() {
                        st.width_share = 0.5;
                    }
                    st.selection.focus = from;
                    st.selection.anchor = from;
                    st.selection.selected.fill(false);
                }
                self.hide_drop_line();
                self.refresh_classes();
                self.relayout(true);
                let name = self.state.borrow().tiles[from].name.clone();
                crate::log!("split", "'{name}' dropped at {drop:?}");
            }
            None => {
                self.hide_drop_line();
                self.slide_to(from, &widgets[from], rects[from]);
                crate::log!("split", "drag ended with no drop; sliding back");
            }
        }
    }

    /// Abandon a drag (a lost sequence): put the tile back and drop the line.
    fn cancel_drag(&self) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        if let Drag::Tile { from, .. } = drag {
            let (rect, widget) = {
                let st = self.state.borrow();
                (st.rects.get(from).copied(), st.widgets.get(from).cloned())
            };
            if let (Some(rect), Some(widget)) = (rect, widget) {
                self.slide_to(from, &widget, rect);
            }
        }
        self.hide_drop_line();
    }

    /// A double-click on a border resets it to equal shares, animated.
    fn reset_border(&self, x: f64, y: f64) {
        let slots = self.state.borrow().slots();
        match border_at(&slots, x as i32, y as i32) {
            Some(Border::Columns) => {
                self.state.borrow_mut().width_share = 0.5;
                self.relayout(true);
                crate::log!("split", "reset the column border to equal shares");
            }
            Some(Border::Heights { column, .. }) => {
                {
                    let mut st = self.state.borrow_mut();
                    st.heights[column] = equal_shares(st.columns[column].len());
                }
                self.relayout(true);
                crate::log!("split", "reset column {column} to equal shares");
            }
            None => {}
        }
    }

    /// The hover cursor over a border.
    fn hover(&self, x: f64, y: f64) {
        let slots = self.state.borrow().slots();
        let cursor = match border_at(&slots, x as i32, y as i32) {
            Some(Border::Columns) => Cursor::Width,
            Some(Border::Heights { .. }) => Cursor::Height,
            None => Cursor::None,
        };
        self.set_cursor(cursor);
    }

    fn set_cursor(&self, cursor: Cursor) {
        if self.cursor.get() == cursor {
            return;
        }
        self.cursor.set(cursor);
        self.fixed.set_cursor_from_name(cursor.name());
    }

    fn show_drop_line(&self, drop: Option<Drop>) {
        let slots = {
            let st = self.state.borrow();
            st.slots()
        };
        let scale = self.state.borrow().scale;
        match drop.and_then(|d| drop_line(&slots, d, scale)) {
            Some(rect) => {
                let last = self.fixed.last_child();
                self.line.insert_after(&self.fixed, last.as_ref());
                self.fixed.move_(&self.line, rect.x as f64, rect.y as f64);
                self.line.set_size_request(rect.w, rect.h);
                self.line.set_visible(true);
            }
            None => self.hide_drop_line(),
        }
    }

    fn hide_drop_line(&self) {
        self.line.set_visible(false);
    }

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

    fn apply_selection(&self, slot: usize, ctrl: bool, shift: bool) {
        {
            let mut st = self.state.borrow_mut();
            st.selection = selection_after(&st.selection, slot, ctrl, shift);
        }
        self.refresh_classes();
    }

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

    /// A realistic split view: 1200 × 1400, so four tiles are above the
    /// pill + one grid row minimum.
    const VIEW: (i32, i32) = (1200, 1400);

    fn slots(columns: &[Vec<usize>; 2], heights: &[Vec<f64>; 2]) -> [Vec<Rect>; 2] {
        layout(columns, heights, 0.5, VIEW.0, VIEW.1, 1.0)
    }

    fn rects_for(columns: &[Vec<usize>; 2]) -> Vec<Rect> {
        let heights = [
            equal_shares(columns[0].len()),
            equal_shares(columns[1].len()),
        ];
        let s = slots(columns, &heights);
        rects_by_tile(columns, &s)
    }

    fn frames() -> [Vec<f64>; 2] {
        [equal_shares(2), equal_shares(2)]
    }

    #[test]
    fn tiles_fill_the_height_and_split_the_width() {
        let columns = initial_columns(2);
        let s = slots(&columns, &[equal_shares(1), equal_shares(1)]);
        assert_eq!(s[0][0].y, 0); // the pill tops sit level with the panels
        assert_eq!(s[0][0].h, VIEW.1 - scaled(GAP_2XL, 1.0)); // bottom inset only
        assert_eq!(s[0][0].x, scaled(GAP_2XL, 1.0));
        assert_eq!(s[0][0].w, s[1][0].w);
        // The gutter between the columns is gap-l.
        assert_eq!(s[1][0].x, s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0));
        assert!(rects_by_tile(&initial_columns(0), &[vec![], vec![]]).is_empty());
        assert!(
            layout(
                &initial_columns(2),
                &[vec![1.0], vec![1.0]],
                0.5,
                0,
                800,
                1.0
            )[0]
            .is_empty()
        );
    }

    #[test]
    fn initial_columns_fill_the_shorter_column() {
        assert_eq!(initial_columns(1), [vec![0], vec![]]);
        assert_eq!(initial_columns(2), [vec![0], vec![1]]);
        assert_eq!(initial_columns(3), [vec![0, 2], vec![1]]);
        assert_eq!(initial_columns(4), [vec![0, 2], vec![1, 3]]);
        assert_eq!(initial_columns(5), [vec![0, 2, 4], vec![1, 3]]);
        assert_eq!(initial_columns(6), [vec![0, 2, 4], vec![1, 3, 5]]);
        let mut columns = [vec![0], vec![1, 2]];
        assert_eq!(add_to_shorter(&mut columns, 3), 0);
        assert_eq!(columns, [vec![0, 3], vec![1, 2]]);
        let mut columns = [vec![0, 1], vec![2]];
        assert_eq!(add_to_shorter(&mut columns, 3), 1);
        assert_eq!(columns, [vec![0, 1], vec![2, 3]]);
    }

    #[test]
    fn uneven_columns_make_their_tiles_taller() {
        // 3 tiles: two stacked left, one full-height right.
        let three = rects_for(&initial_columns(3));
        let avail = VIEW.1 - scaled(GAP_2XL, 1.0) - scaled(GAP_L, 1.0);
        assert_eq!(three[0].h, avail / 2);
        assert_eq!(three[1].h, VIEW.1 - scaled(GAP_2XL, 1.0));
        assert_eq!(three[2].y, three[0].h + scaled(GAP_L, 1.0));
        // The left column fills the visible height (rounded shares may leave
        // one pixel).
        assert!(three[2].y + three[2].h >= VIEW.1 - scaled(GAP_2XL, 1.0) - 1);
        // 4 tiles: equal 2 + 2 columns.
        let four = rects_for(&initial_columns(4));
        assert_eq!((four[0].h, four[1].h), (four[2].h, four[3].h));
        assert_eq!(four[2].y, four[3].y);
        // 6 tiles: the half-visible-height floor makes both columns scroll.
        let six = rects_for(&initial_columns(6));
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let half = (f.inner_h - f.gutter) / 2;
        assert_eq!(six[4].h, half);
        assert!(six[4].y + six[4].h > VIEW.1);
        assert!(six[5].y + six[5].h > VIEW.1);
        // The pill + gap-s leave the frame this much of each slot.
        let frame_h = |r: Rect| r.h - scaled(CONTROL_H, 1.0) - scaled(GAP_S, 1.0);
        assert!(frame_h(three[1]) > frame_h(three[0]));
    }

    #[test]
    fn closing_from_either_column_leaves_the_other_taller() {
        let four = initial_columns(4);
        let left_closed = columns_without(&four, 2);
        assert_eq!(column_counts(&left_closed), (1, 2));
        let r = rects_for(&left_closed);
        assert_eq!(r[0].x, scaled(GAP_2XL, 1.0)); // the left survivor
        assert!(r[0].h > r[1].h);
        let right_closed = columns_without(&four, 3);
        assert_eq!(column_counts(&right_closed), (2, 1));
        let r = rects_for(&right_closed);
        assert!(r[1].h > r[0].h);
        let mut all: Vec<usize> = left_closed.iter().flatten().copied().collect();
        all.sort_unstable();
        assert_eq!(all, [0, 1, 2]);
    }

    #[test]
    fn a_close_rebalances_two_or_more_apart() {
        // 6 tiles (3 + 3): close two from the right column. 3 + 2 is one
        // apart and kept; 3 + 1 is two apart and the left column's bottom
        // tile moves across, leaving 2 + 2.
        let six = initial_columns(6);
        let five = columns_without(&six, 5);
        assert_eq!(five, [vec![0, 2, 4], vec![1, 3]]);
        let four = columns_without(&five, 3);
        assert_eq!(column_counts(&four), (2, 2));
        // The moved tile is the longer column's *bottom* one (the old id 4,
        // renumbered to 3): the left column keeps 0 and 2.
        assert_eq!(four, [vec![0, 2], vec![1, 3]]);
        assert_eq!(rects_for(&four)[3].x, rects_for(&four)[1].x);
        // Closing the left column's bottom instead behaves the same way.
        let five = columns_without(&six, 4);
        assert_eq!(five, [vec![0, 2], vec![1, 3, 4]]);
        let four = columns_without(&five, 2);
        assert_eq!(column_counts(&four), (2, 2));
    }

    #[test]
    fn a_hand_made_layout_survives_a_close_that_leaves_one_gap() {
        // 3 + 1 by hand: closing from the long column leaves 2 + 1 (one
        // apart), so the lone right tile stays where it is.
        let hand = [vec![0, 1, 2], vec![3]];
        let after = columns_without(&hand, 2);
        assert_eq!(after, [vec![0, 1], vec![2]]);
        assert_eq!(column_counts(&after), (2, 1));
    }

    #[test]
    fn closing_the_lone_tile_rebalances() {
        // 3 + 1: the lone tile closes, the other column's last tile moves
        // across, leaving 2 + 1.
        let hand = [vec![0, 1, 2], vec![3]];
        let after = columns_without(&hand, 3);
        assert_eq!(column_counts(&after), (2, 1));
        assert_eq!(after, [vec![0, 1], vec![2]]);
        // 2 tiles always end one per column, both full height.
        let two = columns_without(&initial_columns(3), 1);
        assert_eq!(two, [vec![0], vec![1]]);
        let r = rects_for(&two);
        assert_eq!(r[0].h, r[1].h);
        assert_ne!(r[0].x, r[1].x);
    }

    #[test]
    fn the_drop_rule_still_holds_after_a_close() {
        let three = initial_columns(3);
        let two = columns_without(&three, 1);
        assert_eq!(two, [vec![0], vec![1]]);
        let swapped = columns_after_swap(&two, 0, 1).unwrap();
        assert_eq!(swapped, [vec![1], vec![0]]);
        let r = rects_for(&swapped);
        assert!(r[0].x > r[1].x); // each is in the other's column now
        assert_eq!(r[0].h, r[1].h);
        // A same-column drop after a close still reorders within that column.
        let four = initial_columns(4);
        let three = columns_without(&four, 2);
        assert_eq!(three, [vec![0], vec![1, 2]]);
        let reordered = columns_after_swap(&three, 1, 2).unwrap();
        assert_eq!(reordered, [vec![0], vec![2, 1]]);
    }

    #[test]
    fn dropping_on_a_tile_swaps_across_and_reorders_within() {
        let four = initial_columns(4);
        let swapped = columns_after_swap(&four, 0, 1).unwrap();
        assert_eq!(swapped, [vec![1, 2], vec![0, 3]]);
        let r = rects_for(&swapped);
        assert!(r[0].x > r[1].x); // tile 0 is on the right now
        assert_eq!(r[0].h, r[1].h);
        let reordered = columns_after_swap(&four, 0, 2).unwrap();
        assert_eq!(reordered, [vec![2, 0], vec![1, 3]]);
        assert!(rects_for(&reordered)[0].y > 0);
        let back = columns_after_swap(&four, 3, 1).unwrap();
        assert_eq!(back, [vec![0, 2], vec![3, 1]]);
        assert_eq!(columns_after_swap(&four, 2, 2), None);
    }

    #[test]
    fn dropping_in_a_gap_inserts_and_the_column_gets_longer() {
        let four = initial_columns(4); // 2 + 2
        // Tile 0 (left, top) into the left gap below tile 2: it moves below
        // it, which changes that column's order only.
        assert_eq!(
            columns_after_insert(&four, 0, 0, 2).unwrap(),
            [vec![2, 0], vec![1, 3]]
        );
        // The gap directly below tile 0 is where it already is: no drop.
        assert_eq!(columns_after_insert(&four, 0, 0, 1), None);
        // Tile 0 into the right column below its last tile (row 2): the
        // right column gets longer, the left one shorter.
        let moved = columns_after_insert(&four, 0, 1, 2).unwrap();
        assert_eq!(moved, [vec![2], vec![1, 3, 0]]);
        assert_eq!(column_counts(&moved), (1, 3));
        // Tile 3 (right) below the left column's last tile (row 2).
        let moved = columns_after_insert(&four, 3, 0, 2).unwrap();
        assert_eq!(moved, [vec![0, 2, 3], vec![1]]);
        // The shorter column's tiles get taller, the longer one's shorter.
        let tall = rects_for(&moved);
        assert!(tall[1].h > tall[0].h);
    }

    #[test]
    fn a_drop_never_empties_a_column() {
        // 3 + 1: moving the lone right tile into the left column would leave
        // the right one empty while three tiles exist: refused.
        let hand = [vec![0, 1, 2], vec![3]];
        assert_eq!(columns_after_insert(&hand, 3, 0, 1), None);
        assert_eq!(
            apply_drop(&hand, 3, Drop::Insert { column: 0, row: 2 }),
            None
        );
        // Moving a tile *out of* a two-tile column is fine.
        let two = [vec![0, 1], vec![2]];
        assert!(columns_after_insert(&two, 0, 1, 1).is_some());
        // With a single tile there is nothing to refuse.
        let one = [vec![0], vec![]];
        assert!(columns_after_insert(&one, 0, 0, 1).is_none());
    }

    #[test]
    fn the_drop_zone_is_the_gap_plus_a_fifth_on_each_side() {
        let four = initial_columns(4);
        let rects = rects_for(&four);
        let s = slots_of(&four, &rects);
        let gap = scaled(GAP_L, 1.0);
        let band = s[0][0].h / 5;
        // Tile 0 is left, row 0; tile 1 right, row 0.
        let other = s[1][0];
        assert_eq!(
            drop_at(&four, &rects, 0, other.x + 10, other.y + other.h / 2, 1.0),
            Some(Drop::Swap(1))
        );
        // Just inside the band above the gap between the left tiles.
        let gap_y = s[0][1].y - gap / 2;
        assert_eq!(
            drop_at(&four, &rects, 0, s[0][0].x + 10, gap_y - band + 1, 1.0),
            Some(Drop::Insert { column: 0, row: 1 })
        );
        // Above the band it is the tile itself: a reorder, not a gap.
        assert_eq!(
            drop_at(&four, &rects, 0, s[0][0].x + 10, gap_y - band - 1, 1.0),
            None
        );
        // Below the left column's last tile is the last gap.
        let last = s[0][1];
        assert_eq!(
            drop_at(&four, &rects, 0, last.x + 10, last.y + last.h + 2, 1.0),
            Some(Drop::Insert { column: 0, row: 2 })
        );
        // Dropping on the other left tile (clear of the band) reorders.
        let lower = s[0][1];
        assert_eq!(
            drop_at(&four, &rects, 0, lower.x + 10, lower.y + lower.h / 2, 1.0),
            Some(Drop::Swap(2))
        );
        // The gutter itself is no drop.
        let gutter = s[0][0].x + s[0][0].w + gap / 2;
        assert_eq!(drop_at(&four, &rects, 0, gutter, 300, 1.0), None);
    }

    #[test]
    fn the_drop_line_spans_the_gap() {
        let four = initial_columns(4);
        let rects = rects_for(&four);
        let s = slots_of(&four, &rects);
        let line = drop_line(&s, Drop::Insert { column: 0, row: 1 }, 1.0).unwrap();
        assert_eq!(line.x, s[0][0].x);
        assert_eq!(line.w, s[0][0].w);
        assert_eq!(line.h, scaled(LINE_THIN, 1.0));
        assert!(line.y + line.h / 2 < s[0][1].y);
        assert!(line.y > s[0][0].y + s[0][0].h - scaled(GAP_L, 1.0));
        assert_eq!(drop_line(&s, Drop::Swap(1), 1.0), None);
    }

    #[test]
    fn border_drags_clamp_at_the_minimums() {
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let total = 4;
        let min = slot_minimum(total, &f, 1.0);
        let avail = f.inner_h - 3 * f.gutter;
        // Pull the first border far up: the first slot stops at the minimum.
        let shares = heights_after_drag(&equal_shares(2), avail, 1, -2000, min);
        let first = (avail as f64 * shares[0]).round() as i32;
        let second = (avail as f64 * shares[1]).round() as i32;
        assert_eq!(first, min);
        assert!(second >= min);
        assert!(first + second <= avail);
        // And far down: the second slot stops at the minimum.
        let shares = heights_after_drag(&equal_shares(2), avail, 1, 2000, min);
        let first = (avail as f64 * shares[0]).round() as i32;
        let second = (avail as f64 * shares[1]).round() as i32;
        assert_eq!(second, min);
        assert!(first >= min);
        // The width border clamps to one grid column / the pill, both sides.
        let min_w = min_column_width(1.0);
        let avail_w = f.inner_w - f.gutter;
        assert_eq!(
            (avail_w as f64 * width_share_after_drag(0.5, avail_w, -9999, min_w)).round() as i32,
            min_w
        );
        assert_eq!(
            (avail_w as f64 * width_share_after_drag(0.5, avail_w, 9999, min_w)).round() as i32,
            avail_w - min_w
        );
        assert!(min_w >= scaled(CELL_W, 1.0));
        // A pair that is already below two minimums cannot be resized.
        let tight = vec![0.5, 0.5];
        assert_eq!(heights_after_drag(&tight, 100, 1, 10, 202), tight);
    }

    #[test]
    fn a_double_click_resets_a_border_to_equal_shares() {
        // The reset is the model's own equal shares.
        assert_eq!(equal_shares(3), vec![1.0 / 3.0; 3]);
        let dragged = heights_after_drag(&equal_shares(3), 1000, 1, 120, 100);
        assert_ne!(dragged, equal_shares(3));
        assert_eq!(equal_shares(3), vec![1.0 / 3.0; 3]);
        // The column border's reset is the equal width share.
        let min_w = min_column_width(1.0);
        let dragged = width_share_after_drag(0.5, 1000, 120, min_w);
        assert!((dragged - 0.62).abs() < 0.001);
        assert_eq!(0.5, 0.5);
    }

    #[test]
    fn shares_survive_a_window_resize() {
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let avail = f.inner_h - f.gutter;
        let shares = heights_after_drag(&equal_shares(2), avail, 1, 200, slot_minimum(4, &f, 1.0));
        let big = layout(
            &initial_columns(4),
            &[shares.clone(), equal_shares(2)],
            0.7,
            1400,
            1600,
            1.0,
        );
        let small = layout(
            &initial_columns(4),
            &[shares.clone(), equal_shares(2)],
            0.7,
            1000,
            1200,
            1.0,
        );
        // The ratio of the two stacked tiles stays the same at both sizes.
        let ratio = |s: &[Rect]| s[0].h as f64 / s[1].h as f64;
        assert!((ratio(&big[0]) - ratio(&small[0])).abs() < 0.05);
        // And so does the column width share.
        let w = |s: &[Vec<Rect>; 2]| s[0][0].w as f64 / (s[0][0].w + s[1][0].w) as f64;
        assert!((w(&big) - w(&small)).abs() < 0.05);
        // At scale 0.5 the proportions hold too.
        let half = layout(
            &initial_columns(4),
            &[shares, equal_shares(2)],
            0.7,
            1400,
            1600,
            0.5,
        );
        assert!((w(&big) - w(&half)).abs() < 0.05);
    }

    #[test]
    fn adding_or_closing_resets_that_column() {
        let four = initial_columns(4);
        let dragged = [vec![0.7, 0.3], equal_shares(2)];
        // A close that only changes the left column resets it and keeps the
        // right one.
        let next = columns_without(&four, 2);
        let heights = heights_after_change(&four, &dragged, &next);
        assert_eq!(heights[0], equal_shares(1));
        assert_eq!(heights[1], dragged[1]);
        // An insert changes both lengths: both go back to equal shares.
        let moved = columns_after_insert(&four, 0, 1, 2).unwrap();
        let heights = heights_after_change(&four, &dragged, &moved);
        assert_eq!(heights[0], equal_shares(1));
        assert_eq!(heights[1], equal_shares(3));
        // A swap changes no length: every tile keeps the slot it lands in.
        let swapped = columns_after_swap(&four, 0, 1).unwrap();
        assert_eq!(
            heights_after_change(&four, &dragged, &swapped)[0],
            dragged[0]
        );
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
                    if let Some(next) = columns_after_swap(&columns, from, to) {
                        cases.push(next);
                    }
                }
                for column in 0..2 {
                    for row in 0..=columns[column].len() {
                        if let Some(next) = columns_after_insert(&columns, from, column, row) {
                            cases.push(next);
                        }
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
    fn hit_test() {
        let columns = initial_columns(4);
        let rects = rects_for(&columns);
        let slots = slots_of(&columns, &rects);
        assert!(slots[0][0].contains(100, 100));
        assert!(slots[1][0].contains(900, 100));
        assert!(slots[0][1].contains(100, 900));
        assert!(slots[1][1].contains(900, 900));
        // The gutter holds no tile, so a drop there has no target.
        let gutter = slots[0][0].x + slots[0][0].w + scaled(GAP_L, 1.0) / 2;
        assert!(!slots[0][0].contains(gutter, 100));
        assert!(!slots[1][0].contains(gutter, 100));
        assert_eq!(drop_at(&columns, &rects, 0, gutter, 100, 1.0), None);
        assert_eq!(drop_at(&columns, &rects, 0, -5, 100, 1.0), None);
    }

    #[test]
    fn border_hit_test() {
        let columns = initial_columns(4);
        let rects = rects_for(&columns);
        let slots = slots_of(&columns, &rects);
        let gap = scaled(GAP_L, 1.0);
        // The column gutter, at any height.
        let x = slots[0][0].x + slots[0][0].w + gap / 2;
        assert_eq!(border_at(&slots, x, 40), Some(Border::Columns));
        // The border between the left column's tiles.
        let y = slots[0][0].y + slots[0][0].h + gap / 2;
        assert_eq!(
            border_at(&slots, slots[0][0].x + 40, y),
            Some(Border::Heights { column: 0, row: 1 })
        );
        // A tile's middle is no border.
        assert_eq!(border_at(&slots, slots[0][0].x + 40, 100), None);
        // The right column's own border is at its own y.
        let y = slots[1][0].y + slots[1][0].h + gap / 2;
        assert_eq!(
            border_at(&slots, slots[1][0].x + 40, y),
            Some(Border::Heights { column: 1, row: 1 })
        );
    }

    #[test]
    fn pill_hit_test() {
        let rects = rects_for(&initial_columns(4));
        let r = rects[0];
        let px = r.x + (r.w - scaled(PILL_W, 1.0)) / 2;
        let ph = scaled(CONTROL_H, 1.0);
        assert_eq!(pill_at(&rects, px + 10, r.y + 10, 1.0), Some(0));
        assert_eq!(pill_at(&rects, r.x + 10, r.y + 10, 1.0), None);
        assert_eq!(pill_at(&rects, px + 10, r.y + ph, 1.0), None);
        assert!(!press_on_pill(10.0, 10.0, 500, 1.0));
        assert!(press_on_pill(240.0, 10.0, 500, 1.0));
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
        let removed = selection_after(&added, 2, true, false);
        assert_eq!(removed.selected, [false, false, false, false]);
        assert_eq!(removed.anchor, 0);
    }

    #[test]
    fn shift_click_selects_the_range_from_the_previous_focus() {
        let mut state = Selection::new(5);
        state = selection_after(&state, 0, false, false);
        let range = selection_after(&state, 3, false, true);
        assert_eq!(range.selected, [true, true, true, true, false]);
        assert_eq!(range.focus, 3);
        assert_eq!(range.anchor, 0);
        let back = selection_after(&range, 1, false, true);
        assert_eq!(back.selected, [true, true, false, false, false]);
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
    fn drag_follows_the_pointer_and_previews_the_drop() {
        let columns = initial_columns(4);
        let heights = frames();
        let rects = rects_for(&columns);
        let s = slots_of(&columns, &rects);
        let other = s[1][0];
        // Tile 0 over tile 1: a swap; every other tile sits at its new slot.
        let board = Board {
            columns: &columns,
            heights: &heights,
            width_share: 0.5,
            view: Viewport {
                w: VIEW.0,
                h: VIEW.1,
                scale: 1.0,
            },
        };
        let (drop, preview) = drag_layout(&board, &rects, 0, other.x + 10, other.y + 10, (0, 0));
        assert_eq!(drop, Some(Drop::Swap(1)));
        assert_eq!(
            preview[0],
            Rect {
                x: other.x + 10,
                y: other.y + 10,
                ..rects[0]
            }
        );
        let next = columns_after_swap(&columns, 0, 1).unwrap();
        let final_rects = rects_for(&next);
        for tile in 1..4 {
            assert_eq!(preview[tile], final_rects[tile], "tile {tile}");
        }
        // Over the gutter nothing is displaced, but the tile still follows.
        let gutter = s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0) / 2;
        let (drop, preview) = drag_layout(&board, &rects, 0, gutter, 40, (0, 0));
        assert_eq!(drop, None);
        for tile in 1..4 {
            assert_eq!(preview[tile], rects[tile]);
        }
    }

    #[test]
    fn a_drag_respects_the_grab_offset() {
        let columns = initial_columns(4);
        let heights = frames();
        let rects = rects_for(&columns);
        let board = Board {
            columns: &columns,
            heights: &heights,
            width_share: 0.5,
            view: Viewport {
                w: VIEW.0,
                h: VIEW.1,
                scale: 1.0,
            },
        };
        let (_, preview) = drag_layout(&board, &rects, 0, 900, 700, (30, 20));
        assert_eq!(
            preview[0],
            Rect {
                x: 870,
                y: 680,
                ..rects[0]
            }
        );
    }
}
