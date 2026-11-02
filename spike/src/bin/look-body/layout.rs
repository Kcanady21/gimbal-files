// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure body layout (correction 2): how the window's width splits into the
//! left sidebar, the content column and the preview sidebar, and the minimum
//! window width that still shows one grid column. Both Paneds are driven
//! from this function; the content column itself just takes what is left.
//! Since Nov 1 the side panels are inset from the window's outer and bottom
//! edges by `gap-s` (the sketch has no window frame), so the widths returned
//! here are visible panel widths and every position helper adds the inset.

use crate::style::{
    CELL_W, GAP_2XL, GAP_L, GAP_M, GAP_S, GRID_MARGIN, ICON_ROW, LINE_MID, PREVIEW_MIN,
    SIDEBAR_MIN, scaled,
};

/// The 1 px separator Adwaita draws between a paned's children
/// (libadwaita-1.9.1:src/stylesheet/widgets/_paned.scss:3–5); it consumes
/// allocation (gtk-4.22.4:gtk/gtkpaned.c:1355–1376: the end child starts at
/// start + handle), and `position` lives in a (width − handle) space
/// (gtkpaned.c:1362: calc_position gets MAX(1, width − handle_size)).
pub const HANDLE: i32 = 1;

/// A `gap-s` inset at one scale: the side panels' window-side and bottom
/// margin, and the space `body_widths` reserves on each side of the window.
pub fn inset(scale: f64) -> i32 {
    scaled(GAP_S, scale)
}

/// The outer paned's position for a left panel `left` px wide (its allocation
/// includes the inset as its start margin).
pub fn left_position(left: i32, scale: f64) -> i32 {
    inset(scale) + left
}

/// The inner paned's position for a preview `preview` px wide (its allocation
/// includes the inset as its end margin).
pub fn preview_position(inner_w: i32, preview: i32, scale: f64) -> i32 {
    inner_w - HANDLE - inset(scale) - preview
}

/// A visible left-panel width from the outer paned's position (inverse of
/// `left_position`).
pub fn left_from_position(position: i32, scale: f64) -> i32 {
    (position - inset(scale)).max(0)
}

/// A visible preview width from the inner paned's position (inverse of
/// `preview_position`).
pub fn preview_from_position(inner_w: i32, position: i32, scale: f64) -> i32 {
    (inner_w - HANDLE - inset(scale) - position).max(0)
}

/// The collapsed sidebar's strip width. The strip carries the same
/// `icon-row` button (with its `gap-s` padding) as the panel title row, plus
/// the panel's `gap-m` padding and `line-mid` border on its inner and top
/// edges. Those paddings put the expand button at exactly the window position
/// the collapse button had (P1m item 7). The strip is therefore wider than
/// `gap-2xl`; that is the button's own minimum.
pub fn strip_width(scale: f64) -> i32 {
    let button = scaled(ICON_ROW, scale) + 2 * scaled(GAP_S, scale);
    scaled(GAP_2XL, scale).max(button) + scaled(GAP_M, scale) + scaled(LINE_MID, scale)
}

/// The sidebar button's horizontal inset from the panel's window-side edge:
/// the panel's `line-mid` border plus its `gap-m` padding. The button is an
/// overlay child of the host (r9), so this is a margin, not a row position.
pub fn panel_button_inset(scale: f64) -> i32 {
    scaled(GAP_M, scale) + scaled(LINE_MID, scale)
}

/// The sidebar button's inset at collapse progress `p` (0 expanded, 1 in the
/// strip). On the left it is a start margin, on the preview an end margin.
/// The strip has no horizontal padding; GTK's box gives the first of the two
/// expanding spacers the odd pixel, so the start gap is the ceiling and the
/// end gap the floor of `(strip − button) / 2` (the P1m timer measured 10 / 9
/// at scale 1 and 5 / 5 at 0.5).
pub fn button_inset(scale: f64, left: bool, progress: f64) -> i32 {
    let button = scaled(ICON_ROW, scale) + 2 * scaled(GAP_S, scale);
    let spare = (strip_width(scale) - button).max(0);
    let strip = if left { (spare + 1) / 2 } else { spare / 2 };
    let panel = panel_button_inset(scale);
    (panel as f64 + (strip - panel) as f64 * progress.clamp(0.0, 1.0)).round() as i32
}

