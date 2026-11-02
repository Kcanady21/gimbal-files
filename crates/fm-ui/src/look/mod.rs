// SPDX-License-Identifier: GPL-3.0-or-later
//! One shared look: tokens, generated stylesheet, live theme, opacity and
//! motion. The process owns one `Look`; each window owns its own blur.
//! No other module builds CSS.

pub mod blur;
pub mod css;
pub mod icons;
pub mod motion;
pub mod theme;
pub mod tokens;

use css::css;

pub struct Look {
    provider: gtk::CssProvider,
    /// The watcher owns its monitors; dropping this value stops them.
    _theme: theme::Watcher,
    scale: f64,
    opacity: f64,
    font: Option<String>,
}

impl Look {
    /// Install the one stylesheet provider for the display and start the
    /// theme watcher. Call once per process, before the first window.
    pub fn new(scale: f64, opacity: f64, font: Option<String>) -> Self {
        let provider = gtk::CssProvider::new();
        if let Some(display) = gtk::gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        } else {
            crate::log!(
                "look",
                "no default display; the stylesheet is not installed"
            );
        }
        let theme = theme::Watcher::new();
        let look = Self {
            provider,
            _theme: theme,
            scale,
            opacity,
            font,
        };
        look.reload();
        look
    }

    /// Regenerate the stylesheet on the same provider.
    pub fn reload(&self) {
        let css = css(self.scale, self.font.as_deref(), self.opacity);
        self.provider.load_from_string(&css);
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        self.reload();
    }

    pub fn set_opacity(&mut self, opacity: f64) {
        self.opacity = opacity;
        self.reload();
    }
}
