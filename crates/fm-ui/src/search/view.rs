// SPDX-License-Identifier: GPL-3.0-or-later
//! The transient, virtualized results page over a window's content area.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use fm_core::folder_size::EntryKind;
use fm_core::search::{MatchScore, SearchChange, SearchMatch};
use fm_core::workspace::WindowId;
use gtk::glib;
use gtk::prelude::*;

use crate::app::Process;
use crate::look::tokens::*;
use crate::search::{SearchView, scope_text, status_text, status_tooltip};

#[derive(Clone, Debug)]
struct ResultRow {
    name: String,
    parent: String,
    meta: String,
    tooltip: String,
    kind: EntryKind,
}

pub struct SearchPage {
    root: gtk::Box,
    heading: gtk::Label,
    scope: gtk::Label,
    query: gtk::Label,
    notice: gtk::Label,
    status: gtk::Label,
    store: gtk::gio::ListStore,
    selection: gtk::SingleSelection,
    list: gtk::ListView,
    scroller: gtk::ScrolledWindow,
    process: Weak<Process>,
    window: WindowId,
    generation: Cell<u64>,
    root_path: RefCell<Option<PathBuf>>,
    walked_root: RefCell<Option<PathBuf>>,
    /// Ranked identities in the exact order of the list store. The path is the
    /// stable identity; the index is only the current row position.
    positions: RefCell<Vec<(MatchScore, PathBuf)>>,
    updating: Cell<bool>,
    scale: Cell<f64>,
}

