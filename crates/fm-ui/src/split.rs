// SPDX-License-Identifier: GPL-3.0-or-later
//! The split view: one tile per tab in two columns.
//!
//! The column rules (which tab sits where, the rebalance, the drops) live in
//! `fm-core` (P2d). This module is the pixel side, ported from the spike's
//! `look-body/split.rs` after P1m-r9: the frame, the slot layout, the pinned
//! short column, the stuck tile, the pill layout, the drop hit test and line,
//! the border hit test, and the drag preview. Every tile owns one P2i folder
//! view, kept for as long as the tile is shown.
//!
//! Widgets that move as one (a tile and its pill) are placed by one function
//! from one rect, so a slide can never move one without the other.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use adw::prelude::*;
use fm_core::config::{SortBy, SortOrder, SortRule};
use fm_core::workspace::{
    Columns, Drop, Group, Heights, SplitLayout, TabId, Window, WindowId, apply_drop,
    heights_after_change,
};
use gtk::gdk;
use gtk::glib;
use gtk::pango;

use crate::app::Process;
use crate::dnd;
use crate::folder::FolderView;
use crate::look::motion;
use crate::look::tokens::*;

// ---------------------------------------------------------------------------
// Pure pixel rules
// ---------------------------------------------------------------------------

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

/// The parts of a group's `SplitLayout` plus the viewport and scroll offset.
#[derive(Clone)]
pub struct LayoutState {
    pub columns: Columns,
    pub heights: [Heights; 2],
    pub width_permille: u16,
}

impl LayoutState {
    pub fn from_layout(layout: &SplitLayout) -> Self {
        Self {
            columns: layout.columns.clone(),
            heights: layout.heights.clone(),
            width_permille: layout.width_permille,
        }
    }
}

/// The narrowest a column may be dragged: one grid column wide. A tile can be
/// narrower than its `PILL_W` pill: the pill loses its 270 px minimum below
/// that width and its label ellipsizes, and the tile's content sits in a
/// clipping scroller, so nothing draws over the gutter.
pub fn min_column_width(scale: f64) -> i32 {
    scaled(CELL_W, scale)
}

/// Whether a tab pill at `tile_w` is wide enough for the design's `PILL_W`.
pub fn pill_is_wide(tile_w: i32, scale: f64) -> bool {
    tile_w >= scaled(PILL_W, scale)
}

/// The label's natural-width cap in characters for a tile `tile_w` wide.
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

/// The width the pill actually takes in a tile `tile_w` wide.
pub fn pill_width(tile_w: i32, scale: f64) -> i32 {
    if tile_w <= 0 {
        return 0;
    }
    if pill_is_wide(tile_w, scale) {
        scaled(PILL_W, scale).min(tile_w)
    } else {
        (pill_overhead(scale) + 6 * pill_char_width(scale)).min(tile_w)
    }
}

/// The shortest a tile's slot may be dragged: its pill, the pill's gap and
/// one grid row of content.
pub fn min_slot_height(scale: f64) -> i32 {
    scaled(CONTROL_H + GAP_S + CELL_H, scale)
}

/// The half-visible-height floor that a default layout applies beyond four
/// tiles, else 0.
pub fn half_floor(total: usize, f: &Frame) -> i32 {
    if total > 4 {
        ((f.inner_h - f.gutter) / 2).max(1)
    } else {
        0
    }
}

/// Equal heights for `n` slots in design pixels.
pub fn default_heights(n: usize, inner_h: i32, gap: i32, floor: i32, scale: f64) -> Vec<i32> {
    if n == 0 {
        return vec![];
    }
    let avail = (inner_h - (n as i32 - 1) * gap).max(1);
    let each = (avail / n as i32).max(1).max(floor);
    vec![(each as f64 / scale).round().max(1.0) as i32; n]
}

/// The heights a column's slots remember, before the last tile's fill.
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

/// The heights actually drawn for a column ending above the visible height.
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

/// The result of dragging a stacked border: remembered and drawn heights.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    pub stored: Vec<i32>,
    pub shown: Vec<i32>,
}

/// Push the border at `border` (1..n) down by `dy` unscaled design pixels.
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

/// (left, right) column widths for a view width and the left column's share.
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

/// The slot rects of both columns, left first, top to bottom.
pub fn layout(
    columns: &Columns,
    heights: &[Heights; 2],
    width_permille: u16,
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
    let (left_w, right_w) = column_widths(&f, f64::from(width_permille) / 1000.0, counts, scale);
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

/// Each column's y offset for a scroll value: a fitting column stays pinned.
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

/// The tile's frame inside its slot: the slot minus the pill band above it.
pub fn frame_rect(slot: Rect, scale: f64) -> Rect {
    let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
    Rect {
        x: slot.x,
        y: slot.y + band,
        w: slot.w,
        h: (slot.h - band).max(0),
    }
}

/// The grid scroller's size inside a tile: the frame's content box.
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

/// A tile's stuck geometry, in content coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stuck {
    pub pill_y: i32,
    pub frame: Rect,
    pub content_offset: i32,
}

pub fn stuck_geometry(
    slot: Rect,
    frame: Rect,
    scroll: i32,
    visible_h: i32,
    pill_h: i32,
    minimum: i32,
) -> Stuck {
    let natural = slot.y - scroll;
    let gap = frame.y - slot.y - pill_h;
    let band = pill_h + gap;
    let bottom = frame.y + frame.h - scroll;
    let minimum = minimum.clamp(0, frame.h.max(0));
    let not_stuck = Stuck {
        pill_y: slot.y,
        frame,
        content_offset: 0,
    };
    if natural >= 0 || frame.y - scroll >= visible_h {
        return not_stuck;
    }
    if bottom - band < minimum {
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

/// Where a tile's pill wrap sits and how its frame content is offset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PillLayout {
    pub wrap: Rect,
    pub frame_top: i32,
    pub content_offset: i32,
}

pub fn pill_layout(slot: Rect, scroll: i32, visible_h: i32, scale: f64) -> PillLayout {
    let pill_h = scaled(CONTROL_H, scale);
    let band = pill_h + scaled(GAP_S, scale);
    let stuck = stuck_geometry(
        slot,
        frame_rect(slot, scale),
        scroll,
        visible_h,
        pill_h,
        min_frame_height(scale),
    );
    PillLayout {
        wrap: Rect {
            x: slot.x,
            y: stuck.pill_y,
            w: slot.w.max(1),
            h: band.max(1),
        },
        frame_top: stuck.frame.y - slot.y,
        content_offset: stuck.content_offset,
    }
}

/// The dragged tile's pill layout: its natural geometry.
pub fn dragged_pill_layout(slot: Rect, scale: f64) -> PillLayout {
    let band = scaled(CONTROL_H, scale) + scaled(GAP_S, scale);
    PillLayout {
        wrap: Rect {
            x: slot.x,
            y: slot.y,
            w: slot.w.max(1),
            h: band.max(1),
        },
        frame_top: band,
        content_offset: 0,
    }
}

/// The pill's rect in content coordinates, at its stuck position.
pub fn pill_content_rect(slot: Rect, scroll: i32, visible_h: i32, scale: f64) -> Rect {
    let pill_h = scaled(CONTROL_H, scale);
    let pw = pill_width(slot.w, scale);
    Rect {
        x: slot.x + (slot.w - pw) / 2,
        y: pill_layout(slot, scroll, visible_h, scale).wrap.y,
        w: pw,
        h: pill_h,
    }
}

/// The rect of every tile in `order`, from its column and row.
pub fn rects_by_tile(order: &[TabId], columns: &Columns, slots: &[Vec<Rect>; 2]) -> Vec<Rect> {
    let mut out = vec![
        Rect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        };
        order.len()
    ];
    for (column, tiles) in columns.iter().enumerate() {
        for (row, tile) in tiles.iter().enumerate() {
            if let Some(slot) = slots.get(column).and_then(|s| s.get(row))
                && let Some(index) = order.iter().position(|id| id == tile)
            {
                out[index] = *slot;
            }
        }
    }
    out
}

/// The per-column slot rects of a committed layout, from rects in `order`.
pub fn slots_of(columns: &Columns, order: &[TabId], rects: &[Rect]) -> [Vec<Rect>; 2] {
    [0, 1].map(|column| {
        columns[column]
            .iter()
            .filter_map(|tile| {
                order
                    .iter()
                    .position(|id| id == tile)
                    .and_then(|index| rects.get(index).copied())
            })
            .collect()
    })
}

/// The drop under the pointer. The gap band wins over the tile under it.
pub fn drop_at(
    columns: &Columns,
    order: &[TabId],
    rects: &[Rect],
    from: TabId,
    px: i32,
    py: i32,
    scale: f64,
) -> Option<Drop> {
    let slots = slots_of(columns, order, rects);
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
    for (column, tiles) in columns.iter().enumerate() {
        for (row, tile) in tiles.iter().enumerate() {
            if *tile != from && slots[column][row].contains(px, py) {
                return Some(Drop::Swap(*tile));
            }
        }
    }
    None
}

/// The drop line's rect: across the target column at the gap.
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

/// A border under the pointer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Border {
    Columns,
    Heights { column: usize, row: usize },
}

fn pill_zone(r: Rect, x: i32, y: i32, scroll: i32, visible_h: i32, scale: f64) -> bool {
    pill_content_rect(r, scroll, visible_h, scale).contains(x, y)
}

/// The whole strip between two stacked tiles grabs, except on any drawn pill.
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

/// The layout, viewport and scroll a live drag preview needs.
pub struct Board<'a> {
    pub layout: &'a LayoutState,
    pub view: Viewport,
    pub scroll: i32,
}

