// SPDX-License-Identifier: GPL-3.0-or-later
//! The one design-token table and the one colour-role table.
//!
//! These are the merged tables from `spike/src/bin/look-body/style.rs` and
//! `spike/src/bin/look-header/style.rs`. A name both tables share has the same
//! value here; `tokens_lost_nothing_in_the_merge` checks that.
//!
//! The table is the whole spec: later briefs consume the tokens P2g's frame
//! does not use yet, so keep every one.
#![allow(dead_code)]

// Spacing and common control sizes.
pub const GAP_S: f64 = 8.0;
pub const GAP_M: f64 = 16.0;
pub const GAP_L: f64 = 24.0;
pub const GAP_XL: f64 = 32.0;
pub const GAP_2XL: f64 = 40.0;
pub const CONTROL_H: f64 = 50.0;
pub const HEADER_H: f64 = 96.0;
pub const HEADER_INSET: f64 = 24.0;

// Lines and corners.
pub const LINE_HAIR: f64 = 1.0;
pub const LINE_THIN: f64 = 2.0;
pub const LINE_MID: f64 = 3.0;
/// Unused since Nov 1: the window frame was removed.
pub const LINE_HEAVY: f64 = 10.0;
pub const RADIUS: f64 = 12.0;
pub const RADIUS_PILL: f64 = 999.0;
pub const RADIUS_ICON: f64 = 4.0;

// The body's rows, panels and grid cells.
pub const GRID_MARGIN: f64 = 72.0;
pub const SECTION_H: f64 = 32.0;
pub const SECTION_GAP: f64 = 6.0;
pub const PIN_ROW_H: f64 = 44.0;
pub const DIR_ROW_H: f64 = 32.0;
pub const STATUS_H: f64 = 64.0;
pub const BAR_W: f64 = 209.0;
pub const BAR_H: f64 = 18.0;
pub const MAIN_BAR_W: f64 = 228.0;
pub const MAIN_BAR_H: f64 = 20.0;
pub const CELL_W: f64 = 97.0;
pub const CELL_H: f64 = 144.0;
pub const SIDEBAR_W: f64 = 348.0;
pub const SIDEBAR_MIN: f64 = 300.0;
pub const PREVIEW_W: f64 = 648.0;
pub const PREVIEW_MIN: f64 = 360.0;
pub const PREVIEW_GUTTER: f64 = 64.0;

// The header's controls.
pub const PILL_W: f64 = 270.0;
pub const SEARCH_W: f64 = 496.0;
pub const SEARCH_HINT_W: f64 = 128.0;
pub const PATH_MIN: f64 = 320.0;
pub const TRASH: f64 = 48.0;
pub const DEFAULT_W: i32 = 2560;
pub const DEFAULT_H: i32 = 1440;

// Type.
pub const TEXT_TITLE: f64 = 32.0;
pub const TEXT_ITEM: f64 = 22.0;
pub const TEXT_CONTROL: f64 = 24.0;
pub const TEXT_ROW: f64 = 20.0;
pub const TEXT_MONO_ISH: f64 = 16.0;
pub const TEXT_SMALL: f64 = 14.0;
pub const TEXT_TINY: f64 = 12.0;

// Icons.
pub const ICON_ROW: f64 = 32.0;
pub const ICON_PIN: f64 = 24.0;
pub const ICON_GRID: f64 = 80.0;
pub const ICON_TAB_CLOSE: f64 = 28.0;
pub const ICON_NAV: f64 = 32.0;
pub const ICON_SEARCH: f64 = 32.0;
pub const ICON_WINDOW: f64 = 32.0;

