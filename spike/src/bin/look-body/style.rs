// SPDX-License-Identifier: GPL-3.0-or-later
//! Tokens, colour roles, the drive-bar class function and the generated
//! stylesheet. Everything follows look-style-spec.md.

// Constant tables (the only #[rustfmt::skip] left in this binary, per the
// P1j-r1 review).
#[rustfmt::skip]
pub mod tok {
    pub const GAP_S: f64 = 8.0; pub const GAP_M: f64 = 16.0; pub const GAP_L: f64 = 24.0; pub const GAP_XL: f64 = 32.0; pub const GAP_2XL: f64 = 40.0;
    pub const CONTROL_H: f64 = 50.0; pub const HEADER_H: f64 = 96.0;
    pub const LINE_HAIR: f64 = 1.0; pub const LINE_THIN: f64 = 2.0; pub const LINE_MID: f64 = 3.0;
    pub const RADIUS: f64 = 12.0; pub const RADIUS_PILL: f64 = 999.0;
    pub const GRID_MARGIN: f64 = 72.0; pub const SECTION_H: f64 = 32.0; pub const SECTION_GAP: f64 = 6.0;
    pub const PIN_ROW_H: f64 = 64.0; pub const DIR_ROW_H: f64 = 32.0; pub const STATUS_H: f64 = 64.0;
    pub const BAR_W: f64 = 209.0; pub const BAR_H: f64 = 18.0; pub const MAIN_BAR_W: f64 = 228.0; pub const MAIN_BAR_H: f64 = 20.0;
    pub const CELL_W: f64 = 97.0; pub const CELL_H: f64 = 144.0;
    pub const SIDEBAR_W: f64 = 348.0; pub const SIDEBAR_MIN: f64 = 300.0;
    pub const PREVIEW_W: f64 = 648.0; pub const PREVIEW_MIN: f64 = 360.0;
    pub const PILL_W: f64 = 270.0;
    pub const TEXT_TITLE: f64 = 32.0; pub const TEXT_ITEM: f64 = 22.0; pub const TEXT_CONTROL: f64 = 24.0; pub const TEXT_ROW: f64 = 20.0;
    pub const TEXT_MONO_ISH: f64 = 16.0; pub const TEXT_SMALL: f64 = 14.0; pub const TEXT_TINY: f64 = 12.0;
    pub const ICON_ROW: f64 = 32.0; pub const ICON_PIN: f64 = 24.0; pub const ICON_GRID: f64 = 80.0; pub const ICON_TAB_CLOSE: f64 = 28.0;
    pub const DEFAULT_W: i32 = 2560; pub const DEFAULT_H: i32 = 1440;
}
pub use tok::*;

#[rustfmt::skip]
pub mod col {
    pub const BG: &str = "@window_bg_color"; pub const FG: &str = "@window_fg_color"; pub const OUTLINE: &str = "@accent_color";
    pub const RAISED: &str = "@card_bg_color"; pub const RAISED_FG: &str = "@card_fg_color";
    pub const RECESSED: &str = "shade(@window_bg_color, 0.8)";
    pub const SELECTED: &str = "mix(shade(@window_bg_color, 0.8), @accent_color, 0.5)";
    pub const SELECTED_ACTIVE: &str = "mix(@card_bg_color, @accent_color, 0.5)";
    pub const FOCUS_RING: &str = "@accent_color";
    pub const HOVER: &str = "alpha(@window_fg_color, 0.1)"; pub const HOVER_STRONG: &str = "alpha(@window_fg_color, 0.15)";
    pub const RULE: &str = "alpha(@window_fg_color, 0.3)";
    pub const PINNED_FG: &str = "@sidebar_fg_color";
    pub const PIN_SEL: &str = "mix(@sidebar_bg_color, @sidebar_fg_color, 0.12)";
}
pub use col::*;