/// The live drag layout: the drop under the pointer and every tile's rect.
pub fn drag_layout(
    board: &Board<'_>,
    order: &[TabId],
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
    let Some(&from_id) = order.get(from) else {
        return (None, out);
    };
    let mut target = drop_at(
        &board.layout.columns,
        order,
        rects,
        from_id,
        px,
        py,
        board.view.scale,
    );
    if let Some(drop) = target {
        match apply_drop(&board.layout.columns, from_id, drop) {
            Some(next) => {
                let next_heights =
                    heights_after_change(&board.layout.columns, &board.layout.heights, &next);
                let mut slots = layout_grid(
                    &next,
                    &next_heights,
                    board.layout.width_permille,
                    board.view,
                );
                offset_slots(&mut slots, board.view.h, board.view.scale, board.scroll);
                for (column, tiles) in next.iter().enumerate() {
                    for (row, tile) in tiles.iter().enumerate() {
                        if let (Some(slot), Some(index)) = (
                            slots[column].get(row),
                            order.iter().position(|id| id == tile),
                        ) {
                            out[index] = *slot;
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

fn layout_grid(
    columns: &Columns,
    heights: &[Heights; 2],
    width_permille: u16,
    view: Viewport,
) -> [Vec<Rect>; 2] {
    layout(columns, heights, width_permille, view.w, view.h, view.scale)
}

/// The pill hit test: which tile's drawn pill holds the point.
pub fn pill_at(
    order: &[TabId],
    rects: &[Rect],
    x: i32,
    y: i32,
    scroll: i32,
    visible_h: i32,
    scale: f64,
) -> Option<TabId> {
    rects
        .iter()
        .enumerate()
        .position(|(_, r)| pill_content_rect(*r, scroll, visible_h, scale).contains(x, y))
        .and_then(|index| order.get(index).copied())
}

/// Which tile a click at (x, y) in content coordinates focuses.
#[cfg(test)]
pub fn tile_at(order: &[TabId], rects: &[Rect], x: i32, y: i32) -> Option<TabId> {
    rects
        .iter()
        .position(|r| r.contains(x, y))
        .and_then(|index| order.get(index).copied())
}

/// Whether a press at (x, y) inside a `tile_w`-wide tile landed on its pill.
#[cfg(test)]
pub fn press_on_pill(x: f64, y: f64, tile_w: i32, pill_top: i32, scale: f64) -> bool {
    if tile_w <= 0 {
        return false;
    }
    let pw = pill_width(tile_w, scale);
    let px = (tile_w - pw) / 2;
    let ph = scaled(CONTROL_H, scale);
    x >= px as f64 && x < (px + pw) as f64 && y >= pill_top as f64 && y < (pill_top + ph) as f64
}

// ---------------------------------------------------------------------------
// The widgets
// ---------------------------------------------------------------------------

struct Slot {
    pos: Rc<Cell<(f64, f64)>>,
    size: Rc<Cell<(i32, i32)>>,
    generation: Rc<Cell<u64>>,
    slide: Option<adw::TimedAnimation>,
}

/// One drawn tile or pill: its tab, its Fixed position and its size.
type VisualState = (TabId, (f64, f64), (i32, i32));

type DisplayedFolder = Rc<RefCell<Option<PathBuf>>>;

struct Tile {
    id: TabId,
    outer: gtk::Box,
    frame: gtk::Box,
    grid_plane: gtk::Fixed,
    grid: gtk::ScrolledWindow,
    pill: gtk::Box,
    pill_wrap: gtk::ScrolledWindow,
    view: Rc<FolderView>,
    /// Shared, so `sync_folder` writes the tile's own cell: a cloned
    /// `RefCell` was a copy, so every model event re-listed every tile and
    /// dropped its selection (Claude's review, after Kyle's P2k report).
    displayed_folder: Rc<RefCell<Option<PathBuf>>>,
    gate: Rc<Cell<bool>>,
    _name: String,
}

#[derive(Clone)]
enum Drag {
    Tile {
        from: TabId,
        from_index: usize,
        grab: (i32, i32),
        press: (i32, i32),
        targets: Vec<Rect>,
    },
    Width {
        permille: u16,
        press_x: i32,
    },
    Height {
        column: usize,
        border: usize,
        heights: Vec<i32>,
        press_y: i32,
    },
}

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
    window: WindowId,
    process: Weak<Process>,
    status: Rc<crate::folder::StatusLine>,
    outer: gtk::ScrolledWindow,
    fixed: gtk::Fixed,
    line: gtk::Box,
    tiles: RefCell<Vec<Tile>>,
    slots: RefCell<HashMap<TabId, Slot>>,
    model: RefCell<LayoutState>,
    preview: RefCell<Option<LayoutState>>,
    view: Cell<(i32, i32)>,
    scale: Cell<f64>,
    scroll: Cell<i32>,
    drag: RefCell<Option<Drag>>,
    focused: Cell<Option<TabId>>,
    menu_blocked: Cell<bool>,
    cursor: Cell<Cursor>,
    scroll_pending: Cell<bool>,
}

#[derive(Clone)]
pub struct SplitView {
    inner: Rc<Inner>,
}

impl SplitView {
    pub fn new(
        process: &Rc<Process>,
        window: WindowId,
        status: Rc<crate::folder::StatusLine>,
        scale: f64,
    ) -> Self {
        let fixed = gtk::Fixed::new();
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        line.add_css_class("gl-drop-line");
        line.set_visible(false);
        fixed.put(&line, 0.0, 0.0);
        line.set_size_request(1, scaled(LINE_THIN, 1.0).max(1));

        let outer = gtk::ScrolledWindow::new();
        outer.set_child(Some(&fixed));
        outer.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        outer.set_propagate_natural_width(false);
        outer.set_propagate_natural_height(false);
        outer.set_vexpand(true);
        outer.set_hexpand(true);

        let this = Self {
            inner: Rc::new(Inner {
                window,
                process: Rc::downgrade(process),
                status,
                outer,
                fixed,
                line,
                tiles: RefCell::new(Vec::new()),
                slots: RefCell::new(HashMap::new()),
                model: RefCell::new(LayoutState {
                    columns: [Vec::new(), Vec::new()],
                    heights: [Heights::Default, Heights::Default],
                    width_permille: 500,
                }),
                preview: RefCell::new(None),
                view: Cell::new((1200, 1280)),
                scale: Cell::new(scale),
                scroll: Cell::new(0),
                drag: RefCell::new(None),
                focused: Cell::new(None),
                menu_blocked: Cell::new(false),
                cursor: Cell::new(Cursor::None),
                scroll_pending: Cell::new(false),
            }),
        };
        this.wire();
        this
    }

    pub fn widget(&self) -> &gtk::ScrolledWindow {
        &self.inner.outer
    }

    pub fn show(&self, window: &Window, group: &Group, restore: bool) {
        self.inner.show(window, group, restore);
    }

    pub fn set_scale(&self, scale: f64) {
        self.inner.scale.set(scale);
        let views: Vec<Rc<FolderView>> = self
            .inner
            .tiles
            .borrow()
            .iter()
            .map(|tile| tile.view.clone())
            .collect();
        for view in views {
            view.apply_scale(scale);
        }
        self.inner.relayout(false);
    }

    /// Give the focused tile's grid the keyboard again after a transient page.
    pub fn focus_focused(&self) {
        let Some(id) = self.inner.focused.get() else {
            return;
        };
        if let Some(view) = self.inner.view_of(id) {
            view.grab_grid_focus();
        }
    }

    pub fn blur(&self) {
        for tile in self.inner.tiles.borrow().iter() {
            tile.gate.set(false);
        }
    }

    /// Drop folder monitors, thumbnail demand and folder-size walks in every
    /// tile showing a folder under one mount root.
    pub fn release_root(&self, root: &Path) -> usize {
        let tiles: Vec<(Rc<FolderView>, DisplayedFolder)> = self
            .inner
            .tiles
            .borrow()
            .iter()
            .map(|tile| (tile.view.clone(), tile.displayed_folder.clone()))
            .collect();
        let mut released = 0;
        for (view, displayed) in tiles {
            if view.release_root(root) {
                *displayed.borrow_mut() = None;
                released += 1;
            }
        }
        released
    }

    /// Re-read every administrator tile. Returns how many were re-read.
    pub fn reload_admin(&self) -> usize {
        let views: Vec<Rc<FolderView>> = self
            .inner
            .tiles
            .borrow()
            .iter()
            .map(|tile| tile.view.clone())
            .collect();
        views.iter().filter(|view| view.reload_if_admin()).count()
    }

    pub fn reload_focused(&self) {
        let Some(id) = self.inner.focused.get() else {
            return;
        };
        if let Some(view) = self.inner.view_of(id) {
            view.reload();
        }
    }

    pub fn listed_names(&self) -> Vec<OsString> {
        let Some(id) = self.inner.focused.get() else {
            return Vec::new();
        };
        self.inner
            .view_of(id)
            .map(|view| view.listed_names())
            .unwrap_or_default()
    }

    pub fn refresh_sort(&self, folder: Option<&Path>) {
        let Some(process) = self.inner.process.upgrade() else {
            return;
        };
        let tiles: Vec<(Rc<FolderView>, Option<PathBuf>)> = self
            .inner
            .tiles
            .borrow()
            .iter()
            .map(|tile| (tile.view.clone(), tile.displayed_folder.borrow().clone()))
            .collect();
        for (view, shown) in tiles {
            let Some(shown) = shown else {
                continue;
            };
            if folder.is_none_or(|folder| folder == shown) {
                let rule = {
                    let config = process.config.borrow();
                    config.config().sort_for(&shown)
                };
                view.set_sort(rule);
            }
        }
    }

    pub fn cancel_drag(&self) -> bool {
        self.inner.cancel_drag()
    }

    fn wire(&self) {
        let weak = Rc::downgrade(&self.inner);
        self.inner
            .outer
            .vadjustment()
            .connect_notify_local(Some("page-size"), move |adj, _| {
                if let Some(inner) = weak.upgrade() {
                    let size = adj.page_size() as i32;
                    if size > 0 && inner.view.get().1 != size {
                        inner.view.set((inner.view.get().0, size));
                        inner.relayout(false);
                    }
                }
            });
        let weak = Rc::downgrade(&self.inner);
        self.inner
            .outer
            .hadjustment()
            .connect_notify_local(Some("page-size"), move |adj, _| {
                if let Some(inner) = weak.upgrade() {
                    let size = adj.page_size() as i32;
                    if size > 0 && inner.view.get().0 != size {
                        inner.view.set((size, inner.view.get().1));
                        inner.relayout(false);
                    }
                }
            });
        let weak = Rc::downgrade(&self.inner);
        self.inner
            .outer
            .vadjustment()
            .connect_value_changed(move |adj| {
                if let Some(inner) = weak.upgrade() {
                    let value = adj.value().round() as i32;
                    if inner.scroll.get() != value {
                        inner.scroll.set(value);
                        inner.schedule_scroll();
                    }
                }
            });

        let drag = gtk::GestureDrag::new();
        let weak = Rc::downgrade(&self.inner);
        drag.connect_drag_begin(move |gesture, sx, sy| {
            if let Some(inner) = weak.upgrade() {
                inner.begin_drag(gesture, sx, sy);
            }
        });
        let weak = Rc::downgrade(&self.inner);
        drag.connect_drag_update(move |gesture, dx, dy| {
            if let Some(inner) = weak.upgrade() {
                inner.update_drag(gesture, dx, dy);
            }
        });
        let weak = Rc::downgrade(&self.inner);
        drag.connect_drag_end(move |gesture, dx, dy| {
            if let Some(inner) = weak.upgrade() {
                inner.end_drag(gesture, dx, dy);
            }
        });
        let weak = Rc::downgrade(&self.inner);
        drag.connect_cancel(move |_, _| {
            if let Some(inner) = weak.upgrade() {
                inner.cancel_drag();
            }
        });
        self.inner.fixed.add_controller(drag);

        let reset = gtk::GestureClick::new();
        reset.set_button(1);
        let weak = Rc::downgrade(&self.inner);
        reset.connect_pressed(move |_, n_press, x, y| {
            if n_press >= 2
                && let Some(inner) = weak.upgrade()
            {
                inner.reset_border(x, y);
            }
        });
        self.inner.fixed.add_controller(reset);

        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&self.inner);
        motion.connect_motion(move |_, x, y| {
            if let Some(inner) = weak.upgrade() {
                inner.hover(x, y);
            }
        });
        let weak = Rc::downgrade(&self.inner);
        motion.connect_leave(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.set_cursor(Cursor::None);
            }
        });
        self.inner.fixed.add_controller(motion);
    }
}

/// The pointer in the Fixed's content space.
fn pointer_in_fixed<W: IsA<gtk::Widget>>(
    fixed: &W,
    event: Option<&gdk::Event>,
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

fn folder_name(folder: &Path) -> String {
    match folder.file_name() {
        Some(name) if !name.is_empty() => name.to_string_lossy().into_owned(),
        _ => folder.display().to_string(),
    }
}

fn default_rule() -> SortRule {
    SortRule {
        by: SortBy::Name,
        order: SortOrder::Ascending,
        folders_first: true,
    }
}

fn flatten(columns: &Columns) -> Vec<TabId> {
    columns.iter().flatten().copied().collect()
}

impl Inner {
    fn viewport(&self) -> Viewport {
        let (w, h) = self.view.get();
        Viewport {
            w,
            h,
            scale: self.scale.get(),
        }
    }

    fn current_layout(&self) -> LayoutState {
        self.preview
            .borrow()
            .clone()
            .unwrap_or_else(|| self.model.borrow().clone())
    }

    fn rects(&self) -> (Vec<TabId>, Vec<Rect>) {
        let layout = self.current_layout();
        let order = flatten(&layout.columns);
        let view = self.viewport();
        let mut slots = layout_grid(
            &layout.columns,
            &layout.heights,
            layout.width_permille,
            view,
        );
        offset_slots(&mut slots, view.h, view.scale, self.scroll.get());
        let rects = rects_by_tile(&order, &layout.columns, &slots);
        (order, rects)
    }

    fn view_of(&self, id: TabId) -> Option<Rc<FolderView>> {
        self.tiles
            .borrow()
            .iter()
            .find(|tile| tile.id == id)
            .map(|tile| tile.view.clone())
    }

    fn show(self: &Rc<Self>, window: &Window, group: &Group, restore: bool) {
        *self.model.borrow_mut() = LayoutState::from_layout(&group.layout);
        let order = flatten(&group.layout.columns);
        self.sync_tiles(&order, window, group, restore);
        self.focused.set(Some(window.focused_tab));
        self.update_gates(window.focused_tab);
        let (order, rects) = self.rects();
        if rects.iter().any(|rect| rect.w > 0 && rect.h > 0) {
            self.place_all(&order, &rects, true);
        }
        self.refresh_classes(window.focused_tab);
        self.log_layout();
    }

    fn sync_tiles(self: &Rc<Self>, order: &[TabId], window: &Window, group: &Group, restore: bool) {
        let current: Vec<TabId> = self.tiles.borrow().iter().map(|tile| tile.id).collect();
        for id in order {
            if !current.contains(id)
                && let Some(tab) = group.tab(*id)
            {
                self.add_tile(*id, &folder_name(&tab.folder));
            }
        }
        let stale: Vec<TabId> = self
            .tiles
            .borrow()
            .iter()
            .map(|tile| tile.id)
            .filter(|id| !order.contains(id))
            .collect();
        for id in stale {
            self.remove_tile(id);
        }
        let ids: Vec<TabId> = self.tiles.borrow().iter().map(|tile| tile.id).collect();
        for id in ids {
            self.sync_folder(id, window, group, restore);
        }
    }

    fn add_tile(self: &Rc<Self>, id: TabId, name: &str) {
        let Some(process) = self.process.upgrade() else {
            return;
        };
        let scale = self.scale.get();
        let gate = Rc::new(Cell::new(false));
        let view = FolderView::new(
            &process,
            self.window,
            self.status.clone(),
            scale,
            Some(id),
            Some(gate.clone()),
        );
        let (outer, frame, grid_plane, grid, pill, close, pill_wrap) =
            tile_widget(name, &view, scale);

        self.fixed.put(&outer, 0.0, 0.0);
        outer.set_size_request(1, 1);
        self.fixed.put(&pill_wrap, 0.0, 0.0);
        pill_wrap.set_size_request(1, 1);
        pill_wrap.insert_after(&self.fixed, Some(&outer));

        attach_handlers(Rc::downgrade(self), id, &outer, &pill, &close);

        self.tiles.borrow_mut().push(Tile {
            id,
            outer,
            frame,
            grid_plane,
            grid,
            pill,
            pill_wrap,
            view,
            displayed_folder: Rc::new(RefCell::new(None)),
            gate,
            _name: name.to_string(),
        });
        self.slots.borrow_mut().insert(
            id,
            Slot {
                pos: Rc::new(Cell::new((0.0, 0.0))),
                size: Rc::new(Cell::new((1, 1))),
                generation: Rc::new(Cell::new(0)),
                slide: None,
            },
        );
    }

    fn remove_tile(&self, id: TabId) {
        let removed = {
            let mut tiles = self.tiles.borrow_mut();
            tiles
                .iter()
                .position(|tile| tile.id == id)
                .map(|index| tiles.remove(index))
        };
        if let Some(tile) = removed {
            self.bump_gen(id);
            self.fixed.remove(&tile.outer);
            self.fixed.remove(&tile.pill_wrap);
            self.slots.borrow_mut().remove(&id);
            if let Some(Drag::Tile { from, .. }) = self.drag.borrow().clone()
                && from == id
            {
                *self.drag.borrow_mut() = None;
            }
        }
    }

    fn sync_folder(self: &Rc<Self>, id: TabId, window: &Window, group: &Group, restore: bool) {
        let Some(tab) = group.tab(id) else {
            return;
        };
        let (view, displayed, gate, pill) = {
            let tiles = self.tiles.borrow();
            let Some(tile) = tiles.iter().find(|tile| tile.id == id) else {
                return;
            };
            (
                tile.view.clone(),
                tile.displayed_folder.clone(),
                tile.gate.clone(),
                tile.pill.clone(),
            )
        };
        let folder = tab.folder.clone();
        let changed = displayed.borrow().as_ref() != Some(&folder);
        if changed {
            let first = displayed.borrow().is_none();
            let back_or_forward = restore && !first;
            let restore_anchor = first || back_or_forward;
            let anchor = if restore_anchor {
                tab.scroll_anchor.clone()
            } else {
                None
            };
            let rule = self
                .process
                .upgrade()
                .map(|process| process.config.borrow().config().sort_for(&folder))
                .unwrap_or_else(default_rule);
            view.set_folder(&folder, rule, anchor, restore_anchor, back_or_forward);
            // The tile's pill names its folder, so it follows navigation
            // inside the tile (Kyle, after P2k: it kept the first name).
            if let Some(label) = pill.first_child().and_downcast::<gtk::Label>() {
                label.set_text(&folder_name(&folder));
            }
            *displayed.borrow_mut() = Some(folder);
        }
        gate.set(id == window.focused_tab);
        view.set_selection(&tab.selection);
        if id == window.focused_tab {
            view.refresh_status();
        }
    }

    fn update_gates(&self, focused: TabId) {
        for tile in self.tiles.borrow().iter() {
            let on = tile.id == focused;
            tile.gate.set(on);
            tile.view.set_status_gate(on);
        }
    }

    fn refresh_classes(&self, focused: TabId) {
        for tile in self.tiles.borrow().iter() {
            tile.frame.remove_css_class("focused");
            tile.pill.remove_css_class("active");
            if tile.id == focused {
                tile.frame.add_css_class("focused");
                tile.pill.add_css_class("active");
            }
        }
    }

    fn place_all(self: &Rc<Self>, order: &[TabId], rects: &[Rect], animate: bool) {
        let scale = self.scale.get();
        for (index, id) in order.iter().enumerate() {
            let Some(rect) = rects.get(index).copied() else {
                continue;
            };
            let Some(tile) = self.tile_parts(*id) else {
                continue;
            };
            let wide = pill_is_wide(rect.w, scale);
            if wide {
                tile.pill.add_css_class("wide");
            } else {
                tile.pill.remove_css_class("wide");
            }
            if let Some(label) = tile.pill.first_child().and_downcast::<gtk::Label>() {
                label.set_max_width_chars(if wide {
                    -1
                } else {
                    pill_label_chars(rect.w, scale)
                });
            }
            tile.pill.set_width_request(pill_width(rect.w, scale));
            let fresh = self
                .slots
                .borrow()
                .get(id)
                .map(|slot| slot.size.get().0 <= 1)
                .unwrap_or(true);
            if animate && !fresh {
                self.slide_to(*id, &tile.outer, rect);
            } else {
                self.set_rect(*id, &tile.outer, rect);
            }
        }
    }

    fn tile_parts(&self, id: TabId) -> Option<TileParts> {
        self.tiles
            .borrow()
            .iter()
            .find(|tile| tile.id == id)
            .map(|tile| TileParts {
                outer: tile.outer.clone(),
                frame: tile.frame.clone(),
                grid_plane: tile.grid_plane.clone(),
                grid: tile.grid.clone(),
                pill: tile.pill.clone(),
                pill_wrap: tile.pill_wrap.clone(),
            })
    }

    fn relayout(self: &Rc<Self>, animate: bool) {
        let (order, rects) = self.rects();
        if !self.tiles.borrow().is_empty() && rects.iter().all(|r| r.w <= 0 || r.h <= 0) {
            return;
        }
        self.place_all(&order, &rects, animate);
        if self.drag.borrow().is_none() {
            self.log_layout();
        }
    }

    /// One line per layout: the model's columns, heights and share, every
    /// tile's rect and every pill's drawn position (content coordinates).
    fn log_layout(&self) {
        let layout = self.current_layout();
        let (order, rects) = self.rects();
        let view = self.viewport();
        let scroll = self.scroll.get();
        let scale = view.scale;
        let pills: Vec<(TabId, Rect)> = order
            .iter()
            .enumerate()
            .filter_map(|(index, id)| {
                rects
                    .get(index)
                    .map(|rect| (*id, pill_content_rect(*rect, scroll, view.h, scale)))
            })
            .collect();
        let actual: Vec<VisualState> = order
            .iter()
            .filter_map(|id| {
                self.slots
                    .borrow()
                    .get(id)
                    .map(|slot| (*id, slot.pos.get(), slot.size.get()))
            })
            .collect();
        let pill_actual: Vec<VisualState> = self
            .tiles
            .borrow()
            .iter()
            .map(|tile| {
                let position = self.fixed.child_position(&tile.pill_wrap);
                (
                    tile.id,
                    position,
                    (tile.pill_wrap.width(), tile.pill_wrap.height()),
                )
            })
            .collect();
        crate::log_verbose!(
            "split",
            "layout columns {:?} heights {:?} share {} at scale {scale:.2}; rects {rects:?}; pills {pills:?}; actual {actual:?}; pill wraps {pill_actual:?}",
            layout.columns,
            layout.heights,
            layout.width_permille
        );
    }

    fn schedule_scroll(self: &Rc<Self>) {
        if self.scroll_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(inner) = weak.upgrade() {
                inner.scroll_pending.set(false);
                if inner.drag.borrow().is_none() {
                    inner.relayout(false);
                }
            }
        });
    }

    fn set_visual(&self, id: TabId, widget: &gtk::Box, x: f64, y: f64, w: i32, h: i32) {
        if let Some(slot) = self.slots.borrow().get(&id) {
            slot.pos.set((x, y));
            slot.size.set((w, h));
        }
        self.fixed.move_(widget, x, y);
        widget.set_size_request(w, h);
    }

    fn bump_gen(&self, id: TabId) {
        if let Some(slot) = self.slots.borrow().get(&id) {
            slot.generation.set(slot.generation.get() + 1);
        }
    }

    fn set_rect(&self, id: TabId, widget: &gtk::Box, rect: Rect) {
        self.bump_gen(id);
        if let Some(slot) = self.slots.borrow_mut().get_mut(&id) {
            slot.slide = None;
        }
        self.set_visual(id, widget, rect.x as f64, rect.y as f64, rect.w, rect.h);
        self.place_parts(id, rect);
    }

    fn slide_to(self: &Rc<Self>, id: TabId, widget: &gtk::Box, end: Rect) {
        let (start, size) = {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(&id) else {
                return;
            };
            (slot.pos.get(), slot.size.get())
        };
        let (ex, ey) = (end.x as f64, end.y as f64);
        if (start.0 - ex).abs() < 0.5 && (start.1 - ey).abs() < 0.5 && size == (end.w, end.h) {
            self.set_rect(id, widget, end);
            return;
        }
        let generation_cell = {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(&id) else {
                return;
            };
            slot.generation.clone()
        };
        let generation = generation_cell.get() + 1;
        generation_cell.set(generation);
        let (cell, size_cell) = {
            let slots = self.slots.borrow();
            let Some(slot) = slots.get(&id) else {
                return;
            };
            (slot.pos.clone(), slot.size.clone())
        };
        let weak = Rc::downgrade(self);
        let fixed = self.fixed.clone();
        let widget = widget.clone();
        let animation_widget = widget.clone();
        let (start_w, start_h) = size;
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if generation_cell.get() != generation {
                return;
            }
            let x = start.0 + (ex - start.0) * v;
            let y = start.1 + (ey - start.1) * v;
            let width = (start_w as f64 + (end.w - start_w) as f64 * v).round() as i32;
            let height = (start_h as f64 + (end.h - start_h) as f64 * v).round() as i32;
            let width = width.max(1);
            let height = height.max(1);
            cell.set((x, y));
            size_cell.set((width, height));
            fixed.move_(&widget, x, y);
            widget.set_size_request(width, height);
            if let Some(inner) = weak.upgrade() {
                inner.place_parts(
                    id,
                    Rect {
                        x: x.round() as i32,
                        y: y.round() as i32,
                        w: width,
                        h: height,
                    },
                );
            }
        });
        let animation =
            adw::TimedAnimation::new(&animation_widget, 0.0, 1.0, motion::ms(200), target);
        if let Some(slot) = self.slots.borrow_mut().get_mut(&id) {
            slot.slide = Some(animation.clone());
        }
        animation.play();
    }

    fn place_parts(&self, id: TabId, rect: Rect) {
        let view = self.viewport();
        let layout = pill_layout(rect, self.scroll.get(), view.h, self.scale.get());
        self.apply_parts(id, rect, layout);
    }

    fn apply_parts(&self, id: TabId, rect: Rect, layout: PillLayout) {
        let Some(tile) = self.tile_parts(id) else {
            return;
        };
        tile.pill_wrap
            .set_size_request(layout.wrap.w, layout.wrap.h);
        self.fixed
            .move_(&tile.pill_wrap, layout.wrap.x as f64, layout.wrap.y as f64);
        tile.pill_wrap.insert_after(&self.fixed, Some(&tile.outer));
        if tile.frame.margin_top() != layout.frame_top {
            tile.frame.set_margin_top(layout.frame_top);
        }
        let (w, h) = grid_size(rect, self.scale.get());
        tile.grid.set_size_request(w, h);
        tile.grid_plane
            .move_(&tile.grid, 0.0, -(layout.content_offset as f64));
    }

    fn place_dragged(&self, id: TabId, widget: &gtk::Box, rect: Rect) {
        self.set_visual(id, widget, rect.x as f64, rect.y as f64, rect.w, rect.h);
        let scale = self.scale.get();
        self.apply_parts(id, rect, dragged_pill_layout(rect, scale));
    }

    fn begin_drag(self: &Rc<Self>, gesture: &gtk::GestureDrag, sx: f64, sy: f64) {
        let button = gesture.current_button();
        if button != 0 && button != 1 {
            return;
        }
        let (order, rects) = self.rects();
        let view = self.viewport();
        let scroll = self.scroll.get();
        let scale = view.scale;
        let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
            .map(|(x, y)| (x.round() as i32, y.round() as i32))
            .unwrap_or((sx.round() as i32, sy.round() as i32));
        let slots = slots_of(&self.current_layout().columns, &order, &rects);
        let border = border_at(&slots, px, py, scroll, view.h, scale);
        crate::log!("split", "border press at ({px}, {py}) hit {border:?}");
        if let Some(id) = pill_at(&order, &rects, px, py, scroll, view.h, scale) {
            let Some(index) = order.iter().position(|candidate| *candidate == id) else {
                return;
            };
            let Some(rect) = rects.get(index).copied() else {
                return;
            };
            self.start_tile_drag(id, index, rect, px, py, rects);
            return;
        }
        let Some(border) = border else {
            return;
        };
        let layout = self.current_layout();
        match border {
            Border::Columns => {
                let current = self.current_layout();
                *self.preview.borrow_mut() = Some(current);
                *self.drag.borrow_mut() = Some(Drag::Width {
                    permille: layout.width_permille,
                    press_x: px,
                });
                crate::log!("split", "resizing the column border from x {px}");
            }
            Border::Heights { column, row } => {
                let f = frame(view.w, view.h, scale);
                let total = layout.columns[0].len() + layout.columns[1].len();
                let heights = match &layout.heights[column] {
                    Heights::Default => slots[column]
                        .iter()
                        .map(|r| (r.h as f64 / scale).round() as i32)
                        .collect(),
                    Heights::Hand(_) => stored_heights(
                        &layout.heights[column],
                        layout.columns[column].len(),
                        f.inner_h,
                        f.gutter,
                        half_floor(total, &f),
                        scale,
                    ),
                };
                *self.preview.borrow_mut() = Some(layout);
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

    fn start_tile_drag(
        self: &Rc<Self>,
        id: TabId,
        index: usize,
        rect: Rect,
        px: i32,
        py: i32,
        rects: Vec<Rect>,
    ) {
        let Some(tile) = self.tile_parts(id) else {
            return;
        };
        *self.drag.borrow_mut() = Some(Drag::Tile {
            from: id,
            from_index: index,
            grab: (px - rect.x, py - rect.y),
            press: (px, py),
            targets: rects,
        });
        let current = self.current_layout();
        *self.preview.borrow_mut() = Some(current);
        self.bump_gen(id);
        if let Some(slot) = self.slots.borrow_mut().get_mut(&id) {
            slot.slide = None;
        }
        if self.fixed.last_child().as_ref() != Some(tile.outer.upcast_ref::<gtk::Widget>()) {
            let last = self.fixed.last_child();
            tile.outer.insert_after(&self.fixed, last.as_ref());
        }
        self.place_dragged(id, &tile.outer, rect);
    }

    fn update_drag(self: &Rc<Self>, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let Some(drag) = self.drag.borrow().clone() else {
            return;
        };
        match drag {
            Drag::Tile {
                from,
                from_index,
                grab,
                press,
                targets,
            } => {
                let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    .unwrap_or((press.0 + dx.round() as i32, press.1 + dy.round() as i32));
                self.update_tile_drag(from, from_index, grab, press, targets, px, py);
            }
            Drag::Width { permille, press_x } => {
                let view = self.viewport();
                let f = frame(view.w, view.h, view.scale);
                let avail = (f.inner_w - f.gutter).max(1);
                let (px, _) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(x, _)| (x.round() as i32, 0))
                    .unwrap_or((press_x + dx.round() as i32, 0));
                let share = width_share_after_drag(
                    f64::from(permille) / 1000.0,
                    avail,
                    px - press_x,
                    min_column_width(view.scale),
                );
                let next = (share * 1000.0).round().clamp(0.0, 1000.0) as u16;
                if let Some(preview) = self.preview.borrow_mut().as_mut() {
                    preview.width_permille = next;
                }
                self.relayout(false);
            }
            Drag::Height {
                column,
                border,
                heights,
                press_y,
            } => {
                let view = self.viewport();
                let (_, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(_, y)| (0, y.round() as i32))
                    .unwrap_or((0, press_y + dy.round() as i32));
                let dragged = py - press_y;
                let f = frame(view.w, view.h, view.scale);
                let design_dy = (dragged as f64 / view.scale).round() as i32;
                let visible = (f.inner_h as f64 / view.scale).round() as i32;
                let gap = (f.gutter as f64 / view.scale).round() as i32;
                let pushed = heights_after_push(
                    &heights,
                    border,
                    design_dy,
                    min_slot_height(1.0),
                    visible,
                    gap,
                );
                let shown: Vec<i32> = pushed
                    .shown
                    .iter()
                    .map(|height| scaled(*height as f64, view.scale))
                    .collect();
                crate::log!("split", "border drag dy {dragged} heights {shown:?}");
                if let Some(preview) = self.preview.borrow_mut().as_mut() {
                    preview.heights[column] = Heights::Hand(pushed.stored.clone());
                }
                self.relayout(false);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn update_tile_drag(
        self: &Rc<Self>,
        from: TabId,
        from_index: usize,
        grab: (i32, i32),
        press: (i32, i32),
        targets: Vec<Rect>,
        px: i32,
        py: i32,
    ) {
        let layout = self.current_layout();
        let view = self.viewport();
        let scroll = self.scroll.get();
        let (order, rects) = self.rects();
        let Some(&rect) = rects.get(from_index) else {
            return;
        };
        let board = Board {
            layout: &layout,
            view,
            scroll,
        };
        let (drop, preview) = drag_layout(&board, &order, &rects, from_index, px, py, grab);
        let size = (rect.w, rect.h);
        for (index, r) in preview.iter().enumerate() {
            let Some(id) = order.get(index).copied() else {
                continue;
            };
            if id == from {
                if let Some(tile) = self.tile_parts(id) {
                    self.place_dragged(
                        id,
                        &tile.outer,
                        Rect {
                            x: r.x,
                            y: r.y,
                            w: size.0,
                            h: size.1,
                        },
                    );
                }
            } else if targets.get(index).copied() != Some(*r)
                && let Some(tile) = self.tile_parts(id)
            {
                self.slide_to(id, &tile.outer, *r);
            }
        }
        self.show_drop_line(drop);
        *self.drag.borrow_mut() = Some(Drag::Tile {
            from,
            from_index,
            grab,
            press,
            targets: preview,
        });
    }

    fn end_drag(self: &Rc<Self>, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
        let drag = self.drag.borrow().clone();
        let Some(drag) = drag else {
            return;
        };
        match drag {
            Drag::Tile { press, .. } => {
                let (px, py) = pointer_in_fixed(&self.fixed, gesture.current_event().as_ref())
                    .map(|(x, y)| (x.round() as i32, y.round() as i32))
                    .unwrap_or((press.0 + dx.round() as i32, press.1 + dy.round() as i32));
                self.finish_tile_drag(px, py);
            }
            Drag::Width { permille, .. } => self.commit_width(permille),
            Drag::Height {
                column, heights, ..
            } => self.commit_heights(column, heights),
        }
    }

    fn finish_tile_drag(self: &Rc<Self>, px: i32, py: i32) {
        let Some(Drag::Tile { from, .. }) = self.drag.borrow().clone() else {
            return;
        };
        let layout = self.current_layout();
        let view = self.viewport();
        let (order, rects) = self.rects();
        let drop = drop_at(&layout.columns, &order, &rects, from, px, py, view.scale);
        let accepted =
            drop.and_then(|d| apply_drop(&layout.columns, from, d).map(|next| (d, next)));
        let Some((drop, _next)) = accepted else {
            *self.drag.borrow_mut() = None;
            *self.preview.borrow_mut() = None;
            self.hide_drop_line();
            self.restore_tiles();
            crate::log!("split", "drag ended with no drop; sliding everything back");
            return;
        };
        *self.drag.borrow_mut() = None;
        *self.preview.borrow_mut() = None;
        self.hide_drop_line();
        if let Some(process) = self.process.upgrade() {
            process.apply_tile_drop(self.window, from, drop);
        }
        crate::log!("split", "{from} dropped at {drop:?}");
    }

    fn restore_tiles(self: &Rc<Self>) {
        let (order, rects) = self.rects();
        for (index, id) in order.iter().enumerate() {
            if let (Some(rect), Some(tile)) = (rects.get(index), self.tile_parts(*id)) {
                self.slide_to(*id, &tile.outer, *rect);
            }
        }
        self.hide_drop_line();
    }

    fn cancel_drag(self: &Rc<Self>) -> bool {
        let drag = self.drag.borrow_mut().take();
        let Some(drag) = drag else {
            return false;
        };
        match drag {
            Drag::Tile { .. } => {
                *self.preview.borrow_mut() = None;
                self.restore_tiles();
                self.menu_blocked.set(true);
                // The same right press must not open the context menu.
                let weak = self.menu_blocked.clone();
                glib::idle_add_local_once(move || weak.set(false));
                crate::log!("split", "tile drag cancelled; sliding everything back");
            }
            Drag::Width { permille, .. } => {
                self.commit_width(permille);
                crate::log!("split", "border drag cancelled; its sizes stay");
            }
            Drag::Height {
                column, heights, ..
            } => {
                self.commit_heights(column, heights);
                crate::log!("split", "border drag cancelled; its sizes stay");
            }
        }
        true
    }

    fn commit_width(self: &Rc<Self>, permille: u16) {
        let permille = self
            .preview
            .borrow()
            .as_ref()
            .map(|preview| preview.width_permille)
            .unwrap_or(permille);
        *self.drag.borrow_mut() = None;
        *self.preview.borrow_mut() = None;
        if let Some(process) = self.process.upgrade() {
            process.set_width_share(self.window, permille);
        }
    }

    fn commit_heights(self: &Rc<Self>, column: usize, heights: Vec<i32>) {
        let heights = self
            .preview
            .borrow()
            .as_ref()
            .and_then(|preview| match &preview.heights[column] {
                Heights::Hand(values) => Some(values.clone()),
                Heights::Default => None,
            })
            .unwrap_or(heights);
        *self.drag.borrow_mut() = None;
        *self.preview.borrow_mut() = None;
        if let Some(process) = self.process.upgrade() {
            process.set_column_heights(self.window, column, Heights::Hand(heights));
        }
    }

    fn reset_border(self: &Rc<Self>, x: f64, y: f64) {
        let (order, rects) = self.rects();
        let view = self.viewport();
        let scroll = self.scroll.get();
        let slots = slots_of(&self.current_layout().columns, &order, &rects);
        match border_at(
            &slots,
            x.round() as i32,
            y.round() as i32,
            scroll,
            view.h,
            view.scale,
        ) {
            Some(Border::Columns) => {
                if let Some(process) = self.process.upgrade() {
                    process.set_width_share(self.window, 500);
                }
                crate::log!("split", "reset the column border to equal shares");
            }
            Some(Border::Heights { column, .. }) => {
                if let Some(process) = self.process.upgrade() {
                    process.set_column_heights(self.window, column, Heights::Default);
                }
                crate::log!("split", "reset column {column} to equal shares");
            }
            None => {}
        }
    }

    fn hover(self: &Rc<Self>, x: f64, y: f64) {
        let (order, rects) = self.rects();
        let view = self.viewport();
        let scroll = self.scroll.get();
        let slots = slots_of(&self.current_layout().columns, &order, &rects);
        let cursor = match border_at(
            &slots,
            x.round() as i32,
            y.round() as i32,
            scroll,
            view.h,
            view.scale,
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
        let (order, rects) = self.rects();
        let slots = slots_of(&self.current_layout().columns, &order, &rects);
        let scale = self.scale.get();
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

    fn popup_menu(self: &Rc<Self>, id: TabId) {
        let pill = {
            let tiles = self.tiles.borrow();
            tiles
                .iter()
                .find(|tile| tile.id == id)
                .map(|tile| tile.pill.clone())
        };
        let Some(pill) = pill else {
            return;
        };
        let popover = gtk::Popover::new();
        popover.set_parent(&pill);
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 0);
        menu.add_css_class("gl-menu");

        let weak = Rc::downgrade(self);
        menu_item(&popover, &menu, "Close", move || {
            if let Some(inner) = weak.upgrade()
                && let Some(process) = inner.process.upgrade()
            {
                process.focus_tab_and_dispatch(
                    inner.window,
                    id,
                    fm_core::command::Invocation::new("tab.close", Vec::new()),
                );
            }
        });
        let weak = Rc::downgrade(self);
        menu_item(&popover, &menu, "Open in New Window", move || {
            if let Some(inner) = weak.upgrade()
                && let Some(process) = inner.process.upgrade()
            {
                process.focus_tab_and_dispatch(
                    inner.window,
                    id,
                    fm_core::command::Invocation::new("tab.open-in-new-window", Vec::new()),
                );
            }
        });
        let weak = Rc::downgrade(self);
        menu_item(&popover, &menu, "Open in Own Tab", move || {
            if let Some(inner) = weak.upgrade()
                && let Some(process) = inner.process.upgrade()
                && let Some(window) = process.window_snapshot(inner.window)
                && window.group_of(id).is_some_and(|group| group.is_split())
            {
                process.focus_tab_and_dispatch(
                    inner.window,
                    id,
                    fm_core::command::Invocation::new("tab.open-in-own-tab", Vec::new()),
                );
            }
        });
        let weak = Rc::downgrade(self);
        menu_item(&popover, &menu, "Close Group", move || {
            if let Some(inner) = weak.upgrade()
                && let Some(process) = inner.process.upgrade()
                && let Some(window) = process.window_snapshot(inner.window)
                && let Some(group) = window.group_of(id)
                && let Some(index) = window.pill_index(group.id)
                && let Some(ui) = process.window(inner.window)
            {
                ui.confirm_and_close_group(index, group.tabs.len());
            }
        });

        popover.set_child(Some(&menu));
        popover.connect_closed(|popover| {
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        crate::popup::show(&popover);
    }
}

struct TileParts {
    outer: gtk::Box,
    frame: gtk::Box,
    grid_plane: gtk::Fixed,
    grid: gtk::ScrolledWindow,
    pill: gtk::Box,
    pill_wrap: gtk::ScrolledWindow,
}

fn tile_widget(
    name: &str,
    view: &Rc<FolderView>,
    scale: f64,
) -> (
    gtk::Box,
    gtk::Box,
    gtk::Fixed,
    gtk::ScrolledWindow,
    gtk::Box,
    gtk::Button,
    gtk::ScrolledWindow,
) {
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
    pill.add_css_class("tile");
    pill.set_halign(gtk::Align::Center);
    let label = gtk::Label::new(Some(name));
    label.set_hexpand(true);
    label.set_ellipsize(pango::EllipsizeMode::End);
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
    frame.set_overflow(gtk::Overflow::Hidden);
    let grid = gtk::ScrolledWindow::new();
    grid.set_policy(gtk::PolicyType::External, gtk::PolicyType::External);
    grid.set_propagate_natural_width(false);
    grid.set_propagate_natural_height(false);
    grid.set_child(Some(view.widget()));
    let grid_plane = gtk::Fixed::new();
    grid_plane.set_hexpand(true);
    grid_plane.set_vexpand(true);
    grid_plane.put(&grid, 0.0, 0.0);
    frame.append(&grid_plane);
    content.append(&frame);
    clip.set_child(Some(&content));
    outer.append(&clip);
    (outer, frame, grid_plane, grid, pill, close, pill_wrap)
}

fn attach_handlers(
    weak: Weak<Inner>,
    id: TabId,
    outer: &gtk::Box,
    pill: &gtk::Box,
    close: &gtk::Button,
) {
    if let Some(inner) = weak.upgrade()
        && let Some(process) = inner.process.upgrade()
    {
        let weak_folder = weak.clone();
        dnd::attach_folder_target(
            pill,
            Rc::downgrade(&process),
            inner.window,
            Rc::new(move || {
                let inner = weak_folder.upgrade()?;
                let process = inner.process.upgrade()?;
                process.tab_folder(inner.window, id)
            }),
            None,
            None,
        );
        let spring = process.clone();
        let window = inner.window;
        dnd::spring_load(pill, Rc::new(move || spring.spring_focus_tab(window, id)));
    }

    let click = gtk::GestureClick::new();
    click.set_button(1);
    click.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak_click = weak.clone();
    click.connect_pressed(move |_, _, _, _| {
        if let Some(inner) = weak_click.upgrade()
            && let Some(process) = inner.process.upgrade()
        {
            process.focus_tab(inner.window, id);
            let view = inner
                .tiles
                .borrow()
                .iter()
                .find(|tile| tile.id == id)
                .map(|tile| tile.view.clone());
            if let Some(view) = view {
                view.grab_grid_focus();
            }
        }
    });
    outer.add_controller(click);

    let pill_click = gtk::GestureClick::new();
    pill_click.set_button(0);
    let weak_pill = weak.clone();
    pill_click.connect_pressed(move |gesture, _, _, _| {
        let Some(inner) = weak_pill.upgrade() else {
            return;
        };
        match gesture.current_button() {
            3 if inner.drag.borrow().is_none() && !inner.menu_blocked.get() => {
                inner.popup_menu(id);
            }
            1 => {
                if let Some(process) = inner.process.upgrade() {
                    process.focus_tab(inner.window, id);
                }
            }
            _ => {}
        }
    });
    pill.add_controller(pill_click);

    let weak_close = weak.clone();
    close.connect_clicked(move |_| {
        if let Some(inner) = weak_close.upgrade()
            && let Some(process) = inner.process.upgrade()
        {
            process.focus_tab_and_dispatch(
                inner.window,
                id,
                fm_core::command::Invocation::new("tab.close", Vec::new()),
            );
        }
    });
}

fn menu_item(popover: &gtk::Popover, menu: &gtk::Box, text: &str, action: impl Fn() + 'static) {
    let item = gtk::Button::new();
    item.add_css_class("gl-menu-item");
    item.set_child(Some(
        &gtk::Label::builder()
            .label(text)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build(),
    ));
    let popover = popover.clone();
    item.connect_clicked(move |_| {
        popover.popdown();
        if popover.parent().is_some() {
            popover.unparent();
        }
        action();
    });
    menu.append(&item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::workspace::{
        columns_after_insert, columns_after_swap, columns_without, initial_columns,
    };

    const VIEW: (i32, i32) = (1200, 1400);

    fn tab(raw: u64) -> TabId {
        TabId::from_raw(raw)
    }

    fn tabs(ids: &[u64]) -> Vec<TabId> {
        ids.iter().map(|raw| tab(*raw)).collect()
    }

    fn defaults() -> [Heights; 2] {
        [Heights::Default, Heights::Default]
    }

    fn state(columns: Columns) -> LayoutState {
        LayoutState {
            columns,
            heights: defaults(),
            width_permille: 500,
        }
    }

    fn order_of(columns: &Columns) -> Vec<TabId> {
        let mut order = flatten(columns);
        order.sort_unstable();
        order
    }

    fn slots(columns: &Columns, heights: &[Heights; 2]) -> [Vec<Rect>; 2] {
        layout(columns, heights, 500, VIEW.0, VIEW.1, 1.0)
    }

    fn rects_for(columns: &Columns) -> Vec<Rect> {
        let order = order_of(columns);
        let s = slots(columns, &defaults());
        rects_by_tile(&order, columns, &s)
    }

    #[test]
    fn tiles_fill_the_height_and_split_the_width() {
        let columns = initial_columns(&tabs(&[0, 1]));
        let s = slots(&columns, &defaults());
        assert_eq!(s[0][0].y, 0);
        assert_eq!(s[0][0].h, VIEW.1 - scaled(GAP_2XL, 1.0));
        assert_eq!(s[0][0].x, scaled(GAP_2XL, 1.0));
        assert_eq!(s[0][0].w, s[1][0].w);
        assert_eq!(s[1][0].x, s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0));
        assert!(layout(&initial_columns(&tabs(&[])), &defaults(), 500, 0, 800, 1.0)[0].is_empty());
    }

    #[test]
    fn uneven_columns_make_their_tiles_taller() {
        let three = rects_for(&initial_columns(&tabs(&[0, 1, 2])));
        let avail = VIEW.1 - scaled(GAP_2XL, 1.0) - scaled(GAP_L, 1.0);
        assert_eq!(three[0].h, avail / 2);
        assert_eq!(three[1].h, VIEW.1 - scaled(GAP_2XL, 1.0));
        assert_eq!(three[2].y, three[0].h + scaled(GAP_L, 1.0));
        assert!(three[2].y + three[2].h >= VIEW.1 - scaled(GAP_2XL, 1.0) - 1);
        let four = rects_for(&initial_columns(&tabs(&[0, 1, 2, 3])));
        assert_eq!((four[0].h, four[1].h), (four[2].h, four[3].h));
        assert_eq!(four[2].y, four[3].y);
        let six = rects_for(&initial_columns(&tabs(&[0, 1, 2, 3, 4, 5])));
        let f = frame(VIEW.0, VIEW.1, 1.0);
        let half = (f.inner_h - f.gutter) / 2;
        assert_eq!(six[4].h, half);
        assert!(six[4].y + six[4].h > VIEW.1);
        assert!(six[5].y + six[5].h > VIEW.1);
        let frame_h = |r: Rect| r.h - scaled(CONTROL_H, 1.0) - scaled(GAP_S, 1.0);
        assert!(frame_h(three[1]) > frame_h(three[0]));
    }

    #[test]
    fn closing_from_either_column_leaves_the_other_taller() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let left_closed = columns_without(&four, tab(2));
        assert_eq!((left_closed[0].len(), left_closed[1].len()), (1, 2));
        let r = rects_for(&left_closed);
        assert_eq!(r[0].x, scaled(GAP_2XL, 1.0));
        assert!(r[0].h > r[1].h);
        let right_closed = columns_without(&four, tab(3));
        assert_eq!((right_closed[0].len(), right_closed[1].len()), (2, 1));
        let r = rects_for(&right_closed);
        assert!(r[1].h > r[0].h);
    }

    #[test]
    fn a_close_rebalances_two_or_more_apart() {
        let six = initial_columns(&tabs(&[0, 1, 2, 3, 4, 5]));
        let five = columns_without(&six, tab(5));
        assert_eq!(five, [tabs(&[0, 2, 4]), tabs(&[1, 3])]);
        let four = columns_without(&five, tab(3));
        assert_eq!((four[0].len(), four[1].len()), (2, 2));
        assert_eq!(four, [tabs(&[0, 2]), tabs(&[1, 4])]);
        let order = order_of(&four);
        let r = rects_for(&four);
        let moved = order.iter().position(|id| *id == tab(4)).unwrap();
        let partner = order.iter().position(|id| *id == tab(1)).unwrap();
        assert_eq!(r[moved].x, r[partner].x);
    }

    #[test]
    fn a_hand_made_layout_survives_a_close_that_leaves_one_gap() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        let after = columns_without(&hand, tab(2));
        assert_eq!(after, [tabs(&[0, 1]), tabs(&[3])]);
        assert_eq!((after[0].len(), after[1].len()), (2, 1));
    }

    #[test]
    fn closing_the_lone_tile_rebalances() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        let after = columns_without(&hand, tab(3));
        assert_eq!((after[0].len(), after[1].len()), (2, 1));
        assert_eq!(after, [tabs(&[0, 1]), tabs(&[2])]);
        let two = columns_without(&initial_columns(&tabs(&[0, 1, 2])), tab(1));
        assert_eq!(two, [tabs(&[0]), tabs(&[2])]);
        let r = rects_for(&two);
        assert_eq!(r[0].h, r[1].h);
    }

    #[test]
    fn dropping_on_a_tile_swaps_across_and_reorders_within() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let swapped = columns_after_swap(&four, tab(0), tab(1)).unwrap();
        assert_eq!(swapped, [tabs(&[1, 2]), tabs(&[0, 3])]);
        let r = rects_for(&swapped);
        assert!(r[0].x > r[1].x);
        assert_eq!(r[0].h, r[1].h);
        let reordered = columns_after_swap(&four, tab(0), tab(2)).unwrap();
        assert_eq!(reordered, [tabs(&[2, 0]), tabs(&[1, 3])]);
        assert!(rects_for(&reordered)[0].y > 0);
        let back = columns_after_swap(&four, tab(3), tab(1)).unwrap();
        assert_eq!(back, [tabs(&[0, 2]), tabs(&[3, 1])]);
        assert_eq!(columns_after_swap(&four, tab(2), tab(2)), None);
    }

    #[test]
    fn dropping_in_a_gap_inserts_and_the_column_gets_longer() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        assert_eq!(
            columns_after_insert(&four, tab(0), 0, 2).unwrap(),
            [tabs(&[2, 0]), tabs(&[1, 3])]
        );
        assert_eq!(columns_after_insert(&four, tab(0), 0, 1), None);
        let moved = columns_after_insert(&four, tab(0), 1, 2).unwrap();
        assert_eq!(moved, [tabs(&[2]), tabs(&[1, 3, 0])]);
        assert_eq!((moved[0].len(), moved[1].len()), (1, 3));
        let moved = columns_after_insert(&four, tab(3), 0, 2).unwrap();
        assert_eq!(moved, [tabs(&[0, 2, 3]), tabs(&[1])]);
        let tall = rects_for(&moved);
        assert!(tall[1].h > tall[0].h);
    }

    #[test]
    fn a_drop_never_empties_a_column() {
        let hand = [tabs(&[0, 1, 2]), tabs(&[3])];
        assert_eq!(columns_after_insert(&hand, tab(3), 0, 1), None);
        assert_eq!(
            apply_drop(&hand, tab(3), Drop::Insert { column: 0, row: 2 }),
            None
        );
        let two = [tabs(&[0, 1]), tabs(&[2])];
        assert!(columns_after_insert(&two, tab(0), 1, 1).is_some());
        let one = [tabs(&[0]), vec![]];
        assert!(columns_after_insert(&one, tab(0), 0, 1).is_none());
    }

    #[test]
    fn the_drop_zone_is_the_gap_plus_a_fifth_on_each_side() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&four);
        let order = order_of(&four);
        let s = slots_of(&four, &order, &rects);
        let gap = scaled(GAP_L, 1.0);
        let band = s[0][0].h / 5;
        let other = s[1][0];
        assert_eq!(
            drop_at(
                &four,
                &order,
                &rects,
                tab(0),
                other.x + 10,
                other.y + other.h / 2,
                1.0
            ),
            Some(Drop::Swap(tab(1)))
        );
        let gap_y = s[0][1].y - gap / 2;
        assert_eq!(
            drop_at(
                &four,
                &order,
                &rects,
                tab(0),
                s[0][0].x + 10,
                gap_y - band + 1,
                1.0
            ),
            Some(Drop::Insert { column: 0, row: 1 })
        );
        assert_eq!(
            drop_at(
                &four,
                &order,
                &rects,
                tab(0),
                s[0][0].x + 10,
                gap_y - band - 1,
                1.0
            ),
            None
        );
        let last = s[0][1];
        assert_eq!(
            drop_at(
                &four,
                &order,
                &rects,
                tab(0),
                last.x + 10,
                last.y + last.h + 2,
                1.0
            ),
            Some(Drop::Insert { column: 0, row: 2 })
        );
        let lower = s[0][1];
        assert_eq!(
            drop_at(
                &four,
                &order,
                &rects,
                tab(0),
                lower.x + 10,
                lower.y + lower.h / 2,
                1.0
            ),
            Some(Drop::Swap(tab(2)))
        );
        let gutter = s[0][0].x + s[0][0].w + gap / 2;
        assert_eq!(
            drop_at(&four, &order, &rects, tab(0), gutter, 300, 1.0),
            None
        );
    }

    #[test]
    fn the_drop_line_spans_the_gap() {
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&four);
        let order = order_of(&four);
        let s = slots_of(&four, &order, &rects);
        let line = drop_line(&s, Drop::Insert { column: 0, row: 1 }, 1.0).unwrap();
        assert_eq!(line.x, s[0][0].x);
        assert_eq!(line.w, s[0][0].w);
        assert_eq!(line.h, scaled(LINE_THIN, 1.0));
        assert!(line.y + line.h / 2 < s[0][1].y);
        assert!(line.y > s[0][0].y + s[0][0].h - scaled(GAP_L, 1.0));
        assert_eq!(drop_line(&s, Drop::Swap(tab(1)), 1.0), None);
    }

    #[test]
    fn dragging_down_grows_the_tile_above_and_moves_the_rest_down() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
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
        let after = slots(&columns, &[Heights::Hand(pushed.stored), Heights::Default]);
        assert_eq!(after[0][0].h, above + 200);
        assert_eq!(after[0][1].h, below);
        assert_eq!(after[0][1].y, before[0][1].y + 200);
    }

    #[test]
    fn dragging_up_shrinks_to_the_minimum_and_moves_the_rest_up() {
        let columns = [tabs(&[0, 1, 2]), tabs(&[3])];
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
        assert_eq!(pushed.stored[0], min_slot_height(1.0));
        assert_eq!(&pushed.stored[1..], &heights[1..]);
        let filled = f.inner_h - (pushed.stored[0] + heights[1]) - 2 * f.gutter;
        assert_eq!(pushed.shown[2], filled.max(heights[2]));
        let after = slots(&columns, &[Heights::Hand(pushed.stored), Heights::Default]);
        let shrink = heights[0] - min_slot_height(1.0);
        assert_eq!(after[0][1].y, before[0][1].y - shrink);
        assert_eq!(after[0][1].h, before[0][1].h);
        assert_eq!(after[0][2].y + after[0][2].h, f.inner_h);
    }

    #[test]
    fn the_last_tile_fills_a_column_shorter_than_the_view() {
        let columns = [tabs(&[0, 1]), tabs(&[2])];
        let heights = [Heights::Hand(vec![300, 300]), Heights::Default];
        let s = slots(&columns, &heights);
        let f = frame(VIEW.0, VIEW.1, 1.0);
        assert_eq!(s[0][0].h, 300);
        assert_eq!(s[0][1].h, f.inner_h - f.gutter - 300);
        assert_eq!(s[0][1].y + s[0][1].h, f.inner_h);
        assert_eq!(
            fill_last(&[300, 300], f.inner_h, f.gutter),
            vec![300, f.inner_h - f.gutter - 300]
        );
        assert_eq!(fill_last(&[900, 900], f.inner_h, f.gutter), vec![900, 900]);
    }

    #[test]
    fn hand_heights_keep_their_design_px_through_resizes_and_scale_changes() {
        let columns = [tabs(&[0, 1, 2]), tabs(&[3])];
        let heights = [Heights::Hand(vec![500, 300, 200]), Heights::Default];
        let at = |view_h: i32, scale: f64| layout(&columns, &heights, 500, 1200, view_h, scale);
        let short = at(800, 1.0);
        let tall = at(1200, 1.0);
        for s in [&short, &tall] {
            assert_eq!((s[0][0].h, s[0][1].h), (500, 300));
        }
        let half = at(800, 0.5);
        assert_eq!((half[0][0].h, half[0][1].h), (250, 150));
        let f = frame(1200, 800, 0.5);
        assert_eq!(half[0][2].h, f.inner_h - 2 * f.gutter - 250 - 150);
    }

    #[test]
    fn a_reset_returns_to_the_equal_default() {
        let columns = [tabs(&[0, 1]), tabs(&[2, 3])];
        let hand = [Heights::Hand(vec![700, 200]), Heights::Default];
        let dragged = layout(&columns, &hand, 500, VIEW.0, VIEW.1, 1.0);
        assert_ne!(dragged[0][0].h, dragged[0][1].h);
        let reset = layout(&columns, &defaults(), 500, VIEW.0, VIEW.1, 1.0);
        assert_eq!(reset[0][0].h, reset[0][1].h);
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let swapped = columns_after_swap(&four, tab(0), tab(1)).unwrap();
        assert_eq!(heights_after_change(&four, &hand, &swapped)[0], hand[0]);
        let moved = columns_after_insert(&four, tab(0), 1, 2).unwrap();
        let after = heights_after_change(&four, &hand, &moved);
        assert_eq!(after[0], Heights::Default);
        assert_eq!(after[1], Heights::Default);
    }

    #[test]
    fn a_column_that_grows_past_the_view_stops_being_pinned() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
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
        let ids = [0, 1, 2, 3, 4, 5];
        let mut cases = vec![];
        for n in 2..=6 {
            cases.push(initial_columns(&tabs(&ids[..n])));
        }
        for n in 3..=6 {
            let columns = initial_columns(&tabs(&ids[..n]));
            for from in 0..n {
                for to in 0..n {
                    if let Some(next) =
                        columns_after_swap(&columns, tab(from as u64), tab(to as u64))
                    {
                        cases.push(next);
                    }
                }
                for column in 0..2 {
                    for row in 0..=columns[column].len() {
                        if let Some(next) =
                            columns_after_insert(&columns, tab(from as u64), column, row)
                        {
                            cases.push(next);
                        }
                    }
                }
            }
            for tile in 0..n {
                cases.push(columns_without(&columns, tab(tile as u64)));
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
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let s = slots_of(&columns, &order, &rects);
        assert!(s[0][0].contains(100, 100));
        assert!(s[1][0].contains(900, 100));
        assert!(s[0][1].contains(100, 900));
        assert!(s[1][1].contains(900, 900));
        let gutter = s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0) / 2;
        assert!(!s[0][0].contains(gutter, 100));
        assert!(!s[1][0].contains(gutter, 100));
        assert_eq!(
            drop_at(&columns, &order, &rects, tab(0), gutter, 100, 1.0),
            None
        );
        assert_eq!(
            drop_at(&columns, &order, &rects, tab(0), -5, 100, 1.0),
            None
        );
    }

    #[test]
    fn which_tile_a_click_focuses() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let left = rects[order.iter().position(|id| *id == tab(0)).unwrap()];
        let right = rects[order.iter().position(|id| *id == tab(1)).unwrap()];
        assert_eq!(
            tile_at(&order, &rects, left.x + 10, left.y + left.h / 2),
            Some(tab(0))
        );
        assert_eq!(
            tile_at(&order, &rects, right.x + 10, right.y + right.h / 2),
            Some(tab(1))
        );
        let gutter = left.x + left.w + scaled(GAP_L, 1.0) / 2;
        assert_eq!(tile_at(&order, &rects, gutter, left.y + 10), None);
    }

    #[test]
    fn border_hit_test_covers_the_whole_strip_except_the_pill() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let s = slots_of(&columns, &order, &rects);
        let gap = scaled(GAP_L, 1.0);
        let x = s[0][0].x + s[0][0].w + gap / 2;
        assert_eq!(border_at(&s, x, 40, 0, VIEW.1, 1.0), Some(Border::Columns));
        let edge = s[0][0].x + 5;
        let top = s[0][0].y + s[0][0].h;
        let bottom = s[0][1].y + scaled(CONTROL_H, 1.0) + scaled(GAP_S, 1.0);
        for y in top..bottom {
            assert_eq!(
                border_at(&s, edge, y, 0, VIEW.1, 1.0),
                Some(Border::Heights { column: 0, row: 1 }),
                "y {y} in the strip"
            );
        }
        assert_eq!(border_at(&s, edge, bottom, 0, VIEW.1, 1.0), None);
        let lower = s[0][1];
        let pw = scaled(PILL_W, 1.0).min(lower.w);
        let pill_x = lower.x + (lower.w - pw) / 2 + pw / 2;
        let pill_y = lower.y + scaled(CONTROL_H, 1.0) / 2;
        assert_eq!(border_at(&s, pill_x, pill_y, 0, VIEW.1, 1.0), None);
        assert_eq!(
            border_at(&s, pill_x, top + 1, 0, VIEW.1, 1.0),
            Some(Border::Heights { column: 0, row: 1 })
        );
        assert_eq!(border_at(&s, edge, 100, 0, VIEW.1, 1.0), None);
        let top = s[1][0].y + s[1][0].h;
        let bottom = s[1][1].y + scaled(CONTROL_H, 1.0) + scaled(GAP_S, 1.0);
        for y in top..bottom {
            assert_eq!(
                border_at(&s, s[1][0].x + 5, y, 0, VIEW.1, 1.0),
                Some(Border::Heights { column: 1, row: 1 }),
                "right y {y}"
            );
        }
    }

    #[test]
    fn a_short_column_is_pinned_and_a_tall_one_scrolls() {
        let four_one = [tabs(&[0, 1, 2, 3]), tabs(&[4])];
        let s = layout(&four_one, &defaults(), 500, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 90), [0, 90]);
        assert!(!column_fits(&s[0], VIEW.1, 1.0));
        assert!(column_fits(&s[1], VIEW.1, 1.0));
        let three_three = initial_columns(&tabs(&[0, 1, 2, 3, 4, 5]));
        let s = layout(&three_three, &defaults(), 500, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 90), [0, 0]);
        let two_one = [tabs(&[0, 1]), tabs(&[2])];
        let s = layout(&two_one, &defaults(), 500, VIEW.0, VIEW.1, 1.0);
        assert_eq!(column_offsets(&s, VIEW.1, 1.0, 0), [0, 0]);
        let four = initial_columns(&tabs(&[0, 1, 2, 3]));
        let tall = layout(&four, &defaults(), 500, VIEW.0, 1400, 1.0);
        let short = layout(&four, &defaults(), 500, VIEW.0, 100, 1.0);
        assert!(column_fits(&tall[0], 1400, 1.0));
        assert!(!column_fits(&short[0], 100, 1.0));
        assert_eq!(column_offsets(&short, 100, 1.0, 30), [0, 0]);
    }

    #[test]
    fn a_hand_drag_reaches_the_minimum_below_the_half_floor() {
        let four_one = [tabs(&[0, 1, 2, 3]), tabs(&[4])];
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
        let hand = slots(&four_one, &[Heights::Hand(pushed.stored), Heights::Default]);
        assert_eq!(hand[0][0].h, min);
        assert_eq!(hand[0][1].h, half);
    }

    #[test]
    fn a_stuck_tiles_strip_follows_its_frame_top() {
        let s = [
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
                border_at(&s, 45, y, 900, VIEW.1, 1.0),
                Some(Border::Heights { column: 0, row: 1 }),
                "y {y} in the stuck strip"
            );
        }
        assert_eq!(border_at(&s, 45, 958, 900, VIEW.1, 1.0), None);
        assert_eq!(border_at(&s, 200, 925, 900, VIEW.1, 1.0), None);
        assert_eq!(
            border_at(&s, 45, 955, 900, VIEW.1, 1.0),
            Some(Border::Heights { column: 0, row: 1 })
        );
    }

    #[test]
    fn pill_hit_test() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let r = rects[0];
        let px = r.x + (r.w - scaled(PILL_W, 1.0)) / 2;
        let ph = scaled(CONTROL_H, 1.0);
        assert_eq!(
            pill_at(&order, &rects, px + 10, r.y + 10, 0, VIEW.1, 1.0),
            Some(tab(0))
        );
        assert_eq!(
            pill_at(&order, &rects, r.x + 10, r.y + 10, 0, VIEW.1, 1.0),
            None
        );
        assert_eq!(
            pill_at(&order, &rects, px + 10, r.y + ph, 0, VIEW.1, 1.0),
            None
        );
        assert!(!press_on_pill(10.0, 10.0, 500, 0, 1.0));
        assert!(press_on_pill(240.0, 10.0, 500, 0, 1.0));
    }

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
    fn a_pills_wrap_sits_with_its_tile_when_not_stuck() {
        let (slot, _) = sticky_case();
        let p = pill_layout(slot, 0, VIEW.1, 1.0);
        assert_eq!(
            p.wrap,
            Rect {
                x: 40,
                y: 100,
                w: 300,
                h: 58
            }
        );
        assert_eq!(p.frame_top, 58);
        assert_eq!(p.content_offset, 0);
        let after = pill_layout(slot, 40, VIEW.1, 1.0);
        assert_eq!(after, p);
        assert_eq!(after.wrap.y - 40, 60);
    }

    #[test]
    fn a_stuck_pills_wrap_holds_the_view_top_with_its_frame() {
        let (slot, _) = sticky_case();
        let p = pill_layout(slot, 200, VIEW.1, 1.0);
        assert_eq!(
            p.wrap,
            Rect {
                x: 40,
                y: 200,
                w: 300,
                h: 58
            }
        );
        assert_eq!(p.wrap.y - 200, 0);
        assert_eq!(p.frame_top, 158);
        assert_eq!(p.content_offset, 100);
        assert_eq!(pill_content_rect(slot, 200, VIEW.1, 1.0).y, p.wrap.y);
    }

    #[test]
    fn a_dragged_tile_keeps_its_pill_at_its_own_top() {
        let (slot, _) = sticky_case();
        let p = dragged_pill_layout(slot, 1.0);
        assert_eq!(
            p.wrap,
            Rect {
                x: 40,
                y: 100,
                w: 300,
                h: 58
            }
        );
        assert_eq!(p.frame_top, 58);
        assert_eq!(p.content_offset, 0);
        assert_eq!(pill_layout(slot, 200, VIEW.1, 1.0).wrap.y, 200);
    }

    #[test]
    fn a_visible_slot_leaves_its_pill_at_the_slot_top() {
        let (slot, frame) = sticky_case();
        let g = stuck_at(0);
        assert_eq!(g.pill_y, 100);
        assert_eq!(g.frame, frame);
        assert_eq!(g.content_offset, 0);
        let g = stuck_at(40);
        assert_eq!(g.pill_y, 100);
        assert_eq!(g.pill_y - 40, 60);
        assert_eq!(g.frame, frame);
        assert_eq!(pill_content_rect(slot, 40, VIEW.1, 1.0).y, 100);
    }

    #[test]
    fn a_slot_top_above_the_view_sticks_the_pill_and_the_frame_top() {
        let g = stuck_at(200);
        assert_eq!(g.pill_y, 200);
        assert_eq!(g.pill_y - 200, 0);
        assert_eq!(g.frame.y, 200 + 58);
        assert_eq!(g.frame.y + g.frame.h, 800);
        assert_eq!(g.frame.y + g.frame.h - 200, 600);
        assert_eq!(g.frame.h, 542);
        assert_eq!(g.content_offset, 100);
        assert_eq!(pill_content_rect(sticky_case().0, 200, VIEW.1, 1.0).y, 200);
        let g = stuck_at(500);
        assert_eq!((g.pill_y, g.frame.y), (500, 558));
        assert_eq!(g.frame.h, 242);
        assert_eq!(g.content_offset, 400);
    }

    #[test]
    fn the_frame_shrinks_to_one_grid_row_at_the_minimum() {
        let g = stuck_at(598);
        assert_eq!(g.pill_y - 598, 0);
        assert_eq!(g.frame.y - 598, 58);
        assert_eq!(g.frame.h, min_frame_height(1.0));
        assert_eq!(g.frame.h, 144);
        assert_eq!(g.content_offset, 598 - 100);
    }

    #[test]
    fn below_the_minimum_the_whole_tile_scrolls_off() {
        let at = stuck_at(598);
        let past = stuck_at(650);
        assert_eq!(past.pill_y, at.pill_y);
        assert_eq!(past.frame, at.frame);
        assert_eq!(past.content_offset, at.content_offset);
        assert_eq!(past.pill_y - 650, -52);
        assert_eq!(past.frame.h, 144);
        let gone = stuck_at(800);
        assert_eq!(gone.frame.y + gone.frame.h - 800, 0);
        assert_eq!(gone.pill_y + scaled(CONTROL_H, 1.0) - 800, -152);
    }

    #[test]
    fn a_pinned_column_never_moves_its_pills() {
        let columns = [tabs(&[0, 1, 2, 3]), tabs(&[4])];
        let mut slots = layout(&columns, &defaults(), 500, VIEW.0, VIEW.1, 1.0);
        offset_slots(&mut slots, VIEW.1, 1.0, 300);
        let pinned = slots[1][0];
        let g = stuck_geometry(
            pinned,
            frame_rect(pinned, 1.0),
            300,
            VIEW.1,
            scaled(CONTROL_H, 1.0),
            min_frame_height(1.0),
        );
        assert_eq!(g.pill_y, pinned.y);
        assert_eq!(g.content_offset, 0);
        assert_eq!(pill_content_rect(slots[0][0], 300, VIEW.1, 1.0).y, 300);
    }

    #[test]
    fn drag_follows_the_pointer_and_previews_the_drop() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let state = state(columns.clone());
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let s = slots_of(&columns, &order, &rects);
        let other = s[1][0];
        let view = Viewport {
            w: VIEW.0,
            h: VIEW.1,
            scale: 1.0,
        };
        let index = order.iter().position(|id| *id == tab(0)).unwrap();
        let board = Board {
            layout: &state,
            view,
            scroll: 0,
        };
        let (drop, preview) = drag_layout(
            &board,
            &order,
            &rects,
            index,
            other.x + 10,
            other.y + 10,
            (0, 0),
        );
        assert_eq!(drop, Some(Drop::Swap(tab(1))));
        assert_eq!(
            preview[index],
            Rect {
                x: other.x + 10,
                y: other.y + 10,
                ..rects[index]
            }
        );
        let next = columns_after_swap(&columns, tab(0), tab(1)).unwrap();
        let final_rects = rects_for(&next);
        for id in [tab(1), tab(2), tab(3)] {
            let i = order.iter().position(|candidate| *candidate == id).unwrap();
            assert_eq!(preview[i], final_rects[id.raw() as usize], "tile {id}");
        }
        let gutter = s[0][0].x + s[0][0].w + scaled(GAP_L, 1.0) / 2;
        let (drop, preview) = drag_layout(&board, &order, &rects, index, gutter, 40, (0, 0));
        assert_eq!(drop, None);
        for index in 0..rects.len() {
            if order[index] != tab(0) {
                assert_eq!(preview[index], rects[index]);
            }
        }
    }

    #[test]
    fn a_drag_respects_the_grab_offset() {
        let columns = initial_columns(&tabs(&[0, 1, 2, 3]));
        let state = state(columns.clone());
        let rects = rects_for(&columns);
        let order = order_of(&columns);
        let view = Viewport {
            w: VIEW.0,
            h: VIEW.1,
            scale: 1.0,
        };
        let index = order.iter().position(|id| *id == tab(0)).unwrap();
        let board = Board {
            layout: &state,
            view,
            scroll: 0,
        };
        let (_, preview) = drag_layout(&board, &order, &rects, index, 900, 700, (30, 20));
        assert_eq!(
            preview[index],
            Rect {
                x: 870,
                y: 680,
                ..rects[index]
            }
        );
    }

    #[test]
    fn the_model_layout_converts_to_tile_rects_at_scale() {
        let layout = SplitLayout {
            columns: [tabs(&[0, 2]), tabs(&[1, 3])],
            heights: [Heights::Hand(vec![800, 600]), Heights::Default],
            width_permille: 620,
        };
        let state = LayoutState::from_layout(&layout);
        let view = Viewport {
            w: 1600,
            h: 1000,
            scale: 0.75,
        };
        let mut slots = layout_grid(&state.columns, &state.heights, state.width_permille, view);
        offset_slots(&mut slots, view.h, view.scale, 0);
        let order = order_of(&state.columns);
        let rects = rects_by_tile(&order, &state.columns, &slots);
        let f = frame(view.w, view.h, view.scale);
        let (left, _right) = column_widths(&f, 0.62, [2, 2], view.scale);
        assert_eq!(rects[0].x, f.inset);
        assert_eq!(rects[1].x, f.inset + left + f.gutter);
        assert_eq!(rects[0].w, left);
        assert_eq!(rects[0].h, scaled(800.0, 0.75));
        assert_eq!(
            rects[2].h,
            scaled(600.0, 0.75).max(scaled(CONTROL_H + GAP_S, 0.75) + 1)
        );
        assert_eq!(rects[1].h, rects[3].h);
    }

    #[test]
    fn the_state_keeps_the_model_permille() {
        let layout = SplitLayout {
            columns: [tabs(&[0]), tabs(&[1])],
            heights: defaults(),
            width_permille: 700,
        };
        let state = LayoutState::from_layout(&layout);
        assert_eq!(state.width_permille, 700);
        assert_eq!(state.columns[0].len(), 1);
    }
}