/// The visible sidebar width at `t` (0..=1) on the way from `start` to `end`.
/// Both directions use the same line; a reverse starts again from the width
/// the previous frame ended at.
pub fn motion_width(start: i32, end: i32, t: f64) -> i32 {
    let t = t.clamp(0.0, 1.0);
    (start as f64 + (end - start) as f64 * t).round() as i32
}

/// How far a sidebar is collapsed at a visible width: 0 at `expanded`, 1 at
/// the strip. `expanded` is the panel's own (clipped) width for this
/// animation, so a panel that was already narrowed by the window counts that
/// narrowed width as expanded.
pub fn collapse_progress(width: i32, expanded: i32, scale: f64) -> f64 {
    let strip = strip_width(scale);
    let span = (expanded - strip).max(1) as f64;
    ((expanded - width) as f64 / span).clamp(0.0, 1.0)
}

/// The panel outline's opacity at collapse progress `p`: full until the last
/// 35 % of the slide, then linear to zero at the strip. Expanding is the same
/// curve backwards, so the outline is back before the edge reaches the panel.
pub fn outline_opacity(progress: f64) -> f64 {
    const FADE: f64 = 0.35;
    ((1.0 - progress.clamp(0.0, 1.0)) / FADE).clamp(0.0, 1.0)
}

/// The narrowest content column: the worse of the single view (a
/// `grid-margin` inset each side plus one cell) and the split view (a
/// `gap-2xl` Fixed inset each side, the `gap-l` gutter and two columns of the
/// true tile minimum). Since P1m-r4 a tile may be narrower than a full pill
/// (`min_column_width` is one cell and the pill shrinks), so two real
/// columns — not one — are what the split needs. The window floor follows
/// this number, so at half width the sidebars give way before the two
/// columns can overlap.
pub fn one_column_width(scale: f64) -> i32 {
    let cell = scaled(CELL_W, scale);
    let single = 2 * scaled(GRID_MARGIN, scale) + cell;
    let split = 2 * scaled(GAP_2XL, scale) + scaled(GAP_L, scale) + 2 * cell;
    single.max(split)
}

/// The body's widths at one window width. `left` and `preview` are visible
/// panel widths; `content` is the content column between the two paned
/// handles.
#[derive(Debug, PartialEq, Eq)]
pub struct BodyWidths {
    pub left: i32,
    pub content: i32,
    pub preview: i32,
}

