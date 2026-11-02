// SPDX-License-Identifier: GPL-3.0-or-later
//! One shared look: tokens, generated stylesheet, live theme, opacity/blur and
//! motion. The app owns one `Look` for the process; no other module builds CSS.

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
    blur: blur::Blur,
    scale: f64,
    opacity: f64,
    font: Option<String>,
}

impl Look {
    pub fn new(window: &gtk::Window, scale: f64, opacity: f64, font: Option<String>) -> Self {
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
        let blur = blur::Blur::attach(window, scale, opacity);
        let look = Self {
            provider,
            _theme: theme,
            blur,
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
        self.blur.set_scale(scale);
    }

    pub fn set_opacity(&mut self, opacity: f64) {
        self.opacity = opacity;
        self.reload();
        self.blur.set_opacity(opacity);
    }
}
