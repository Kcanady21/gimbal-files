// SPDX-License-Identifier: GPL-3.0-or-later
//! Tokens, colour roles, the drive-bar class function and the generated
//! stylesheet. Everything follows look-style-spec.md.

// Constant tables (the only #[rustfmt::skip] left in this binary, per the
// P1j-r1 review).
#[rustfmt::skip]
pub mod tok {
    pub const GAP_S: f64 = 8.0; pub const GAP_M: f64 = 16.0; pub const GAP_L: f64 = 24.0; pub const GAP_XL: f64 = 32.0; pub const GAP_2XL: f64 = 40.0;
    pub const CONTROL_H: f64 = 50.0; pub const HEADER_H: f64 = 96.0;
    pub const LINE_HAIR: f64 = 1.0; pub const LINE_THIN: f64 = 3.0; pub const LINE_MID: f64 = 6.0; pub const LINE_HEAVY: f64 = 10.0;
    pub const RADIUS: f64 = 12.0; pub const RADIUS_PILL: f64 = 999.0;
    pub const GRID_MARGIN: f64 = 72.0; pub const SECTION_H: f64 = 40.0; pub const DRIVE_PITCH: f64 = 54.0;
    pub const PIN_ROW_H: f64 = 64.0; pub const DIR_ROW_H: f64 = 32.0; pub const STATUS_H: f64 = 64.0;
    pub const BAR_W: f64 = 209.0; pub const BAR_H: f64 = 18.0; pub const MAIN_BAR_W: f64 = 228.0; pub const MAIN_BAR_H: f64 = 20.0;
    pub const CELL_W: f64 = 120.0; pub const CELL_H: f64 = 144.0;
    pub const SIDEBAR_W: f64 = 348.0; pub const SIDEBAR_MIN: f64 = 260.0;
    pub const PREVIEW_W: f64 = 648.0; pub const PREVIEW_MIN: f64 = 360.0;
    pub const PILL_W: f64 = 270.0;
    pub const TEXT_TITLE: f64 = 32.0; pub const TEXT_SECTION: f64 = 28.0; pub const TEXT_CONTROL: f64 = 24.0; pub const TEXT_ROW: f64 = 20.0;
    pub const TEXT_MONO_ISH: f64 = 16.0; pub const TEXT_SMALL: f64 = 14.0; pub const TEXT_TINY: f64 = 12.0;
    pub const ICON_ROW: f64 = 32.0; pub const ICON_GRID: f64 = 80.0; pub const ICON_TAB_CLOSE: f64 = 28.0;
    pub const DEFAULT_W: i32 = 2560; pub const DEFAULT_H: i32 = 1440;
}
pub use tok::*;

#[rustfmt::skip]
pub mod col {
    pub const BG: &str = "@window_bg_color"; pub const FG: &str = "@window_fg_color"; pub const OUTLINE: &str = "@accent_color";
    pub const RAISED: &str = "@card_bg_color"; pub const RAISED_FG: &str = "@card_fg_color";
    pub const RECESSED: &str = "shade(@window_bg_color, 0.8)";
    pub const SELECTED: &str = "mix(shade(@window_bg_color, 0.8), @accent_color, 0.3)";
    pub const SELECTED_ACTIVE: &str = "mix(@card_bg_color, @accent_color, 0.3)";
    pub const FOCUS_RING: &str = "alpha(@accent_color, 0.5)";
    pub const HOVER: &str = "alpha(@window_fg_color, 0.1)"; pub const HOVER_STRONG: &str = "alpha(@window_fg_color, 0.15)";
    pub const RULE: &str = "alpha(@window_fg_color, 0.3)";
    pub const PINNED_BG: &str = "@sidebar_bg_color"; pub const PINNED_FG: &str = "@sidebar_fg_color";
    pub const PIN_SEL: &str = "mix(@sidebar_bg_color, @sidebar_fg_color, 0.12)";
}
pub use col::*;

