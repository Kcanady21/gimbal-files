// SPDX-License-Identifier: GPL-3.0-or-later
//! The body's content area: one folder grid for a normal pill, the split view
//! for a group of two or more, and the placeholder when there is no tab.
//!
//! The progress row sits above the one status line. A finished operation holds
//! the status line for a few seconds; a click on it opens the failure details.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk::glib;
use gtk::prelude::*;

use fm_core::config::{SortBy, SortOrder, SortRule};
use fm_core::workspace::{TabId, Window, WindowId};

use crate::app::Process;
use crate::fileops::ProgressView;
use crate::fileops::dialogs;
use crate::folder::{FolderView, StatusLine, status_text};
use crate::look::motion;
use crate::look::tokens::*;
use crate::search::SearchPage;
use crate::split::SplitView;

pub struct Content {
    root: gtk::Box,
    stack: gtk::Stack,
    placeholder: gtk::Label,
    folder: Rc<FolderView>,
    split: SplitView,
    search: Rc<SearchPage>,
    rule: gtk::Box,
    status: Rc<StatusLine>,
    progress_row: gtk::Box,
    progress_label: gtk::Label,
    progress_bar: gtk::ProgressBar,
    process: Weak<Process>,
    displayed_folder: RefCell<Option<PathBuf>>,
    displayed_tab: Cell<Option<TabId>>,
    /// Set by nav.back/forward so the next folder change restores its anchor.
    restore_anchor: Cell<bool>,
    split_shown: Cell<bool>,
    /// Transient presentation state. This is deliberately not in the model:
    /// a session restore always comes back to an ordinary folder page.
    trash_active: Cell<bool>,
    search_active: Cell<bool>,
    summary_gen: Rc<Cell<u64>>,
    last_failures: Rc<RefCell<Option<String>>>,
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

