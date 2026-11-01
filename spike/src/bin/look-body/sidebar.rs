// SPDX-License-Identifier: GPL-3.0-or-later
//! The left sidebar: title row, pinned list, section pills and drive bars.
//! (The preview sidebar lives in preview.rs; both share the helpers here.)
use crate::style::{BAR_W, MAIN_BAR_W, scaled};
use gtk::prelude::*;

// Constant tables.
#[rustfmt::skip]
const DEFAULT_PINS: [&str; 7] = ["Home", "Desktop", "Documents", "Downloads", "Music", "Pictures", "Videos"];
#[rustfmt::skip]
const BAR_FILL_CLASSES: [&str; 11] = ["bar-lv0", "bar-lv1", "bar-lv2", "bar-lv3", "bar-lv4", "bar-lv5", "bar-lv6", "bar-lv7", "bar-lv8", "bar-lv9", "bar-full"];
const BAR_LABEL_CLASSES: [&str; 3] = ["bar-label-lo", "bar-label-hi", "bar-label-err"];

pub fn pin_icon(name: &str) -> &'static str {
    match name {
        "Home" => "user-home-symbolic",
        "Documents" => "folder-documents-symbolic",
        "Downloads" => "folder-download-symbolic",
        "Music" => "folder-music-symbolic",
        "Pictures" => "folder-pictures-symbolic",
        "Videos" => "folder-videos-symbolic",
        _ => "folder-symbolic",
    }
}

pub fn rule() -> gtk::Box {
    let r = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    r.add_css_class("gl-rule");
    r
}

pub fn icon_button(icon: &str) -> gtk::Button {
    let b = gtk::Button::new();
    b.set_child(Some(&gtk::Image::from_icon_name(icon)));
    b.add_css_class("gl-icon-button");
    b
}

pub fn title_row(title: &str, icon: &str) -> (gtk::Box, gtk::Button) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("gl-title");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    let button = icon_button(icon);
    row.append(&label);
    row.append(&button);
    (row, button)
}

/// Adds `class` to `w` only for the main drive block.
fn mark(w: &impl gtk::glib::object::IsA<gtk::Widget>, main: bool, class: &str) {
    if main {
        w.add_css_class(class);
    }
}

/// A drive bar: track overlay with a proportional fill and a centred % label.
fn drive_block(name: &str, cap: &str, main: bool) -> (gtk::Box, gtk::Box, gtk::Label) {
    let track = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    track.add_css_class("gl-drive-track");
    mark(&track, main, "main");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&track));
    let fill = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    fill.add_css_class("gl-drive-fill");
    mark(&fill, main, "main");
    fill.set_halign(gtk::Align::Start);
    fill.set_valign(gtk::Align::Fill);
    overlay.add_overlay(&fill);
    let label = gtk::Label::new(None);
    label.add_css_class("gl-drive-label");
    mark(&label, main, "gl-main-label");
    label.set_halign(gtk::Align::Center);
    label.set_valign(gtk::Align::Center);
    overlay.add_overlay(&label);
    let block = gtk::Box::new(gtk::Orientation::Vertical, 0);
    block.add_css_class("gl-drive-block");
    let name_label = gtk::Label::new(Some(name));
    name_label.add_css_class("gl-drive-name");
    block.append(&name_label);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    row.set_halign(gtk::Align::Center);
    row.append(&overlay);
    let cap_label = gtk::Label::new(Some(cap));
    cap_label.add_css_class("gl-drive-cap");
    mark(&cap_label, main, "gl-main-label");
    row.append(&cap_label);
    block.append(&row);
    (block, fill, label)
}

fn set_fullness(fill: &gtk::Box, label: &gtk::Label, fullness: f64, bar_w: f64, scale: f64) {
    let f = fullness.clamp(0.0, 1.0);
    fill.set_width_request(scaled(bar_w * f, scale));
    let (fill_class, label_class) = crate::style::bar_class(f);
    for class in BAR_FILL_CLASSES {
        fill.remove_css_class(class);
    }
    fill.add_css_class(fill_class);
    for class in BAR_LABEL_CLASSES {
        label.remove_css_class(class);
    }
    label.add_css_class(label_class);
    label.set_text(&format!("{}% Full", (f * 100.0).round() as i32));
}

fn dir_row(text: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.add_css_class("gl-dir-row");
    row.append(&gtk::Label::new(Some(text)));
    row
}

pub struct LeftPanel {
    pub root: gtk::Box,
    pub collapse_button: gtk::Button,
    pins_box: gtk::Box,
    pins: Vec<String>,
    devices_content: gtk::Box,
    drives: Vec<(gtk::Box, gtk::Label, f64)>,
    main: Option<(gtk::Box, gtk::Label)>,
    scale: f64,
}