/// The mockup folder's five symbolic colour slots, in the order the table in
/// look-style-spec.md gives them. `foreground` (the `color` property) is the
/// paper tab; the four remappable slots carry the accent-derived faces. GTK
/// 4.22 includes `accent` in `-gtk-icon-palette`'s lookup (gtk-4.22.4:
/// gtk/gtkcssstyle.c:883-900, gtk/gtkcsspalettevalue.c:298-312), so the folder
/// needs no success/warning/error fallback.
pub const FOLDER_PALETTE: &str = "accent @accent_color, success shade(@accent_color, 0.7), warning shade(@accent_color, 0.6), error shade(@accent_color, 0.45)";
/// Save As' pencil outline is the window background behind the button.
pub const SAVE_AS_PALETTE: &str = "error @window_bg_color";

pub const DEFAULT_OPACITY: f64 = 0.85;

pub fn base() -> f64 {
    std::env::var("GIMBAL_LOOK_BASE")
        .ok()
        .and_then(|v| v.parse().ok())
        .filter(|v: &f64| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0)
}

/// A valid `GIMBAL_LOOK_OPACITY` value, or `None` when it is unparsable or
/// outside 0.50–1.00. The caller falls back to `DEFAULT_OPACITY`.
pub fn parse_opacity(raw: &str) -> Option<f64> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && (0.50..=1.00).contains(v))
}

