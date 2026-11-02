// SPDX-License-Identifier: GPL-3.0-or-later
//! The mockup's six SVGs as shared `gtk::Svg` paintables.
//!
//! Loading a `-symbolic.svg` by name through the icon theme sends it down
//! GTK's older symbolic path, which ignores the `gpa:` slots and paints
//! everything in the text colour. `GtkSvg` implements `GtkSymbolicPaintable`,
//! so an image built with `Image::from_paintable` passes its CSS `color` and
//! `-gtk-icon-palette` to the SVG's slots instead
//! (gtk-4.22.4:gtk/gtkrendericon.c:97-134).
//!
//! P2h and P2m use the palettes and buttons; P2g only embeds them.
#![allow(dead_code)]

use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

/// The mockup folder's five symbolic colour slots. `foreground` (the `color`
/// property) is the paper tab; the four remappable slots carry the
/// accent-derived faces (gtk-4.22.4:gtk/gtkcssstyle.c:883-900,
/// gtk/gtkcsspalettevalue.c:298-312).
pub const FOLDER_PALETTE: &str = "accent @accent_color, success shade(@accent_color, 0.7), warning shade(@accent_color, 0.6), error shade(@accent_color, 0.45)";
/// Save As' pencil outline is the window background behind the button.
pub const SAVE_AS_PALETTE: &str = "error @window_bg_color";
/// The float icon's front-window fill hides the window behind it.
pub const FLOAT_PALETTE: &str = "error @window_bg_color";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockupIcon {
    Folder,
    Save,
    SaveAs,
    Discard,
    Float,
    Close,
}

impl MockupIcon {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Folder => include_bytes!("../../icons/gimbal-folder-symbolic.svg"),
            Self::Save => include_bytes!("../../icons/gimbal-save-symbolic.svg"),
            Self::SaveAs => include_bytes!("../../icons/gimbal-save-as-symbolic.svg"),
            Self::Discard => include_bytes!("../../icons/gimbal-discard-symbolic.svg"),
            Self::Float => include_bytes!("../../icons/float.svg"),
            Self::Close => include_bytes!("../../icons/close.svg"),
        }
    }
}

thread_local! {
    /// One `Svg` per icon, shared by every image that draws it.
    static CACHE: RefCell<Vec<(MockupIcon, gtk::Svg)>> = const { RefCell::new(Vec::new()) };
}

/// The shared paintable for `icon`; built once on the GTK main thread.
pub fn svg(icon: MockupIcon) -> gtk::Svg {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, svg)) = cache.iter().find(|(cached, _)| *cached == icon) {
            return svg.clone();
        }
        let svg = gtk::Svg::from_bytes(&glib::Bytes::from_static(icon.bytes()));
        cache.push((icon, svg.clone()));
        svg
    })
}

pub fn image(icon: MockupIcon) -> gtk::Image {
    let paintable = svg(icon);
    gtk::Image::from_paintable(Some(&paintable))
}

/// A bare `gl-icon-button` whose child is the SVG paintable.
pub fn button(icon: MockupIcon) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    button.set_child(Some(&image(icon)));
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_six_mockup_icons_are_embedded() {
        for icon in [
            MockupIcon::Folder,
            MockupIcon::Save,
            MockupIcon::SaveAs,
            MockupIcon::Discard,
            MockupIcon::Float,
            MockupIcon::Close,
        ] {
            let bytes = icon.bytes();
            assert!(!bytes.is_empty(), "{icon:?} is empty");
            let text = std::str::from_utf8(bytes).expect("SVG is UTF-8");
            assert!(text.contains("<svg"), "{icon:?} is not an SVG");
        }
    }

    #[test]
    fn palettes_stay_on_theme_colours() {
        for palette in [FOLDER_PALETTE, SAVE_AS_PALETTE, FLOAT_PALETTE] {
            assert!(!palette.contains('#'), "literal colour in {palette:?}");
            assert!(!palette.contains("rgb("), "literal colour in {palette:?}");
        }
    }
}
