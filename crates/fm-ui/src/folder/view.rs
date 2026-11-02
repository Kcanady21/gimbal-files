// SPDX-License-Identifier: GPL-3.0-or-later
//! The live folder grid: `DirectoryList` -> hidden filter -> `SortListModel` ->
//! `MultiSelection` -> `GridView`, with the spike's cell and the P1e column cap.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gtk::gdk;
use gtk::gdk::prelude::*;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fm_core::config::{SortBy, SortOrder, SortRule};
use fm_core::workspace::{FileSelection, TabId, WindowId};

use crate::app::Process;
use crate::dnd::{self, DragItem};
use crate::folder::{ClickRequest, StatusLine, click_request, is_hidden_name, sort};
use crate::look::icons::{self, MockupIcon};
use crate::look::tokens::*;

const LABEL_CHARS: i32 = 9;
const MAX_COLUMNS: u32 = 64;
const ANCHOR_DEBOUNCE_MS: u64 = 300;

/// The `max-columns` cap for a grid viewport `view_w` wide: how many columns fit
/// at the minimum cell width, plus one. A cap one above what fits never binds,
/// while keeping the pool near the number of cells actually on screen. Never
/// pass `G_MAXUINT`: it makes the grid keep billions of cells (P1e).
pub fn max_columns(view_w: i32, scale: f64) -> i32 {
    let cell = scaled(CELL_W, scale).max(1);
    let gap = scaled(GAP_XL, scale);
    (((view_w + gap) / (cell + gap)).max(1) + 1).clamp(1, MAX_COLUMNS as i32)
}

pub struct FolderView {
    root: gtk::Overlay,
    scroller: gtk::ScrolledWindow,
    grid: gtk::GridView,
    dir_list: gtk::DirectoryList,
    sorted: gtk::SortListModel,
    selection: gtk::MultiSelection,
    status: Rc<StatusLine>,
    error: gtk::Label,
    process: Weak<Process>,
    window: WindowId,
    /// The tab this grid shows, when it is a split tile. A click or a scroll
    /// anchor then names that tab before the model acts on "the focused tab".
    tab: Option<TabId>,
    /// A split tile only writes the one status line while it is the focused
    /// tile; the single folder view has no gate.
    status_gate: Rc<Cell<bool>>,

    current: RefCell<Option<PathBuf>>,
    sort_rule: Rc<Cell<SortRule>>,
    started: Cell<Instant>,
    first_logged: Cell<bool>,
    loaded: Cell<bool>,

    /// The model's selection, applied again when a listing finishes.
    wanted: RefCell<Vec<OsString>>,
    selected: Cell<usize>,
    index: RefCell<HashMap<OsString, u32>>,
    counts: Cell<(u64, u64)>,

    /// The per-folder anchor, kept for a back or a tab switch. A new folder
    /// (nav.go or nav.up) always opens at the top.
    anchors: RefCell<HashMap<PathBuf, OsString>>,
    pending_anchor: RefCell<Option<OsString>>,
    scroll_top: Cell<bool>,

    scale: Rc<Cell<f64>>,
    cell_positions: Rc<RefCell<Vec<(gtk::Widget, u32)>>>,
    scroll_gen: Cell<u64>,
    settle_pending: Rc<Cell<bool>>,
    cap_pending: Rc<Cell<bool>>,
    /// Set by a right press on a cell so the scroller's background gesture
    /// does not open a second menu for the same press.
    context_on_item: Rc<Cell<bool>>,
}

impl FolderView {
    pub fn new(
        process: &Rc<Process>,
        window: WindowId,
        status: Rc<StatusLine>,
        scale: f64,
        tab: Option<TabId>,
        status_gate: Option<Rc<Cell<bool>>>,
    ) -> Rc<Self> {
        let default_rule = SortRule {
            by: SortBy::Name,
            order: SortOrder::Ascending,
            folders_first: true,
        };
        let sort_rule = Rc::new(Cell::new(default_rule));
        let attributes = sort::attributes(default_rule);
        let dir_list = gtk::DirectoryList::new(Some(attributes.as_str()), None::<&gio::File>);

        let filter = gtk::CustomFilter::new(|object| {
            object
                .downcast_ref::<gio::FileInfo>()
                .is_none_or(|info| !is_hidden_name(&info.name()))
        });
        let filtered = gtk::FilterListModel::new(Some(dir_list.clone()), Some(filter));

        let sorter = gtk::CustomSorter::new({
            let rule = sort_rule.clone();
            move |a, b| compare_objects(a, b, rule.get())
        });
        let sorted = gtk::SortListModel::new(Some(filtered), Some(sorter));
        // Off, as in P1e-r1: each batch is sorted before the next lands, so the
        // grid's anchor can be an item position when `loading` goes false.
        sorted.set_incremental(false);

        let selection = gtk::MultiSelection::new(Some(sorted.clone()));
        let factory = gtk::SignalListItemFactory::new();
        let grid = gtk::GridView::builder()
            .model(&selection)
            .factory(&factory)
            .min_columns(1)
            .max_columns(1)
            .build();
        grid.add_css_class("gl-grid");

        let scroller = gtk::ScrolledWindow::new();
        scroller.set_child(Some(&grid));
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_width(false);
        scroller.set_propagate_natural_height(false);
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);