/// Split `window_w` into left | content | preview. The expanded sidebars sit
/// at their saved widths (design px × scale) while the content still has one
/// grid column; below that they give way — left first — down to their
/// minimums, so the content keeps its column. A collapsed sidebar is its
/// `gap-2xl` strip. The window's `gap-s` inset on each side and the two paned
/// handles are subtracted first. Also returns the minimum window width that
/// keeps one column visible, which the caller sets as the window's size
/// request so nothing draws outside the window.
pub fn body_widths(
    window_w: i32,
    scale: f64,
    left_saved: f64,
    right_saved: f64,
    left_collapsed: bool,
    right_collapsed: bool,
) -> (BodyWidths, i32) {
    let edge = inset(scale);
    let strip = strip_width(scale);
    let (lmin, rmin) = (scaled(SIDEBAR_MIN, scale), scaled(PREVIEW_MIN, scale));
    let min_content = one_column_width(scale);
    let avail = (window_w - 2 * edge).max(0);
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
    let mut content = avail - left - 2 * HANDLE - preview; // may go negative
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
    let min_window = 2 * edge
        + if left_collapsed { strip } else { lmin }
        + 2 * HANDLE
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
    use crate::style::GAP_S;

    #[test]
    fn defaults_match_the_sketch() {
        let (w, min) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        assert_eq!((w.left, w.preview), (348, 648));
        assert_eq!(w.content, 2560 - 2 * inset(1.0) - 348 - 2 * HANDLE - 648);
        // Two handles (outer and inner) and a 2 × gap-s inset instead of the
        // old 2 × line-heavy frame: 16 + 2 in place of 20 + 1, so three px
        // more content than the old 1543; the sketch's content is 1544.
        assert_eq!(w.content, 1546);
        assert_eq!(
            min,
            2 * inset(1.0) + scaled(SIDEBAR_MIN, 1.0) + 2 * HANDLE + one_column_width(1.0) + 360
        );
        // The split view needs two true columns now: 2 × gap-2xl view inset +
        // gap-l gutter + 2 × one cell = 298, so it sets the floor (976).
        assert_eq!(min, 976);
    }

    #[test]
    fn one_column_counts_the_two_split_columns() {
        let cell = scaled(CELL_W, 1.0);
        let split = 2 * scaled(GAP_2XL, 1.0) + scaled(GAP_L, 1.0) + 2 * cell;
        assert_eq!(split, 298);
        // The single view's 2 × grid-margin + cell (241) is narrower.
        assert_eq!(one_column_width(1.0), 298);
        assert_eq!(one_column_width(0.5), 150);
        assert!(one_column_width(1.0) > 2 * scaled(GAP_2XL, 1.0) + cell);
    }

    #[test]
    fn insets_and_positions_agree() {
        assert_eq!(inset(1.0), scaled(GAP_S, 1.0));
        assert_eq!(inset(0.5), 4);
        let (w, _) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        // The left panel's allocation starts gap-s in from the window edge.
        let outer = left_position(w.left, 1.0);
        assert_eq!(outer, 356);
        assert_eq!(outer - w.left, inset(1.0));
        // The inner paned starts after the outer handle; its position is the
        // content width, and the preview keeps its own gap-s from the right.
        let inner_w = 2560 - outer - HANDLE;
        let inner = preview_position(inner_w, w.preview, 1.0);
        assert_eq!(inner, w.content);
        let inner_x = outer + HANDLE;
        assert_eq!(2560 - (inner_x + inner + HANDLE + w.preview), inset(1.0));
        // The inverse helpers round-trip a dragged border.
        assert_eq!(left_from_position(500, 1.0), 492);
        assert_eq!(
            preview_from_position(inner_w, 700, 1.0),
            inner_w - 1 - 8 - 700
        );
    }

    #[test]
    fn narrowing_drops_columns_then_sidebars_give_way() {
        // Still room for one column: both sidebars keep their saved widths.
        let (w, _) = body_widths(1400, 1.0, 348.0, 648.0, false, false);
        assert_eq!(
            (w.left, w.content, w.preview),
            (348, 1400 - 2 * inset(1.0) - 348 - 2 * HANDLE - 648, 648)
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
        // One pixel narrower and the content loses its column.
        let (w, _) = body_widths(min - 1, 1.0, 348.0, 648.0, false, false);
        assert!(w.content < one_column_width(1.0));
    }

    #[test]
    fn collapsed_sidebars_are_strips() {
        let (w, min) = body_widths(2560, 1.0, 348.0, 648.0, true, true);
        assert_eq!((w.left, w.preview), (strip_width(1.0), strip_width(1.0)));
        assert_eq!(w.left, 67);
        assert!(w.content > one_column_width(1.0));
        assert_eq!(
            min,
            2 * inset(1.0)
                + strip_width(1.0)
                + 2 * HANDLE
                + one_column_width(1.0)
                + strip_width(1.0)
        );
        assert_eq!(min, 450);
    }

    #[test]
    fn the_strip_centres_the_expand_button() {
        for scale in [1.0, 0.5] {
            let button = scaled(ICON_ROW, scale) + 2 * scaled(GAP_S, scale);
            let strip = strip_width(scale);
            // No horizontal padding: the 48 px button centres in the 67 px
            // strip (34 px at scale 0.5 in a 34 px strip).
            assert!(button <= strip);
            let left = (strip - button) / 2;
            assert!((left * 2 + button - strip).abs() <= 1);
            // The vertical position is unchanged: panel border + padding
            // equals the strip's top padding.
            let pad = scaled(GAP_M, scale) + scaled(LINE_MID, scale);
            assert_eq!(scaled(LINE_MID, scale) + scaled(GAP_M, scale), pad);
        }
    }

    #[test]
    fn sidebar_motion_starts_mid_and_ends_on_both_sides() {
        let (w, _) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        let strip = strip_width(1.0);
        // Collapse, left and preview: same width arithmetic.
        for expanded in [w.left, w.preview] {
            assert_eq!(motion_width(expanded, strip, 0.0), expanded);
            assert_eq!(motion_width(expanded, strip, 1.0), strip);
            let mid = motion_width(expanded, strip, 0.5);
            assert!(mid < expanded && mid > strip);
            // The mid frame still places the paned edge where the width says.
            let inner_w = 2000;
            assert_eq!(
                left_from_position(left_position(mid, 1.0), 1.0),
                mid,
                "left edge at the mid width"
            );
            assert_eq!(
                preview_from_position(inner_w, preview_position(inner_w, mid, 1.0), 1.0),
                mid,
                "preview edge at the mid width"
            );
        }
        // Expand is the same line from the strip back out.
        for expanded in [w.left, w.preview] {
            assert_eq!(motion_width(strip, expanded, 0.0), strip);
            assert_eq!(
                motion_width(strip, expanded, 0.5),
                motion_width(expanded, strip, 0.5)
            );
            assert_eq!(motion_width(strip, expanded, 1.0), expanded);
        }
    }

    #[test]
    fn a_reverse_restarts_from_the_current_width() {
        let (w, _) = body_widths(2560, 1.0, 348.0, 648.0, false, false);
        let strip = strip_width(1.0);
        let mid = motion_width(w.left, strip, 0.5);
        assert_eq!(mid, 208);
        // Clicking at 50 % expands from 208, not from either end.
        assert_eq!(motion_width(mid, w.left, 0.0), mid);
        assert_eq!(motion_width(mid, w.left, 0.5), 278);
        assert_eq!(motion_width(mid, w.left, 1.0), w.left);
        // The same for a collapse interrupted while expanding.
        assert_eq!(motion_width(mid, strip, 0.0), mid);
        assert_eq!(motion_width(mid, strip, 0.5), 138);
        assert_eq!(motion_width(mid, strip, 1.0), strip);
    }

    #[test]
    fn a_retarget_uses_the_new_end_width() {
        let strip = strip_width(1.0);
        let mid = motion_width(strip, 348, 0.5);
        assert_eq!(mid, 208);
        // A window resize or scale change gives the expand a wider target;
        // the retarget starts from the current 208 and reaches 500.
        assert_eq!(motion_width(mid, 500, 0.0), mid);
        assert_eq!(motion_width(mid, 500, 0.5), 354);
        assert_eq!(motion_width(mid, 500, 1.0), 500);
        // A narrower target retargets the same way.
        assert_eq!(motion_width(mid, 320, 1.0), 320);
    }

    #[test]
    fn the_button_glides_from_the_panel_to_the_strip_centre() {
        // Scale 1: panel inset 19; strip gaps 10 (start) and 9 (end).
        assert_eq!(panel_button_inset(1.0), 19);
        assert_eq!(button_inset(1.0, true, 0.0), 19);
        assert_eq!(button_inset(1.0, true, 0.5), 15);
        assert_eq!(button_inset(1.0, true, 1.0), 10);
        assert_eq!(button_inset(1.0, false, 0.0), 19);
        assert_eq!(button_inset(1.0, false, 0.5), 14);
        assert_eq!(button_inset(1.0, false, 1.0), 9);
        // Scale 0.5: panel inset 10, both strip gaps 5 (34 − 24 = 10 spare).
        assert_eq!(panel_button_inset(0.5), 10);
        assert_eq!(button_inset(0.5, true, 0.0), 10);
        assert_eq!(button_inset(0.5, true, 0.5), 8);
        assert_eq!(button_inset(0.5, true, 1.0), 5);
        assert_eq!(button_inset(0.5, false, 1.0), 5);
        // The button's vertical inset is the panel's border + padding.
        assert_eq!(
            panel_button_inset(1.0),
            scaled(LINE_MID, 1.0) + scaled(GAP_M, 1.0)
        );
    }

    #[test]
    fn the_outline_fades_over_the_last_part_only() {
        assert_eq!(outline_opacity(0.0), 1.0);
        assert_eq!(outline_opacity(0.5), 1.0);
        assert_eq!(outline_opacity(0.65), 1.0);
        assert!((outline_opacity(0.8) - 0.571).abs() < 0.001);
        assert_eq!(outline_opacity(1.0), 0.0);
        // Expanding is the same curve backwards: as progress falls the
        // outline comes back.
        assert!(outline_opacity(0.9) < outline_opacity(0.8));
        assert!(outline_opacity(0.8) < outline_opacity(0.7));
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
            2 * inset(0.5) + scaled(SIDEBAR_MIN, 0.5) + 2 * HANDLE + one_column_width(0.5) + 180
        );
    }
}
