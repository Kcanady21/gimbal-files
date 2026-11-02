// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view (Kyle's tab groups): tiles in two columns inside a
//! gtk::Fixed, with identity (a tile's name moves with it), focus,
//! selection, close, the fade, live drag rearranging, gap drops with an
//! accent line, and draggable borders.
//!
//! The model is three pure things this file's tests exercise:
//!
//! * **columns** — which tile ids each column holds, top to bottom;
//! * **heights** — either the default (equal shares of the visible height,
//!   with the half-height floor beyond four tiles) or hand-set heights in
//!   unscaled design pixels, and the left column's width as a fraction of the
//!   width left after the gutter. Storing hand heights in design pixels keeps
//!   a hand-sized tile's size through a window resize or a scale change; the
//!   last tile of a short column stretches to fill it;
//! * **layout** — the slot rects those heights produce.
//!
//! The drag gesture rides the static `Fixed` (never a moving pill: every
//! widget's event coordinates are translated into its own space at
//! dispatch, so a gesture on the moving pill would see its offsets
//! collapse). One gesture decides at the press whether it will move a tile
//! (press on a pill) or a border (press on a gutter), so the two can never
//! fight; nothing is claimed or moved before the drag threshold.
use crate::style::{
    CELL_H, CELL_W, CONTROL_H, GAP_2XL, GAP_L, GAP_M, GAP_S, ICON_TAB_CLOSE, LINE_MID, LINE_THIN,
    PILL_W, TEXT_CONTROL, scaled,
};
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

/// Everything a layout needs: the columns, their heights, the column width
/// share, the viewport and the scroll offset. A `Default` column uses the
/// half-height floor beyond four tiles; a `Hand` one has no floor.
#[derive(Clone, Copy)]
pub struct Board<'a> {
    pub columns: &'a [Vec<usize>; 2],
    pub heights: &'a [Heights; 2],
    pub width_share: f64,
    pub view: Viewport,
    pub scroll: i32,
}

impl Board<'_> {
    /// The committed slot layout of these columns and heights, with each
    /// column's scroll offset applied: a column that fits in the viewport
    /// stays pinned at the scroll position while the taller one scrolls.
    pub fn slots(&self) -> [Vec<Rect>; 2] {
        let mut slots = layout(
            self.columns,
            self.heights,
            self.width_share,
            self.view.w,
            self.view.h,
            self.view.scale,
        );
        offset_slots(&mut slots, self.view.h, self.view.scale, self.scroll);
        slots
    }

    /// The rect of every tile, indexed by tile id.
    pub fn rects(&self) -> Vec<Rect> {
        rects_by_tile(self.columns, &self.slots())
    }
}

/// The narrowest a column may be dragged: one grid column wide. A tile can be
/// narrower than its `PILL_W` pill: the pill loses its 270 px minimum below
/// that width and its label ellipsizes, and the tile's content sits in a
/// clipping scroller, so nothing draws over the gutter (P1m-r4 item 4).
pub fn min_column_width(scale: f64) -> i32 {
    scaled(CELL_W, scale)
}

/// Whether a tab pill at `tile_w` is wide enough for the design's `PILL_W`.
/// Below it the `wide` class (and CSS `min-width`) comes off.
pub fn pill_is_wide(tile_w: i32, scale: f64) -> bool {
    tile_w >= scaled(PILL_W, scale)
}

/// The label's natural-width cap in characters for a tile `tile_w` wide, so a
/// pill below `PILL_W` shrinks with the column and ellipsizes instead of
/// drawing its close button outside the tile. The overhead is the pill's
/// padding, border, close ring and its gap-m margin.
pub fn pill_label_chars(tile_w: i32, scale: f64) -> i32 {
    let overhead = pill_overhead(scale);
    let char_w = pill_char_width(scale);
    ((tile_w - overhead).max(0) / char_w).max(1)
}

fn pill_overhead(scale: f64) -> i32 {
    2 * scaled(GAP_M, scale)
        + 2 * scaled(LINE_MID, scale)
        + scaled(ICON_TAB_CLOSE, scale)
        + scaled(GAP_M, scale)
}

fn pill_char_width(scale: f64) -> i32 {
    (scaled(TEXT_CONTROL, scale) * 3 / 5).max(1)
}

/// The width the pill actually takes in a tile `tile_w` wide: `PILL_W` when
/// the tile is wide enough, otherwise the shrinking pill's overhead plus its
/// capped label. The hit tests use this so the strip beside a narrow pill
/// still grabs.
pub fn pill_width(tile_w: i32, scale: f64) -> i32 {
    if tile_w <= 0 {
        return 0;
    }
    if pill_is_wide(tile_w, scale) {
        scaled(PILL_W, scale).min(tile_w)
    } else {
        // The names are "Tab 1".."Tab 10": six characters is the widest, so
        // the shrunk pill is never wider than this, and the strip beside it
        // keeps its grab area.
        (pill_overhead(scale) + 6 * pill_char_width(scale)).min(tile_w)
    }
}

/// The shortest a tile's slot may be dragged: its pill, the pill's gap and
/// one grid row of content.
pub fn min_slot_height(scale: f64) -> i32 {
    scaled(CONTROL_H + GAP_S + CELL_H, scale)
}

/// The half-visible-height floor that a default layout applies beyond four
/// tiles, else 0. A hand-dragged column ignores it (item 2): its border may
/// shrink a tile to `min_slot_height`.
pub fn half_floor(total: usize, f: &Frame) -> i32 {
    if total > 4 {
        ((f.inner_h - f.gutter) / 2).max(1)
    } else {
        0
    }
}

/// One column's slot heights.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Heights {
    /// Equal shares of the visible height, with the half-height floor beyond
    /// four tiles. What a reset (double-click, add, close) sets.
    Default,
    /// Hand-set heights in unscaled design pixels, one per slot, top to
    /// bottom. Only the tile above a dragged border changes; everything below
    /// keeps its height. The last tile's fill is not stored here.
    Hand(Vec<i32>),
}

/// Equal heights for `n` slots in design pixels; the stored form of a
/// default layout in a column of `n` tiles whose visible inner height is
/// `inner_h` device px, with `floor` already in device px.
pub fn default_heights(n: usize, inner_h: i32, gap: i32, floor: i32, scale: f64) -> Vec<i32> {
    if n == 0 {
        return vec![];
    }
    let avail = (inner_h - (n as i32 - 1) * gap).max(1);
    let each = (avail / n as i32).max(1).max(floor);
    vec![(each as f64 / scale).round().max(1.0) as i32; n]
}

/// The heights a column's slots remember, before the last tile's fill: the
/// equal default or the hand-set design pixels (padded from the default when
/// a hand vector is short).
pub fn stored_heights(
    heights: &Heights,
    n: usize,
    inner_h: i32,
    gap: i32,
    floor: i32,
    scale: f64,
) -> Vec<i32> {
    let default = default_heights(n, inner_h, gap, floor, scale);
    match heights {
        Heights::Default => default,
        Heights::Hand(v) => (0..n)
            .map(|row| v.get(row).copied().unwrap_or(default[row]))
            .collect(),
    }
}

/// The heights actually drawn for a column ending above the visible height:
/// the last tile stretches to fill what is left, the others keep their
/// heights. `visible` and `gap` are unscaled design pixels.
pub fn fill_last(heights: &[i32], visible: i32, gap: i32) -> Vec<i32> {
    let n = heights.len();
    if n == 0 {
        return vec![];
    }
    let mut out = heights.to_vec();
    let used: i32 = out.iter().sum::<i32>() + (n as i32 - 1) * gap;
    if used < visible
        && let Some(last) = out.last_mut()
    {
        *last += visible - used;
    }
    out
}

/// The result of dragging a stacked border: the heights to remember (the last
/// tile's fill excluded, so it can change on the next resize) and the heights
/// to draw (the last tile filled).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    pub stored: Vec<i32>,
    pub shown: Vec<i32>,
}

