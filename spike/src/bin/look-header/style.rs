// SPDX-License-Identifier: GPL-3.0-or-later
//! Tokens, colour roles, the header fit calculation, the pill-drag arithmetic and
//! the generated stylesheet.
#[rustfmt::skip]
pub mod tok {
    pub const GAP_S: f64 = 8.0; pub const GAP_M: f64 = 16.0; pub const GAP_L: f64 = 24.0; pub const GAP_XL: f64 = 32.0; pub const GAP_2XL: f64 = 40.0;
    pub const CONTROL_H: f64 = 50.0; pub const HEADER_H: f64 = 96.0; pub const HEADER_INSET: f64 = 24.0;
    pub const LINE_THIN: f64 = 3.0; pub const LINE_MID: f64 = 6.0; pub const LINE_HEAVY: f64 = 10.0;
    pub const RADIUS: f64 = 12.0; pub const RADIUS_PILL: f64 = 999.0; pub const RADIUS_ICON: f64 = 4.0;
    pub const TEXT_TITLE: f64 = 32.0; pub const TEXT_CONTROL: f64 = 24.0; pub const TEXT_ROW: f64 = 20.0;
    pub const ICON_NAV: f64 = 32.0; pub const ICON_SEARCH: f64 = 44.0; pub const ICON_WINDOW: f64 = 24.0; pub const ICON_TAB_CLOSE: f64 = 28.0;
    pub const PILL_W: f64 = 270.0; pub const SEARCH_W: f64 = 496.0; pub const PATH_MIN: f64 = 320.0;
    pub const TRASH: f64 = 48.0; pub const DEFAULT_W: i32 = 2560; pub const DEFAULT_H: i32 = 1440;
}
pub use tok::*;

#[rustfmt::skip]
pub mod col {
    pub const BG: &str = "@window_bg_color"; pub const FG: &str = "@window_fg_color"; pub const OUTLINE: &str = "@accent_color";
    pub const RAISED: &str = "@card_bg_color"; pub const RAISED_FG: &str = "@card_fg_color";
    pub const RECESSED: &str = "shade(@window_bg_color, 0.8)";
    pub const SELECTED: &str = "mix(shade(@window_bg_color, 0.8), @accent_color, 0.3)";
    pub const SELECTED_ACTIVE: &str = "mix(@card_bg_color, @accent_color, 0.3)"; pub const FOCUS_RING: &str = "alpha(@accent_color, 0.5)";
    pub const HOVER: &str = "alpha(@window_fg_color, 0.1)"; pub const ACTIVE: &str = "alpha(@window_fg_color, 0.2)";
    pub const HOVER_STRONG: &str = "alpha(@window_fg_color, 0.15)"; pub const SELECTION: &str = "alpha(@accent_color, 0.35)";
    pub const PLACEHOLDER: &str = "alpha(@window_fg_color, 0.5)";
}
pub use col::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fit {
    pub fits: bool,
    pub path_width: i32,
}

/// Where a dragged pill lands and how far every other pill slides to fill the hole.
#[derive(Clone, Debug, PartialEq)]
pub struct DragLayout {
    /// The slot the dragged pill lands in.
    pub to: usize,
    /// The dragged pill's offset from its own slot, clamped so it cannot leave the strip.
    pub offset: f64,
    /// One offset per pill: the dragged pill is 0, the pills it passes slide by one pitch
    /// towards the slot it left.
    pub slides: Vec<f64>,
}

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

/// Whether every pill still fits, and how much width the free space would give the
/// path bar. The header applies only `fits`; the path bar itself is `hexpand` with a
/// constant `PATH_MIN × scale` request, so nothing measured becomes a minimum.
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

/// The reorder arithmetic behind a pill drag: which slot `from` lands in for a given
/// horizontal offset, and where the pills it passes go.
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