impl SearchPage {
    pub fn new(process: &Rc<Process>, window: WindowId, scale: f64) -> Rc<Self> {
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.add_css_class("gl-search-page");
        root.set_vexpand(true);
        root.set_hexpand(true);

        let heading = gtk::Label::new(None);
        heading.add_css_class("gl-search-title");
        heading.set_xalign(0.0);
        heading.set_single_line_mode(true);
        heading.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        heading.set_hexpand(true);
        heading.set_width_chars(1);
        heading.set_max_width_chars(1);
        root.append(&heading);

        let scope = gtk::Label::new(None);
        scope.add_css_class("gl-search-scope");
        scope.set_xalign(0.0);
        scope.set_wrap(true);
        scope.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        root.append(&scope);

        let query = gtk::Label::new(None);
        query.add_css_class("gl-search-query");
        query.set_xalign(0.0);
        query.set_single_line_mode(true);
        query.set_ellipsize(gtk::pango::EllipsizeMode::End);
        query.set_hexpand(true);
        query.set_width_chars(1);
        query.set_max_width_chars(1);
        root.append(&query);

        let notice = gtk::Label::new(None);
        notice.add_css_class("gl-search-notice");
        notice.set_xalign(0.0);
        notice.set_wrap(true);
        notice.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        notice.set_visible(false);
        root.append(&notice);

        let store = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection = gtk::SingleSelection::new(Some(store.clone()));
        // No result is selected just because it arrived. A click or a keyboard
        // move selects exactly one row and asks for its preview.
        selection.set_autoselect(false);
        selection.set_can_unselect(true);
        let list = gtk::ListView::new(Some(selection.clone()), None::<gtk::SignalListItemFactory>);
        list.add_css_class("gl-search-list");
        list.set_show_separators(true);
        list.set_single_click_activate(false);
        let factory = gtk::SignalListItemFactory::new();
        list.set_factory(Some(&factory));

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&list));
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_width(false);
        scroller.set_propagate_natural_height(false);
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);
        root.append(&scroller);

        let status = gtk::Label::new(None);
        status.add_css_class("gl-search-status");
        status.set_xalign(0.0);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status.set_hexpand(true);
        status.set_width_chars(1);
        status.set_max_width_chars(1);
        root.append(&status);

        let page = Rc::new(Self {
            root,
            heading,
            scope,
            query,
            notice,
            status,
            store,
            selection,
            list,
            scroller: scroller.clone(),
            process: Rc::downgrade(process),
            window,
            generation: Cell::new(0),
            root_path: RefCell::new(None),
            walked_root: RefCell::new(None),
            positions: RefCell::new(Vec::new()),
            updating: Cell::new(false),
            scale: Cell::new(scale),
        });
        page.wire_factory(&factory);
        page.wire_selection();
        page.apply_scale(scale);
        page
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn apply_scale(&self, scale: f64) {
        self.scale.set(scale);
        let p = |token: f64| scaled(token, scale);
        self.root.set_margin_start(p(GAP_2XL));
        self.root.set_margin_end(p(GAP_2XL));
        self.root.set_margin_bottom(p(GAP_2XL));
        self.root.set_spacing(p(GAP_S));
        self.scope.set_margin_bottom(p(GAP_M));
        self.notice.set_margin_top(p(GAP_S));
        self.status.set_margin_top(p(GAP_M));
        self.list.set_margin_top(p(GAP_S));
    }

    /// Start (or replace) a search view: clear every old row and heading.
    pub fn begin(&self, view: &SearchView) {
        self.updating.set(true);
        self.store.remove_all();
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
        self.positions.borrow_mut().clear();
        self.updating.set(false);
        self.scroller.vadjustment().set_value(0.0);
        self.generation.set(view.generation);
        *self.root_path.borrow_mut() = Some(view.root.clone());
        *self.walked_root.borrow_mut() = view.walked_root.clone();
        self.heading
            .set_text(&format!("Searching in {}", view.root.display()));
        self.heading
            .set_tooltip_text(Some(&view.root.display().to_string()));
        self.query.set_text(&format!("Query: {}", view.query));
        self.query.set_tooltip_text(Some(&view.query));
        self.apply_view(view);
    }

    /// Update counts/status without rebuilding rows.
    pub fn status(&self, view: &SearchView) {
        if view.generation != self.generation.get() {
            return;
        }
        *self.root_path.borrow_mut() = Some(view.root.clone());
        if view.walked_root.is_some() {
            *self.walked_root.borrow_mut() = view.walked_root.clone();
        }
        self.apply_view(view);
    }

    fn apply_view(&self, view: &SearchView) {
        self.scope.set_text(&scope_text(view));
        let text = status_text(view);
        self.status.set_text(&text);
        self.status
            .set_tooltip_text(status_tooltip(view).as_deref());
        self.status.remove_css_class("partial");
        self.status.remove_css_class("error");
        match view.phase {
            crate::search::SearchPhase::Partial(_) => self.status.add_css_class("partial"),
            crate::search::SearchPhase::Unavailable(_) => self.status.add_css_class("error"),
            _ => {}
        }
        match &view.notice {
            Some(notice) => {
                self.notice.set_text(notice);
                self.notice.set_visible(true);
            }
            None => {
                self.notice.set_text("");
                self.notice.set_visible(false);
            }
        }
    }

    /// Forget a closed session's rows. The page stays parented but empty.
    pub fn reset(&self) {
        self.updating.set(true);
        self.store.remove_all();
        self.selection.set_selected(gtk::INVALID_LIST_POSITION);
        self.positions.borrow_mut().clear();
        self.updating.set(false);
        self.generation.set(0);
        self.status.set_text("");
        self.status.set_tooltip_text(None);
        self.status.remove_css_class("partial");
        self.status.remove_css_class("error");
        self.scope.set_text("");
        self.notice.set_text("");
        self.notice.set_visible(false);
        *self.root_path.borrow_mut() = None;
        *self.walked_root.borrow_mut() = None;
    }

    /// Apply one worker batch of ranked inserts/removals in order. The selected
    /// path and the scroll position survive a reorder; a removed selection is
    /// explicitly unselected rather than silently pointing at a new row.
    pub fn apply_changes(&self, changes: &[SearchChange]) {
        if changes.is_empty() {
            return;
        }
        let selected_path = {
            let position = self.selection.selected();
            if position == gtk::INVALID_LIST_POSITION {
                None
            } else {
                self.positions
                    .borrow()
                    .get(position as usize)
                    .map(|(_, path)| path.clone())
            }
        };
        let scroll = self.scroller.vadjustment().value();
        let root = self.root_path.borrow().clone();
        let walked = self.walked_root.borrow().clone();
        self.updating.set(true);
        {
            let mut positions = self.positions.borrow_mut();
            for change in changes {
                match change {
                    SearchChange::Insert { index, row } => {
                        let index = (*index).min(positions.len());
                        let object = glib::BoxedAnyObject::new(result_row(
                            root.as_deref(),
                            walked.as_deref(),
                            row,
                        ));
                        self.store.splice(index as u32, 0, &[object]);
                        positions.insert(index, (row.score, row.path.clone()));
                    }
                    SearchChange::Remove { index, path } => {
                        let position = if positions
                            .get(*index)
                            .is_some_and(|(_, current)| current == path)
                        {
                            *index
                        } else {
                            let Some(position) =
                                positions.iter().position(|(_, current)| current == path)
                            else {
                                continue;
                            };
                            position
                        };
                        self.store
                            .splice(position as u32, 1, &[] as &[glib::Object]);
                        positions.remove(position);
                    }
                }
            }
            if let Some(path) = selected_path {
                let selected = positions
                    .iter()
                    .position(|(_, current)| current == &path)
                    .map_or(gtk::INVALID_LIST_POSITION, |index| index as u32);
                self.selection.set_selected(selected);
            }
        }
        self.updating.set(false);
        let adjustment = self.scroller.vadjustment();
        let maximum = (adjustment.upper() - adjustment.page_size()).max(0.0);
        adjustment.set_value(scroll.min(maximum));
    }

    pub fn show_notice(&self, text: &str) {
        self.notice.set_text(text);
        self.notice.set_visible(!text.is_empty());
    }

    #[cfg(test)]
    pub(crate) fn store_count_for_tests(&self) -> u32 {
        self.store.n_items()
    }

    #[cfg(test)]
    pub(crate) fn row_texts_for_tests(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        for index in 0..self.store.n_items() {
            let Some(item) = self.store.item(index) else {
                continue;
            };
            let Ok(boxed) = item.downcast::<glib::BoxedAnyObject>() else {
                continue;
            };
            if let Ok(row) = boxed.try_borrow::<ResultRow>() {
                rows.push((row.name.clone(), row.parent.clone()));
            }
        }
        rows
    }

    #[cfg(test)]
    pub(crate) fn row_paths_for_tests(&self) -> Vec<PathBuf> {
        self.positions
            .borrow()
            .iter()
            .map(|(_, path)| path.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn row_score_for_tests(&self, index: usize) -> Option<MatchScore> {
        self.positions.borrow().get(index).map(|(score, _)| *score)
    }

    #[cfg(test)]
    pub(crate) fn list_mapped_for_tests(&self) -> bool {
        self.list.is_mapped()
    }

    #[cfg(test)]
    pub(crate) fn selection_for_tests(&self) -> &gtk::SingleSelection {
        &self.selection
    }

    fn wire_selection(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.selection.connect_selected_notify(move |selected| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            if page.updating.get() {
                return;
            }
            let position = selected.selected();
            if position == gtk::INVALID_LIST_POSITION {
                return;
            }
            let Some(item) = selected.item(position) else {
                return;
            };
            if item.downcast_ref::<glib::BoxedAnyObject>().is_none() {
                return;
            }
            let Some(process) = page.process.upgrade() else {
                return;
            };
            process.search_preview(page.window, page.generation.get(), position as usize);
        });
        let weak = Rc::downgrade(self);
        self.list.connect_activate(move |_, position| {
            let Some(page) = weak.upgrade() else {
                return;
            };
            if let Some(process) = page.process.upgrade() {
                process.search_activate(page.window, page.generation.get(), position as usize);
            }
        });
    }

    fn wire_factory(self: &Rc<Self>, factory: &gtk::SignalListItemFactory) {
        let scale = self.scale.clone();
        factory.connect_setup(move |_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let row = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_M, scale.get()));
            row.add_css_class("gl-search-row");
            let icon = gtk::Image::new();
            icon.add_css_class("gl-search-icon");
            let name = gtk::Label::new(None);
            name.add_css_class("gl-search-name");
            name.set_xalign(0.0);
            name.set_hexpand(true);
            name.set_single_line_mode(true);
            name.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            name.set_width_chars(1);
            name.set_max_width_chars(1);
            let parent = gtk::Label::new(None);
            parent.add_css_class("gl-search-parent");
            parent.set_xalign(0.0);
            parent.set_single_line_mode(true);
            parent.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            parent.set_width_chars(12);
            parent.set_max_width_chars(32);
            let meta = gtk::Label::new(None);
            meta.add_css_class("gl-search-meta");
            meta.set_xalign(1.0);
            meta.set_single_line_mode(true);
            meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
            meta.set_width_chars(18);
            meta.set_max_width_chars(32);
            row.append(&icon);
            row.append(&name);
            row.append(&parent);
            row.append(&meta);
            item.set_child(Some(&row));
        });
        factory.connect_bind(|_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let Some(boxed) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
                return;
            };
            let Ok(result) = boxed.try_borrow::<ResultRow>() else {
                return;
            };
            let Some(row) = item.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let icon = row.first_child().and_downcast::<gtk::Image>();
            let mut child = row.first_child();
            if let Some(icon) = icon {
                icon.set_icon_name(Some(icon_name(result.kind)));
                child = icon.next_sibling();
            }
            if let Some(name) = child.clone().and_downcast::<gtk::Label>() {
                name.set_text(&result.name);
                name.update_property(&[gtk::accessible::Property::Label(&result.name)]);
                child = name.next_sibling();
            }
            if let Some(parent) = child.clone().and_downcast::<gtk::Label>() {
                parent.set_text(&result.parent);
                child = parent.next_sibling();
            }
            if let Some(meta) = child.and_downcast::<gtk::Label>() {
                meta.set_text(&result.meta);
            }
            row.set_tooltip_text(Some(&result.tooltip));
            row.set_has_tooltip(true);
            row.update_property(&[gtk::accessible::Property::Description(&result.tooltip)]);
        });
        factory.connect_unbind(|_, object| {
            let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            if let Some(row) = item.child().and_downcast::<gtk::Box>() {
                row.set_has_tooltip(false);
            }
        });
    }
}