/// Push the border at `border` (1..n, between slots border−1 and border) down
/// by `dy` unscaled design pixels. Only the tile above the border changes: it
/// grows by `dy` or shrinks, but never below `min_h`. Every tile below keeps
/// its height and moves with the border, so the stored column total grows or
/// shrinks by exactly `dy`; a column that ends short of `visible` has its last
/// tile filled.
pub fn heights_after_push(
    heights: &[i32],
    border: usize,
    dy: i32,
    min_h: i32,
    visible: i32,
    gap: i32,
) -> Pushed {
    let mut stored = heights.to_vec();
    if stored.len() >= 2 && border > 0 && border < stored.len() {
        stored[border - 1] = (stored[border - 1] + dy).max(min_h.max(1));
    }
    let shown = fill_last(&stored, visible, gap);
    Pushed { stored, shown }
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

/// The slot rects of both columns, left first, top to bottom. A slot is the
/// tile's pill plus its frame: the pill is `control-h` tall and sits `gap-s`
/// above the frame. A default column shares its available height equally, so
/// a column with fewer tiles gets taller ones, whichever column it is; beyond
/// four tiles the default keeps every slot at least half the visible height
/// and the view scrolls. A hand-set column draws its stored design heights
/// (scaled), with no floor, and its last tile fills any space left under it.
pub fn layout(
    columns: &[Vec<usize>; 2],
    heights: &[Heights; 2],
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
        // The drawn heights in device pixels: the default's equal share or
        // the scaled hand heights, then the last tile fills a column that
        // ends short of the visible height.
        let mut shown: Vec<i32> = match &heights[column] {
            Heights::Default => {
                let avail = (f.inner_h - (n as i32 - 1) * gap).max(1);
                vec![(avail / n as i32).max(pill + 1).max(half); n]
            }
            Heights::Hand(v) => {
                let default = default_heights(n, f.inner_h, gap, half, scale);
                (0..n)
                    .map(|row| {
                        let design = v.get(row).copied().unwrap_or(default[row]);
                        scaled(design as f64, scale).max(pill + 1).max(1)
                    })
                    .collect()
            }
        };
        let used: i32 = shown.iter().sum::<i32>() + (n as i32 - 1) * gap;
        if used < f.inner_h
            && let Some(last) = shown.last_mut()
        {
            *last += f.inner_h - used;
        }
        let mut y = 0;
        let mut out = Vec::with_capacity(n);
        for h in shown {
            out.push(Rect { x, y, w, h });
            y += h + gap;
        }
        out
    })
}

/// Whether one column's tiles fit inside the visible height.
pub fn column_fits(slots: &[Rect], view_h: i32, scale: f64) -> bool {
    let f = frame(1, view_h, scale);
    slots
        .last()
        .map(|last| last.y + last.h <= f.inner_h + 1)
        .unwrap_or(true)
}

/// Each column's y offset for a scroll value (item 1): a column whose tiles
/// fit stays pinned at the scroll offset, so it does not move on screen; a
/// taller column keeps offset 0 and scrolls with the shared content.
pub fn column_offsets(slots: &[Vec<Rect>; 2], view_h: i32, scale: f64, scroll: i32) -> [i32; 2] {
    let scroll = scroll.max(0);
    [0, 1].map(|column| {
        if column_fits(&slots[column], view_h, scale) {
            scroll
        } else {
            0
        }
    })
}

/// Move each column's slots by its scroll offset, in place.
pub fn offset_slots(slots: &mut [Vec<Rect>; 2], view_h: i32, scale: f64, scroll: i32) {
    let offsets = column_offsets(slots, view_h, scale, scroll);
    for (column, column_slots) in slots.iter_mut().enumerate() {
        for slot in column_slots {
            slot.y += offsets[column];
        }
    }
}

/// The tile's frame inside its slot: the slot minus the pill band above it
/// (`control-h` plus `gap-s`). The frame's bottom is the slot's bottom.
pub fn frame_rect(slot: Rect, scale: f64) -> Rect {
    let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
    Rect {
        x: slot.x,
        y: slot.y + band,
        w: slot.w,
        h: (slot.h - band).max(0),
    }
}

/// The grid scroller's size inside a tile: the frame's content box. The grid
/// view's own `gap-m` margins sit inside the scroller. It does not shrink
/// while the tile is stuck, so the contents never re-lay out, only slide
/// under the frame's top border.
pub fn grid_size(slot: Rect, scale: f64) -> (i32, i32) {
    let thin = scaled(LINE_THIN, scale);
    let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
    (
        (slot.w - 2 * thin).max(1),
        (slot.h - band - 2 * thin).max(1),
    )
}

/// The shortest a stuck tile's frame may stay visible: one grid row.
pub fn min_frame_height(scale: f64) -> i32 {
    scaled(CELL_H, scale).max(1)
}

/// A tile's stuck geometry (P1m-r7). `slot` and `frame` are in content
/// coordinates (`frame` is `frame_rect(slot, scale)`), `scroll` is the outer
/// scroller's value, and the result is in content coordinates too, because
/// that is the space the split's Fixed children live in: subtract `scroll`
/// for view coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stuck {
    /// The pill's content y. View y is this minus `scroll`.
    pub pill_y: i32,
    /// The frame's on-screen box in content coordinates. While the tile is
    /// stuck its top border stays `gap-s` under the pill (view y
    /// `pill_h + gap_s`) and its bottom keeps moving with the page.
    pub frame: Rect,
    /// How far the contents move up inside the frame: the distance the page
    /// has scrolled past the slot's top. The grid scrolls with the page and
    /// is clipped under the frame's own top border.
    pub content_offset: i32,
}

/// The frame's top in content coordinates for a slot: its natural top until
/// the slot's top goes above the view, then the fixed stuck top, and finally
/// (with the tile down to its minimum) the frame moving off with its bottom.
pub fn stuck_geometry(
    slot: Rect,
    frame: Rect,
    scroll: i32,
    visible_h: i32,
    pill_h: i32,
    minimum: i32,
) -> Stuck {
    let natural = slot.y - scroll;
    // The pill band above the frame: the pill itself plus `gap-s`.
    let gap = frame.y - slot.y - pill_h;
    let band = pill_h + gap;
    let bottom = frame.y + frame.h - scroll;
    let minimum = minimum.clamp(0, frame.h.max(0));
    let not_stuck = Stuck {
        pill_y: slot.y,
        frame,
        content_offset: 0,
    };
    // The slot's top is still in view (or the whole tile is below the view):
    // the pill and the frame simply travel with the page.
    if natural >= 0 || frame.y - scroll >= visible_h {
        return not_stuck;
    }
    if bottom - band < minimum {
        // Below the minimum the whole tile, pill included, scrolls off
        // together: pill, frame top and frame bottom move up as one, and the
        // contents freeze at the last scrolled position.
        let d = band + minimum - bottom;
        return Stuck {
            pill_y: scroll - d,
            frame: Rect {
                x: frame.x,
                y: frame.y + frame.h - minimum,
                w: frame.w,
                h: minimum,
            },
            content_offset: (frame.h - minimum).max(0),
        };
    }
    // Stuck: the pill at the view's top, the frame's top `gap-s` under it,
    // the frame's bottom still on the slot's bottom, contents scrolled past
    // the slot top and clipped under the frame's top border.
    Stuck {
        pill_y: scroll,
        frame: Rect {
            x: frame.x,
            y: scroll + band,
            w: frame.w,
            h: bottom - band,
        },
        content_offset: scroll - slot.y,
    }
}

