// SPDX-License-Identifier: GPL-3.0-or-later
//! The preview sidebar: a read-only fake text preview with line numbers and
//! the three action buttons (which only log — editing is Phase 2+).
use crate::icons::{MockupIcon, button as icon_image_button};
use crate::sidebar::{rule, title_row};
use gtk::prelude::*;

const LINES: u32 = 40;

fn fake_line(n: u32) -> String {
    format!("Line {n}: the preview shows fake text, nothing is read from disk.")
}

pub struct PreviewPanel {
    pub root: gtk::Box,
    pub collapse_button: gtk::Button,
}

impl PreviewPanel {
    pub fn new() -> Self {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-panel");
        let (row, button) = title_row("File Name (25 MB)", "sidebar-show-right-symbolic", false);
        root.append(&row);
        root.append(&rule());
        // The lines scroll on their own; the action row stays at the bottom.
        // EXTERNAL horizontally: the viewport width, not the text's natural
        // width, allocates the labels, so their text wraps to the panel as it
        // shrinks (a Never policy would clip a wider label instead).
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        scroll.set_propagate_natural_width(false);
        scroll.set_vexpand(true);
        let lines = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for n in 1..=LINES {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            let num = gtk::Label::new(Some(&n.to_string()));
            num.add_css_class("gl-num");
            num.set_xalign(1.0);
            num.set_valign(gtk::Align::Start);
            num.set_width_chars(2);
            let text = gtk::Label::new(Some(&fake_line(n)));
            text.add_css_class("gl-pline");
            text.set_xalign(0.0);
            text.set_valign(gtk::Align::Start);
            text.set_hexpand(true);
            text.set_wrap(true);
            text.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            row.append(&num);
            row.append(&text);
            lines.append(&row);
        }
        scroll.set_child(Some(&lines));
        root.append(&scroll);
        root.append(&rule());
        // Discard on the left; Save and Save As on the right, gap-m apart
        // (the .gl-action-row CSS margin). All three only log.
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        actions.add_css_class("gl-action-row");
        let discard = icon_image_button(MockupIcon::Discard);
        let save = icon_image_button(MockupIcon::Save);
        let save_as = icon_image_button(MockupIcon::SaveAs);
        for (button, name) in [
            (discard.clone(), "discard"),
            (save.clone(), "save"),
            (save_as.clone(), "save as"),
        ] {
            button.connect_clicked(move |_| {
                crate::log!("preview", "{name} clicked (logging only)");
            });
        }
        actions.append(&discard);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&spacer);
        actions.append(&save);
        actions.append(&save_as);
        root.append(&actions);
        Self {
            root,
            collapse_button: button,
        }
    }
}
