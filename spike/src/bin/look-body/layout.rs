// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure body layout (correction 2): how the window's width splits into the
//! left sidebar, the content column and the preview sidebar, and the minimum
//! window width that still shows one grid column. Both Paneds are driven
//! from this function; the content column itself just takes what is left.

use crate::style::{
    CELL_W, GAP_2XL, GRID_MARGIN, LINE_HEAVY, LINE_THIN, PREVIEW_MIN, SIDEBAR_MIN, scaled,
};

/// The 1 px separator Adwaita draws between a paned's children
/// (libadwaita-1.9.1:src/stylesheet/widgets/_paned.scss:3–5); it consumes
/// allocation (gtk-4.22.4:gtk/gtkpaned.c:1355–1376: the end child starts at
/// start + handle), and `position` lives in a (width − handle) space
/// (gtkpaned.c:1362: calc_position gets MAX(1, width − handle_size)).
pub const HANDLE: i32 = 1;

/// The narrowest content column that still shows one grid column: the worse
/// of the single view (a `grid-margin` inset each side plus one cell) and
/// the split view (a `gap-2xl` Fixed inset each side, the tile's
/// `line-thin` border, a `gap-2xl` tile-grid margin each side, one cell).
pub fn one_column_width(scale: f64) -> i32 {
    let cell = scaled(CELL_W, scale);
    let single = 2 * scaled(GRID_MARGIN, scale) + cell;
    let split = 4 * scaled(GAP_2XL, scale) + 2 * scaled(LINE_THIN, scale) + cell;
    single.max(split)
}

/// The body's widths at one window width.
#[derive(Debug, PartialEq, Eq)]
pub struct BodyWidths {
    pub left: i32,
    pub content: i32,
    pub preview: i32,
}

/// Split `window_w` (minus the `gl-frame` border) into left | content |
/// preview. The expanded sidebars sit at their saved widths (design px ×
/// scale) while the content still has one grid column; below that they give
/// way — left first — down to their minimums, so the content keeps its
/// column. A collapsed sidebar is its `gap-2xl` strip. Also returns the
/// minimum window width that keeps one column visible, which the caller
/// sets as the window's size request so nothing draws outside the window.
pub fn body_widths(
    window_w: i32,
    scale: f64,
    left_saved: f64,
    right_saved: f64,
    left_collapsed: bool,
    right_collapsed: bool,
) -> (BodyWidths, i32) {
    let frame = 2 * scaled(LINE_HEAVY, scale);
    let strip = scaled(GAP_2XL, scale);
    let (lmin, rmin) = (scaled(SIDEBAR_MIN, scale), scaled(PREVIEW_MIN, scale));
    let min_content = one_column_width(scale);
    let avail = (window_w - frame).max(0);
    let mut left = if left_collapsed {
        strip
    } else {
        scaled(left_saved, scale).max(lmin)
    };
    let mut preview = if right_collapsed {
        strip
    } else {
        scaled(right_saved, scale).max(rmin)
    };
    let mut content = avail - left - HANDLE - preview; // may go negative
    // window_w == 0 means "not yet allocated" (a pre-allocation call): the
    // saved widths must be applied as they are, no give-way yet.
    if window_w > 0 && content < min_content && !left_collapsed {
        let give = (left - lmin).min(min_content - content);
        left -= give;
        content += give;
    }
    if window_w > 0 && content < min_content && !right_collapsed {
        let give = (preview - rmin).min(min_content - content);
        preview -= give;
        content += give;
    }
    let content = content.max(0);
    let min_window = frame
        + if left_collapsed { strip } else { lmin }
        + HANDLE
        + min_content
        + if right_collapsed { strip } else { rmin };
    (
        BodyWidths {
            left,
            content,
            preview,
        },
        min_window,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_sketch() {
        let (w, min) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        assert_eq!((w.left, w.preview), (348, 648));
        assert_eq!(w.content, 2560 - 20 - 348 - HANDLE - 648);
        // The sketch's 1544 px content minus the paned handle.
        assert_eq!(w.content, 1543);
        assert_eq!(
            min,
            20 + scaled(SIDEBAR_MIN, 1.0) + HANDLE + one_column_width(1.0) + 360
        );
    }

    #[test]
    fn narrowing_drops_columns_then_sidebars_give_way() {
        // Still room for one column: both sidebars keep their saved widths.
        let (w, _) = body_widths(1400, 1.0, 348.0, 648.0, false, false);
        assert_eq!(
            (w.left, w.content, w.preview),
            (348, 1400 - 20 - 348 - HANDLE - 648, 648)
        );
        // Midway: the left gives way first, the preview part-way, and the
        // content is pinned at exactly one column.
        let (w, _) = body_widths(1000, 1.0, 348.0, 648.0, false, false);
        assert_eq!(w.left, scaled(SIDEBAR_MIN, 1.0));
        assert!(w.preview > 360 && w.preview < 648);
        assert_eq!(w.content, one_column_width(1.0));
        // At the implied minimum: sidebars at their minimums, one column.
        let (_, min) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        let (w, _) = body_widths(min, 1.0, 348.0, 648.0, false, false);
        assert_eq!(
            (w.left, w.content, w.preview),
            (scaled(SIDEBAR_MIN, 1.0), one_column_width(1.0), 360)
        );
    }

    #[test]
    fn collapsed_sidebars_are_strips() {
        let (w, min) = body_widths(2560, 1.0, 348.0, 648.0, true, true);
        assert_eq!((w.left, w.preview), (40, 40));
        assert!(w.content > one_column_width(1.0));
        assert_eq!(min, 20 + 40 + HANDLE + one_column_width(1.0) + 40);
    }

    #[test]
    fn giving_way_never_touches_the_saved_widths() {
        // A narrowed window shrinks the returned widths only; the caller
        // keeps the saved widths and re-applies them once it widens again.
        let (narrow, _) = body_widths(950, 1.0, 348.0, 648.0, false, false);
        assert!(narrow.left < 348 && narrow.preview < 648);
        let (wide, _) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        assert_eq!((wide.left, wide.preview), (348, 648));
        // Scale 0.5 halves every width, minimums included.
        let (w, min) = body_widths(2560, 0.5, 348.0, 648.0, false, false);
        assert_eq!((w.left, w.preview), (174, 324));
        assert_eq!(
            min,
            10 + scaled(SIDEBAR_MIN, 0.5) + HANDLE + one_column_width(0.5) + 180
        );
    }
}