// Colour roles. Every colour is a theme colour or a GTK colour function of
// one; no literal colour appears anywhere in the generated stylesheet.
pub const BG: &str = "@window_bg_color";
pub const FG: &str = "@window_fg_color";
pub const OUTLINE: &str = "@accent_color";
pub const RAISED: &str = "@card_bg_color";
pub const RAISED_FG: &str = "@card_fg_color";
pub const RECESSED: &str = "shade(@window_bg_color, 0.8)";
pub const SELECTED: &str = "mix(shade(@window_bg_color, 0.8), @accent_color, 0.5)";
pub const SELECTED_ACTIVE: &str = "mix(@card_bg_color, @accent_color, 0.5)";
pub const FOCUS_RING: &str = "@accent_color";
pub const HOVER: &str = "alpha(@window_fg_color, 0.1)";
pub const ACTIVE: &str = "alpha(@window_fg_color, 0.2)";
pub const HOVER_STRONG: &str = "alpha(@window_fg_color, 0.15)";
pub const SELECTION: &str = "alpha(@accent_color, 0.35)";
pub const PLACEHOLDER: &str = "alpha(@window_fg_color, 0.5)";
pub const RULE: &str = "alpha(@window_fg_color, 0.3)";
pub const PINNED_FG: &str = "@sidebar_fg_color";
pub const PIN_SEL: &str = "mix(@sidebar_bg_color, @sidebar_fg_color, 0.12)";

/// One token at the current scale, rounded to whole pixels. A non-zero token
/// never becomes zero, so a border never disappears.
pub fn scaled(token: f64, scale: f64) -> i32 {
    let value = (token * scale).round() as i32;
    if token > 0.0 { value.max(1) } else { value }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_tokens_keep_a_visible_pixel() {
        assert_eq!(scaled(GAP_L, 0.5), 12);
        assert_eq!(scaled(LINE_HAIR, 0.5), 1);
        assert_eq!(scaled(GAP_2XL, 1.0), 40);
        assert_eq!(scaled(0.0, 1.0), 0);
    }

    #[test]
    fn tokens_lost_nothing_in_the_merge() {
        // Every shared name kept its value; each table's own extras are here.
        let shared = [
            (GAP_S, 8.0),
            (GAP_M, 16.0),
            (GAP_L, 24.0),
            (GAP_XL, 32.0),
            (GAP_2XL, 40.0),
            (CONTROL_H, 50.0),
            (HEADER_H, 96.0),
            (LINE_THIN, 2.0),
            (LINE_MID, 3.0),
            (RADIUS, 12.0),
            (RADIUS_PILL, 999.0),
            (TEXT_TITLE, 32.0),
            (TEXT_CONTROL, 24.0),
            (TEXT_ROW, 20.0),
            (ICON_TAB_CLOSE, 28.0),
            (PILL_W, 270.0),
        ];
        for (value, expected) in shared {
            assert_eq!(value, expected);
        }
        // Body-only tokens.
        for value in [
            SECTION_H,
            SECTION_GAP,
            PIN_ROW_H,
            DIR_ROW_H,
            STATUS_H,
            BAR_W,
            BAR_H,
            MAIN_BAR_W,
            MAIN_BAR_H,
            CELL_W,
            CELL_H,
            SIDEBAR_W,
            SIDEBAR_MIN,
            PREVIEW_W,
            PREVIEW_MIN,
            PREVIEW_GUTTER,
            TEXT_ITEM,
            TEXT_MONO_ISH,
            TEXT_SMALL,
            TEXT_TINY,
            ICON_ROW,
            ICON_PIN,
            ICON_GRID,
            LINE_HAIR,
        ] {
            assert!(value > 0.0);
        }
        // Header-only tokens.
        for value in [
            HEADER_INSET,
            LINE_HEAVY,
            RADIUS_ICON,
            ICON_NAV,
            ICON_SEARCH,
            ICON_WINDOW,
            SEARCH_W,
            SEARCH_HINT_W,
            PATH_MIN,
            TRASH,
        ] {
            assert!(value > 0.0);
        }
        assert_eq!((DEFAULT_W, DEFAULT_H), (2560, 1440));
    }
}