fn result_row(root: Option<&Path>, walked: Option<&Path>, hit: &SearchMatch) -> ResultRow {
    let base = walked.or(root);
    let parent = hit.path.parent().unwrap_or_else(|| Path::new(""));
    let parent = match base.and_then(|base| parent.strip_prefix(base).ok()) {
        Some(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Some(relative) => relative.display().to_string(),
        None => parent.display().to_string(),
    };
    let modified = hit
        .modified_sec
        .and_then(|seconds| {
            glib::DateTime::from_unix_local(seconds)
                .ok()
                .and_then(|date| date.format("%Y-%m-%d %H:%M").ok())
                .map(|text| text.to_string())
        })
        .unwrap_or_else(|| "Modified unavailable".to_string());
    let kind = kind_label(hit.kind);
    let meta = match hit.kind {
        EntryKind::File => format!(
            "{kind} · {} · {modified}",
            hit.size
                .map(fm_core::preview::format_size)
                .unwrap_or_else(|| "Size unavailable".to_string())
        ),
        EntryKind::Directory => format!("Folder · {modified}"),
        EntryKind::Symlink => format!("Link · {modified}"),
        EntryKind::Other => format!("Special file · {modified}"),
    };
    let tooltip = format!("{}\n{}\n{}", hit.display_name, hit.path.display(), meta);
    ResultRow {
        name: hit.display_name.clone(),
        parent,
        meta,
        tooltip,
        kind: hit.kind,
    }
}

fn kind_label(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "File",
        EntryKind::Directory => "Folder",
        EntryKind::Symlink => "Link",
        EntryKind::Other => "Special file",
    }
}

fn icon_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "text-x-generic-symbolic",
        EntryKind::Directory => "folder-symbolic",
        EntryKind::Symlink => "emblem-symbolic-link",
        EntryKind::Other => "dialog-question-symbolic",
    }
}