        let root = gtk::Overlay::new();
        root.set_child(Some(&scroller));
        let error = gtk::Label::new(None);
        error.add_css_class("gl-title");
        error.set_justify(gtk::Justification::Center);
        error.set_wrap(true);
        error.set_halign(gtk::Align::Center);
        error.set_valign(gtk::Align::Center);
        error.set_margin_start(scaled(GAP_L, scale));
        error.set_margin_end(scaled(GAP_L, scale));
        error.set_visible(false);
        root.add_overlay(&error);

        let this = Rc::new(Self {
            root,
            scroller,
            grid,
            dir_list,
            sorted,
            selection,
            status,
            error,
            process: Rc::downgrade(process),
            window,
            tab,
            status_gate: status_gate.unwrap_or_else(|| Rc::new(Cell::new(true))),
            current: RefCell::new(None),
            sort_rule,
            started: Cell::new(Instant::now()),
            first_logged: Cell::new(false),
            loaded: Cell::new(false),
            wanted: RefCell::new(Vec::new()),
            selected: Cell::new(0),
            index: RefCell::new(HashMap::new()),
            counts: Cell::new((0, 0)),
            anchors: RefCell::new(HashMap::new()),
            pending_anchor: RefCell::new(None),
            scroll_top: Cell::new(true),
            scale: Rc::new(Cell::new(scale)),
            cell_positions: Rc::new(RefCell::new(Vec::new())),
            scroll_gen: Cell::new(0),
            settle_pending: Rc::new(Cell::new(false)),
            cap_pending: Rc::new(Cell::new(false)),
            context_on_item: Rc::new(Cell::new(false)),
        });

