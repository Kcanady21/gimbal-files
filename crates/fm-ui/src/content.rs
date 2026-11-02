// SPDX-License-Identifier: GPL-3.0-or-later
//! The body's content area: one folder grid for a normal pill, the split view
//! for a group of two or more, and the placeholder when there is no tab.
//!
//! Every page already exists, so a group forming or dissolving only moves the
//! stack; the split view keeps one folder view per tab for as long as the tile
//! is shown.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::prelude::*;

use fm_core::config::{SortBy, SortOrder, SortRule};
use fm_core::workspace::{TabId, Window, WindowId};

use crate::app::Process;
use crate::folder::{FolderView, status_text};
use crate::look::motion;
use crate::split::SplitView;

pub struct Content {
    root: gtk::Box,
    stack: gtk::Stack,
    placeholder: gtk::Label,
    folder: Rc<FolderView>,
    split: SplitView,
    status: gtk::Label,
    process: Weak<Process>,
    displayed_folder: RefCell<Option<PathBuf>>,
    displayed_tab: Cell<Option<TabId>>,
    /// Set by nav.back/forward so the next folder change restores its anchor.
    restore_anchor: Cell<bool>,
    split_shown: Cell<bool>,
}

impl Content {
    pub fn new(process: &Rc<Process>, window: WindowId, scale: f64) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_vexpand(true);
        root.set_hexpand(true);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(motion::ms(250));

        let placeholder = gtk::Label::builder()
            .label("")
            .css_classes(["gl-title"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .vexpand(true)
            .build();

        let status = gtk::Label::new(Some(&status_text(0, 0, 0)));
        status.add_css_class("gl-status");
        status.set_hexpand(true);
        status.set_xalign(0.5);

        let folder = FolderView::new(process, window, status.clone(), scale, None, None);
        let split = SplitView::new(process, window, status.clone(), scale);
        stack.add_named(&placeholder, Some("placeholder"));
        stack.add_named(folder.widget(), Some("folder"));
        stack.add_named(split.widget(), Some("split"));
        stack.set_visible_child_name("folder");

        root.append(&stack);
        let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rule.add_css_class("gl-rule");
        rule.add_css_class("inset");
        root.append(&rule);
        root.append(&status);

        Rc::new(Self {
            root,
            stack,
            placeholder,
            folder,
            split,
            status,
            process: Rc::downgrade(process),
            displayed_folder: RefCell::new(None),
            displayed_tab: Cell::new(None),
            restore_anchor: Cell::new(false),
            split_shown: Cell::new(false),
        })
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn arm_anchor_restore(&self) {
        self.restore_anchor.set(true);
    }

    pub fn clear_anchor_restore(&self) {
        self.restore_anchor.set(false);
    }

    fn on_split_page(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("split")
    }

    pub fn listed_names(&self) -> Vec<OsString> {
        if self.on_split_page() {
            return self.split.listed_names();
        }
        if self.stack.visible_child_name().as_deref() != Some("folder") {
            return Vec::new();
        }
        self.folder.listed_names()
    }

    pub fn reload(&self) {
        if self.on_split_page() {
            self.split.reload_focused();
        } else {
            self.folder.reload();
        }
    }

    /// Cancel a split tile drag: Esc, a lost grab, a right button mid-drag or
    /// the window losing focus. Returns whether a drag was active.
    pub fn cancel_drag(&self) -> bool {
        self.split.cancel_drag()
    }

    pub fn apply_scale(&self, scale: f64) {
        self.folder.apply_scale(scale);
        self.split.set_scale(scale);
    }

    /// Re-sort every tile showing `folder`, or every tile when it is `None`.
    pub fn refresh_sort(&self, folder: Option<&Path>) {
        self.split.refresh_sort(folder);
        if self.on_split_page() {
            return;
        }
        let Some(current) = self.displayed_folder.borrow().clone() else {
            return;
        };
        if folder.is_none_or(|folder| folder == current) {
            let Some(process) = self.process.upgrade() else {
                return;
            };
            let rule = {
                let config = process.config.borrow();
                config.config().sort_for(&current)
            };
            self.folder.set_sort(rule);
        }
    }

    /// Reflect the focused tab. A normal pill shows its folder grid; a split
    /// group shows one tile per tab.
    pub fn show(&self, window: &Window) {
        let Some(group) = window.focused_group() else {
            self.show_placeholder("No tabs");
            return;
        };
        if group.is_split() {
            let restore = self.restore_anchor.replace(false);
            self.folder.set_status_gate(false);
            self.split.show(window, group, restore);
            self.stack.set_visible_child_name("split");
            self.split_shown.set(true);
            return;
        }

        self.folder.set_status_gate(true);

        if self.split_shown.replace(false) {
            self.split.blur();
            // Leaving a split must rebuild the single grid from the model,
            // restoring that tab's own anchor.
            *self.displayed_folder.borrow_mut() = None;
            self.displayed_tab.set(None);
        }

        let Some(tab) = group.tabs.first() else {
            self.show_placeholder("No tabs");
            return;
        };

        let folder = tab.folder.clone();
        let tab_changed = self.displayed_tab.replace(Some(tab.id)) != Some(tab.id);
        let folder_changed = self.displayed_folder.borrow().as_ref() != Some(&folder);
        if folder_changed {
            let back_or_forward = self.restore_anchor.replace(false);
            let restore = back_or_forward || tab_changed;
            let anchor = if restore {
                tab.scroll_anchor.clone()
            } else {
                None
            };
            let rule = self.folder_rule(&folder).unwrap_or(SortRule {
                by: SortBy::Name,
                order: SortOrder::Ascending,
                folders_first: true,
            });
            self.folder
                .set_folder(&folder, rule, anchor, restore, back_or_forward);
            *self.displayed_folder.borrow_mut() = Some(folder);
        } else if tab_changed {
            self.folder.restore_anchor(tab.scroll_anchor.clone(), false);
        }
        self.stack.set_visible_child_name("folder");
        self.folder.set_selection(&tab.selection);
    }

    pub fn show_placeholder(&self, text: &str) {
        self.folder.set_status_gate(false);
        if self.split_shown.replace(false) {
            self.split.blur();
        }
        self.placeholder.set_text(text);
        self.stack.set_visible_child_name("placeholder");
        self.status.set_text(&status_text(0, 0, 0));
    }

    fn folder_rule(&self, folder: &Path) -> Option<SortRule> {
        let process = self.process.upgrade()?;
        let config = process.config.borrow();
        Some(config.config().sort_for(folder))
    }
}