impl LeftPanel {
    pub fn new(drives: &[f64]) -> Self {
        let mut this = Self {
            root: gtk::Box::new(gtk::Orientation::Vertical, 0),
            collapse_button: gtk::Button::new(),
            pins_box: gtk::Box::new(gtk::Orientation::Vertical, 0),
            pins: DEFAULT_PINS.iter().map(|s| s.to_string()).collect(),
            devices_content: gtk::Box::new(gtk::Orientation::Vertical, 0),
            drives: vec![],
            main: None,
            scale: 1.0,
        };
        this.root.add_css_class("gl-panel");
        let (row, button) = title_row("Pinned Folders", "sidebar-show-symbolic");
        this.collapse_button = button;
        this.root.append(&row);
        this.root.append(&rule());
        // The whole panel scrolls when the pins outgrow it; the main drive
        // block stays pinned below the scroller.
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroll.set_vexpand(true);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        this.pins_box.add_css_class("gl-pinned");
        body.append(&this.pins_box.clone());
        body.append(&rule());
        // Three collapsible sections: the pill toggles a SlideDown revealer.
        let recent = gtk::Box::new(gtk::Orientation::Vertical, 0);
        for _ in 0..5 {
            recent.append(&dir_row("Directory Name"));
        }
        let no_remotes = dir_row("No remotes");
        let sections: [(&gtk::Box, &str); 3] = [
            (&recent, "Recent Directories"),
            (&this.devices_content, "Devices"),
            (&no_remotes, "Remote"),
        ];
        for (content, name) in sections {
            let section = gtk::Box::new(gtk::Orientation::Vertical, 0);
            section.add_css_class("gl-section");
            let pill = gtk::Button::new();
            pill.add_css_class("gl-section-pill");
            pill.set_child(Some(&gtk::Label::new(Some(name))));
            let revealer = gtk::Revealer::new();
            revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);
            revealer.set_transition_duration(200);
            revealer.set_reveal_child(true);
            revealer.set_child(Some(content));
            section.append(&pill);
            section.append(&revealer);
            body.append(&section);
            let rev = revealer.clone();
            pill.connect_clicked(move |_| rev.set_reveal_child(!rev.reveals_child()));
        }
        scroll.set_child(Some(&body));
        this.root.append(&scroll);
        this.root.append(&rule());
        let (block, fill, label) = drive_block("Main Drive Name", "1 TB", true);
        this.main = Some((fill, label));
        this.root.append(&block);
        for f in drives {
            let (block, fill, label) = drive_block("Drive Name", "512 GB", false);
            this.devices_content.append(&block);
            this.drives.push((fill, label, *f));
        }
        this.set_scale(1.0);
        this.reload_pins();
        this
    }

    pub fn add_pin(&mut self) {
        self.pins.push(format!("Pinned {}", self.pins.len() + 1));
        self.reload_pins();
    }

    pub fn remove_pin(&mut self) {
        if self.pins.len() > 1 {
            self.pins.pop();
            self.reload_pins();
        }
    }

    pub fn set_pin_count(&mut self, n: usize) {
        while self.pins.len() < n {
            self.add_pin();
        }
        while self.pins.len() > n.max(1) {
            self.remove_pin();
        }
        self.reload_pins();
    }

    pub fn set_drive(&mut self, index: usize, fullness: f64) {
        if let Some(entry) = self.drives.get_mut(index) {
            entry.2 = fullness.clamp(0.0, 1.0);
            set_fullness(&entry.0, &entry.1, entry.2, BAR_W, self.scale);
        }
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale;
        for (fill, label, f) in &self.drives {
            set_fullness(fill, label, *f, BAR_W, scale);
        }
        if let Some((fill, label)) = &self.main {
            set_fullness(fill, label, 0.75, MAIN_BAR_W, scale);
        }
    }

    fn reload_pins(&self) {
        while let Some(child) = self.pins_box.first_child() {
            self.pins_box.remove(&child);
        }
        for (i, name) in self.pins.iter().enumerate() {
            // Spacing between icon and label comes from the CSS margin on
            // .gl-pinned-row label (style spec rule 3), not from Box spacing.
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            row.add_css_class("gl-pinned-row");
            if i == 0 {
                row.add_css_class("sel");
            }
            row.append(&gtk::Image::from_icon_name(pin_icon(name)));
            row.append(&gtk::Label::new(Some(name)));
            self.pins_box.append(&row);
        }
    }
}