pub fn base() -> f64 {
    std::env::var("GIMBAL_LOOK_BASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
}

pub fn scaled(token: f64, scale: f64) -> i32 {
    let value = (token * scale).round() as i32;
    if token > 0.0 { value.max(1) } else { value }
}

/// Fullness → (fill class, label class). Contrast rises up to 94 %, error from 95 %.
pub fn bar_class(fullness: f64) -> (&'static str, &'static str) {
    if !(0.0..0.95).contains(&fullness) {
        return ("bar-full", "bar-label-err");
    }
    let n = ((fullness * 10.0).floor() as usize).min(9);
    let fill = [
        "bar-lv0", "bar-lv1", "bar-lv2", "bar-lv3", "bar-lv4", "bar-lv5", "bar-lv6", "bar-lv7",
        "bar-lv8", "bar-lv9",
    ][n];
    let label = if 0.2 + 0.08 * n as f64 > 0.5 {
        "bar-label-hi"
    } else {
        "bar-label-lo"
    };
    (fill, label)
}

/// Every colour expression lives in this one table (the bar levels).
pub fn bar_css() -> String {
    let levels: Vec<String> = (0..10)
        .map(|n| format!(".gl-drive-fill.bar-lv{n} {{ background-color: mix(@card_bg_color, @window_fg_color, {}); }}", 0.2 + 0.08 * n as f64))
        .collect();
    let mut lines = levels;
    lines.push(".gl-drive-fill.bar-full { background-color: @error_bg_color; }".to_string());
    lines.push(".bar-label-lo { color: @window_fg_color; }".to_string());
    lines.push(".bar-label-hi { color: @window_bg_color; }".to_string());
    lines.push(".bar-label-err { color: @error_fg_color; }".to_string());
    lines.join("\n")
}

pub fn css(scale: f64, font: Option<&str>) -> String {
    let p = |t: f64| scaled(t, scale);
    let font = font
        .map(|f| format!("font-family: \"{}\";", f.replace(['"', '\\', ';'], "")))
        .unwrap_or_default();
    let (gs, gm, gl, gxl, g2) = (p(GAP_S), p(GAP_M), p(GAP_L), p(GAP_XL), p(GAP_2XL));
    let (hair, thin, mid, heavy) = (p(LINE_HAIR), p(LINE_THIN), p(LINE_MID), p(LINE_HEAVY));
    let (r, rp) = (p(RADIUS), p(RADIUS_PILL));
    let (tt, tsec, tc, trow, tmono, tsmall, ttiny) = (
        p(TEXT_TITLE),
        p(TEXT_SECTION),
        p(TEXT_CONTROL),
        p(TEXT_ROW),
        p(TEXT_MONO_ISH),
        p(TEXT_SMALL),
        p(TEXT_TINY),
    );
    let (irow, itc) = (p(ICON_ROW), p(ICON_TAB_CLOSE));
    // min-width/min-height are content-box: subtract padding and border.
    let pill_w = (p(PILL_W) - 2 * gm - 2 * mid).max(1);
    let pill_h = (p(CONTROL_H) - 2 * mid).max(1);
    let sect_h = (p(SECTION_H) - 2 * thin).max(1);
    let close = (itc - 2 * thin).max(1);
    format!(
        "\
window.gimbal-look {{ background-color: {BG}; color: {FG}; font-size: {tc}px; {font} }}
.gl-frame {{ border: {heavy}px solid {OUTLINE}; border-radius: {r}px; background-color: {BG}; }} .gl-header-slot {{ min-height: {hh}px; }}
.gl-panel {{ border: {mid}px solid {OUTLINE}; border-radius: {r}px; padding: {gm}px; background-color: {BG}; }}
.gl-title {{ font-size: {tt}px; }} .gl-rule {{ min-height: {hair}px; background-color: {RULE}; }}
.gl-rule.inset {{ margin-left: {gxl}px; margin-right: {gxl}px; }}
.gl-icon-button {{ all: unset; color: {FG}; padding: {gs}px; min-width: {irow}px; min-height: {irow}px; border-radius: {r}px; -gtk-icon-size: {irow}px; }}
.gl-icon-button:hover {{ background-color: {HOVER}; }} .gl-icon-button:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pill {{ all: unset; min-height: {pill_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {rp}px; background-color: {RECESSED}; color: {FG}; transition: opacity 250ms; }}
.gl-pill label {{ font-size: {tc}px; }} .gl-pill.tab {{ min-width: {pill_w}px; }}
.gl-pill.active {{ background-color: {RAISED}; color: {RAISED_FG}; }}
.gl-pill.selected {{ background-color: {SELECTED}; }} .gl-pill.active.selected {{ background-color: {SELECTED_ACTIVE}; }}
.gl-pill-close {{ all: unset; min-width: {close}px; min-height: {close}px; -gtk-icon-size: {close}px; padding: 0; border: {thin}px solid {FG}; border-radius: {rp}px; color: {FG}; margin-left: {gm}px; }}
.gl-pill-close:hover {{ background-color: {HOVER_STRONG}; }} .gl-pill-close:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pill.active .gl-pill-close {{ color: {RAISED_FG}; border-color: {RAISED_FG}; }} .gl-tile .gl-pill {{ margin-bottom: {gl}px; }}
.gl-section {{ margin-top: {gxl}px; }}
.gl-section-pill {{ all: unset; min-height: {sect_h}px; margin: 0 {gm}px {gs}px {gm}px; border: {thin}px solid {OUTLINE}; border-radius: {rp}px; }}
.gl-section-pill label {{ font-size: {tsec}px; }}
.gl-section-pill:hover {{ background-color: {HOVER}; }} .gl-section-pill:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pinned {{ background-color: {PINNED_BG}; color: {PINNED_FG}; border-radius: {r}px; margin-top: {gs}px; }}
.gl-pinned-row {{ all: unset; min-height: {pinrow}px; padding: 0 {gs}px; }}
.gl-pinned-row image {{ -gtk-icon-size: {irow}px; }} .gl-pinned-row label {{ font-size: {tsec}px; margin-left: {gs}px; }}
.gl-pinned-row.sel {{ background-color: {PIN_SEL}; }}
.gl-dir-row {{ min-height: {dirrow}px; }} .gl-dir-row label {{ font-size: {trow}px; }}
.gl-drive-name {{ font-size: {trow}px; }} .gl-drive-block {{ margin-bottom: {dpitch}px; }}
.gl-drive-track {{ min-width: {barw}px; min-height: {barh}px; background-color: {RAISED}; }} .gl-drive-track.main {{ min-width: {mbarw}px; min-height: {mbarh}px; }}
.gl-drive-fill {{ min-height: {barh}px; }} .gl-drive-fill.main {{ min-height: {mbarh}px; }}
{bars}
.gl-drive-label {{ font-size: {ttiny}px; }} .gl-drive-cap {{ font-size: {ttiny}px; margin-left: {gs}px; }} .gl-main-label {{ font-size: {tsmall}px; }}
.gl-grid {{ border-spacing: {gs}px; margin: {g2}px {gmar}px 0 {gmar}px; }} .gl-grid.tile {{ margin: {g2}px; }}
.gl-cell {{ min-width: {cellw}px; min-height: {cellh}px; }}
.gl-cell image {{ -gtk-icon-size: {igrid}px; color: {FG}; }} .gl-cell label {{ font-size: {trow}px; }}
.gl-tile {{ background-color: {RAISED}; border: {thin}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; }}
.gl-tile.focused {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-status {{ min-height: {statush}px; font-size: {tc}px; }} .gl-num {{ font-size: {tmono}px; }} .gl-pline {{ font-size: {tmono}px; margin-left: {gm}px; }}
.gl-collapse-strip {{ min-width: {g2}px; }} .gl-action-row .gl-icon-button {{ margin-left: {gm}px; }}",
        hh = p(HEADER_H), pinrow = p(PIN_ROW_H), dirrow = p(DIR_ROW_H), dpitch = p(DRIVE_PITCH),
        barw = p(BAR_W), barh = p(BAR_H), mbarw = p(MAIN_BAR_W), mbarh = p(MAIN_BAR_H),
        cellw = p(CELL_W), cellh = p(CELL_H), igrid = p(ICON_GRID), gmar = p(GRID_MARGIN),
        statush = p(STATUS_H), bars = bar_css(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_rules() {
        let plain = css(1.0, None);
        for bad in [
            "#", "rgb(", "hsl(", "red", "white", "black", "gray", "green",
        ] {
            assert!(!plain.contains(bad), "colour literal {bad:?}");
        }
        assert!(!plain.contains("font-family"));
        assert!(css(1.0, Some("Fira Sans")).contains("font-family: \"Fira Sans\";"));
    }

    #[test]
    fn scaling() {
        assert_eq!(scaled(GAP_L, 0.5), 12);
        assert_eq!(scaled(LINE_HAIR, 0.5), 1);
        assert_eq!(scaled(GAP_2XL, 1.0), 40); // non-zero border keeps 1 px
    }

    #[test]
    fn drive_classes() {
        assert_eq!(bar_class(0.0), ("bar-lv0", "bar-label-lo"));
        assert_eq!(bar_class(0.5), ("bar-lv5", "bar-label-hi"));
        assert_eq!(bar_class(0.94), ("bar-lv9", "bar-label-hi"));
        assert_eq!(bar_class(0.949), ("bar-lv9", "bar-label-hi"));
        assert_eq!(bar_class(0.95), ("bar-full", "bar-label-err"));
        assert_eq!(bar_class(1.0), ("bar-full", "bar-label-err"));
    }
}