        this.wire_factory(&factory);
        this.wire_signals();
        this.attach_background_drop_target();
        this
    }

    pub fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    fn wire_factory(self: &Rc<Self>, factory: &gtk::SignalListItemFactory) {
        {
            let weak = Rc::downgrade(self);
            let scale = self.scale.clone();
            factory.connect_setup(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let cell = make_cell(scale.get());
                item.set_child(Some(&cell));
                // The model is the selection's truth: GTK's own click
                // selection would run after the cell's gesture and undo a
                // Ctrl-click (Claude's P2i review). `select_item` from
                // `apply_selection` still works; activation still opens.
                item.set_selectable(false);
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let click = gtk::GestureClick::new();
                click.set_button(1);
                let right_item = item.downgrade();
                let item = item.downgrade();
                let drag_item = item.clone();
                let weak = Rc::downgrade(&view);
                click.connect_released(move |gesture, _, _, _| {
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let Some(item) = item.upgrade() else {
                        return;
                    };
                    let Some(info) = item.item().and_downcast::<gio::FileInfo>() else {
                        return;
                    };
                    let modifiers = gesture
                        .current_event()
                        .map(|event| event.modifier_state())
                        .unwrap_or(gdk::ModifierType::empty());
                    view.click_item(info.name().into_os_string(), modifiers);
                });
                cell.add_controller(click);

                let right = gtk::GestureClick::new();
                right.set_button(3);
                let weak = Rc::downgrade(&view);
                right.connect_pressed(move |gesture, _, _, _| {
                    let Some(view) = weak.upgrade() else {
                        return;
                    };
                    let Some(item) = right_item.upgrade() else {
                        return;
                    };
                    let Some(info) = item.item().and_downcast::<gio::FileInfo>() else {
                        return;
                    };
                    view.context_on_item.set(true);
                    // The scroller's own right press sees this flag in the
                    // same event, before this idle runs.
                    let flag = view.context_on_item.clone();
                    glib::idle_add_local_once(move || flag.set(false));
                    view.right_click_item(info.name().into_os_string(), gesture.widget());
                });
                cell.add_controller(right);
                view.setup_cell_drag_source(&cell, drag_item.clone());
                view.setup_cell_drop_target(&cell, drag_item);
            });
        }
        {
            let weak = Rc::downgrade(self);
            let scale = self.scale.clone();
            factory.connect_bind(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(info) = item.item().and_downcast::<gio::FileInfo>() else {
                    return;
                };
                let Some(cell) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                cell.set_size_request(scaled(CELL_W, scale.get()), -1);
                if let Some(image) = cell.first_child().and_downcast::<gtk::Image>() {
                    if info.file_type() == gio::FileType::Directory {
                        image.set_paintable(Some(&icons::svg(MockupIcon::Folder)));
                    } else if let Some(icon) = info.icon() {
                        image.set_from_gicon(&icon);
                    } else {
                        image.set_icon_name(Some("text-x-generic"));
                    }
                }
                if let Some(label) = cell.last_child().and_downcast::<gtk::Label>() {
                    label.set_text(&info.display_name());
                }
                if let Some(view) = weak.upgrade() {
                    let widget: gtk::Widget = cell.upcast();
                    view.map_cell(&widget, item.position());
                }
            });
        }
        {
            let positions = self.cell_positions.clone();
            factory.connect_unbind(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(cell) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let widget: gtk::Widget = cell.upcast();
                positions
                    .borrow_mut()
                    .retain(|(candidate, _)| candidate != &widget);
            });
        }
    }

    fn wire_signals(self: &Rc<Self>) {
        {
            let weak = Rc::downgrade(self);
            self.grid.connect_activate(move |_, position| {
                if let Some(view) = weak.upgrade() {
                    view.open_position(position);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.scroller
                .hadjustment()
                .connect_notify_local(Some("page-size"), move |_, _| {
                    if let Some(view) = weak.upgrade() {
                        view.schedule_cap();
                    }
                });
        }
        {
            let right = gtk::GestureClick::new();
            right.set_button(3);
            let weak = Rc::downgrade(self);
            right.connect_pressed(move |gesture, _, x, y| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                if view.context_on_item.replace(false) {
                    return;
                }
                let Some(folder) = view.current.borrow().clone() else {
                    return;
                };
                view.background_click(folder, gesture.widget(), (x, y));
            });
            self.scroller.add_controller(right);
        }
        {
            let weak = Rc::downgrade(self);
            let adjustment = self.scroller.vadjustment();
            adjustment.connect_value_changed(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.schedule_anchor();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.dir_list
                .connect_items_changed(move |list, _, _, added| {
                    if let Some(view) = weak.upgrade() {
                        if added > 0 && !view.first_logged.replace(true) {
                            crate::log!(
                                "folder",
                                "first items in model: +{added} ({} so far) in {} ms",
                                list.n_items(),
                                view.started.get().elapsed().as_millis()
                            );
                        }
                        view.schedule_settle();
                    }
                });
        }
        {
            let weak = Rc::downgrade(self);
            self.dir_list.connect_loading_notify(move |list| {
                if list.is_loading() {
                    return;
                }
                if let Some(view) = weak.upgrade() {
                    view.finished();
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.dir_list.connect_error_notify(move |list| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                match list.error() {
                    Some(error) => view.show_error(&error.to_string()),
                    None => view.error.set_visible(false),
                }
            });
        }
    }

    /// Show one folder. `restore` is true for a tab switch, a restore or a
    /// back/forward; `anchor` is the model's recorded name when it has one.
    pub fn set_folder(
        &self,
        folder: &Path,
        rule: SortRule,
        anchor: Option<OsString>,
        restore: bool,
        folder_memory: bool,
    ) {
        let folder = folder.to_path_buf();
        // The per-folder memory is for back/forward only, where the model has
        // cleared the tab's anchor. A tab switch uses the tab's own anchor or
        // the top: another tab's place in the same folder must not leak in
        // (Claude's P2i review).
        let stored = restore
            .then(|| {
                anchor.clone().or_else(|| {
                    folder_memory
                        .then(|| self.anchors.borrow().get(&folder).cloned())
                        .flatten()
                })
            })
            .flatten();
        *self.current.borrow_mut() = Some(folder.clone());
        self.sort_rule.set(rule);
        self.started.set(Instant::now());
        self.first_logged.set(false);
        self.loaded.set(false);
        self.wanted.borrow_mut().clear();
        self.selected.set(0);
        self.counts.set((0, 0));
        self.update_status();
        self.error.set_visible(false);
        *self.pending_anchor.borrow_mut() = stored.clone();
        self.scroll_top.set(!restore || stored.is_none());
        self.invalidate_scroll();

        let attributes = sort::attributes(rule);
        if self.dir_list.attributes().as_deref() != Some(attributes.as_str()) {
            self.dir_list.set_attributes(Some(attributes.as_str()));
        }
        if let Some(sorter) = self.sorted.sorter().and_downcast::<gtk::CustomSorter>() {
            sorter.changed(gtk::SorterChange::Different);
        }
        crate::log!(
            "folder",
            "listing {} (sort {} {}, folders first {})",
            folder.display(),
            rule.by.as_str(),
            rule.order.as_str(),
            rule.folders_first
        );
        self.dir_list.set_file(Some(&gio::File::for_path(&folder)));
    }

    /// Re-read the current folder, keeping the selection and the top item.
    pub fn reload(&self) {
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        let rule = self.sort_rule.get();
        let anchor = self.first_visible_name();
        let wanted = self.wanted.borrow().clone();
        let selected = self.selected.get();
        self.set_folder(&folder, rule, anchor, true, true);
        *self.wanted.borrow_mut() = wanted;
        self.selected.set(selected);
        self.update_status();
    }

    /// Re-sort after `view.set-sort`. A new attribute may need a fresh query;
    /// an order-only change re-sorts the model in place.
    pub fn set_sort(self: &Rc<Self>, rule: SortRule) {
        let previous = self.sort_rule.get();
        if previous == rule {
            return;
        }
        let old_attributes = sort::attributes(previous);
        let new_attributes = sort::attributes(rule);
        self.sort_rule.set(rule);
        if old_attributes != new_attributes {
            let anchor = self.first_visible_name();
            *self.pending_anchor.borrow_mut() = anchor;
            self.scroll_top.set(false);
            self.loaded.set(false);
            self.started.set(Instant::now());
            self.first_logged.set(false);
            self.invalidate_scroll();
            self.dir_list.set_attributes(Some(new_attributes.as_str()));
            crate::log!(
                "folder",
                "re-listing for sort {} {} (the attribute list changed)",
                rule.by.as_str(),
                rule.order.as_str()
            );
        } else if let Some(sorter) = self.sorted.sorter().and_downcast::<gtk::CustomSorter>() {
            sorter.changed(gtk::SorterChange::Different);
            self.schedule_settle();
            crate::log!(
                "folder",
                "re-sorted by {} {}",
                rule.by.as_str(),
                rule.order.as_str()
            );
        }
    }

    pub fn set_selection(&self, selection: &FileSelection) {
        *self.wanted.borrow_mut() = selection.names.iter().cloned().collect();
        self.selected.set(selection.names.len());
        self.update_status();
        if self.loaded.get() {
            self.apply_selection();
        }
    }

    /// Turn this grid's share of the one status line on or off. A split tile
    /// only writes it while it is the focused tile.
    pub fn set_status_gate(&self, on: bool) {
        self.status_gate.set(on);
        if on {
            self.update_status();
        }
    }

    pub fn refresh_status(&self) {
        self.update_status();
    }

    /// Put the view back on a tab's anchor without re-reading the folder.
    pub fn restore_anchor(&self, anchor: Option<OsString>, folder_memory: bool) {
        let stored = anchor.or_else(|| {
            folder_memory
                .then(|| {
                    self.current
                        .borrow()
                        .as_ref()
                        .and_then(|folder| self.anchors.borrow().get(folder).cloned())
                })
                .flatten()
        });
        if let Some(name) = stored
            && let Some(position) = self.position_of_name(&name)
        {
            crate::log!(
                "folder",
                "restoring scroll anchor {:?} at item {position}",
                name.to_string_lossy()
            );
            self.scroll_to_item(position);
            return;
        }
        if self.grid.is_mapped() {
            self.scroll_to_item(0);
        }
        self.grid
            .vadjustment()
            .inspect(|adjustment| adjustment.set_value(0.0));
    }

    pub fn apply_scale(self: &Rc<Self>, scale: f64) {
        self.scale.set(scale);
        let children = self.grid.observe_children();
        for position in 0..children.n_items() {
            if let Some(item) = children.item(position).and_downcast::<gtk::Widget>()
                && let Some(cell) = item.first_child()
            {
                cell.set_size_request(scaled(CELL_W, scale), -1);
            }
        }
        self.schedule_cap();
    }

    pub fn listed_names(&self) -> Vec<OsString> {
        (0..self.selection.n_items())
            .filter_map(|position| {
                self.selection
                    .item(position)
                    .and_downcast::<gio::FileInfo>()
                    .map(|info| info.name().into_os_string())
            })
            .collect()
    }

    fn info_for_name(&self, name: &OsString) -> Option<gio::FileInfo> {
        (0..self.selection.n_items())
            .filter_map(|position| self.selection.item(position))
            .filter_map(|object| object.downcast::<gio::FileInfo>().ok())
            .find(|info| info.name().as_os_str() == name.as_os_str())
    }

    /// The model's selection becomes the drag set. An unselected pressed item
    /// is made the only selection first, through the model, exactly as a
    /// plain click would.
    fn drag_items(&self, name: OsString) -> Vec<DragItem> {
        let Some(process) = self.process.upgrade() else {
            return Vec::new();
        };
        if let Some(tab) = self.tab {
            process.focus_tab(self.window, tab);
        }
        let selected = self
            .wanted
            .borrow()
            .iter()
            .any(|candidate| candidate.as_os_str() == name.as_os_str());
        if !selected {
            process.file_click_tab(self.window, self.tab, ClickRequest::Plain(name));
        }
        let Some(folder) = self.current.borrow().clone() else {
            return Vec::new();
        };
        let names = self.wanted.borrow().clone();
        names
            .into_iter()
            .map(|name| {
                let is_dir = self
                    .info_for_name(&name)
                    .is_some_and(|info| info.file_type() == gio::FileType::Directory);
                DragItem {
                    path: folder.join(name),
                    is_dir,
                }
            })
            .collect()
    }

    fn setup_cell_drag_source(
        self: &Rc<Self>,
        cell: &gtk::Box,
        item: glib::WeakRef<gtk::ListItem>,
    ) {
        let source = gtk::DragSource::new();
        source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
        let weak = Rc::downgrade(self);
        let scale = self.scale.clone();
        source.connect_prepare(move |source, _, _| {
            let view = weak.upgrade()?;
            let item = item.upgrade()?;
            let info = item.item().and_downcast::<gio::FileInfo>()?;
            let name = info.name().into_os_string();
            let items = view.drag_items(name.clone());
            if items.is_empty() {
                return None;
            }
            let first = items
                .first()
                .and_then(|item| item.path.file_name())
                .map(std::ffi::OsString::from);
            let icon = first
                .and_then(|name| view.info_for_name(&name))
                .and_then(|info| info.icon());
            let paths: Vec<PathBuf> = items.iter().map(|item| item.path.clone()).collect();
            dnd::set_internal_drag(items.clone());
            let widget = dnd::drag_icon(icon.as_ref(), items.len(), scale.get());
            let paintable = gtk::WidgetPaintable::new(Some(&widget));
            let hot = scaled(ICON_GRID, scale.get()) / 2;
            source.set_icon(Some(&paintable), hot, hot);
            crate::log!(
                "dnd",
                "prepare: {} item(s), first {}",
                items.len(),
                items
                    .first()
                    .map(|item| item.path.display().to_string())
                    .unwrap_or_default()
            );
            dnd::content_for(&paths)
        });
        let weak_end = Rc::downgrade(self);
        source.connect_drag_end(move |_, drag, delete_data| {
            dnd::clear_internal_drag();
            // GTK4 sets `delete_data` when a drop succeeds with MOVE, and
            // Firefox/Zen reports MOVE on every Wayland drop. Honouring it
            // would destroy sources on a copy drag; the drop side moves.
            crate::log!(
                "dnd",
                "drag-end: action={:?} delete_data={} (ignored)",
                drag.selected_action(),
                delete_data
            );
            if let Some(view) = weak_end.upgrade()
                && let Some(process) = view.process.upgrade()
            {
                process.drag_finished(view.window);
            }
        });
        let weak_cancel = Rc::downgrade(self);
        source.connect_drag_cancel(move |_, _, _| {
            dnd::clear_internal_drag();
            if let Some(view) = weak_cancel.upgrade()
                && let Some(process) = view.process.upgrade()
            {
                process.drag_finished(view.window);
            }
            false
        });
        cell.add_controller(source);
    }

    fn setup_cell_drop_target(
        self: &Rc<Self>,
        cell: &gtk::Box,
        item: glib::WeakRef<gtk::ListItem>,
    ) {
        let weak = Rc::downgrade(self);
        let folder: Rc<dyn Fn() -> Option<PathBuf>> = Rc::new(move || {
            let view = weak.upgrade()?;
            let item = item.upgrade()?;
            let info = item.item().and_downcast::<gio::FileInfo>()?;
            if info.file_type() != gio::FileType::Directory {
                return None;
            }
            let folder = view.current.borrow().clone()?;
            Some(folder.join(info.name()))
        });
        dnd::attach_folder_target(
            cell,
            self.process.clone(),
            self.window,
            folder,
            Some("only folders are drop targets"),
            None,
        );
    }

    fn attach_background_drop_target(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let folder: Rc<dyn Fn() -> Option<PathBuf>> =
            Rc::new(move || weak.upgrade()?.current.borrow().clone());
        // The grid is behind every cell. While a cell is under the pointer
        // its own target owns the drop and the grid bows out, so a file cell
        // does not light the whole grid.
        let grid = self.grid.clone();
        let mask: Rc<dyn Fn(f64, f64) -> bool> = Rc::new(move |x, y| {
            let Some(mut widget) = grid.pick(x, y, gtk::PickFlags::DEFAULT) else {
                return true;
            };
            loop {
                if widget.has_css_class("gl-cell") {
                    return false;
                }
                let Some(parent) = widget.parent() else {
                    return true;
                };
                widget = parent;
            }
        });
        dnd::attach_folder_target(
            &self.grid,
            self.process.clone(),
            self.window,
            folder,
            None,
            Some(mask),
        );
    }

    fn click_item(&self, name: OsString, modifiers: gdk::ModifierType) {
        let order = self.listed_names();
        let request = click_request(
            modifiers.contains(gdk::ModifierType::CONTROL_MASK),
            modifiers.contains(gdk::ModifierType::SHIFT_MASK),
            name,
            &order,
        );
        if let Some(process) = self.process.upgrade() {
            process.file_click_tab(self.window, self.tab, request);
        }
    }

    fn right_click_item(&self, name: OsString, anchor: Option<gtk::Widget>) {
        let Some(anchor) = anchor else {
            return;
        };
        if let Some(process) = self.process.upgrade() {
            process.right_click_item(self.window, self.tab, name, &anchor);
        }
    }

    fn background_click(&self, folder: PathBuf, anchor: Option<gtk::Widget>, at: (f64, f64)) {
        let Some(anchor) = anchor else {
            return;
        };
        if let Some(process) = self.process.upgrade() {
            process.right_click_background(self.window, self.tab, folder, &anchor, at);
        }
    }

    fn open_position(&self, position: u32) {
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        let Some(info) = self
            .selection
            .item(position)
            .and_downcast::<gio::FileInfo>()
        else {
            return;
        };
        let path = folder.join(info.name());
        if let Some(process) = self.process.upgrade() {
            process.open_path(self.window, path);
        }
    }

    fn schedule_cap(self: &Rc<Self>) {
        if self.cap_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.cap_pending.set(false);
                view.update_cap();
            }
        });
    }

    fn update_cap(&self) {
        let width = self.scroller.hadjustment().page_size() as i32;
        let cap = max_columns(width, self.scale.get()).clamp(1, MAX_COLUMNS as i32) as u32;
        if self.grid.max_columns() != cap {
            self.grid.set_max_columns(cap);
        }
        crate::log!(
            "folder",
            "viewport {width} px at scale {:.2} -> cap {cap}",
            self.scale.get()
        );
    }

    fn schedule_anchor(self: &Rc<Self>) {
        let generation = self.scroll_gen.get() + 1;
        self.scroll_gen.set(generation);
        let weak = Rc::downgrade(self);
        // No SourceId is kept: `SourceId::remove` panics once a source has
        // fired, and a navigation can race a timer that is already due. The
        // generation makes every older timer a harmless no-op instead.
        glib::timeout_add_local_once(Duration::from_millis(ANCHOR_DEBOUNCE_MS), move || {
            if let Some(view) = weak.upgrade()
                && view.scroll_gen.get() == generation
            {
                view.record_anchor();
            }
        });
    }

    fn invalidate_scroll(&self) {
        // Older 300 ms timers see the new generation and do nothing.
        self.scroll_gen.set(self.scroll_gen.get() + 1);
    }

    fn record_anchor(&self) {
        let Some(name) = self.first_visible_name() else {
            return;
        };
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        self.anchors.borrow_mut().insert(folder, name.clone());
        if let Some(process) = self.process.upgrade() {
            process.record_scroll_anchor_for(self.window, self.tab, name);
        }
    }

    fn first_visible_name(&self) -> Option<OsString> {
        let height = self.grid.height().max(1) as f32;
        let mut best: Option<(f32, u32)> = None;
        let positions = self.cell_positions.borrow();
        let mut child = self.grid.first_child();
        while let Some(widget) = child {
            let cell = widget.first_child().unwrap_or_else(|| widget.clone());
            let position = positions
                .iter()
                .find(|(candidate, _)| candidate == &cell)
                .or_else(|| positions.iter().find(|(candidate, _)| candidate == &widget))
                .map(|(_, position)| *position);
            if let Some(position) = position
                && let Some(bounds) = widget.compute_bounds(&self.grid)
            {
                let top = bounds.y();
                if top + bounds.height() > 0.0
                    && top < height
                    && best.is_none_or(|(best_top, _)| top < best_top)
                {
                    best = Some((top, position));
                }
            }
            child = widget.next_sibling();
        }
        best.and_then(|(_, position)| self.name_at(position))
    }

    fn name_at(&self, position: u32) -> Option<OsString> {
        self.selection
            .item(position)
            .and_downcast::<gio::FileInfo>()
            .map(|info| info.name().into_os_string())
    }

    /// `GridView::scroll_to` asserts the position is in the list
    /// (`gtkgridview.c`), so an empty folder or a stale index position must
    /// not reach it (Claude's P2i review: opening an empty folder logged a
    /// Gtk-CRITICAL).
    fn scroll_to_item(&self, position: u32) {
        if position < self.selection.n_items() {
            self.grid
                .scroll_to(position, gtk::ListScrollFlags::NONE, None);
        }
    }

    fn restore_scroll(&self) {
        let anchor = self.pending_anchor.borrow_mut().take();
        let top = self.scroll_top.replace(false);
        if let Some(name) = anchor.as_ref() {
            if let Some(position) = self
                .index
                .borrow()
                .get(name.as_os_str())
                .copied()
                .or_else(|| self.position_of_name(name))
            {
                crate::log!(
                    "folder",
                    "restoring scroll anchor {:?} at item {position}",
                    name.to_string_lossy()
                );
                self.scroll_to_item(position);
                return;
            }
            crate::log!(
                "folder",
                "scroll anchor {:?} is gone; opening at the top",
                name.to_string_lossy()
            );
        }
        if top || anchor.is_none() {
            if self.grid.is_mapped() {
                self.scroll_to_item(0);
            }
            self.grid
                .vadjustment()
                .inspect(|adjustment| adjustment.set_value(0.0));
        }
    }

    fn position_of_name(&self, name: &OsString) -> Option<u32> {
        (0..self.selection.n_items()).find(|position| {
            self.selection
                .item(*position)
                .and_downcast::<gio::FileInfo>()
                .is_some_and(|info| info.name().as_os_str() == name.as_os_str())
        })
    }

    fn finished(&self) {
        if let Some(error) = self.dir_list.error() {
            self.show_error(&error.to_string());
            return;
        }
        self.loaded.set(true);
        crate::log!(
            "folder",
            "listing complete: {} items ({} visible) in {} ms",
            self.dir_list.n_items(),
            self.selection.n_items(),
            self.started.get().elapsed().as_millis()
        );
        self.rebuild_index();
        self.apply_selection();
        self.count_listed();
        self.update_status();
        self.restore_scroll();
    }

    fn show_error(&self, message: &str) {
        if self.error.is_visible() {
            return;
        }
        let path = self
            .current
            .borrow()
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        crate::log!("folder", "cannot read {path}: {message}");
        self.error.set_text(message);
        self.error.set_visible(true);
        self.counts.set((0, 0));
        self.selected.set(0);
        self.update_status();
    }

    fn schedule_settle(self: &Rc<Self>) {
        if !self.loaded.get() || self.settle_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(view) = weak.upgrade() {
                view.settle_pending.set(false);
                if view.loaded.get() {
                    view.rebuild_index();
                    view.apply_selection();
                    view.count_listed();
                    view.update_status();
                }
            }
        });
    }

    fn rebuild_index(&self) {
        let mut index = HashMap::with_capacity(self.selection.n_items() as usize);
        for position in 0..self.selection.n_items() {
            if let Some(info) = self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
            {
                index.insert(info.name().into_os_string(), position);
            }
        }
        *self.index.borrow_mut() = index;
    }

    fn apply_selection(&self) {
        let names = self.wanted.borrow().clone();
        let total = self.selection.n_items();
        if names.is_empty() {
            self.selection.unselect_all();
            return;
        }
        if total > 0 && names.len() == total as usize {
            self.selection.select_all();
            return;
        }
        self.selection.unselect_all();
        let index = self.index.borrow();
        if !index.is_empty() {
            for name in &names {
                if let Some(position) = index.get(name.as_os_str()) {
                    self.selection.select_item(*position, false);
                }
            }
            return;
        }
        for position in 0..total {
            let Some(info) = self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
            else {
                continue;
            };
            if names
                .iter()
                .any(|name| name.as_os_str() == info.name().as_os_str())
            {
                self.selection.select_item(position, false);
            }
        }
    }

    fn count_listed(&self) {
        let mut folders: u32 = 0;
        let total = self.selection.n_items();
        for position in 0..total {
            if let Some(info) = self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
                && info.file_type() == gio::FileType::Directory
            {
                folders += 1;
            }
        }
        self.counts
            .set((u64::from(total - folders), u64::from(folders)));
    }

    fn update_status(&self) {
        if !self.status_gate.get() {
            return;
        }
        let (files, folders) = self.counts.get();
        self.status.set_counts(crate::folder::status_text(
            files,
            folders,
            self.selected.get(),
        ));
    }

    fn map_cell(&self, cell: &gtk::Widget, position: u32) {
        let mut cells = self.cell_positions.borrow_mut();
        if let Some((_, slot)) = cells.iter_mut().find(|(candidate, _)| candidate == cell) {
            *slot = position;
        } else {
            cells.push((cell.clone(), position));
        }
    }
}

