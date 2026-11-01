// SPDX-License-Identifier: GPL-3.0-or-later
//! The mockup's icons as shared `gtk::Svg` paintables. Loading a
//! `-symbolic.svg` by name through the icon theme sends it down GTK's older
//! symbolic path, which ignores the `gpa:` slots and paints everything in the
//! text colour. `GtkSvg` implements `GtkSymbolicPaintable`, so an image built
//! with `Image::from_paintable` passes its CSS `color` and
//! `-gtk-icon-palette` to the SVG's slots instead
//! (gtk-4.22.4:gtk/gtkrendericon.c:97-134: the style looks the symbolic
//! colours up and calls `gtk_symbolic_paintable_snapshot_with_weight`).

use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MockupIcon {
    Folder,
    Save,
    SaveAs,
    Discard,
}

impl MockupIcon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Folder => include_bytes!("icons/gimbal-folder-symbolic.svg"),
            Self::Save => include_bytes!("icons/gimbal-save-symbolic.svg"),
            Self::SaveAs => include_bytes!("icons/gimbal-save-as-symbolic.svg"),
            Self::Discard => include_bytes!("icons/gimbal-discard-symbolic.svg"),
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