/// The window opacity from the environment (the config file's stand-in).
/// An invalid value falls back to 0.85 with exactly one log line.
pub fn opacity() -> f64 {
    match std::env::var("GIMBAL_LOOK_OPACITY") {
        Ok(raw) => parse_opacity(&raw).unwrap_or_else(|| {
            crate::log!(
                "opacity",
                "GIMBAL_LOOK_OPACITY={raw:?} is not 0.50–1.00; using {DEFAULT_OPACITY:.2}"
            );
            DEFAULT_OPACITY
        }),
        Err(_) => DEFAULT_OPACITY,
    }
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
pub fn bar_css(opacity: f64) -> String {
    let o = opacity.clamp(0.50, 1.00);
    let levels: Vec<String> = (0..10)
        .map(|n| format!(".gl-drive-fill.bar-lv{n} {{ background-color: alpha(mix(@card_bg_color, @window_fg_color, {}), {o:.2}); }}", 0.2 + 0.08 * n as f64))
        .collect();
    let mut lines = levels;
    lines.push(format!(
        ".gl-drive-fill.bar-full {{ background-color: alpha(@error_bg_color, {o:.2}); }}"
    ));
    lines.push(".bar-label-lo { color: @window_fg_color; }".to_string());
    lines.push(".bar-label-hi { color: @window_bg_color; }".to_string());
    lines.push(".bar-label-err { color: @error_fg_color; }".to_string());
    lines.join("\n")
}

pub fn css(scale: f64, font: Option<&str>, opacity: f64) -> String {
    let p = |t: f64| scaled(t, scale);
    let font = font
        .map(|f| format!("font-family: \"{}\";", f.replace(['"', '\\', ';'], "")))
        .unwrap_or_default();
    let o = opacity.clamp(0.50, 1.00);
    let fill = |role: &str| format!("alpha({role}, {o:.2})");
    let (gs, gm, gxl, g2) = (p(GAP_S), p(GAP_M), p(GAP_XL), p(GAP_2XL));
    let section_gap = p(SECTION_GAP);
    // The gap after an expanded section's content. The next section adds its
    // own 6 px top margin, so the content's bottom margin carries the rest.
    let section_tail = (p(GAP_XL) - section_gap).max(0);
    let (hair, thin, mid) = (p(LINE_HAIR), p(LINE_THIN), p(LINE_MID));
    let strip_pad = gm + mid;
    let (r, rp) = (p(RADIUS), p(RADIUS_PILL));
    let (tt, titem, tc, trow, tmono, tsmall, ttiny) = (
        p(TEXT_TITLE),
        p(TEXT_ITEM),
        p(TEXT_CONTROL),
        p(TEXT_ROW),
        p(TEXT_MONO_ISH),
        p(TEXT_SMALL),
        p(TEXT_TINY),
    );
    let (irow, ipin, itc) = (p(ICON_ROW), p(ICON_PIN), p(ICON_TAB_CLOSE));
    // min-width/min-height are content-box: subtract padding and border.
    let pill_w = (p(PILL_W) - 2 * gm - 2 * mid).max(1);
    let pill_h = (p(CONTROL_H) - 2 * mid).max(1);
    let sect_h = (p(SECTION_H) - 2 * thin).max(1);
    let close = (itc - 2 * thin).max(1);
    let window_bg = fill(BG);
    let pill_bg = fill(RECESSED);
    let pill_active = fill(RAISED);
    let pill_selected = fill(SELECTED);
    let pill_selected_active = fill(SELECTED_ACTIVE);
    let pin_sel = fill(PIN_SEL);
    let track = fill(RAISED);
    let tile = fill(RAISED);
    format!(
        "\
window.gimbal-look, window.gimbal-look.background {{ background-color: {window_bg}; color: {FG}; font-size: {tc}px; {font} }}
.gl-header-slot {{ min-height: {hh}px; }}
.gl-panel {{ border: {mid}px solid {OUTLINE}; border-radius: {r}px; padding: {gm}px; }}
.gl-title {{ font-size: {tt}px; }} .gl-rule {{ min-height: {hair}px; background-color: {RULE}; }}
.gl-rule.inset {{ margin-left: {gxl}px; margin-right: {gxl}px; }}
.gl-icon-button {{ all: unset; color: {FG}; padding: {gs}px; min-width: {irow}px; min-height: {irow}px; border-radius: {r}px; -gtk-icon-size: {irow}px; }}
.gl-icon-button:hover {{ background-color: {HOVER}; }} .gl-icon-button:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pill {{ all: unset; min-height: {pill_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {rp}px; background-color: {pill_bg}; color: {FG}; }}
.gl-pill label {{ font-size: {tc}px; }} .gl-pill.tab {{ min-width: {pill_w}px; }}
.gl-pill.active {{ background-color: {pill_active}; color: {RAISED_FG}; }}
.gl-pill.selected {{ background-color: {pill_selected}; }} .gl-pill.active.selected {{ background-color: {pill_selected_active}; }}
.gl-pill-close {{ all: unset; min-width: {close}px; min-height: {close}px; -gtk-icon-size: {close}px; padding: 0; border: {thin}px solid {FG}; border-radius: {rp}px; color: {FG}; margin-left: {gm}px; }}
.gl-pill-close:hover {{ background-color: {HOVER_STRONG}; }} .gl-pill-close:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pill.active .gl-pill-close {{ color: {RAISED_FG}; border-color: {RAISED_FG}; }} .gl-pill.tab {{ margin-bottom: {gs}px; }}
.gl-section {{ margin-top: {section_gap}px; }}
.gl-section-pill {{ all: unset; min-height: {sect_h}px; margin: 0 {gm}px; border: {thin}px solid {OUTLINE}; border-radius: {rp}px; }}
.gl-section-content {{ margin: {gs}px 0 {section_tail}px 0; }}
.gl-section:last-child .gl-section-content {{ margin-bottom: 0; }}
.gl-section-pill label {{ font-size: {titem}px; }}
.gl-section-pill:hover {{ background-color: {HOVER}; }} .gl-section-pill:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pinned {{ color: {PINNED_FG}; border-radius: {r}px; margin-top: {gs}px; }}
.gl-pinned-row {{ all: unset; min-height: {pinrow}px; padding: 0 {gs}px; }}
.gl-pinned-row image {{ -gtk-icon-size: {ipin}px; }} .gl-pinned-row label {{ font-size: {titem}px; margin-left: {gs}px; }}
.gl-pinned-row.sel {{ background-color: {pin_sel}; }}
.gl-dir-row {{ min-height: {dirrow}px; }} .gl-dir-row label {{ font-size: {titem}px; }}
.gl-drive-name {{ font-size: {titem}px; }} .gl-drive-block {{ margin-bottom: {gs}px; }}
.gl-drive-track {{ min-width: {barw}px; min-height: {barh}px; background-color: {track}; }} .gl-drive-track.main {{ min-width: {mbarw}px; min-height: {mbarh}px; }}
.gl-drive-fill {{ min-height: {barh}px; }} .gl-drive-fill.main {{ min-height: {mbarh}px; }}
{bars}
.gl-drive-label {{ font-size: {ttiny}px; }} .gl-drive-cap {{ font-size: {ttiny}px; margin-left: {gs}px; }} .gl-main-label {{ font-size: {tsmall}px; }}
.gl-grid {{ border-spacing: {gxl}px; margin: {g2}px {gmar}px 0 {gmar}px; background-color: transparent; }} .gl-grid.tile {{ margin: {gm}px; }}
gridview > child {{ padding: 0; }}
.gl-cell {{ min-width: {cellw}px; min-height: {cellh}px; }}
.gl-cell image {{ -gtk-icon-size: {igrid}px; color: {FG}; -gtk-icon-palette: {folder_palette}; }} .gl-cell label {{ font-size: {trow}px; }}
.gl-tile {{ background-color: {tile}; border: {thin}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; }}
.gl-tile.focused {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-status {{ min-height: {statush}px; font-size: {tc}px; }} .gl-num {{ font-size: {tmono}px; }} .gl-pline {{ font-size: {tmono}px; margin-left: {gm}px; }}
.gl-collapse-strip {{ min-width: {g2}px; }} .gl-collapse-strip.left {{ padding: {strip_pad}px 0 0 0; }} .gl-collapse-strip.right {{ padding: {strip_pad}px 0 0 0; }} .gl-action-row .gl-icon-button {{ margin-left: {gm}px; }} .gl-action-row image {{ color: {FG}; -gtk-icon-palette: {save_as_palette}; }}
window.gimbal-look .gl-frame gridview, window.gimbal-look .gl-frame listview, window.gimbal-look .gl-frame textview, window.gimbal-look .gl-frame textview > text, window.gimbal-look .gl-frame scrolledwindow, window.gimbal-look .gl-frame viewport, window.gimbal-look .gl-frame gridview > child, window.gimbal-look windowhandle {{ background-color: transparent; }}",
        hh = p(HEADER_H), pinrow = p(PIN_ROW_H), dirrow = p(DIR_ROW_H),
        barw = p(BAR_W), barh = p(BAR_H), mbarw = p(MAIN_BAR_W), mbarh = p(MAIN_BAR_H),
        cellw = p(CELL_W), cellh = p(CELL_H), igrid = p(ICON_GRID), gmar = p(GRID_MARGIN),
        statush = p(STATUS_H), bars = bar_css(o),
        folder_palette = FOLDER_PALETTE, save_as_palette = SAVE_AS_PALETTE,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_rules() {
        let plain = css(1.0, None, DEFAULT_OPACITY);
        for bad in [
            "#", "rgb(", "hsl(", "red", "white", "black", "gray", "green",
        ] {
            assert!(!plain.contains(bad), "colour literal {bad:?}");
        }
        assert!(!plain.contains("font-family"));
        assert!(
            css(1.0, Some("Fira Sans"), DEFAULT_OPACITY).contains("font-family: \"Fira Sans\";")
        );
        assert!(plain.contains("alpha(@window_bg_color, 0.85)"));
        assert!(plain.contains("alpha(mix(@card_bg_color, @window_fg_color, 0.2), 0.85)"));
        assert!(!plain.contains(".gl-panel { background-color"));
    }

    #[test]
    fn nov_1_outlines_and_contrast() {
        let plain = css(1.0, None, DEFAULT_OPACITY);
        // line-mid 3 and line-thin 2 (was 6 and 3).
        assert!(plain.contains("border: 3px solid @accent_color"));
        assert!(plain.contains("border: 2px solid @window_fg_color"));
        // Selection mixes 0.5 accent, and the focus ring is the full accent.
        assert!(plain.contains("mix(shade(@window_bg_color, 0.8), @accent_color, 0.5)"));
        assert!(plain.contains("mix(@card_bg_color, @accent_color, 0.5)"));
        assert!(plain.contains("0 0 0 2px @accent_color"));
        assert!(!plain.contains("alpha(@accent_color"));
    }

    #[test]
    fn pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart() {
        let plain = css(1.0, None, DEFAULT_OPACITY);
        // The rows sit straight on the panel; only the selected row fills.
        assert!(plain.contains(".gl-pinned { color: @sidebar_fg_color"));
        assert!(!plain.contains(".gl-pinned { background-color"));
        assert!(plain.contains(".gl-pinned-row.sel { background-color:"));
        // Collapsed section pills are 6 apart; an expanded section's content
        // sits 8 below its pill and 32 before the next pill (26 + the next
        // section's 6 px top margin).
        assert!(plain.contains(".gl-section { margin-top: 6px; }"));
        assert!(plain.contains(".gl-section-content { margin: 8px 0 26px 0; }"));
        // The last section has no next pill, so it keeps no bottom gap.
        assert!(plain.contains(".gl-section:last-child .gl-section-content { margin-bottom: 0; }"));
        assert!(plain.contains(".gl-section-pill { all: unset; min-height: 28px; margin: 0 16px;"));
    }

    #[test]
    fn icon_palettes_map_the_spec_table() {
        let plain = css(1.0, None, DEFAULT_OPACITY);
        // The four remappable slots are set from theme colours, live.
        for entry in [
            "accent @accent_color",
            "success shade(@accent_color, 0.7)",
            "warning shade(@accent_color, 0.6)",
            "error shade(@accent_color, 0.45)",
        ] {
            assert!(plain.contains(entry), "folder palette lacks {entry:?}");
        }
        assert!(plain.contains("-gtk-icon-palette: error @window_bg_color"));
        // The folder's parts carry slots, not the mockup's grey stand-ins.
        let folder = include_str!("icons/gimbal-folder-symbolic.svg");
        for slot in [
            "gpa:fill=\"accent\"",
            "gpa:fill=\"success\"",
            "gpa:fill=\"warning\"",
            "gpa:fill=\"foreground\"",
            "gpa:fill=\"error\"",
        ] {
            assert!(folder.contains(slot), "folder icon lacks {slot:?}");
        }
        for grey in ["#333333", "#656565", "#565656", "#8e8e8e", "#000000"] {
            assert!(!folder.contains(grey), "fixed colour {grey:?} left behind");
        }
        // Save As keeps its pencil fill in the text colour and its outline in
        // the window background.
        let save_as = include_str!("icons/gimbal-save-as-symbolic.svg");
        assert!(save_as.contains("gpa:fill=\"foreground\""));
        assert!(save_as.contains("gpa:stroke=\"error\""));
        assert!(!save_as.contains("#000000") && !save_as.contains("#d4d4d4"));
    }

    #[test]
    fn opacity_parsing_falls_back() {
        assert_eq!(parse_opacity("0.50"), Some(0.50));
        assert_eq!(parse_opacity("1"), Some(1.0));
        assert_eq!(parse_opacity(" 0.85 "), Some(0.85));
        for bad in ["0.49", "1.01", "x", "", "NaN", "inf"] {
            assert_eq!(parse_opacity(bad), None, "{bad:?} should be invalid");
        }
        assert!(css(1.0, None, 0.2).contains("alpha(@window_bg_color, 0.50)"));
        assert!(css(1.0, None, 1.5).contains("alpha(@window_bg_color, 1.00)"));
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