/// The pill's rect in content coordinates (what the hit tests use): centred
/// in the slot, `pill_h` tall, at its stuck position. `slot` already carries
/// its column's scroll offset, like `State.rects`.
pub fn pill_content_rect(slot: Rect, scroll: i32, visible_h: i32, scale: f64) -> Rect {
    let pill_h = scaled(CONTROL_H, scale);
    let stuck = stuck_geometry(
        slot,
        frame_rect(slot, scale),
        scroll,
        visible_h,
        pill_h,
        min_frame_height(scale),
    );
    let pw = pill_width(slot.w, scale);
    Rect {
        x: slot.x + (slot.w - pw) / 2,
        y: stuck.pill_y,
        w: pw,
        h: pill_h,
    }
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
/// tile goes back to the default; the others keep theirs. A swap or a reorder
/// changes no length, so each column keeps its heights and a tile takes the
/// slot height it lands in.
pub fn heights_after_change(
    old_columns: &[Vec<usize>; 2],
    old_heights: &[Heights; 2],
    new_columns: &[Vec<usize>; 2],
) -> [Heights; 2] {
    [0, 1].map(|column| {
        if old_columns[column].len() == new_columns[column].len() {
            old_heights[column].clone()
        } else {
            Heights::Default
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

/// The pill's own region inside a slot (its sticky position, centred and at
/// most the column's width).
fn pill_zone(r: Rect, x: i32, y: i32, scroll: i32, visible_h: i32, scale: f64) -> bool {
    pill_content_rect(r, scroll, visible_h, scale).contains(x, y)
}

/// A border under the pointer: the gutter between the columns, or the whole
/// strip between two stacked tiles of one column (item 2). The strip runs
/// from the upper tile's frame bottom to the lower tile's frame top, and
/// grabs everywhere except on a pill (wherever scrolling has drawn it).
pub fn border_at(
    slots: &[Vec<Rect>; 2],
    x: i32,
    y: i32,
    scroll: i32,
    visible_h: i32,
    scale: f64,
) -> Option<Border> {
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
    // A sticky pill can be drawn over a strip; it is never a border.
    if slots
        .iter()
        .flatten()
        .any(|r| pill_zone(*r, x, y, scroll, visible_h, scale))
    {
        return None;
    }
    let pill_h = scaled(CONTROL_H, scale);
    let minimum = min_frame_height(scale);
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
            // The strip's lower end is the next frame's *visible* top: while
            // that tile is stuck it is pinned under the pill, so the strip
            // follows the frame's top; its upper end already follows a stuck
            // tile's moving frame bottom through `top`.
            let next_frame_top = stuck_geometry(
                s[row],
                frame_rect(s[row], scale),
                scroll,
                visible_h,
                pill_h,
                minimum,
            )
            .frame
            .y;
            if y >= top && y < next_frame_top {
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
                let mut slots = layout(
                    &next,
                    &next_heights,
                    board.width_share,
                    board.view.w,
                    board.view.h,
                    board.view.scale,
                );
                offset_slots(&mut slots, board.view.h, board.view.scale, board.scroll);
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

/// The pill hit test: which tile's pill region holds the point, in content
/// coordinates. The pill is centred above the tile's frame and moves with
/// `scroll` once its slot's top has gone above the view (P1m-r6).
pub fn pill_at(
    rects: &[Rect],
    x: i32,
    y: i32,
    scroll: i32,
    visible_h: i32,
    scale: f64,
) -> Option<usize> {
    rects
        .iter()
        .position(|r| pill_content_rect(*r, scroll, visible_h, scale).contains(x, y))
}

#[cfg(test)]
#[cfg(test)]
/// Whether a press at (x, y) inside a `tile_w`-wide tile landed on its pill,
/// whose top sits `pill_top` down from the tile's top (0 unless it is stuck).
pub fn press_on_pill(x: f64, y: f64, tile_w: i32, pill_top: i32, scale: f64) -> bool {
    if tile_w <= 0 {
        return false;
    }
    let pw = pill_width(tile_w, scale);
    let px = (tile_w - pw) / 2;
    let ph = scaled(CONTROL_H, scale);
    x >= px as f64 && x < (px + pw) as f64 && y >= pill_top as f64 && y < (pill_top + ph) as f64
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
    /// Each column's slot heights: the default or hand-set design pixels.
    heights: [Heights; 2],
    /// The left column's share of the width left after the gutter.
    width_share: f64,
    /// The outer scroller's vertical value: a fitting column is pinned there.
    scroll: i32,
    widgets: Vec<gtk::Box>, // indexed by tile id: the outer pill+frame box
    frames: Vec<gtk::Box>,
    /// The fixed planes that hold each tile's grid and the grid scrollers
    /// themselves, moved up while the tile is stuck so the contents keep
    /// scrolling with the page (P1m-r7).
    grid_planes: Vec<gtk::Fixed>,
    grids: Vec<gtk::ScrolledWindow>,
    pills: Vec<gtk::Box>,
    /// The pill clipping scrollers, indexed by tile id: Fixed children that
    /// follow the tile's slot and the scroll value (P1m-r6).
    pill_wraps: Vec<gtk::ScrolledWindow>,
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
            scroll: self.scroll,
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
        press: (i32, i32),
        targets: Vec<Rect>,
    },
    /// The column border: the width share follows the pointer.
    Width { share: f64, press_x: i32 },
    /// A border between two stacked tiles: the height of the tile above
    /// follows the pointer, pushing the tiles below. `heights` are the column's
    /// design-pixel heights at the press, so every update applies the whole
    /// pointer delta to the same starting sizes.
    Height {
        column: usize,
        border: usize,
        heights: Vec<i32>,
        press_y: i32,
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

/// One tile's widget tree (P1m-r6/r7). The tile itself holds only the framed
/// pane; its pill lives in `pill_wrap`, a clipping scroller that is a sibling
/// of the tile in the split's Fixed, so the pill can stay at the top of the
/// view while the tile scrolls and still be cut to the tile's width. The
/// frame clips its own contents and holds `grid_plane`, a fixed that moves the
/// grid up inside it by the amount the page scrolled past the slot (P1m-r7).
struct TileWidgets {
    outer: gtk::Box,
    frame: gtk::Box,
    grid_plane: gtk::Fixed,
    grid: gtk::ScrolledWindow,
    pill: gtk::Box,
    close: gtk::Button,
    pill_wrap: gtk::ScrolledWindow,
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
    scroll_pending: Cell<bool>,
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

/// Follow the outer scroller's value (item 1). The layout change is scheduled
/// in an idle: a `value` notify can arrive while the ScrolledWindow is inside
/// its own allocation, where moving or resizing children is ignored or warns.
fn watch_scroll(inner: &Rc<Inner>, adj: &gtk::Adjustment) {
    let weak = Rc::downgrade(inner);
    adj.connect_value_changed(move |adj| {
        if let Some(inner) = weak.upgrade() {
            let value = adj.value().round() as i32;
            if inner.state.borrow().scroll != value {
                inner.state.borrow_mut().scroll = value;
                inner.schedule_scroll();
            }
        }
    });
}

/// The pointer in the Fixed's own content space: the surface position minus
/// the window's surface transform (the client-side shadow GTK subtracts for
/// gesture points) and the Fixed's bounds in the window. Presses, hover and
/// every drag update go through this one conversion (item 5).
fn pointer_in_fixed<W: IsA<gtk::Widget>>(
    fixed: &W,
    event: Option<&gtk::gdk::Event>,
) -> Option<(f64, f64)> {
    let (sx, sy) = event?.position()?;
    let transform = fixed
        .native()
        .map(|native| native.surface_transform())
        .unwrap_or((0.0, 0.0));
    let root = fixed.root()?;
    let bounds = fixed.compute_bounds(&root)?;
    Some((
        sx - transform.0 - bounds.x() as f64,
        sy - transform.1 - bounds.y() as f64,
    ))
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
                    heights: [Heights::Default, Heights::Default],
                    width_share: 0.5,
                    scroll: 0,
                    widgets: vec![],
                    frames: vec![],
                    grid_planes: vec![],
                    grids: vec![],
                    pills: vec![],
                    pill_wraps: vec![],
                    rects: vec![],
                    selection: Selection::new(0),
                    scale: 1.0,
                    view: [1200, 1280],
                    next_uid: 0,
                }),
                on_exit,
                handle: RefCell::new(None),
                slots: RefCell::new(vec![]),
                drag: RefCell::new(None),
                cursor: Cell::new(Cursor::None),
                scroll_pending: Cell::new(false),
            }),
        };
        this.inner.handle.replace(Some(this.clone()));
        watch_viewport(&this.inner, 1, &this.inner.outer.vadjustment());
        watch_viewport(&this.inner, 0, &this.inner.outer.hadjustment());
        watch_scroll(&this.inner, &this.inner.outer.vadjustment());
        // One drag gesture for tiles and borders: the press decides which.
        // It never claims (gtkgesturedrag.c claims nothing), so clicks keep
        // working; nothing moves before the threshold.
        let drag = gtk::GestureDrag::new();
        let begin_view = this.clone();
        drag.connect_drag_begin(move |g, sx, sy| begin_view.inner.begin_drag(g, sx, sy));
        let update_view = this.clone();
        drag.connect_drag_update(move |g, dx, dy| update_view.inner.update_drag(g, dx, dy));
        let end_view = this.clone();
        drag.connect_drag_end(move |g, dx, dy| end_view.inner.end_drag(g, dx, dy));
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
            for wrap in st.pill_wraps.drain(..) {
                self.fixed.remove(&wrap);
            }
            st.tiles.clear();
            st.columns = [vec![], vec![]];
            st.heights = [Heights::Default, Heights::Default];
            st.width_share = 0.5;
            st.scroll = 0;
            st.pills.clear();
            st.frames.clear();
            st.grid_planes.clear();
            st.grids.clear();
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
        self.state.borrow_mut().heights = [Heights::Default, Heights::Default];
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
        let (index, name, parts) = {
            let mut st = self.state.borrow_mut();
            let index = st.tiles.len();
            let uid = st.next_uid;
            st.next_uid += 1;
            let name = format!("Tab {}", uid + 1);
            let parts = Self::tile_widget(&name, items, st.scale);
            Self::attach_handlers(view, uid, &parts.outer, &parts.pill, &parts.close);
            st.tiles.push(Tile {
                uid,
                name: name.clone(),
            });
            let column = add_to_shorter(&mut st.columns, index);
            // A column that gained a tile goes back to the default.
            st.heights[column] = Heights::Default;
            st.widgets.push(parts.outer.clone());
            st.frames.push(parts.frame.clone());
            st.grid_planes.push(parts.grid_plane.clone());
            st.grids.push(parts.grid.clone());
            st.pills.push(parts.pill.clone());
            st.pill_wraps.push(parts.pill_wrap.clone());
            st.selection.selected.push(false);
            (index, name, parts)
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
        let band = {
            let scale = self.state.borrow().scale;
            scaled(CONTROL_H, scale) + scaled(GAP_S, scale)
        };
        self.fixed.put(&parts.outer, rect.x as f64, rect.y as f64);
        parts.outer.set_size_request(rect.w, rect.h);
        // The pill's scroller sits right after the tile, so the pill paints
        // over its frame, and is cut to the tile's width.
        parts.pill_wrap.set_size_request(rect.w, band);
        self.fixed
            .put(&parts.pill_wrap, rect.x as f64, rect.y as f64);
        parts
            .pill_wrap
            .insert_after(&self.fixed, Some(&parts.outer));
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
        let (widget, wrap, name) = {
            let st = self.state.borrow();
            let Some(widget) = st.widgets.get(index).cloned() else {
                return;
            };
            (
                widget,
                st.pill_wraps.get(index).cloned(),
                st.tiles[index].name.clone(),
            )
        };
        self.bump_gen(index);
        self.fixed.remove(&widget);
        if let Some(wrap) = wrap {
            self.fixed.remove(&wrap);
        }
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
            st.grid_planes.remove(index);
            st.grids.remove(index);
            st.pills.remove(index);
            st.pill_wraps.remove(index);
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
    /// The stuck geometry is a view position, not part of the layout: the
    /// pill's wrap, the frame's top margin and the grid's content offset are
    /// applied here from the same scroll value the pinned column uses.
    fn relayout(&self, animate: bool) {
        let (rects, widgets, pills, wraps, frames, planes, grids, scale, scroll, view_h) = {
            let mut st = self.state.borrow_mut();
            let rects = st.rects();
            // Until the scroller has a viewport, keep the provisional layout:
            // applying an empty one would resize every grid to 0×0 (a GTK
            // list-base assertion at startup).
            if !st.tiles.is_empty() && rects.iter().all(|r| r.w <= 0 || r.h <= 0) {
                return;
            }
            st.rects = rects;
            (
                st.rects.clone(),
                st.widgets.clone(),
                st.pills.clone(),
                st.pill_wraps.clone(),
                st.frames.clone(),
                st.grid_planes.clone(),
                st.grids.clone(),
                st.scale,
                st.scroll,
                st.view[1],
            )
        };
        let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
        let pill_h = scaled(CONTROL_H, scale);
        let minimum = min_frame_height(scale);
        for (slot, widget) in widgets.iter().enumerate() {
            if let Some(&rect) = rects.get(slot) {
                if let Some(pill) = pills.get(slot) {
                    let wide = pill_is_wide(rect.w, scale);
                    if wide {
                        pill.add_css_class("wide");
                    } else {
                        pill.remove_css_class("wide");
                    }
                    if let Some(label) = pill.first_child().and_downcast::<gtk::Label>() {
                        label.set_max_width_chars(if wide {
                            -1
                        } else {
                            pill_label_chars(rect.w, scale)
                        });
                    }
                }
                let stuck = stuck_geometry(
                    rect,
                    frame_rect(rect, scale),
                    scroll,
                    view_h,
                    pill_h,
                    minimum,
                );
                // The frame's top margin keeps its border `gap-s` under the
                // pill while stuck; its bottom keeps the slot's bottom.
                if let Some(frame) = frames.get(slot) {
                    let top = stuck.frame.y - rect.y;
                    if frame.margin_top() != top {
                        frame.set_margin_top(top);
                    }
                }
                // The grid moves up with the page inside the clipping frame.
                if let (Some(plane), Some(grid)) = (planes.get(slot), grids.get(slot)) {
                    let (w, h) = grid_size(rect, scale);
                    grid.set_size_request(w, h);
                    plane.move_(grid, 0.0, -(stuck.content_offset as f64));
                }
                if let Some(wrap) = wraps.get(slot) {
                    wrap.set_size_request(rect.w.max(1), band.max(1));
                    self.fixed.move_(wrap, rect.x as f64, stuck.pill_y as f64);
                    // The pill paints over its own tile.
                    wrap.insert_after(&self.fixed, Some(widget));
                }
                if animate {
                    self.slide_to(slot, widget, rect);
                } else {
                    self.set_rect(slot, widget, rect);
                }
            }
        }
    }

    /// Re-apply the layout after a scroll, one idle later (see `watch_scroll`).
    fn schedule_scroll(&self) {
        if self.scroll_pending.replace(true) {
            return;
        }
        let Some(handle) = self.handle.borrow().clone() else {
            self.scroll_pending.set(false);
            return;
        };
        let weak = Rc::downgrade(&handle.inner);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                inner.scroll_pending.set(false);
                // A tile drag owns the visuals until it ends.
                if inner.drag.borrow().is_none() {
                    inner.relayout(false);
                }
            }
        });
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

    /// One tile: the framed pane in a clipping scroller, with the frame's
    /// top margin leaving the pill band. The tab pill lives in its own
    /// clipping scroller (`pill_wrap`), a sibling of the tile in the split's
    /// Fixed (P1m-r6): a `GtkFixed` allocates a child its *preferred* size,
    /// not the size it was asked for (gtk-4.22.4:gtk/gtkfixedlayout.c:297-322),
    /// and a ScrolledWindow with an EXTERNAL policy measures zero
    /// (gtk-4.22.4:gtk/gtkscrolledwindow.c:1847-1851), so `set_size_request`
    /// fixes the wrap to the tile's width and the scroller cuts the pill
    /// before it can draw over the neighbour. `relayout` moves it to the
    /// sticky y, outside the slot allocation.
    fn tile_widget(name: &str, items: u32, scale: f64) -> TileWidgets {
        let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let clip = gtk::ScrolledWindow::new();
        clip.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        clip.set_propagate_natural_width(false);
        clip.set_propagate_natural_height(false);
        clip.set_hexpand(true);
        clip.set_vexpand(true);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let pill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill.add_css_class("gl-pill");
        pill.add_css_class("tab");
        pill.set_halign(gtk::Align::Center);
        let label = gtk::Label::new(Some(name));
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        pill.append(&label);
        let close = gtk::Button::new();
        close.add_css_class("gl-pill-close");
        close.set_halign(gtk::Align::Center);
        close.set_valign(gtk::Align::Center);
        close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
        pill.append(&close);
        let pill_row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        pill_row.append(&pill);
        let pill_wrap = gtk::ScrolledWindow::new();
        pill_wrap.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
        pill_wrap.set_propagate_natural_width(false);
        pill_wrap.set_propagate_natural_height(false);
        pill_wrap.set_child(Some(&pill_row));
        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("gl-tile");
        frame.set_vexpand(true);
        frame.set_margin_top(band);
        // The frame is the clip for its contents (P1m-r7): while the tile is
        // stuck the grid moves up inside it and disappears under its own top
        // border, never under the pill.
        frame.set_overflow(gtk::Overflow::Hidden);
        let grid = crate::grid::folder_view(items, true);
        let grid_plane = gtk::Fixed::new();
        grid_plane.set_hexpand(true);
        grid_plane.set_vexpand(true);
        grid_plane.put(&grid, 0.0, 0.0);
        frame.append(&grid_plane);
        content.append(&frame);
        clip.set_child(Some(&content));
        outer.append(&clip);
        TileWidgets {
            outer,
            frame,
            grid_plane,
            grid,
            pill,
            close,
            pill_wrap,
        }
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
        click.connect_pressed(move |_, _, _, _| click_view.inner.focus(id));
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

    /// Put one tile back at its stuck geometry: the pill's wrap, the frame's
    /// top margin and the grid's content offset (P1m-r7).
    fn place_stuck(&self, slot: usize, rect: Rect) {
        let (wrap, widget, frame, plane, grid, scale, scroll, view_h) = {
            let st = self.state.borrow();
            (
                st.pill_wraps.get(slot).cloned(),
                st.widgets.get(slot).cloned(),
                st.frames.get(slot).cloned(),
                st.grid_planes.get(slot).cloned(),
                st.grids.get(slot).cloned(),
                st.scale,
                st.scroll,
                st.view[1],
            )
        };
        let Some(wrap) = wrap else {
            return;
        };
        let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
        let stuck = stuck_geometry(
            rect,
            frame_rect(rect, scale),
            scroll,
            view_h,
            scaled(CONTROL_H, scale),
            min_frame_height(scale),
        );
        wrap.set_size_request(rect.w.max(1), band.max(1));
        self.fixed.move_(&wrap, rect.x as f64, stuck.pill_y as f64);
        if let Some(frame) = frame {
            frame.set_margin_top(stuck.frame.y - rect.y);
        }
        if let (Some(plane), Some(grid)) = (plane, grid) {
            let (w, h) = grid_size(rect, scale);
            grid.set_size_request(w, h);
            plane.move_(&grid, 0.0, -(stuck.content_offset as f64));
        }
        if let Some(widget) = widget {
            wrap.insert_after(&self.fixed, Some(&widget));
        }
    }

    /// While a tile is dragged, its pill rides at the tile's top and the tile
    /// shows its natural geometry, not its stuck one (P1m-r6/r7).
    fn drag_pill(&self, slot: usize, rect: Rect) {
        let (wrap, frame, plane, grid, band) = {
            let st = self.state.borrow();
            (
                st.pill_wraps.get(slot).cloned(),
                st.frames.get(slot).cloned(),
                st.grid_planes.get(slot).cloned(),
                st.grids.get(slot).cloned(),
                scaled(CONTROL_H, st.scale) + scaled(GAP_S, st.scale),
            )
        };
        if let Some(wrap) = wrap {
            self.fixed.move_(&wrap, rect.x as f64, rect.y as f64);
        }
        if let Some(frame) = frame {
            frame.set_margin_top(band);
        }
        if let (Some(plane), Some(grid)) = (plane, grid) {
            plane.move_(&grid, 0.0, 0.0);
        }
    }

    /// A press anywhere in the view: a pill starts a tile drag, a border
    /// starts a resize. Nothing moves until the pointer passes the threshold.
    /// The press point, the hit test and every later update are in the
    /// Fixed's content space via `pointer_in_fixed` (item 5).
    fn begin_drag(&self, gesture: &gtk::GestureDrag, sx: f64, sy: f64) {
        let (rects, slots, scale, width_share, columns, heights, view, scroll) = {
            let st = self.state.borrow();
            (
                st.rects.clone(),
                st.slots(),
                st.scale,
                st.width_share,
                st.columns.clone(),
                st.heights.clone(),
                st.view,
                st.scroll,
            )
        };
        let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
            .map(|(x, y)| (x.round() as i32, y.round() as i32))
            .unwrap_or((sx.round() as i32, sy.round() as i32));
        let border = border_at(&slots, px, py, scroll, view[1], scale);
        crate::log!("split", "border press at ({px}, {py}) hit {border:?}");
        if let Some(slot) = pill_at(&rects, px, py, scroll, view[1], scale) {
            let Some(rect) = rects.get(slot).copied() else {
                return;
            };
            let Some(widget) = self.state.borrow().widgets.get(slot).cloned() else {
                return;
            };
            let name = self.state.borrow().tiles[slot].name.clone();
            *self.drag.borrow_mut() = Some(Drag::Tile {
                from: slot,
                grab: (px - rect.x, py - rect.y),
                press: (px, py),
                targets: rects,
            });
            self.bump_gen(slot);
            self.slots.borrow_mut()[slot].slide = None;
            if self.fixed.last_child().as_ref() != Some(widget.upcast_ref()) {
                let last = self.fixed.last_child();
                widget.insert_after(&self.fixed, last.as_ref());
            }
            // The pill rides at the tile's top during the drag.
            self.drag_pill(slot, rect);
            if let Some(wrap) = self.state.borrow().pill_wraps.get(slot).cloned() {
                wrap.insert_after(&self.fixed, Some(&widget));
            }
            crate::log!(
                "split",
                "tile press at ({px}, {py}) on '{name}' slot {slot}"
            );
            return;
        }
        let Some(border) = border else {
            return;
        };
        match border {
            Border::Columns => {
                *self.drag.borrow_mut() = Some(Drag::Width {
                    share: width_share,
                    press_x: px,
                });
                crate::log!("split", "resizing the column border from x {px}");
            }
            Border::Heights { column, row } => {
                let f = frame(view[0], view[1], scale);
                let total = columns[0].len() + columns[1].len();
                let heights = match &heights[column] {
                    // A default column starts its hand layout at the heights the
                    // pointer sees, so the tiles below keep them exactly. Its
                    // last slot's fill is at most a rounding pixel here.
                    Heights::Default => slots[column]
                        .iter()
                        .map(|r| (r.h as f64 / scale).round() as i32)
                        .collect(),
                    Heights::Hand(_) => stored_heights(
                        &heights[column],
                        columns[column].len(),
                        f.inner_h,
                        f.gutter,
                        half_floor(total, &f),
                        scale,
                    ),
                };
                *self.drag.borrow_mut() = Some(Drag::Height {
                    column,
                    border: row,
                    heights,
                    press_y: py,
                });
                crate::log!("split", "resizing column {column} at border {row}");
            }
        }
    }

    fn update_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow().clone() else {
            return;
        };
        match drag {
            Drag::Tile {
                from,
                grab,
                press,
                targets,
            } => {
                let (columns, heights, width_share, rects, widgets, view, scale, scroll) = {
                    let st = self.state.borrow();
                    (
                        st.columns.clone(),
                        st.heights.clone(),
                        st.width_share,
                        st.rects.clone(),
                        st.widgets.clone(),
                        st.view,
                        st.scale,
                        st.scroll,
                    )
                };
                let Some(&rect) = rects.get(from) else {
                    return;
                };
                let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    .unwrap_or((press.0 + dx.round() as i32, press.1 + dy.round() as i32));
                let board = Board {
                    columns: &columns,
                    heights: &heights,
                    width_share,
                    view: Viewport {
                        w: view[0],
                        h: view[1],
                        scale,
                    },
                    scroll,
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
                        self.drag_pill(slot, *r);
                    } else if *r != targets[slot] {
                        self.slide_to(slot, &widgets[slot], *r);
                    }
                }
                self.show_drop_line(drop);
                *self.drag.borrow_mut() = Some(Drag::Tile {
                    from,
                    grab,
                    press,
                    targets: layout,
                });
            }
            Drag::Width { share, press_x } => {
                let (scale, view) = {
                    let st = self.state.borrow();
                    (st.scale, st.view)
                };
                let f = frame(view[0], view[1], scale);
                let avail = (f.inner_w - f.gutter).max(1);
                let (px, _) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(x, _)| (x.round() as i32, 0))
                    .unwrap_or((press_x + dx.round() as i32, 0));
                let next =
                    width_share_after_drag(share, avail, px - press_x, min_column_width(scale));
                self.state.borrow_mut().width_share = next;
                self.relayout(false);
            }
            Drag::Height {
                column,
                border,
                heights,
                press_y,
            } => {
                let (scale, view) = {
                    let st = self.state.borrow();
                    (st.scale, st.view)
                };
                let (_, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(_, y)| (0, y.round() as i32))
                    .unwrap_or((0, press_y + dy.round() as i32));
                let dragged = py - press_y;
                let f = frame(view[0], view[1], scale);
                // The gesture is in device pixels; the model is in design
                // pixels, so a scale change keeps a hand size.
                let dy = (dragged as f64 / scale).round() as i32;
                let visible = (f.inner_h as f64 / scale).round() as i32;
                let gap = (f.gutter as f64 / scale).round() as i32;
                let pushed =
                    heights_after_push(&heights, border, dy, min_slot_height(1.0), visible, gap);
                {
                    let mut st = self.state.borrow_mut();
                    // A hand drag owns this column's heights: no half floor,
                    // and only the tile above the border changed size.
                    st.heights[column] = Heights::Hand(pushed.stored.clone());
                }
                let shown: Vec<i32> = pushed
                    .shown
                    .iter()
                    .map(|h| scaled(*h as f64, scale))
                    .collect();
                crate::log!("split", "border drag dy {dragged} heights {shown:?}");
                self.relayout(false);
            }
        }
    }

    /// Settle: a tile drop commits, otherwise everything slides back. Border
    /// drags have already written their heights.
    fn end_drag(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow_mut().take() else {
            return;
        };
        let Drag::Tile { from, press, .. } = drag else {
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
        let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
            .map(|(x, y)| (x.round() as i32, y.round() as i32))
            .unwrap_or((press.0 + dx.round() as i32, press.1 + dy.round() as i32));
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
                self.place_stuck(from, rects[from]);
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
                self.place_stuck(from, rect);
            }
        }
        self.hide_drop_line();
    }

    /// A double-click on a border resets it to equal shares, animated.
    fn reset_border(&self, x: f64, y: f64) {
        let (slots, scale, scroll, view_h) = {
            let st = self.state.borrow();
            (st.slots(), st.scale, st.scroll, st.view[1])
        };
        match border_at(
            &slots,
            x.round() as i32,
            y.round() as i32,
            scroll,
            view_h,
            scale,
        ) {
            Some(Border::Columns) => {
                self.state.borrow_mut().width_share = 0.5;
                self.relayout(true);
                crate::log!("split", "reset the column border to equal shares");
            }
            Some(Border::Heights { column, .. }) => {
                self.state.borrow_mut().heights[column] = Heights::Default;
                self.relayout(true);
                crate::log!("split", "reset column {column} to equal shares");
            }
            None => {}
        }
    }

    /// The hover cursor over a border.
    fn hover(&self, x: f64, y: f64) {
        let (slots, scale, scroll, view_h) = {
            let st = self.state.borrow();
            (st.slots(), st.scale, st.scroll, st.view[1])
        };
        let cursor = match border_at(
            &slots,
            x.round() as i32,
            y.round() as i32,
            scroll,
            view_h,
            scale,
        ) {
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

    fn slots(columns: &[Vec<usize>; 2], heights: &[Heights; 2]) -> [Vec<Rect>; 2] {
        layout(columns, heights, 0.5, VIEW.0, VIEW.1, 1.0)
    }

    fn rects_for(columns: &[Vec<usize>; 2]) -> Vec<Rect> {
        let s = slots(columns, &[Heights::Default, Heights::Default]);
        rects_by_tile(columns, &s)
    }

    fn defaults() -> [Heights; 2] {
        [Heights::Default, Heights::Default]
    }

    #[test]
    fn tiles_fill_the_height_and_split_the_width() {
        let columns = initial_columns(2);
        let s = slots(&columns, &defaults());
        assert_eq!(s[0][0].y, 0); // the pill tops sit level with the panels
        assert_eq!(s[0][0].h, VIEW.1 - scaled(GAP_2XL, 1.0)); // bottom inset only
        assert_eq!(s[0][0].x, scaled(GAP_2XL, 1.0));
        assert_eq!(s[0][0].w, s[1][0].w);
        // The gutter between the columns is gap-l.
        assert_eq!(s[1][0].x, s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0));
        assert!(rects_by_tile(&initial_columns(0), &[vec![], vec![]]).is_empty());
        assert!(layout(&initial_columns(2), &defaults(), 0.5, 0, 800, 1.0)[0].is_empty());
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
        // The left column fills the visible height (integer division may leave
        // one pixel, which the last tile's fill takes).
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
    fn dragging_down_grows_the_tile_above_and_moves_the_rest_down() {
        let columns = initial_columns(4); // 2 + 2
        let before = slots(&columns, &defaults());
        let above = before[0][0].h;
        let below = before[0][1].h;
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let pushed = heights_after_push(
            &[above, below],
            1,
            200,
            min_slot_height(1.0),
            f.inner_h,
            f.gutter,
        );
        assert_eq!(pushed.stored, vec![above + 200, below]);
        assert_eq!(pushed.shown, pushed.stored);
        // Only the tile above changed; the one below keeps its height and
        // moves down, so the column is 200 px taller.
        let after = slots(&columns, &[Heights::Hand(pushed.stored), Heights::Default]);
        assert_eq!(after[0][0].h, above + 200);
        assert_eq!(after[0][1].h, below);
        assert_eq!(after[0][1].y, before[0][1].y + 200);
        assert_eq!(
            after[0][1].y + after[0][1].h,
            before[0][1].y + before[0][1].h + 200
        );
    }

    #[test]
    fn dragging_up_shrinks_to_the_minimum_and_moves_the_rest_up() {
        // A three-tile left column beside a lone right tile.
        let columns = [vec![0, 1, 2], vec![3]];
        let before = slots(&columns, &defaults());
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let heights = default_heights(3, f.inner_h, f.gutter, 0, 1.0);
        let pushed = heights_after_push(
            &heights,
            1,
            -10_000,
            min_slot_height(1.0),
            f.inner_h,
            f.gutter,
        );
        // The tile above stops at its minimum; the two below keep theirs.
        assert_eq!(pushed.stored[0], min_slot_height(1.0));
        assert_eq!(&pushed.stored[1..], &heights[1..]);
        // The column now ends short, so the last tile fills the rest.
        let filled = f.inner_h - (pushed.stored[0] + heights[1]) - 2 * f.gutter;
        assert_eq!(pushed.shown[2], filled.max(heights[2]));
        let after = slots(&columns, &[Heights::Hand(pushed.stored), Heights::Default]);
        // The middle tile moved up by the amount the first one shrank and
        // kept its height.
        let shrink = heights[0] - min_slot_height(1.0);
        assert_eq!(after[0][1].y, before[0][1].y - shrink);
        assert_eq!(after[0][1].h, before[0][1].h);
        assert_eq!(after[0][2].y + after[0][2].h, f.inner_h);
    }

    #[test]
    fn the_last_tile_fills_a_column_shorter_than_the_view() {
        let columns = [vec![0, 1], vec![2]];
        let heights = [Heights::Hand(vec![300, 300]), Heights::Default];
        let s = slots(&columns, &heights);
        let f = frame(VIEW.0, VIEW.1, 1.0);
        assert_eq!(s[0][0].h, 300);
        assert_eq!(s[0][1].h, f.inner_h - f.gutter - 300);
        assert_eq!(s[0][1].y + s[0][1].h, f.inner_h);
        // The pure fill matches, and it never shrinks a column that is
        // already taller than the view.
        assert_eq!(
            fill_last(&[300, 300], f.inner_h, f.gutter),
            vec![300, f.inner_h - f.gutter - 300]
        );
        assert_eq!(fill_last(&[900, 900], f.inner_h, f.gutter), vec![900, 900]);
    }

    #[test]
    fn hand_heights_keep_their_design_px_through_resizes_and_scale_changes() {
        // A hand-sized column taller than the view at 800, so no fill can mask
        // a size change.
        let columns = [vec![0, 1, 2], vec![3]];
        let heights = [Heights::Hand(vec![500, 300, 200]), Heights::Default];
        let at = |view_h: i32, scale: f64| layout(&columns, &heights, 0.5, 1200, view_h, scale);
        let short = at(800, 1.0);
        let tall = at(1200, 1.0);
        for s in [&short, &tall] {
            assert_eq!((s[0][0].h, s[0][1].h), (500, 300));
        }
        // Scale 0.5 halves every hand height; the column is now shorter than
        // the view, so only the last tile's fill grows.
        let half = at(800, 0.5);
        assert_eq!((half[0][0].h, half[0][1].h), (250, 150));
        let f = frame(1200, 800, 0.5);
        assert_eq!(half[0][2].h, f.inner_h - 2 * f.gutter - 250 - 150);
    }

    #[test]
    fn a_reset_returns_to_the_equal_default() {
        let columns = [vec![0, 1], vec![2, 3]];
        let hand = [Heights::Hand(vec![700, 200]), Heights::Default];
        let dragged = layout(&columns, &hand, 0.5, VIEW.0, VIEW.1, 1.0);
        assert_ne!(dragged[0][0].h, dragged[0][1].h);
        // The double-click reset writes `Default`, whose layout is equal.
        let reset = layout(&columns, &defaults(), 0.5, VIEW.0, VIEW.1, 1.0);
        assert_eq!(reset[0][0].h, reset[0][1].h);
        // A swap keeps both columns' heights ...
        let four = initial_columns(4);
        let swapped = columns_after_swap(&four, 0, 1).unwrap();
        assert_eq!(heights_after_change(&four, &hand, &swapped)[0], hand[0]);
        // ... while an insert (both lengths change) resets both columns.
        let moved = columns_after_insert(&four, 0, 1, 2).unwrap();
        let after = heights_after_change(&four, &hand, &moved);
        assert_eq!(after[0], Heights::Default);
        assert_eq!(after[1], Heights::Default);
    }

    #[test]
    fn a_column_that_grows_past_the_view_stops_being_pinned() {
        let columns = initial_columns(4); // 2 + 2, both default and fitting
        let before = slots(&columns, &defaults());
        assert!(column_fits(&before[0], VIEW.1, 1.0));
        assert!(column_fits(&before[1], VIEW.1, 1.0));
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let heights = default_heights(2, f.inner_h, f.gutter, 0, 1.0);
        let pushed =
            heights_after_push(&heights, 1, 800, min_slot_height(1.0), f.inner_h, f.gutter);
        let after = slots(&columns, &[Heights::Hand(pushed.stored), Heights::Default]);
        assert!(!column_fits(&after[0], VIEW.1, 1.0));
        assert!(column_fits(&after[1], VIEW.1, 1.0));
        // The grown column scrolls (offset 0); the other stays pinned at the
        // scroll value.
        assert_eq!(column_offsets(&after, VIEW.1, 1.0, 90), [0, 90]);
    }

    #[test]
    fn the_column_border_still_trades_width() {
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let min_w = min_column_width(1.0);
        let avail_w = f.inner_w - f.gutter;
        assert_eq!(min_w, scaled(CELL_W, 1.0));
        assert_eq!(
            (avail_w as f64 * width_share_after_drag(0.5, avail_w, -9999, min_w)).round() as i32,
            min_w
        );
        assert_eq!(
            (avail_w as f64 * width_share_after_drag(0.5, avail_w, 9999, min_w)).round() as i32,
            avail_w - min_w
        );
        let dragged = width_share_after_drag(0.5, 1000, 120, min_w);
        assert!((dragged - 0.62).abs() < 0.001);
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
    fn border_hit_test_covers_the_whole_strip_except_the_pill() {
        let columns = initial_columns(4);
        let rects = rects_for(&columns);
        let slots = slots_of(&columns, &rects);
        let gap = scaled(GAP_L, 1.0);
        // The column gutter, at any height.
        let x = slots[0][0].x + slots[0][0].w + gap / 2;
        assert_eq!(
            border_at(&slots, x, 40, 0, VIEW.1, 1.0),
            Some(Border::Columns)
        );
        // The left column's strip: frame bottom to the next tile's frame top,
        // every integer y, at an x clear of the pill.
        let edge = slots[0][0].x + 5;
        let top = slots[0][0].y + slots[0][0].h;
        let bottom = slots[0][1].y + scaled(CONTROL_H, 1.0) + scaled(GAP_S, 1.0);
        for y in top..bottom {
            assert_eq!(
                border_at(&slots, edge, y, 0, VIEW.1, 1.0),
                Some(Border::Heights { column: 0, row: 1 }),
                "y {y} in the strip"
            );
        }
        // The next frame itself is not a border.
        assert_eq!(border_at(&slots, edge, bottom, 0, VIEW.1, 1.0), None);
        // On the lower pill it is not a border either ...
        let lower = slots[0][1];
        let pw = scaled(PILL_W, 1.0).min(lower.w);
        let pill_x = lower.x + (lower.w - pw) / 2 + pw / 2;
        let pill_y = lower.y + scaled(CONTROL_H, 1.0) / 2;
        assert_eq!(border_at(&slots, pill_x, pill_y, 0, VIEW.1, 1.0), None);
        // ... but the gutter band at the same x still is (the pill is only
        // the lower tile's top control-h).
        assert_eq!(
            border_at(&slots, pill_x, top + 1, 0, VIEW.1, 1.0),
            Some(Border::Heights { column: 0, row: 1 })
        );
        // A tile's middle is no border.
        assert_eq!(border_at(&slots, edge, 100, 0, VIEW.1, 1.0), None);
        // The right column's own strip is at its own y.
        let top = slots[1][0].y + slots[1][0].h;
        let bottom = slots[1][1].y + scaled(CONTROL_H, 1.0) + scaled(GAP_S, 1.0);
        for y in top..bottom {
            assert_eq!(
                border_at(&slots, slots[1][0].x + 5, y, 0, VIEW.1, 1.0),
                Some(Border::Heights { column: 1, row: 1 }),
                "right y {y}"
            );
        }
    }

    #[test]
    fn a_short_column_is_pinned_and_a_tall_one_scrolls() {
        // 4 + 1: the lone tile fits; four floored tiles do not.
        let four_one = [vec![0, 1, 2, 3], vec![4]];
        let s = layout(&four_one, &defaults(), 0.5, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 90), [0, 90]);
        assert!(!column_fits(&s[0], VIEW.1, 1.0));
        assert!(column_fits(&s[1], VIEW.1, 1.0));
        // 3 + 3: both columns are floored past the view, so both scroll.
        let three_three = initial_columns(6);
        let s = layout(&three_three, &defaults(), 0.5, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 90), [0, 0]);
        // 2 + 1: both fit, so both stay pinned (and the scroller has no range).
        let two_one = [vec![0, 1], vec![2]];
        let s = layout(&two_one, &defaults(), 0.5, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 0), [0, 0]);
        assert!(column_fits(&s[0], VIEW.1, 1.0));
        // A 2 + 2 column that fits at 1400 stops fitting at 100: the pill plus
        // one grid row is now taller than half the view.
        let four = initial_columns(4);
        let tall = layout(&four, &defaults(), 0.5, VIEW.0, 1400, 1.0);
        let short = layout(&four, &defaults(), 0.5, VIEW.0, 100, 1.0);
        assert!(column_fits(&tall[0], 1400, 1.0));
        assert!(!column_fits(&short[0], 100, 1.0));
        assert_eq!(column_offsets(&short, 100, 1.0, 30), [0, 0]);
    }

    #[test]
    fn a_hand_drag_reaches_the_minimum_below_the_half_floor() {
        // 4 + 1: the default layout floors the four-tile column at half the
        // view. A hand drag may take one of its tiles down to pill + one grid
        // row, below that floor, and the tiles below keep theirs.
        let four_one = [vec![0, 1, 2, 3], vec![4]];
        let s = slots(&four_one, &defaults());
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let half = half_floor(5, &f);
        assert_eq!(s[0][0].h, half);
        let min = min_slot_height(1.0);
        assert!(min < half);
        let heights = default_heights(4, f.inner_h, f.gutter, half, 1.0);
        let pushed = heights_after_push(&heights, 1, -10_000, min, f.inner_h, f.gutter);
        assert_eq!(pushed.stored[0], min);
        assert_eq!(&pushed.stored[1..], &[half, half, half]);
        // The hand layout drops the floor and renders the minimum; the next
        // tile keeps its half-height.
        let hand = slots(&four_one, &[Heights::Hand(pushed.stored), Heights::Default]);
        assert_eq!(hand[0][0].h, min);
        assert_eq!(hand[0][1].h, half);
        // The default layout still floors every slot.
        assert_eq!(slots(&four_one, &defaults())[0][0].h, half);
    }

    #[test]
    fn a_stuck_tiles_strip_follows_its_frame_top() {
        // Two 700-tall slots: the lower one starts at 824. At scroll 900 its
        // top is above the view, so its frame top sticks at view 58 (content
        // 958) instead of its natural 882. The strip under it runs from the
        // upper frame's bottom (800) to that stuck top.
        let slots = [
            vec![
                Rect {
                    x: 40,
                    y: 100,
                    w: 300,
                    h: 700,
                },
                Rect {
                    x: 40,
                    y: 824,
                    w: 300,
                    h: 700,
                },
            ],
            vec![],
        ];
        for y in 800..958 {
            assert_eq!(
                border_at(&slots, 45, y, 900, VIEW.1, 1.0),
                Some(Border::Heights { column: 0, row: 1 }),
                "y {y} in the stuck strip"
            );
        }
        // The lower frame's border itself is not a border.
        assert_eq!(border_at(&slots, 45, 958, 900, VIEW.1, 1.0), None);
        // Its stuck pill is at content 900..950 and is never a border.
        assert_eq!(border_at(&slots, 200, 925, 900, VIEW.1, 1.0), None);
        // The gap between the stuck pill and the frame top still grabs.
        assert_eq!(
            border_at(&slots, 45, 955, 900, VIEW.1, 1.0),
            Some(Border::Heights { column: 0, row: 1 })
        );
    }

    #[test]
    fn pill_hit_test() {
        let rects = rects_for(&initial_columns(4));
        let r = rects[0];
        let px = r.x + (r.w - scaled(PILL_W, 1.0)) / 2;
        let ph = scaled(CONTROL_H, 1.0);
        assert_eq!(pill_at(&rects, px + 10, r.y + 10, 0, VIEW.1, 1.0), Some(0));
        assert_eq!(pill_at(&rects, r.x + 10, r.y + 10, 0, VIEW.1, 1.0), None);
        assert_eq!(pill_at(&rects, px + 10, r.y + ph, 0, VIEW.1, 1.0), None);
        assert!(!press_on_pill(10.0, 10.0, 500, 0, 1.0));
        assert!(press_on_pill(240.0, 10.0, 500, 0, 1.0));
    }

    /// A slot 700 tall at y 100: its frame runs from 158 to 800.
    fn sticky_case() -> (Rect, Rect) {
        let slot = Rect {
            x: 40,
            y: 100,
            w: 300,
            h: 700,
        };
        (slot, frame_rect(slot, 1.0))
    }

    fn stuck_at(scroll: i32) -> Stuck {
        let (slot, frame) = sticky_case();
        stuck_geometry(
            slot,
            frame,
            scroll,
            VIEW.1,
            scaled(CONTROL_H, 1.0),
            min_frame_height(1.0),
        )
    }

    #[test]
    fn a_visible_slot_leaves_its_pill_at_the_slot_top() {
        let (slot, frame) = sticky_case();
        // The slot fully visible at the top.
        let g = stuck_at(0);
        assert_eq!(g.pill_y, 100);
        assert_eq!(g.frame, frame);
        assert_eq!(g.content_offset, 0);
        // Still fully visible after a 40 px scroll: no change relative to the
        // slot, so in the view it simply follows the scroll.
        let g = stuck_at(40);
        assert_eq!(g.pill_y, 100);
        assert_eq!(g.pill_y - 40, 60);
        assert_eq!(g.frame, frame);
        // In content coordinates the pill is still at the slot's top; the
        // scroller moves both by 40.
        assert_eq!(pill_content_rect(slot, 40, VIEW.1, 1.0).y, 100);
    }

    #[test]
    fn a_slot_top_above_the_view_sticks_the_pill_and_the_frame_top() {
        // The slot's top is 100 px above the visible area.
        let g = stuck_at(200);
        // The pill is at the view's top: content y 200, view y 0.
        assert_eq!(g.pill_y, 200);
        assert_eq!(g.pill_y - 200, 0);
        // The frame's top border stays `gap-s` under the pill; its bottom is
        // still the slot's bottom (800 in content, 600 in the view).
        assert_eq!(g.frame.y, 200 + 58);
        assert_eq!(g.frame.y + g.frame.h, 800);
        assert_eq!(g.frame.y + g.frame.h - 200, 600);
        assert_eq!(g.frame.h, 542);
        // The contents have moved up by the 100 px scrolled past the top.
        assert_eq!(g.content_offset, 100);
        // The hit test sees the pill at the top of the view: content y 200.
        assert_eq!(pill_content_rect(sticky_case().0, 200, VIEW.1, 1.0).y, 200);
        // A further scroll keeps the pill and frame top still and shortens
        // the frame from below.
        let g = stuck_at(500);
        assert_eq!((g.pill_y, g.frame.y), (500, 558));
        assert_eq!(g.frame.h, 242);
        assert_eq!(g.content_offset, 400);
    }

    #[test]
    fn the_frame_shrinks_to_one_grid_row_at_the_minimum() {
        // The frame's visible height reaches one grid row (144) at scroll
        // 598: 800 - 598 - 58 = 144.
        let g = stuck_at(598);
        assert_eq!(g.pill_y - 598, 0);
        assert_eq!(g.frame.y - 598, 58);
        assert_eq!(g.frame.h, min_frame_height(1.0));
        assert_eq!(g.frame.h, 144);
        assert_eq!(g.content_offset, 598 - 100);
    }

    #[test]
    fn below_the_minimum_the_whole_tile_scrolls_off() {
        // At the threshold the pill is at the top and the frame is one row.
        // Past it the whole tile keeps the geometry it had in content
        // coordinates and the page scrolls it off, pill and all.
        let at = stuck_at(598);
        let past = stuck_at(650);
        assert_eq!(past.pill_y, at.pill_y);
        assert_eq!(past.frame, at.frame);
        assert_eq!(past.content_offset, at.content_offset);
        // In the view the pill and the frame's top are 52 px above the top.
        assert_eq!(past.pill_y - 650, -52);
        assert_eq!(past.frame.h, 144);
        // At the frame's last pixel both are off the view, together.
        let gone = stuck_at(800);
        assert_eq!(gone.frame.y + gone.frame.h - 800, 0);
        assert_eq!(gone.pill_y + scaled(CONTROL_H, 1.0) - 800, -152);
        assert!(gone.pill_y + scaled(CONTROL_H, 1.0) - 800 <= 0);
    }

    #[test]
    fn a_pinned_column_never_moves_its_pills() {
        // 4 + 1: the lone tile fits, so its column is pinned at the scroll
        // value and its pill stays at the slot's top.
        let columns = [vec![0, 1, 2, 3], vec![4]];
        let board = Board {
            columns: &columns,
            heights: &defaults(),
            width_share: 0.5,
            view: Viewport {
                w: VIEW.0,
                h: VIEW.1,
                scale: 1.0,
            },
            scroll: 300,
        };
        let slots = board.slots();
        assert!(column_fits(
            &layout(&columns, &defaults(), 0.5, VIEW.0, VIEW.1, 1.0)[1],
            VIEW.1,
            1.0
        ));
        let pinned = slots[1][0];
        let g = stuck_geometry(
            pinned,
            frame_rect(pinned, 1.0),
            300,
            VIEW.1,
            scaled(CONTROL_H, 1.0),
            min_frame_height(1.0),
        );
        assert_eq!(
            g.pill_y, pinned.y,
            "a pinned column's pill top is its slot top"
        );
        assert_eq!(g.content_offset, 0, "a pinned tile stays whole");
        // The tall column's first pill sticks instead.
        assert_eq!(
            pill_content_rect(slots[0][0], 300, VIEW.1, 1.0).y,
            300,
            "the tall column's pill sits at the view's top"
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
        let heights = defaults();
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
            scroll: 0,
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
        let heights = defaults();
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
            scroll: 0,
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