fn make_cell(scale: f64) -> gtk::Box {
    let cell = gtk::Box::new(gtk::Orientation::Vertical, 0);
    cell.add_css_class("gl-cell");
    cell.set_size_request(scaled(CELL_W, scale), -1);
    let icon = gtk::Image::new();
    let label = gtk::Label::new(None);
    label.set_justify(gtk::Justification::Center);
    label.set_wrap(true);
    // WordChar wraps ordinary names at word boundaries and still breaks a
    // long unbroken run, so no name can widen the column (P1e).
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_lines(2);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_width_chars(LABEL_CHARS);
    label.set_max_width_chars(LABEL_CHARS);
    cell.append(&icon);
    cell.append(&label);
    cell
}

fn compare_objects(a: &glib::Object, b: &glib::Object, rule: SortRule) -> gtk::Ordering {
    let (Some(a), Some(b)) = (
        a.downcast_ref::<gio::FileInfo>(),
        b.downcast_ref::<gio::FileInfo>(),
    ) else {
        return gtk::Ordering::Equal;
    };
    let a_name = a.display_name();
    let b_name = b.display_name();
    // Read only the attribute the current sort needs: the listing asks for no
    // others, and GIO logs a critical for a missing one.
    let a_kind = match rule.by {
        SortBy::Type => a.content_type().unwrap_or_default(),
        _ => glib::GString::default(),
    };
    let b_kind = match rule.by {
        SortBy::Type => b.content_type().unwrap_or_default(),
        _ => glib::GString::default(),
    };
    let a_key = sort::EntryKey {
        name: a_name.as_str(),
        is_dir: a.file_type() == gio::FileType::Directory,
        modified: if rule.by == SortBy::Modified {
            a.modification_date_time()
                .map(|date| date.to_unix())
                .unwrap_or(0)
        } else {
            0
        },
        size: if rule.by == SortBy::Size { a.size() } else { 0 },
        kind: a_kind.as_str(),
    };
    let b_key = sort::EntryKey {
        name: b_name.as_str(),
        is_dir: b.file_type() == gio::FileType::Directory,
        modified: if rule.by == SortBy::Modified {
            b.modification_date_time()
                .map(|date| date.to_unix())
                .unwrap_or(0)
        } else {
            0
        },
        size: if rule.by == SortBy::Size { b.size() } else { 0 },
        kind: b_kind.as_str(),
    };
    match sort::compare(&a_key, &b_key, rule) {
        std::cmp::Ordering::Less => gtk::Ordering::Smaller,
        std::cmp::Ordering::Equal => gtk::Ordering::Equal,
        std::cmp::Ordering::Greater => gtk::Ordering::Larger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_column_cap_follows_the_viewport_and_never_is_g_maxuint() {
        assert_eq!(max_columns(1544, 1.0), 13);
        assert_eq!(max_columns(695, 1.0), 6);
        assert_eq!(max_columns(0, 1.0), 2);
        assert!(max_columns(1544, 0.5) >= 2 * 11);
        assert!(max_columns(100_000, 1.0) <= 64);
    }
}
