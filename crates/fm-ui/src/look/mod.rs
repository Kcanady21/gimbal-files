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

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use css::css_with_mono;

pub struct Look {
    provider: gtk::CssProvider,
    /// The watcher owns its monitors; dropping this value stops them.
    _theme: theme::Watcher,
    scale: Cell<f64>,
    opacity: Cell<f64>,
    font: Option<String>,
    mono: RefCell<Option<String>>,
    /// The desktop's monospace setting. Dropping the settings object and its
    /// handler removes the watcher.
    mono_settings: RefCell<Option<gio::Settings>>,
    mono_handler: RefCell<Option<glib::SignalHandlerId>>,
}

impl Look {
    /// Install the one stylesheet provider for the display and start the
    /// theme monitor. Call once per process, before the first window.
    pub fn new(scale: f64, opacity: f64, font: Option<String>) -> Rc<Self> {
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
        let look = Rc::new(Self {
            provider,
            _theme: theme,
            scale: Cell::new(scale),
            opacity: Cell::new(opacity),
            font,
            mono: RefCell::new(None),
            mono_settings: RefCell::new(None),
            mono_handler: RefCell::new(None),
        });
        look.watch_monospace();
        look.reload();
        look
    }

    /// Regenerate the stylesheet on the same provider.
    pub fn reload(&self) {
        let css = css_with_mono(
            self.scale.get(),
            self.font.as_deref(),
            self.mono.borrow().as_deref(),
            self.opacity.get(),
        );
        self.provider.load_from_string(&css);
    }

    pub fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
        self.reload();
    }

    pub fn set_opacity(&self, opacity: f64) {
        self.opacity.set(opacity);
        self.reload();
    }

    /// Follow the desktop's `monospace-font-name` setting. The generic family
    /// is only the fallback when the schema is absent.
    fn watch_monospace(self: &Rc<Self>) {
        let Some(source) = gio::SettingsSchemaSource::default() else {
            return;
        };
        if source.lookup("org.gnome.desktop.interface", true).is_none() {
            crate::log!(
                "look",
                "no org.gnome.desktop.interface schema; monospace uses the generic family"
            );
            return;
        }
        let settings = gio::Settings::new("org.gnome.desktop.interface");
        *self.mono.borrow_mut() = monospace_family(&settings);
        let weak: Weak<Self> = Rc::downgrade(self);
        let handler = settings.connect_changed(Some("monospace-font-name"), move |settings, _| {
            let Some(look) = weak.upgrade() else {
                return;
            };
            *look.mono.borrow_mut() = monospace_family(settings);
            look.reload();
        });
        *self.mono_handler.borrow_mut() = Some(handler);
        *self.mono_settings.borrow_mut() = Some(settings);
    }
}

fn monospace_family(settings: &gio::Settings) -> Option<String> {
    let name = settings.string("monospace-font-name");
    let description = gtk::pango::FontDescription::from_string(name.as_str());
    description.family().map(|family| family.to_string())
}