        let status = StatusLine::new();
        let progress_row = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale));
        progress_row.add_css_class("gl-progress");
        progress_row.set_visible(false);
        let progress_label = gtk::Label::new(None);
        progress_label.add_css_class("gl-progress-label");
        progress_label.set_xalign(0.0);
        progress_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let progress_bar = gtk::ProgressBar::new();
        progress_bar.add_css_class("gl-progress-bar");
        progress_bar.set_hexpand(true);
        progress_bar.set_valign(gtk::Align::Center);
        progress_bar.set_show_text(false);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("gl-menu-item");
        cancel.set_valign(gtk::Align::Center);
        progress_row.append(&progress_label);
        progress_row.append(&progress_bar);
        progress_row.append(&cancel);

        let last_failures: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
        {
            let failures = last_failures.clone();
            let cancel_process = Rc::downgrade(process);
            cancel.connect_clicked(move |_| {
                if let Some(process) = cancel_process.upgrade()
                    && let Some(fileops) = process.fileops()
                {
                    fileops.cancel_current();
                }
            });
            let click = gtk::GestureClick::new();
            click.set_button(1);
            let process = Rc::downgrade(process);
            click.connect_released(move |_, _, _, _| {
                let Some(text) = failures.borrow().clone() else {
                    return;
                };
                if let Some(process) = process.upgrade()
                    && let Some(ui) = process.window(window)
                {
                    dialogs::show_details(&ui, &text);
                }
            });
            status.widget().add_controller(click);
        }

        let folder = FolderView::new(process, window, status.clone(), scale, None, None);
        let split = SplitView::new(process, window, status.clone(), scale);
        let search = SearchPage::new(process, window, scale);
        stack.add_named(&placeholder, Some("placeholder"));
        stack.add_named(folder.widget(), Some("folder"));
        stack.add_named(split.widget(), Some("split"));
        stack.add_named(search.widget(), Some("search"));
        stack.set_visible_child_name("folder");

        root.append(&stack);
        let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rule.add_css_class("gl-rule");
        rule.add_css_class("inset");
        root.append(&rule);
        root.append(&progress_row);
        root.append(status.widget());

        let this = Rc::new(Self {
            root,
            stack,
            placeholder,
            folder,
            split,
            search,
            rule,
            status,
            progress_row,
            progress_label,
            progress_bar,
            process: Rc::downgrade(process),
            displayed_folder: RefCell::new(None),
            displayed_tab: Cell::new(None),
            restore_anchor: Cell::new(false),
            split_shown: Cell::new(false),
            trash_active: Cell::new(false),
            search_active: Cell::new(false),
            summary_gen: Rc::new(Cell::new(0)),
            last_failures,
        });
        this.apply_scale(scale);
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Install the one Trash page in this window's central stack. The normal
    /// folder and split pages stay parented below it, so switching back does
    /// not rebuild their focus, scroll or selection.
    pub fn add_trash_page(&self, page: &impl IsA<gtk::Widget>) {
        if self.stack.child_by_name("trash").is_none() {
            self.stack.add_named(page, Some("trash"));
        }
    }

    pub fn show_trash(&self) {
        self.trash_active.set(true);
        self.search_active.set(false);
        self.folder.set_status_gate(false);
        if self.split_shown.get() {
            self.split.blur();
        }
        self.rule.set_visible(false);
        self.progress_row.set_visible(false);
        self.status.widget().set_visible(false);
        self.stack.set_visible_child_name("trash");
    }

    pub fn hide_trash(&self) {
        self.trash_active.set(false);
        self.rule.set_visible(true);
        self.status.widget().set_visible(true);
        self.progress_row.set_visible(false);
    }

    pub fn trash_active(&self) -> bool {
        self.trash_active.get()
    }

    pub fn search_page(&self) -> &Rc<SearchPage> {
        &self.search
    }

    #[cfg(test)]
    pub(crate) fn visible_page_for_tests(&self) -> Option<String> {
        self.stack.visible_child_name().map(|name| name.to_string())
    }

    pub fn search_active(&self) -> bool {
        self.search_active.get()
    }

    /// Show the transient results page over the unchanged folder/split page.
    /// The underlying widgets stay parented, so leaving search restores their
    /// selection, history, scroll anchor and split layout untouched.
    pub fn show_search(&self) {
        self.search_active.set(true);
        self.folder.set_status_gate(false);
        self.rule.set_visible(false);
        self.progress_row.set_visible(false);
        self.status.widget().set_visible(false);
        self.stack.set_visible_child_name("search");
    }

    pub fn hide_search(&self) {
        self.search_active.set(false);
        if self.trash_active.get() {
            return;
        }
        self.rule.set_visible(true);
        self.status.widget().set_visible(true);
        self.progress_row.set_visible(false);
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

    /// Return keyboard focus to the visible ordinary page after a transient
    /// page closes. Trash and search pages keep their own focus.
    pub fn focus_visible(&self) {
        if self.trash_active.get() || self.search_active.get() {
            return;
        }
        if self.on_split_page() {
            self.split.focus_focused();
        } else if self.stack.visible_child_name().as_deref() == Some("folder") {
            self.folder.grab_grid_focus();
        }
    }

    pub fn listed_names(&self) -> Vec<OsString> {
        // The hidden folder page must never supply names while Trash or search
        // is the visible page; file actions are refused there instead.
        if self.trash_active.get() || self.search_active.get() {
            return Vec::new();
        }
        if self.on_split_page() {
            return self.split.listed_names();
        }
        if self.stack.visible_child_name().as_deref() != Some("folder") {
            return Vec::new();
        }
        self.folder.listed_names()
    }

    /// Re-read every administrator folder view in this window: the single
    /// view and each split tile. Returns how many were re-read.
    pub fn reload_admin(&self) -> usize {
        usize::from(self.folder.reload_if_admin()) + self.split.reload_admin()
    }

    pub fn reload(&self) {
        if self.trash_active.get() || self.search_active.get() {
            return;
        }
        if self.on_split_page() {
            self.split.reload_focused();
        } else {
            self.folder.reload();
        }
    }

    /// Cancel a split tile drag: Esc, a lost grab, a right button mid-drag or
    /// the window losing focus. Returns whether a drag was active.
    /// Release this window's folder listings, thumbnails and folder-size walks
    /// under one mount root. Search is process-wide and released separately.
    pub fn release_root(&self, root: &Path) -> usize {
        let mut released = usize::from(self.folder.release_root(root));
        if released > 0 {
            // A terminal failure rebuilds this same folder; forget the cached
            // display key so `show` asks the released listing for it again.
            *self.displayed_folder.borrow_mut() = None;
            self.displayed_tab.set(None);
        }
        released += self.split.release_root(root);
        released
    }

    pub fn cancel_drag(&self) -> bool {
        self.split.cancel_drag()
    }

    pub fn apply_scale(&self, scale: f64) {
        self.folder.apply_scale(scale);
        self.split.set_scale(scale);
        self.search.apply_scale(scale);
        self.progress_row.set_spacing(scaled(GAP_S, scale));
        self.progress_row.set_margin_start(scaled(GAP_L, scale));
        self.progress_row.set_margin_end(scaled(GAP_L, scale));
    }

    /// A new operation starts: hide the old row and let the counts back in.
    pub fn begin_fileop(&self) {
        if self.search_active.get() {
            return;
        }
        self.summary_gen.set(self.summary_gen.get() + 1);
        self.status.end_summary();
        self.progress_row.set_visible(false);
        self.progress_label.set_text("");
        self.progress_bar.set_fraction(0.0);
        *self.last_failures.borrow_mut() = None;
    }

    pub fn set_progress(&self, view: Option<ProgressView>) {
        if self.trash_active.get() || self.search_active.get() {
            return;
        }
        match view {
            Some(view) => {
                self.progress_label.set_text(&view.label);
                self.progress_bar.set_fraction(view.fraction);
                self.progress_row.set_visible(true);
            }
            None => {
                self.progress_row.set_visible(false);
            }
        }
    }

    /// Hold the status line for a few seconds, then go back to the counts.
    pub fn show_summary(&self, text: &str, failures: Option<String>) {
        if self.trash_active.get() || self.search_active.get() {
            return;
        }
        self.progress_row.set_visible(false);
        *self.last_failures.borrow_mut() = failures;
        self.status.show_summary(text);
        let generation = self.summary_gen.get() + 1;
        self.summary_gen.set(generation);
        let status = self.status.clone();
        let gen_cell = self.summary_gen.clone();
        let failures = self.last_failures.clone();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(crate::fileops::progress::SUMMARY_MS),
            move || {
                if gen_cell.get() == generation {
                    status.end_summary();
                    *failures.borrow_mut() = None;
                }
            },
        );
    }

    pub fn show_nothing_to_undo(&self) {
        self.show_summary("Nothing to undo", None);
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
        if self.search_active.get() {
            return;
        }
        // Model events during a job or monitor refresh update the hidden pages
        // later; they never dismiss the visible Trash page. Navigation leaves
        // Trash explicitly before it asks for a destination.
        if crate::trash::keeps_trash_page(
            self.trash_active.get(),
            crate::trash::RefreshCause::Background,
        ) {
            return;
        }
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
        self.status.set_counts(status_text(0, 0, 0));
    }

    fn folder_rule(&self, folder: &Path) -> Option<SortRule> {
        let process = self.process.upgrade()?;
        let config = process.config.borrow();
        Some(config.config().sort_for(folder))
    }
}