/// One CSS rule per line, so Kyle and Claude can read and diff it live.
pub fn css(scale: f64, font: Option<&str>, opacity: f64) -> String {
    let p = |token: f64| scaled(token, scale);
    let font = font
        .map(|family| format!("font-family: \"{}\";", family.replace(['"', '\\', ';'], "")))
        .unwrap_or_default();
    let o = opacity.clamp(0.50, 1.00);
    let fill = |role: &str| format!("alpha({role}, {o:.2})");
    let (gs, gm, gl, gxl, g2xl) = (p(GAP_S), p(GAP_M), p(GAP_L), p(GAP_XL), p(GAP_2XL));
    let (ch, hh, inset) = (p(CONTROL_H), p(HEADER_H), p(HEADER_INSET));
    let (thin, mid, heavy) = (p(LINE_THIN), p(LINE_MID), p(LINE_HEAVY));
    let (r, rp, ri) = (p(RADIUS), p(RADIUS_PILL), p(RADIUS_ICON));
    let (tc, tt, trow) = (p(TEXT_CONTROL), p(TEXT_TITLE), p(TEXT_ROW));
    let (inav, isearch, iwin, itc) = (
        p(ICON_NAV),
        p(ICON_SEARCH),
        p(ICON_WINDOW),
        p(ICON_TAB_CLOSE),
    );
    // GTK's min-width/min-height are content-box, so subtract the outline and padding
    // here: the outer pill is PILL_W × control-h, matching header_fits.
    let pill_w = (p(PILL_W) - 2 * gm - 2 * mid).max(1);
    let inner_h = (ch - 2 * mid).max(1);
    let search_w = (p(SEARCH_W) - 2 * gm - 2 * mid).max(1);
    let trash_inner = (p(TRASH) - 2 * thin).max(1);
    let close_inner = (itc - 2 * thin).max(1);
    let pitch = p(PILL_W) + p(GAP_L);
    let window_bg = fill(BG);
    let pill_bg = fill(RECESSED);
    let pill_active = fill(RAISED);
    let pill_selected = fill(SELECTED);
    let pill_selected_active = fill(SELECTED_ACTIVE);
    format!(
        "\
window.gimbal-look, window.gimbal-look.background {{ background-color: {window_bg}; color: {FG}; font-size: {tc}px; {font} }}
.gl-header {{ min-height: {hh}px; padding: 0 {inset}px; }}
.gl-icon-button {{ all: unset; color: {FG}; padding: 0 {gs}px; min-width: {inav}px; min-height: {ch}px; border-radius: {ri}px; -gtk-icon-size: {inav}px; }}
.gl-icon-button:hover {{ background-color: {HOVER}; }}
.gl-icon-button:active {{ background-color: {ACTIVE}; }}
.gl-icon-button:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-icon-button.nav {{ margin-right: {gl}px; }}
.gl-icon-button.searchbtn {{ min-width: {isearch}px; -gtk-icon-size: {isearch}px; }}
.gl-icon-button.winbtn {{ min-width: {iwin}px; min-height: {iwin}px; -gtk-icon-size: {iwin}px; margin-left: {gl}px; }}
.gl-search {{ margin-right: {gl}px; }}
.gl-strip {{ margin-right: {gxl}px; }}
.gl-field .gl-icon-button {{ min-width: {inav}px; min-height: {inav}px; -gtk-icon-size: {inav}px; }}
.gl-field {{ min-height: {inner_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; }}
.gl-field:focus-within {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-field label {{ font-size: {tc}px; }}
.gl-field entry {{ all: unset; padding: 0; background-color: transparent; color: {FG}; caret-color: {FG}; font-size: {tc}px; }}
.gl-field entry > text > selection {{ background-color: {SELECTION}; color: {BG}; }}
.gl-field entry > text > placeholder {{ color: {PLACEHOLDER}; }}
.gl-search-field {{ min-width: {search_w}px; }}
.gl-search-glyph {{ -gtk-icon-size: {inav}px; margin-right: {gm}px; color: {FG}; }}
.gl-path-label {{ font-size: {tc}px; }}
.gl-pill {{ all: unset; min-height: {inner_h}px; padding: 0 {gm}px; border: {mid}px solid {OUTLINE}; border-radius: {rp}px; background-color: {pill_bg}; color: {FG}; transition: background-color 120ms; }}
.gl-pill.tab, .gl-pill.group {{ min-width: {pill_w}px; }}
.gl-pill.active {{ background-color: {pill_active}; color: {RAISED_FG}; }}
.gl-pill.selected {{ background-color: {pill_selected}; }}
.gl-pill.active.selected {{ background-color: {pill_selected_active}; }}
.gl-pill-wrap {{ transition: transform 150ms; }}
.gl-pill-wrap.slide-left {{ transform: translateX(-{pitch}px); }}
.gl-pill-wrap.slide-right {{ transform: translateX({pitch}px); }}
.gl-pill label {{ font-size: {tc}px; }}
.gl-pill-close {{ all: unset; min-width: {close_inner}px; min-height: {close_inner}px; -gtk-icon-size: {close_inner}px; border: {thin}px solid {FG}; border-radius: {rp}px; color: {FG}; margin-left: {gm}px; }}
.gl-pill-close:hover {{ background-color: {HOVER_STRONG}; }}
.gl-pill.active .gl-pill-close {{ color: {RAISED_FG}; border-color: {RAISED_FG}; }}
.gl-pill-close:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-frame {{ margin: 0 {gs}px {gs}px {gs}px; border: {heavy}px solid {OUTLINE}; border-radius: {r}px; }}
.gl-title {{ font-size: {tt}px; color: {FG}; }}
.gl-trash {{ all: unset; min-width: {trash_inner}px; min-height: {trash_inner}px; -gtk-icon-size: {iwin}px; border: {thin}px solid {OUTLINE}; border-radius: {r}px; color: {FG}; margin-left: {gl}px; }}
.gl-trash:hover {{ background-color: {HOVER}; }}
.gl-trash:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
.gl-menu {{ padding: {gs}px; }}
.gl-menu-item {{ all: unset; min-height: {g2xl}px; padding: {gs}px {gm}px; border-radius: {ri}px; color: {FG}; font-size: {trow}px; }}
.gl-menu-item:hover {{ background-color: {HOVER}; }}
.gl-menu-item:disabled {{ color: alpha({FG}, 0.45); }}
.gl-menu-item:focus-visible {{ box-shadow: 0 0 0 {thin}px {FOCUS_RING}; }}
window.gimbal-look .gl-frame gridview, window.gimbal-look .gl-frame listview, window.gimbal-look .gl-frame textview, window.gimbal-look .gl-frame textview > text, window.gimbal-look .gl-frame scrolledwindow, window.gimbal-look .gl-frame viewport, window.gimbal-look .gl-frame gridview > child, window.gimbal-look windowhandle {{ background-color: transparent; }}
"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_rules_and_css_shape() {
        let plain = css(1.0, None, DEFAULT_OPACITY);
        for bad in ["#", "rgb(", "hsl(", "red", "white", "black"] {
            assert!(!plain.contains(bad), "colour literal {bad:?}");
        }
        assert!(!plain.contains("font-family"));
        assert!(
            css(1.0, Some("Fira Sans"), DEFAULT_OPACITY).contains("font-family: \"Fira Sans\";")
        );
        assert_eq!(scaled(GAP_L, 0.5), 12);
        for line in plain.lines() {
            assert!(
                line.matches('{').count() <= 1,
                "two rules on one line: {line}"
            );
        }
        assert!(plain.contains("alpha(@window_bg_color, 0.85)"));
        assert!(!plain.contains(".gl-header { background-color"));
    }

    #[test]
    fn opacity_parsing_falls_back() {
        assert_eq!(parse_opacity("0.50"), Some(0.50));
        assert_eq!(parse_opacity("1"), Some(1.0));
        assert_eq!(parse_opacity(" 0.85 "), Some(0.85));
        for bad in ["0.49", "1.01", "x", "", "NaN", "inf"] {
            assert_eq!(parse_opacity(bad), None, "{bad:?} should be invalid");
        }
        // The stylesheet also clamps a value that bypasses the parser (the
        // live slider and the direct callers).
        assert!(css(1.0, None, 0.2).contains("alpha(@window_bg_color, 0.50)"));
        assert!(css(1.0, None, 1.5).contains("alpha(@window_bg_color, 1.00)"));
    }

    #[test]
    fn header_fits_stretches_then_collapses() {
        let wide = header_fits(2560, 2, 1.0);
        assert!(
            wide.fits && wide.path_width > 960,
            "the path must stretch past 960: {wide:?}"
        );
        for available in 1536..=2560 {
            let fit = header_fits(available, 2, 1.0);
            assert!(fit.fits && fit.path_width >= 320, "at {available}: {fit:?}");
        }
        assert_eq!(
            header_fits(1600, 2, 1.0),
            Fit {
                fits: true,
                path_width: 384
            }
        );
        assert_eq!(
            header_fits(1536, 2, 1.0),
            Fit {
                fits: true,
                path_width: 320
            }
        );
        let collapsed = header_fits(1535, 2, 1.0);
        assert!(
            !collapsed.fits && collapsed.path_width == 320,
            "{collapsed:?}"
        );
        assert_eq!(
            header_fits(800, 2, 0.5),
            Fit {
                fits: true,
                path_width: 192
            }
        );
        assert_eq!(
            header_fits(768, 2, 0.5),
            Fit {
                fits: true,
                path_width: 160
            }
        );
        assert!(!header_fits(767, 2, 0.5).fits);
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
