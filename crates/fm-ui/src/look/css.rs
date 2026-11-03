// SPDX-License-Identifier: GPL-3.0-or-later
//! The one generated stylesheet, merged from the spike's header and body
//! generators. It covers every `gl-` class either program styled, so later
//! briefs only add widgets.
//!
//! The header's fit and pill-drag arithmetic is ported here now, with its
//! tests; P2h binds it to real pills.
#![allow(dead_code)]

use crate::look::icons::{FLOAT_PALETTE, FOLDER_PALETTE, SAVE_AS_PALETTE};
use crate::look::motion;
use crate::look::tokens::*;

/// One rule per line, so the stylesheet can be read and diffed live.
pub fn css(scale: f64, font: Option<&str>, opacity: f64) -> String {
    let p = |token: f64| scaled(token, scale);
    let font = font
        .map(|family| format!("font-family: \"{}\";", family.replace(['"', '\\', ';'], "")))
        .unwrap_or_default();
    // The config allows 0.0–1.0. Do not clamp to the old spike range: a very
    // low value is a very see-through window, by the config's choice.
    let o = opacity.clamp(0.0, 1.0);
    let fill = |role: &str| format!("alpha({role}, {o:.2})");
    let (gs, gm, gl, gxl, g2xl) = (p(GAP_S), p(GAP_M), p(GAP_L), p(GAP_XL), p(GAP_2XL));
    let (ch, hh, inset) = (p(CONTROL_H), p(HEADER_H), p(HEADER_INSET));
    let (hair, thin, mid) = (p(LINE_HAIR), p(LINE_THIN), p(LINE_MID));
    let (r, rp, ri) = (p(RADIUS), p(RADIUS_PILL), p(RADIUS_ICON));
    let (tt, titem, tc, trow, tmono, tsmall, ttiny) = (
        p(TEXT_TITLE),
        p(TEXT_ITEM),
        p(TEXT_CONTROL),
        p(TEXT_ROW),
        p(TEXT_MONO_ISH),
        p(TEXT_SMALL),
        p(TEXT_TINY),
    );
    let (irow, ipin, igrid, itc, inav, isearch, iwin) = (
        p(ICON_ROW),
        p(ICON_PIN),
        p(ICON_GRID),
        p(ICON_TAB_CLOSE),
        p(ICON_NAV),
        p(ICON_SEARCH),
        p(ICON_WINDOW),
    );
    let section_gap = p(SECTION_GAP);
    let section_tail = (p(GAP_M) - section_gap).max(0);
    let pill_h = (p(CONTROL_H) - 2 * mid).max(1);
    let pill_w = (p(PILL_W) - 2 * gm - 2 * mid).max(1);
    let sect_h = (p(SECTION_H) - 2 * thin).max(1);
    let close_inner = (itc - 2 * thin).max(1);
    let search_w = (p(SEARCH_W) - 2 * gm - 2 * mid).max(1);
    let search_entry_w = (search_w - isearch - gm).max(1);
    let trash_inner = (p(TRASH) - 2 * thin).max(1);
    let pitch = p(PILL_W) + p(GAP_L);
    let hover_ms = motion::ms(120);
    let slide_ms = motion::ms(150);
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
.gl-header {{ min-height: {hh}px; padding: 0 {inset}px; }}
.gl-header-slot {{ min-height: {hh}px; }}
.gl-panel {{ border: {mid}px solid {OUTLINE}; border-radius: {r}px; padding: {gm}px; }}
.gl-panel.gl-anim {{ border-color: transparent; }}
.gl-panel-outline {{ all: unset; border: {mid}px solid {OUTLINE}; border-radius: {r}px; }}
.gl-icon-slot {{ min-width: {irow}px; min-height: {irow}px; padding: {gs}px; }}
.gl-title {{ font-size: {tt}px; color: {FG}; }}
.gl-rule {{ min-height: {hair}px; background-color: {RULE}; }}
.gl-rule.inset {{ margin-left: {gxl}px; margin-right: {gxl}px; }}
.gl-icon-button {{ all: unset; color: {FG}; padding: {gs}px; min-width: {irow}px; min-height: {irow}px; border-radius: {r}px; -gtk-icon-size: {irow}px; }}
.gl-icon-button:hover {{ background-color: {HOVER}; }}
.gl-icon-button:active {{ background-color: {ACTIVE}; }}
.gl-icon-button:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-icon-button.nav {{ padding: 0 {gs}px; min-width: {inav}px; min-height: {ch}px; border-radius: {ri}px; -gtk-icon-size: {inav}px; margin-right: {gl}px; }}
.gl-icon-button.searchbtn {{ padding: 0 {gs}px; min-width: {isearch}px; min-height: {ch}px; border-radius: {ri}px; -gtk-icon-size: {isearch}px; }}
.gl-icon-button.winbtn {{ padding: 0 {gs}px; min-width: {iwin}px; min-height: {iwin}px; border-radius: {ri}px; -gtk-icon-size: {iwin}px; margin-left: {gl}px; }}
.gl-icon-button.float {{ -gtk-icon-palette: {float_palette}; }}
.gl-icon-button.float image {{ -gtk-icon-size: {iwin}px; }}
.gl-icon-button.close image {{ -gtk-icon-size: {iwin}px; }}
.gl-search {{ margin-right: {gl}px; }}
.gl-strip {{ margin-right: {gxl}px; }}
.gl-field {{ min-height: {pill_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; }}
.gl-field:focus-within {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-field .gl-icon-button {{ padding: 0; min-width: {inav}px; min-height: {inav}px; border-radius: {ri}px; -gtk-icon-size: {inav}px; }}
.gl-field label {{ font-size: {tc}px; }}
.gl-field entry {{ all: unset; padding: 0; background-color: transparent; color: {FG}; caret-color: {FG}; font-size: {tc}px; }}
.gl-field entry > text > selection {{ background-color: {SELECTION}; color: {BG}; }}
.gl-field entry > text > placeholder {{ color: {PLACEHOLDER}; }}
.gl-search-field {{ min-width: {search_w}px; }}
.gl-search-field entry {{ min-width: {search_entry_w}px; }}
.gl-search-glyph {{ -gtk-icon-size: {inav}px; margin-right: {gm}px; color: {FG}; }}
.gl-path-label {{ font-size: {tc}px; }}
.gl-path.invalid {{ border-color: @error_bg_color; }}
.gl-pill {{ all: unset; min-height: {pill_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {rp}px; background-color: {pill_bg}; color: {FG}; transition: background-color {hover_ms}ms; }}
.gl-pill.tab, .gl-pill.group {{ min-width: {pill_w}px; }}
.gl-pill.active {{ background-color: {pill_active}; color: {RAISED_FG}; }}
.gl-pill.selected {{ background-color: {pill_selected}; }}
.gl-pill.active.selected {{ background-color: {pill_selected_active}; }}
.gl-pill label {{ font-size: {tc}px; }}
.gl-pill.tab {{ margin-bottom: {gs}px; }}
.gl-pill.tile {{ min-width: 0; }}
.gl-pill.tile.wide {{ min-width: {pill_w}px; }}
.gl-pill-close {{ all: unset; min-width: {close_inner}px; min-height: {close_inner}px; padding: 0; -gtk-icon-size: {close_inner}px; border: {thin}px solid {FG}; border-radius: {rp}px; color: {FG}; margin-left: {gm}px; }}
.gl-pill-close:hover {{ background-color: {HOVER_STRONG}; }}
.gl-pill-close:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pill.active .gl-pill-close {{ color: {RAISED_FG}; border-color: {RAISED_FG}; }}
.gl-pill-wrap {{ transition: transform {slide_ms}ms; }}
.gl-pill-wrap.slide-left {{ transform: translateX(-{pitch}px); }}
.gl-pill-wrap.slide-right {{ transform: translateX({pitch}px); }}
.gl-section {{ margin-top: {section_gap}px; }}
.gl-section-pill {{ all: unset; min-height: {sect_h}px; margin: 0 {gm}px; border: {thin}px solid {OUTLINE}; border-radius: {rp}px; }}
.gl-section-content {{ margin: {gs}px 0 {section_tail}px 0; }}
.gl-section:last-child .gl-section-content {{ margin-bottom: 0; }}
.gl-section-pill label {{ font-size: {titem}px; }}
.gl-section-pill:hover {{ background-color: {HOVER}; }}
.gl-section-pill:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-pinned {{ color: {PINNED_FG}; border-radius: {r}px; margin-top: {gs}px; }}
.gl-pinned-row {{ all: unset; min-height: {pinrow}px; padding: 0 {gs}px; }}
.gl-pinned-row image {{ -gtk-icon-size: {ipin}px; }}
.gl-pinned-row label {{ font-size: {titem}px; margin-left: {gs}px; }}
.gl-pinned-row.sel {{ background-color: {pin_sel}; }}
.gl-pin-empty {{ min-height: {pinrow}px; }}
.gl-dir-row {{ min-height: {dirrow}px; }}
.gl-dir-row label {{ font-size: {titem}px; }}
.gl-drive-name {{ font-size: {titem}px; }}
.gl-drive-block {{ margin-bottom: {gs}px; }}
.gl-drive-track {{ min-width: {barw}px; min-height: {barh}px; background-color: {track}; }}
.gl-drive-track.main {{ min-width: {mbarw}px; min-height: {mbarh}px; }}
.gl-drive-fill {{ min-height: {barh}px; }}
.gl-drive-fill.main {{ min-height: {mbarh}px; }}
{bars}
.gl-drive-label {{ font-size: {ttiny}px; }}
.gl-drive-cap {{ font-size: {ttiny}px; margin-left: {gs}px; }}
.gl-main-label {{ font-size: {tsmall}px; }}
.gl-grid {{ border-spacing: {gxl}px; margin: {g2xl}px {gmar}px 0 {gmar}px; background-color: transparent; }}
.gl-grid.tile {{ margin: {gm}px; }}
gridview > child {{ padding: 0; }}
.gl-cell {{ min-width: {cellw}px; min-height: {cellh}px; }}
.gl-cell image {{ -gtk-icon-size: {igrid}px; color: {FG}; -gtk-icon-palette: {folder_palette}; }}
.gl-cell label {{ font-size: {trow}px; }}
.gl-tile {{ background-color: {tile}; border: {thin}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; }}
.gl-tile.focused {{ box-shadow: inset 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-drop-line {{ background-color: {OUTLINE}; min-height: {thin}px; border-radius: {rp}px; }}
.gl-drop-target {{ box-shadow: inset 0 0 0 {thin}px {FOCUS_RING}; }}
dnd .gl-drag-badge, .gl-drag-badge {{ min-width: {tsmall}px; min-height: {tsmall}px; padding: {thin}px {gs}px; border-radius: {rp}px; border: {thin}px solid {OUTLINE}; background-color: {BG}; color: {FG}; font-size: {tsmall}px; font-weight: bold; }}
.gl-status {{ min-height: {statush}px; font-size: {tc}px; }}
.gl-progress {{ padding: {gs}px {gl}px; }}
.gl-progress-label {{ font-size: {tsmall}px; color: alpha({FG}, 0.75); }}
.gl-progress-bar trough {{ min-height: {thin}px; border-radius: {rp}px; background-color: {track}; }}
.gl-progress-bar progress {{ min-height: {thin}px; border-radius: {rp}px; background-color: {OUTLINE}; }}
.gl-conflict-apply {{ margin-top: {gm}px; font-size: {trow}px; }}
.gl-num {{ font-size: {tmono}px; }}
.gl-pline {{ font-size: {tmono}px; margin-left: {gm}px; }}
.gl-action-row .gl-icon-button {{ margin-left: {gm}px; }}
.gl-action-row image {{ color: {FG}; -gtk-icon-palette: {save_as_palette}; }}
.gl-trash {{ all: unset; min-width: {trash_inner}px; min-height: {trash_inner}px; -gtk-icon-size: {iwin}px; border: {thin}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; margin-left: {gl}px; }}
.gl-trash:hover {{ background-color: {HOVER}; }}
.gl-trash:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-menu {{ padding: {gs}px; }}
.gl-menu-item {{ all: unset; min-height: {g2xl}px; padding: {gs}px {gm}px; border-radius: {ri}px; color: {FG}; font-size: {trow}px; }}
.gl-menu-item:hover {{ background-color: {HOVER}; }}
.gl-menu-item:disabled {{ color: alpha({FG}, 0.45); }}
.gl-menu-item:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-trash-view {{ padding: {gm}px; color: {FG}; }}
.gl-trash-actions {{ min-height: {ch}px; margin-bottom: {gm}px; }}
.gl-trash-title {{ font-size: {tt}px; color: {FG}; }}
.gl-trash-action {{ min-height: {ch}px; margin-left: {gs}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {r}px; background-color: {pill_bg}; color: {FG}; font-size: {tc}px; }}
.gl-trash-action:hover {{ background-color: {HOVER}; }}
.gl-trash-action:active {{ background-color: {ACTIVE}; }}
.gl-trash-action:disabled {{ color: alpha({FG}, 0.45); }}
.gl-trash-action:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-trash-list {{ background-color: transparent; }}
.gl-trash-row {{ min-height: {dirrow}px; padding: {gs}px {gm}px; }}
.gl-trash-icon {{ -gtk-icon-size: {igrid}px; margin-right: {gm}px; color: {FG}; -gtk-icon-palette: {folder_palette}; }}
.gl-trash-name {{ font-size: {titem}px; color: {FG}; }}
.gl-trash-meta {{ font-size: {tsmall}px; color: alpha({FG}, 0.75); }}
.gl-trash-empty {{ font-size: {tt}px; color: alpha({FG}, 0.75); }}
.gl-trash-status {{ min-height: {statush}px; padding: 0 {gm}px; font-size: {tsmall}px; color: alpha({FG}, 0.75); }}
.gl-trash-progress {{ border-top: {hair}px solid {RULE}; }}
window.gimbal-look .gl-frame gridview, window.gimbal-look .gl-frame listview, window.gimbal-look .gl-frame textview, window.gimbal-look .gl-frame textview > text, window.gimbal-look .gl-frame scrolledwindow, window.gimbal-look .gl-frame viewport, window.gimbal-look .gl-frame gridview > child, window.gimbal-look windowhandle {{ background-color: transparent; }}
window.gimbal-look .gl-frame gridview > child:selected {{ background-color: alpha(@accent_bg_color, 0.45); border-radius: {r}px; }}",
        pinrow = p(PIN_ROW_H),
        dirrow = p(DIR_ROW_H),
        barw = p(BAR_W),
        barh = p(BAR_H),
        mbarw = p(MAIN_BAR_W),
        mbarh = p(MAIN_BAR_H),
        cellw = p(CELL_W),
        cellh = p(CELL_H),
        gmar = p(GRID_MARGIN),
        statush = p(STATUS_H),
        bars = bar_css(o),
        folder_palette = FOLDER_PALETTE,
        save_as_palette = SAVE_AS_PALETTE,
        float_palette = FLOAT_PALETTE,
    )
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

/// Every colour expression lives in the one table (the drive-bar levels).
fn bar_css(opacity: f64) -> String {
    let o = opacity.clamp(0.0, 1.0);
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

/// Where the header's path bar stretches and when the pills must collapse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub fits: bool,
    pub path_width: i32,
}

pub fn header_fits(available: i32, pills: usize, scale: f64) -> Fit {
    let p = |token: f64| scaled(token, scale);
    let insets = 2 * p(HEADER_INSET);
    let nav = 4 * (p(ICON_NAV) + 2 * p(GAP_S) + p(GAP_L));
    let search = p(ICON_SEARCH) + 2 * p(GAP_S) + p(GAP_L);
    let right = p(TRASH) + p(GAP_L) + 2 * (p(ICON_WINDOW) + 2 * p(GAP_S)) + 2 * p(GAP_L);
    let strip = if pills == 0 {
        0
    } else {
        pills as i32 * p(PILL_W) + (pills as i32 - 1) * p(GAP_L)
    };
    let free = available - insets - nav - search - right - p(GAP_XL) - strip;
    let fits = free >= p(PATH_MIN);
    Fit {
        fits,
        path_width: if fits { free } else { p(PATH_MIN) },
    }
}

/// Where a dragged pill lands and how far every other pill slides.
#[derive(Clone, Debug, PartialEq)]
pub struct DragLayout {
    pub to: usize,
    pub offset: f64,
    pub slides: Vec<f64>,
}

pub fn drag_layout(from: usize, dx: f64, pitch: f64, count: usize) -> DragLayout {
    if count == 0 || from >= count || pitch <= 0.0 {
        return DragLayout {
            to: from,
            offset: 0.0,
            slides: Vec::new(),
        };
    }
    let left = from as f64 * pitch;
    let offset = dx.clamp(-left, (count - 1) as f64 * pitch - left);
    let to = (from as f64 + (offset / pitch).round()).clamp(0.0, (count - 1) as f64) as usize;
    let slide = if to > from {
        -pitch
    } else if to < from {
        pitch
    } else {
        0.0
    };
    let slides = (0..count)
        .map(|index| {
            let crossed = (from < index && index <= to) || (to <= index && index < from);
            if crossed { slide } else { 0.0 }
        })
        .collect();
    DragLayout { to, offset, slides }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: f64 = 0.85;

    #[test]
    fn the_focused_tile_ring_is_drawn_inside_the_frame() {
        // Claude's P2j review: each tile sits in a clipping scroller, so an
        // outer box-shadow ring was cut off and the focused tile looked like
        // the others.
        let plain = css(1.0, None, PLAIN);
        assert!(plain.contains(".gl-tile.focused { box-shadow: inset 0 0 0 2px @accent_color; }"));
    }

    #[test]
    fn a_selected_grid_cell_is_filled_after_the_transparent_rule() {
        // Claude's P2i review: the transparent `gridview > child` rule hid
        // the theme's selection, so selected files looked unselected.
        let plain = css(1.0, None, PLAIN);
        let transparent = plain
            .find("window.gimbal-look .gl-frame gridview > child, ")
            .expect("the transparent rule");
        let selected = plain
            .find("window.gimbal-look .gl-frame gridview > child:selected { background-color: alpha(@accent_bg_color, 0.45);")
            .expect("a selected cell is a translucent accent box (Kyle, Nov 2)");
        // The icon and the name keep their normal colours (Kyle, Nov 2).
        assert!(!plain.contains("child:selected .gl-cell"));
        assert!(selected > transparent, "the selected rule must come later");
    }

    #[test]
    fn the_drag_badge_is_bold_contrasting_and_reaches_the_drag_icon_root() {
        let plain = css(1.0, None, PLAIN);
        let rule = plain
            .lines()
            .find(|line| line.contains(".gl-drag-badge"))
            .expect("the drag badge rule");
        // The DragIcon is a separate root (CSS node "dnd"), so the rule must
        // not depend on a `window.gimbal-look` ancestor.
        assert!(
            rule.starts_with("dnd .gl-drag-badge, .gl-drag-badge {"),
            "{rule}"
        );
        assert!(
            rule.contains("background-color: @window_bg_color;"),
            "{rule}"
        );
        assert!(rule.contains("color: @window_fg_color;"), "{rule}");
        assert!(rule.contains("font-weight: bold;"), "{rule}");
        // The small text token, not the tiny one, and token-scaled padding.
        assert!(rule.contains("font-size: 14px;"), "{rule}");
        assert!(rule.contains("padding: 2px 8px;"), "{rule}");
        assert!(rule.contains("border-radius: 999px;"), "{rule}");
        let half = css(0.5, None, PLAIN);
        let rule = half
            .lines()
            .find(|line| line.contains(".gl-drag-badge"))
            .expect("the drag badge rule");
        assert!(rule.contains("font-size: 7px;"), "{rule}");
        assert!(rule.contains("padding: 1px 4px;"), "{rule}");
    }

    #[test]
    fn spec_rules() {
        let plain = css(1.0, None, PLAIN);
        for bad in [
            "#", "rgb(", "hsl(", "red", "white", "black", "gray", "green",
        ] {
            assert!(!plain.contains(bad), "colour literal {bad:?}");
        }
        assert!(!plain.contains("font-family"));
        assert!(css(1.0, Some("Fira Sans"), PLAIN).contains("font-family: \"Fira Sans\";"));
        assert!(plain.contains("alpha(@window_bg_color, 0.85)"));
        assert!(plain.contains("alpha(mix(@card_bg_color, @window_fg_color, 0.2), 0.85)"));
        assert!(!plain.contains(".gl-panel { background-color"));
        // The header's old 10 px frame border was dropped: the Nov 1 spec has
        // no window frame, and the body's frame is an unstyled container.
        assert!(!plain.contains(".gl-frame {"));
    }

    #[test]
    fn nov_1_outlines_and_contrast() {
        let plain = css(1.0, None, PLAIN);
        assert!(plain.contains("border: 3px solid @accent_color"));
        assert!(plain.contains("border: 2px solid @window_fg_color"));
        assert!(plain.contains("mix(shade(@window_bg_color, 0.8), @accent_color, 0.5)"));
        assert!(plain.contains("mix(@card_bg_color, @accent_color, 0.5)"));
        assert!(plain.contains("0 0 0 2px @accent_color"));
        // The focus ring is the full accent; only the text selection uses
        // alpha(accent). The body test's blanket check is narrowed to the
        // focus ring, because the merged header needs the selection colour.
        assert!(!plain.contains("0 0 0 2px alpha(@accent_color"));
        assert!(plain.contains(
            ".gl-drop-line { background-color: @accent_color; min-height: 2px; border-radius: 999px; }"
        ));
    }

    #[test]
    fn pinned_list_has_no_fill_and_collapsed_pills_sit_six_apart() {
        let plain = css(1.0, None, PLAIN);
        assert!(plain.contains(".gl-pinned { color: @sidebar_fg_color"));
        assert!(!plain.contains(".gl-pinned { background-color"));
        assert!(plain.contains(".gl-pinned-row.sel { background-color:"));
        assert!(plain.contains(".gl-section { margin-top: 6px; }"));
        assert!(plain.contains(".gl-section-content { margin: 8px 0 10px 0; }"));
        assert!(plain.contains(".gl-section:last-child .gl-section-content { margin-bottom: 0; }"));
        assert!(plain.contains(".gl-section-pill { all: unset; min-height: 28px; margin: 0 16px;"));
    }

    #[test]
    fn icon_palettes_map_the_spec_table() {
        let plain = css(1.0, None, PLAIN);
        for entry in [
            "accent @accent_color",
            "success shade(@accent_color, 0.7)",
            "warning shade(@accent_color, 0.6)",
            "error shade(@accent_color, 0.45)",
        ] {
            assert!(plain.contains(entry), "folder palette lacks {entry:?}");
        }
        assert!(plain.contains("-gtk-icon-palette: error @window_bg_color"));
        let folder = include_str!("../../icons/gimbal-folder-symbolic.svg");
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
        let save_as = include_str!("../../icons/gimbal-save-as-symbolic.svg");
        assert!(save_as.contains("gpa:fill=\"foreground\""));
        assert!(save_as.contains("gpa:stroke=\"error\""));
        assert!(!save_as.contains("#000000") && !save_as.contains("#d4d4d4"));
    }

    #[test]
    fn opacity_follows_the_config_range() {
        // The config allows 0.0–1.0, so the stylesheet does too. The old
        // spike clamped at 0.50; that would disagree with the config.
        assert!(css(1.0, None, 0.2).contains("alpha(@window_bg_color, 0.20)"));
        assert!(css(1.0, None, 0.0).contains("alpha(@window_bg_color, 0.00)"));
        assert!(css(1.0, None, 1.5).contains("alpha(@window_bg_color, 1.00)"));
    }

    #[test]
    fn scaling() {
        assert_eq!(scaled(GAP_L, 0.5), 12);
        assert_eq!(scaled(LINE_HAIR, 0.5), 1);
        assert_eq!(scaled(GAP_2XL, 1.0), 40);
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

    #[test]
    fn spec_rules_and_css_shape() {
        let plain = css(1.0, None, PLAIN);
        for bad in ["#", "rgb(", "hsl(", "red", "white", "black"] {
            assert!(!plain.contains(bad), "colour literal {bad:?}");
        }
        assert!(!plain.contains("font-family"));
        assert!(css(1.0, Some("Fira Sans"), PLAIN).contains("font-family: \"Fira Sans\";"));
        for line in plain.lines() {
            assert!(
                line.matches('{').count() <= 1,
                "two rules on one line: {line}"
            );
        }
        assert!(plain.contains("alpha(@window_bg_color, 0.85)"));
        assert!(!plain.contains(".gl-header { background-color"));
        assert!(plain.contains("border: 3px solid @accent_color"));
        assert!(plain.contains("border: 2px solid @accent_color"));
        assert!(plain.contains("-gtk-icon-size: 32px"));
        assert!(plain.contains("mix(shade(@window_bg_color, 0.8), @accent_color, 0.5)"));
        assert!(plain.contains("box-shadow: 0 0 0 2px @accent_color"));
        assert!(!plain.contains("alpha(@accent_color, 0.5)"));
    }

    #[test]
    fn float_icon_is_symbolic_and_maps_the_front_fill() {
        let svg = include_str!("../../icons/float.svg");
        assert!(svg.contains("viewBox=\"0 0 28.08 25.12\""));
        assert!(svg.contains("id=\"back-window\"") && svg.contains("id=\"front-window\""));
        assert_eq!(svg.matches("d=\"M ").count(), 2);
        assert_eq!(svg.matches("gpa:stroke=\"foreground\"").count(), 2);
        assert_eq!(svg.matches("gpa:fill=").count(), 1);
        assert!(svg.contains("gpa:fill=\"error\""));
        assert!(!svg.contains('#'));
        assert!(!svg.contains("rgb("));
        let full = css(1.0, None, PLAIN);
        assert!(
            full.contains(".gl-icon-button.float { -gtk-icon-palette: error @window_bg_color; }")
        );
        assert!(full.contains(".gl-icon-button.float image { -gtk-icon-size: 32px; }"));
        let half = css(0.5, None, PLAIN);
        assert!(half.contains(".gl-icon-button.float image { -gtk-icon-size: 16px; }"));
    }

    #[test]
    fn close_icon_is_symbolic_with_foreground_strokes() {
        let svg = include_str!("../../icons/close.svg");
        assert!(svg.contains("viewBox=\"0 0 28.08 28.08\""));
        assert!(svg.contains("id=\"stroke-1\"") && svg.contains("id=\"stroke-2\""));
        assert_eq!(svg.matches("d=\"M ").count(), 2);
        assert_eq!(svg.matches("gpa:stroke=\"foreground\"").count(), 2);
        assert!(!svg.contains("gpa:fill="));
        assert!(!svg.contains('#'));
        assert!(!svg.contains("rgb("));
        let full = css(1.0, None, PLAIN);
        assert!(full.contains(".gl-icon-button.close image { -gtk-icon-size: 32px; }"));
        assert!(!full.contains(".gl-icon-button.close { -gtk-icon-palette"));
        let half = css(0.5, None, PLAIN);
        assert!(half.contains(".gl-icon-button.close image { -gtk-icon-size: 16px; }"));
    }

    #[test]
    fn the_trash_browser_uses_the_shared_tokens() {
        let plain = css(1.0, None, PLAIN);
        for rule in [
            ".gl-trash-view {",
            ".gl-trash-actions {",
            ".gl-trash-row { min-height: 32px;",
            ".gl-trash-name { font-size: 22px;",
            ".gl-trash-meta { font-size: 14px;",
            ".gl-trash-status { min-height: 64px;",
        ] {
            assert!(plain.contains(rule), "missing {rule:?}");
        }
        for line in plain.lines().filter(|line| line.contains(".gl-trash")) {
            assert!(
                !line.contains('#') && !line.contains("rgb("),
                "a fixed colour in {line}"
            );
        }
    }

    #[test]
    fn header_fits_stretches_then_collapses() {
        let wide = header_fits(2560, 2, 1.0);
        assert!(
            wide.fits && wide.path_width > 960,
            "the path must stretch past 960: {wide:?}"
        );
        for available in 1540..=2560 {
            let fit = header_fits(available, 2, 1.0);
            assert!(fit.fits && fit.path_width >= 320, "at {available}: {fit:?}");
        }
        assert_eq!(
            header_fits(1600, 2, 1.0),
            Fit {
                fits: true,
                path_width: 380
            }
        );
        assert_eq!(
            header_fits(1540, 2, 1.0),
            Fit {
                fits: true,
                path_width: 320
            }
        );
        let collapsed = header_fits(1539, 2, 1.0);
        assert!(
            !collapsed.fits && collapsed.path_width == 320,
            "{collapsed:?}"
        );
        assert_eq!(
            header_fits(800, 2, 0.5),
            Fit {
                fits: true,
                path_width: 190
            }
        );
        assert_eq!(
            header_fits(770, 2, 0.5),
            Fit {
                fits: true,
                path_width: 160
            }
        );
        assert!(!header_fits(769, 2, 0.5).fits);
        assert!(header_fits(1600, 2, 0.5).path_width > 480);
    }

    #[test]
    fn drag_arithmetic() {
        let pitch = 294.0;
        let right = drag_layout(1, 1.2 * pitch, pitch, 4);
        assert_eq!((right.to, right.offset), (2, 1.2 * pitch));
        assert_eq!(right.slides, [0.0, 0.0, -pitch, 0.0]);
        let left = drag_layout(2, -0.6 * pitch, pitch, 4);
        assert_eq!(left.to, 1);
        assert_eq!(left.slides, [0.0, pitch, 0.0, 0.0]);
        let still = drag_layout(1, 0.49 * pitch, pitch, 4);
        assert_eq!((still.to, still.slides.clone()), (1, vec![0.0; 4]));
        let edge = drag_layout(0, -2.0 * pitch, pitch, 3);
        assert_eq!((edge.to, edge.offset, edge.slides), (0, 0.0, vec![0.0; 3]));
        let edge = drag_layout(2, 4.0 * pitch, pitch, 3);
        assert_eq!((edge.to, edge.offset, edge.slides), (2, 0.0, vec![0.0; 3]));
        assert_eq!(drag_layout(0, 5.0, pitch, 1).to, 0);
        assert!(drag_layout(0, 5.0, pitch, 0).slides.is_empty());
    }
}
