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
use fm_core::folder_size::RootIdentity;
use fm_core::preview::{self as core_preview, FileRevision};
use fm_core::workspace::{FileSelection, TabId, WindowId};

use crate::app::Process;
use crate::dnd::{self, DragItem};
use crate::folder::{
    ClickRequest, GridKey, KeyboardFocus, NavMode, SelectionEntry, StatusLine, click_request,
    details_text, is_hidden_name, keyboard_move, nav_mode, size, sort, visible_page_rows,
};
use crate::look::icons::{self, MockupIcon};
use crate::look::tokens::*;

/// How long after a change Gimbal first compares GTK's listing with the disk,
/// and how long it waits before treating a disagreement as a stall.
const STALL_FIRST: Duration = Duration::from_millis(800);
const STALL_SECOND: Duration = Duration::from_millis(1500);

/// Whether any changed name is listed when it's gone from disk, or missing
/// from the list when it's on disk. Each pair is (listed, on disk).
fn listing_disagrees(checks: &[(bool, bool)]) -> bool {
    checks.iter().any(|(listed, on_disk)| listed != on_disk)
}
use crate::thumbs::{self, ThumbKey, ThumbKind, Thumbnailer};

const MAX_COLUMNS: u32 = 64;
const ANCHOR_DEBOUNCE_MS: u64 = 300;
const THUMB_POLL_MS: u64 = 30;

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
    /// The visible root: the administrator banner above the grid body.
    root: gtk::Box,
    body: gtk::Overlay,
    banner: adw::Banner,
    scroller: gtk::ScrolledWindow,
    grid: gtk::GridView,
    /// GTK's listing of the shown folder. Replaced by a fresh list on reload:
    /// a `GtkDirectoryList` whose event queue has stalled never recovers
    /// (see `watch_folder`).
    dir_list: RefCell<gtk::DirectoryList>,
    dir_list_handlers: RefCell<Vec<glib::SignalHandlerId>>,
    filtered: gtk::FilterListModel,
    /// This view, for handlers wired to a replacement `dir_list`.
    me: RefCell<Weak<FolderView>>,
    /// Gimbal's own watch on the shown folder. It only notices when GTK's
    /// listing has stopped applying changes; it never edits the list.
    watch: RefCell<Option<gio::FileMonitor>>,
    /// Names changed since the last stall check.
    stall_names: RefCell<Vec<OsString>>,
    stall_gen: Cell<u64>,
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
    /// The keyboard's place in the visible order. It is a file name and
    /// separate from the model's selection, so applying a model selection never
    /// looks like a new user move and a re-sort never moves it to another file.
    cursor: RefCell<KeyboardFocus>,
    thumbnails: Rc<RefCell<Thumbnailer>>,
    thumb_bindings: RefCell<HashMap<u64, ThumbBinding>>,
    thumb_poll: RefCell<Option<glib::SourceId>>,

    size_service: Rc<size::Service>,
    cell_bindings: RefCell<HashMap<gtk::Widget, CellBinding>>,
    size_hover: RefCell<Option<SizeHover>>,
    size_hover_delay: RefCell<size::HoverDelay>,
    size_active: RefCell<Option<SizeActive>>,
    size_poll: RefCell<Option<glib::SourceId>>,
    /// Makes superseded hover timers, retries and results harmless.
    size_gen: Cell<u64>,
    size_hover_gen: Cell<u64>,
}

#[derive(Clone)]
struct CellBinding {
    name: OsString,
    display_name: String,
    modified: String,
    revision: FileRevision,
    root: RootIdentity,
    details: String,
    folder: bool,
}

#[derive(Clone)]
struct SizeHover {
    cell: gtk::Widget,
    generation: u64,
}

struct SizeActive {
    cell: gtk::Widget,
    model: size::TooltipModel,
    started: Option<size::Started>,
    last_progress_version: u64,
    progress_throttle: size::ProgressThrottle,
    generation: u64,
    retry_pending: bool,
    keyboard: bool,
}

struct ThumbBinding {
    cell: glib::WeakRef<gtk::Widget>,
    item: glib::WeakRef<gtk::ListItem>,
    name: OsString,
    revision: FileRevision,
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
        let sorted = gtk::SortListModel::new(Some(filtered.clone()), Some(sorter));
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

        let body = gtk::Overlay::new();
        body.set_child(Some(&scroller));
        let error = gtk::Label::new(None);
        error.add_css_class("gl-title");
        error.set_justify(gtk::Justification::Center);
        error.set_wrap(true);
        error.set_halign(gtk::Align::Center);
        error.set_valign(gtk::Align::Center);
        error.set_margin_start(scaled(GAP_L, scale));
        error.set_margin_end(scaled(GAP_L, scale));
        error.set_visible(false);
        body.add_overlay(&error);

        let banner = adw::Banner::new("Administrator: changes here affect the whole system.");
        banner.add_css_class("gl-admin-banner");
        banner.set_button_label(Some("Leave"));
        banner.set_revealed(false);

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.append(&banner);
        root.append(&body);

        let this = Rc::new(Self {
            root,
            body,
            banner,
            scroller,
            grid,
            dir_list: RefCell::new(dir_list),
            dir_list_handlers: RefCell::new(Vec::new()),
            filtered,
            me: RefCell::new(Weak::new()),
            watch: RefCell::new(None),
            stall_names: RefCell::new(Vec::new()),
            stall_gen: Cell::new(0),
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
            cursor: RefCell::new(KeyboardFocus::default()),
            thumbnails: Rc::new(RefCell::new(Thumbnailer::new())),
            thumb_bindings: RefCell::new(HashMap::new()),
            thumb_poll: RefCell::new(None),
            size_service: size::service(),
            cell_bindings: RefCell::new(HashMap::new()),
            size_hover: RefCell::new(None),
            size_hover_delay: RefCell::new(size::HoverDelay::default()),
            size_active: RefCell::new(None),
            size_poll: RefCell::new(None),
            size_gen: Cell::new(0),
            size_hover_gen: Cell::new(0),
        });

        *this.me.borrow_mut() = Rc::downgrade(&this);
        this.wire_factory(&factory);
        this.wire_signals();
        this.attach_background_drop_target();
        {
            let weak = Rc::downgrade(&this);
            this.banner.connect_button_clicked(move |_| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let Some(process) = view.process.upgrade() else {
                    return;
                };
                if let Some(ui) = process.window(view.window) {
                    ui.dispatch(
                        fm_core::command::Caller::Ui,
                        fm_core::command::Invocation::new("tab.leave-admin", Vec::new()),
                    );
                }
            });
        }
        let weak = Rc::downgrade(&this);
        this.root.connect_destroy(move |_| {
            if let Some(view) = weak.upgrade() {
                view.cancel_size_all();
            }
        });
        this
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    /// Whether the tab this view shows is an administrator tab. A split tile
    /// names its own tab; the single grid follows the focused tab.
    fn tab_admin(&self) -> bool {
        let Some(process) = self.process.upgrade() else {
            return false;
        };
        let Some(window) = process.window_snapshot(self.window) else {
            return false;
        };
        let id = self.tab.unwrap_or(window.focused_tab);
        window.tab(id).is_some_and(|tab| tab.admin)
    }

    /// Show the banner and the red tile border while the tab is an
    /// administrator tab, and remove both in place when it leaves.
    fn refresh_admin_scope(&self) {
        let admin = self.tab_admin();
        self.banner.set_revealed(admin);
        self.grid.set_tooltip_text(admin.then_some(
            "Dragging, recursive search and thumbnails are off in an administrator tab. Alt-click a file you don't own in the preview to edit it as administrator.",
        ));
        let mut parent = self.root.parent();
        while let Some(widget) = parent {
            if widget.has_css_class("gl-tile") {
                if admin {
                    widget.add_css_class("admin");
                } else {
                    widget.remove_css_class("admin");
                }
                break;
            }
            parent = widget.parent();
        }
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
                    let name = info.name().into_os_string();
                    view.cursor.borrow_mut().set(name.clone());
                    view.grid.grab_focus();
                    view.click_item(name, modifiers);
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
                // Query-tooltip identifies the bound item from the cell's
                // own binding, never from the pointer's position.
                let query_weak = Rc::downgrade(&view);
                cell.connect_query_tooltip(move |cell, _, _, keyboard, tooltip| {
                    let Some(view) = query_weak.upgrade() else {
                        return false;
                    };
                    let widget: gtk::Widget = cell.clone().upcast();
                    let Some(text) = view.query_tooltip_text(&widget, keyboard) else {
                        return false;
                    };
                    tooltip.set_text(Some(&text));
                    true
                });
                view.attach_size_motion(&cell);
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
                let details = details_for(&info);
                cell.set_has_tooltip(true);
                cell.update_property(&[gtk::accessible::Property::Description(&details)]);
                // Reset every recycled cell before its own binding is
                // validated. A thumbnail from A must never remain under B.
                if let Some(image) = cell.first_child().and_downcast::<gtk::Image>() {
                    show_type_icon(&image, &info);
                }
                if let Some(label) = cell.last_child().and_downcast::<gtk::Label>() {
                    label.set_text(&info.display_name());
                    label.update_property(&[
                        gtk::accessible::Property::Label(&info.display_name()),
                        gtk::accessible::Property::Description(&details),
                    ]);
                }
                let Some(view) = weak.upgrade() else {
                    return;
                };
                let name = info.name().into_os_string();
                let widget: gtk::Widget = cell.clone().upcast();
                view.bind_size_cell(&widget, &info, &details);
                view.map_cell(&widget, item.position());
                view.update_label_mode(&widget, &name);
                view.request_thumbnail(&info, &widget, item, view.scale.get());
            });
        }
        {
            let positions = self.cell_positions.clone();
            let weak = Rc::downgrade(self);
            factory.connect_unbind(move |_, object| {
                let Some(item) = object.downcast_ref::<gtk::ListItem>() else {
                    return;
                };
                let Some(cell) = item.child().and_downcast::<gtk::Box>() else {
                    return;
                };
                let widget: gtk::Widget = cell.clone().upcast();
                positions
                    .borrow_mut()
                    .retain(|(candidate, _)| candidate != &widget);
                if let Some(view) = weak.upgrade() {
                    view.cancel_thumbnail_for(&widget);
                    view.unbind_size_cell(&widget);
                }
                if let Some(image) = cell.first_child().and_downcast::<gtk::Image>() {
                    image.set_paintable(None::<&gdk::Paintable>);
                }
                cell.set_has_tooltip(false);
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
            let key = gtk::EventControllerKey::new();
            key.set_propagation_phase(gtk::PropagationPhase::Capture);
            let weak = Rc::downgrade(self);
            key.connect_key_pressed(move |_, key, _, modifiers| {
                let Some(view) = weak.upgrade() else {
                    return glib::Propagation::Proceed;
                };
                view.key_pressed(key, modifiers)
            });
            self.grid.add_controller(key);
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
            // GTK 4.22 has no built-in key that opens a tooltip. If a
            // keyboard/assistive query does reach the focused grid, resolve it
            // through the focused cell's binding and start the same states.
            let weak = Rc::downgrade(self);
            self.grid
                .connect_query_tooltip(move |_, _, _, keyboard, tooltip| {
                    if !keyboard {
                        return false;
                    }
                    let Some(view) = weak.upgrade() else {
                        return false;
                    };
                    let Some(text) = view.focused_tooltip_text() else {
                        return false;
                    };
                    tooltip.set_text(Some(&text));
                    true
                });
        }
        {
            // A keyboard/assistive tooltip demand belongs to the focused grid;
            // moving focus away dismisses/cancels it without a pointer.
            let focus = gtk::EventControllerFocus::new();
            let weak = Rc::downgrade(self);
            focus.connect_leave(move |_| {
                if let Some(view) = weak.upgrade() {
                    let keyboard = view
                        .size_active
                        .borrow()
                        .as_ref()
                        .is_some_and(|active| active.keyboard);
                    if keyboard {
                        view.cancel_size_active();
                    }
                }
            });
            self.grid.add_controller(focus);
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
        let list = self.dir_list.borrow().clone();
        self.connect_dir_list(&list);
    }

    /// Wire one `DirectoryList`'s signals to this view, remembering the
    /// handler ids so a replaced list can be disconnected.
    fn connect_dir_list(self: &Rc<Self>, list: &gtk::DirectoryList) {
        let mut handlers = Vec::new();
        {
            let weak = Rc::downgrade(self);
            handlers.push(list.connect_items_changed(move |list, _, _, added| {
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
            }));
        }
        {
            let weak = Rc::downgrade(self);
            handlers.push(list.connect_loading_notify(move |list| {
                if list.is_loading() {
                    return;
                }
                if let Some(view) = weak.upgrade() {
                    view.finished();
                }
            }));
        }
        {
            let weak = Rc::downgrade(self);
            handlers.push(list.connect_error_notify(move |list| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                match list.error() {
                    Some(error) => view.show_error(&error.to_string()),
                    None => view.error.set_visible(false),
                }
            }));
        }
        *self.dir_list_handlers.borrow_mut() = handlers;
    }

    /// Swap in a fresh `DirectoryList` with the same attributes. GTK clears a
    /// list's queued monitor events only in `dispose`
    /// (gtk-4.22.4:gtk/gtkdirectorylist.c:255-272), so `set_file` on the same
    /// list cannot undo a stalled queue. The caller sets the folder again.
    fn fresh_dir_list(&self) {
        let Some(me) = self.me.borrow().upgrade() else {
            return;
        };
        let old = self.dir_list.borrow().clone();
        for id in self.dir_list_handlers.borrow_mut().drain(..) {
            old.disconnect(id);
        }
        old.set_file(None::<&gio::File>);
        let attributes = old.attributes();
        let list = gtk::DirectoryList::new(attributes.as_deref(), None::<&gio::File>);
        me.connect_dir_list(&list);
        *self.dir_list.borrow_mut() = list.clone();
        self.filtered.set_model(Some(&list));
    }

    /// Watch the shown folder ourselves, to notice when GtkDirectoryList stops
    /// applying changes. GTK 4.22 queues each monitor event and handles them
    /// in order; a "created" event whose file is gone before GTK's query
    /// returns (a browser's `.part` file) is never handled, and every later
    /// event waits behind it forever
    /// (gtk-4.22.4:gtk/gtkdirectorylist.c `handle_event`: `if (!info) return
    /// FALSE`). A rename or move then left the old row in the grid (Kyle,
    /// Nov 8, in `~/Downloads`). Administrator views are skipped: they are
    /// re-read after every administrator job, and root-only paths can't be
    /// checked from here.
    fn watch_folder(&self, folder: &Path, scope: fm_core::admin::Scope) {
        self.stop_watching();
        if scope == fm_core::admin::Scope::Admin {
            return;
        }
        let Ok(monitor) = gio::File::for_path(folder)
            .monitor_directory(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let weak = self.me.borrow().clone();
        monitor.connect_changed(move |_, file, other, event| {
            if let Some(view) = weak.upgrade() {
                view.note_change(file, other, event);
            }
        });
        *self.watch.borrow_mut() = Some(monitor);
    }

    fn stop_watching(&self) {
        self.stall_gen.set(self.stall_gen.get() + 1);
        self.stall_names.borrow_mut().clear();
        if let Some(monitor) = self.watch.borrow_mut().take() {
            monitor.cancel();
        }
    }

    fn note_change(
        &self,
        file: &gio::File,
        other: Option<&gio::File>,
        event: gio::FileMonitorEvent,
    ) {
        use gio::FileMonitorEvent as Event;
        let files: Vec<&gio::File> = match event {
            Event::Created | Event::MovedIn | Event::Deleted | Event::MovedOut => vec![file],
            Event::Renamed => std::iter::once(file).chain(other).collect(),
            _ => return,
        };
        {
            let mut names = self.stall_names.borrow_mut();
            for file in files {
                if let Some(name) = file.basename() {
                    names.push(name.into_os_string());
                }
            }
        }
        self.schedule_stall_check(STALL_FIRST, false);
    }

    /// Compare GTK's listing with the disk for the names that changed. A first
    /// disagreement may just be GTK's query still running, so check once more;
    /// a second one means the list has stalled, and it is replaced.
    fn schedule_stall_check(&self, delay: Duration, second: bool) {
        let generation = self.stall_gen.get() + 1;
        self.stall_gen.set(generation);
        let weak = self.me.borrow().clone();
        glib::timeout_add_local_once(delay, move || {
            let Some(view) = weak.upgrade() else {
                return;
            };
            if view.stall_gen.get() != generation {
                return;
            }
            let Some(folder) = view.current.borrow().clone() else {
                return;
            };
            let list = view.dir_list.borrow().clone();
            if list.is_loading() {
                view.schedule_stall_check(STALL_FIRST, second);
                return;
            }
            let listed: std::collections::HashSet<OsString> = (0..list.n_items())
                .filter_map(|position| list.item(position).and_downcast::<gio::FileInfo>())
                .map(|info| info.name().into_os_string())
                .collect();
            let names = view.stall_names.borrow().clone();
            let checks: Vec<(bool, bool)> = names
                .iter()
                .map(|name| {
                    (
                        listed.contains(name),
                        std::fs::symlink_metadata(folder.join(name)).is_ok(),
                    )
                })
                .collect();
            if !listing_disagrees(&checks) {
                view.stall_names.borrow_mut().clear();
                return;
            }
            if !second {
                view.schedule_stall_check(STALL_SECOND, true);
                return;
            }
            crate::log!(
                "folder",
                "GTK's listing of {} stopped following changes ({} name(s) disagree with the disk); re-reading",
                folder.display(),
                checks
                    .iter()
                    .filter(|(listed, on_disk)| listed != on_disk)
                    .count()
            );
            view.reload();
        });
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
        self.cancel_all_thumbnails();
        self.cancel_size_all();
        self.cell_bindings.borrow_mut().clear();
        self.cell_positions.borrow_mut().clear();
        self.cursor.borrow_mut().reset();
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
        let list = self.dir_list.borrow().clone();
        if list.attributes().as_deref() != Some(attributes.as_str()) {
            list.set_attributes(Some(attributes.as_str()));
        }
        if let Some(sorter) = self.sorted.sorter().and_downcast::<gtk::CustomSorter>() {
            sorter.changed(gtk::SorterChange::Different);
        }
        self.refresh_admin_scope();
        let scope = if self.tab_admin() {
            fm_core::admin::Scope::Admin
        } else {
            fm_core::admin::Scope::User
        };
        crate::log!(
            "folder",
            "listing {} ({} scope, sort {} {}, folders first {})",
            folder.display(),
            scope.as_str(),
            rule.by.as_str(),
            rule.order.as_str(),
            rule.folders_first
        );
        list.set_file(Some(&fm_core::admin::gfile(&folder, scope)));
        self.watch_folder(&folder, scope);
    }

    /// Drop this view's folder monitor, thumbnail demand and folder-size walk
    /// when its folder is under a mount that is about to be unmounted. The
    /// relocation or a terminal-failure refresh gives it a new folder.
    pub fn release_root(&self, root: &Path) -> bool {
        let Some(current) = self.current.borrow().clone() else {
            self.cancel_size_all();
            self.cancel_all_thumbnails();
            self.thumbnails.borrow().release_root(root);
            return false;
        };
        if !current.starts_with(root) {
            return false;
        }
        self.cancel_size_all();
        self.cancel_all_thumbnails();
        self.thumbnails.borrow().release_root(root);
        let list = self.dir_list.borrow().clone();
        list.set_file(None::<&gio::File>);
        self.stop_watching();
        *self.current.borrow_mut() = None;
        self.loaded.set(false);
        *self.cell_bindings.borrow_mut() = HashMap::new();
        *self.cell_positions.borrow_mut() = Vec::new();
        self.wanted.borrow_mut().clear();
        self.counts.set((0, 0));
        crate::log!(
            "folder",
            "released listing and handles under {} for {}",
            root.display(),
            current.display()
        );
        true
    }

    /// Re-read the folder if this view shows an administrator tab. GtkDirectoryList
    /// ignores `G_FILE_MONITOR_EVENT_MOVED` (gtk-4.22.4:gtk/gtkdirectorylist.c:790),
    /// and GVfs's admin backend reports a rename that way, so after an
    /// administrator file operation the old name stayed listed beside the new
    /// one. Re-reading after each administrator job keeps the grid true.
    pub fn reload_if_admin(&self) -> bool {
        if !self.tab_admin() {
            return false;
        }
        self.reload();
        true
    }

    /// Re-read the current folder, keeping the selection and the top item.
    pub fn reload(&self) {
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        // An explicit Refresh asks for a fresh total immediately.
        size::invalidate_path(&folder);
        // gtk_directory_list_set_file returns early for an equal GFile
        // (gtk-4.22.4:gtk/gtkdirectorylist.c, g_file_equal), so setting the
        // same folder again re-read nothing while set_folder reset this view's
        // own counts and selection: stale rows and a stuck highlight. A fresh
        // list also drops a stalled event queue, which `set_file` cannot.
        self.fresh_dir_list();
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
            let list = self.dir_list.borrow().clone();
            list.set_attributes(Some(new_attributes.as_str()));
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
            crate::log_verbose!(
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
        let items = self.grid.observe_children();
        for position in 0..items.n_items() {
            if let Some(item) = items.item(position).and_downcast::<gtk::ListItem>()
                && let Some(info) = item.item().and_downcast::<gio::FileInfo>()
                && let Some(cell) = item.child()
            {
                self.request_thumbnail(&info, &cell, &item, scale);
            }
        }
    }

    /// Give the grid keyboard focus after a tile-frame click. The cell click
    /// already does this; the empty part of a split tile needs the same.
    pub fn grab_grid_focus(&self) {
        self.grid.grab_focus();
    }

    /// A 500 ms continuous-hover timer per cell. The tooltip may appear
    /// earlier through GTK browse mode, but no recursive work starts before
    /// this timer's own fresh dwell.
    fn attach_size_motion(self: &Rc<Self>, cell: &gtk::Box) {
        let motion = gtk::EventControllerMotion::new();
        let weak_enter = Rc::downgrade(self);
        let enter_cell = cell.downgrade();
        motion.connect_enter(move |_, _, _| {
            let Some(view) = weak_enter.upgrade() else {
                return;
            };
            let Some(cell) = enter_cell.upgrade() else {
                return;
            };
            let widget: gtk::Widget = cell.upcast();
            view.hover_enter(&widget);
        });
        let weak_leave = Rc::downgrade(self);
        let leave_cell = cell.downgrade();
        motion.connect_leave(move |_| {
            let Some(view) = weak_leave.upgrade() else {
                return;
            };
            let Some(cell) = leave_cell.upgrade() else {
                return;
            };
            let widget: gtk::Widget = cell.upcast();
            view.hover_leave(&widget);
        });
        cell.add_controller(motion);
    }

    fn bind_size_cell(&self, cell: &gtk::Widget, info: &gio::FileInfo, details: &str) {
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        let name = info.name().into_os_string();
        let mut path = folder.join(&name);
        if !path.is_absolute() {
            path = std::path::absolute(&path).unwrap_or(path);
        }
        let binding = CellBinding {
            name,
            display_name: info.display_name().to_string(),
            modified: modified_text(info),
            revision: revision_from_info(info),
            root: RootIdentity {
                path,
                device: u64::from(info.attribute_uint32("unix::device")),
                inode: info.attribute_uint64("unix::inode"),
                modified_sec: info.attribute_uint64("time::modified") as i64,
                modified_nsec: (info.attribute_uint32("time::modified-usec") as i64) * 1_000,
            },
            details: details.to_string(),
            folder: info.file_type() == gio::FileType::Directory,
        };
        self.cell_bindings
            .borrow_mut()
            .insert(cell.clone(), binding);
    }

    fn unbind_size_cell(&self, cell: &gtk::Widget) {
        self.cell_bindings.borrow_mut().remove(cell);
        let hovered = self
            .size_hover
            .borrow()
            .as_ref()
            .is_some_and(|hover| hover.cell == *cell);
        if hovered {
            *self.size_hover.borrow_mut() = None;
            self.size_hover_gen.set(self.size_hover_gen.get() + 1);
            self.size_hover_delay.borrow_mut().leave();
        }
        let active = self
            .size_active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.cell == *cell);
        if active {
            self.cancel_size_active();
        }
    }

    fn focused_tooltip_text(self: &Rc<Self>) -> Option<String> {
        let name = self
            .cursor
            .borrow()
            .name()
            .map(std::ffi::OsStr::to_os_string)
            .or_else(|| self.first_selected_name())?;
        let position = self.position_of_name(&name)?;
        let cell = self
            .cell_positions
            .borrow()
            .iter()
            .find(|(_, candidate)| *candidate == position)
            .map(|(cell, _)| cell.clone())?;
        self.query_tooltip_text(&cell, true)
    }

    fn query_tooltip_text(self: &Rc<Self>, cell: &gtk::Widget, keyboard: bool) -> Option<String> {
        let binding = self.cell_bindings.borrow().get(cell).cloned()?;
        if !binding.folder {
            return Some(binding.details);
        }
        if let Some(text) = self.active_tooltip_text(cell, &binding) {
            return Some(text);
        }
        if keyboard {
            // A keyboard/assistive request carries no pointer position; the
            // cell's own binding says which item it belongs to.
            self.start_size(cell, &binding, true);
            if let Some(text) = self.active_tooltip_text(cell, &binding) {
                return Some(text);
            }
        }
        Some(binding.details)
    }

    fn active_tooltip_text(&self, cell: &gtk::Widget, binding: &CellBinding) -> Option<String> {
        let active = self.size_active.borrow();
        let active = active.as_ref()?;
        (active.cell == *cell
            && active
                .model
                .matches(&binding.root, &binding.name, &binding.revision))
        .then(|| active.model.text())
    }

    fn hover_enter(self: &Rc<Self>, cell: &gtk::Widget) {
        let Some(binding) = self.cell_bindings.borrow().get(cell).cloned() else {
            return;
        };
        if !binding.folder {
            return;
        }
        if self
            .size_active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.cell == *cell)
        {
            return;
        }
        let generation = self.size_hover_gen.get() + 1;
        self.size_hover_gen.set(generation);
        self.size_hover_delay
            .borrow_mut()
            .enter(self.size_service.now_ms());
        *self.size_hover.borrow_mut() = Some(SizeHover {
            cell: cell.clone(),
            generation,
        });
        self.cancel_size_active();
        let weak = Rc::downgrade(self);
        let cell = cell.clone();
        glib::timeout_add_local_once(Duration::from_millis(size::HOVER_DELAY_MS), move || {
            if let Some(view) = weak.upgrade() {
                view.hover_due(&cell, generation);
            }
        });
    }

    fn hover_due(self: &Rc<Self>, cell: &gtk::Widget, generation: u64) {
        let current = self
            .size_hover
            .borrow()
            .as_ref()
            .is_some_and(|hover| hover.cell == *cell && hover.generation == generation);
        if !current
            || !self
                .size_hover_delay
                .borrow()
                .due(self.size_service.now_ms())
        {
            return;
        }
        let Some(binding) = self.cell_bindings.borrow().get(cell).cloned() else {
            return;
        };
        if !binding.folder {
            return;
        }
        self.start_size(cell, &binding, false);
        self.refresh_size_tooltip(cell);
    }

    fn hover_leave(&self, cell: &gtk::Widget) {
        let hovered = self
            .size_hover
            .borrow()
            .as_ref()
            .is_some_and(|hover| hover.cell == *cell);
        if hovered {
            *self.size_hover.borrow_mut() = None;
            self.size_hover_gen.set(self.size_hover_gen.get() + 1);
            self.size_hover_delay.borrow_mut().leave();
        }
        let active = self
            .size_active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.cell == *cell);
        if active {
            self.cancel_size_active();
        }
    }

    fn start_size(self: &Rc<Self>, cell: &gtk::Widget, binding: &CellBinding, keyboard: bool) {
        if !binding.folder {
            return;
        }
        self.cancel_size_active();
        let now = self.size_service.now_ms();
        let identity = size::TooltipIdentity {
            root: binding.root.clone(),
            name: binding.name.clone(),
            revision: binding.revision.clone(),
        };
        let mut model = size::TooltipModel::new(
            identity,
            binding.display_name.clone(),
            binding.modified.clone(),
        );
        crate::log_verbose!(
            "folder-size",
            "request {} (device {}, inode {})",
            binding.root.path.display(),
            binding.root.device,
            binding.root.inode
        );
        match self.size_service.request(binding.root.clone()) {
            size::RequestOutcome::Cached(hit) => {
                let age = now.saturating_sub(hit.completed_at_ms);
                crate::log_verbose!(
                    "folder-size",
                    "cache hit {}: {} contents, {} files, {} folders, age {} ms",
                    binding.root.path.display(),
                    fm_core::folder_size::format_bytes(hit.size.totals.contents_bytes),
                    hit.size.totals.files,
                    hit.size.totals.folders,
                    age
                );
                model.finish(fm_core::folder_size::ScanResult::Sized(hit.size), age, true);
                self.install_size_active(cell, model, None, keyboard, false);
            }
            size::RequestOutcome::Started(started) => {
                let stale = started.stale.is_some();
                model.start(started.stale.clone(), now);
                crate::log_verbose!(
                    "folder-size",
                    "start {} (stale shown: {stale})",
                    binding.root.path.display()
                );
                self.install_size_active(cell, model, Some(started), keyboard, false);
                self.ensure_size_poll();
            }
            size::RequestOutcome::Busy => {
                crate::log_verbose!(
                    "folder-size",
                    "queue full for {}; the current demand will retry",
                    binding.root.path.display()
                );
                model.start(None, now);
                self.install_size_active(cell, model, None, keyboard, true);
                self.schedule_size_retry(cell);
            }
            size::RequestOutcome::Stopped => {
                crate::log_verbose!(
                    "folder-size",
                    "service stopped for {}; showing unavailable",
                    binding.root.path.display()
                );
                model.finish(
                    fm_core::folder_size::ScanResult::Unavailable(
                        fm_core::folder_size::RootState::Unavailable,
                    ),
                    0,
                    false,
                );
                self.install_size_active(cell, model, None, keyboard, false);
            }
        }
    }

    fn install_size_active(
        &self,
        cell: &gtk::Widget,
        model: size::TooltipModel,
        started: Option<size::Started>,
        keyboard: bool,
        retry_pending: bool,
    ) {
        *self.size_active.borrow_mut() = Some(SizeActive {
            cell: cell.clone(),
            model,
            started,
            last_progress_version: 0,
            progress_throttle: size::ProgressThrottle::default(),
            generation: self.size_gen.get(),
            retry_pending,
            keyboard,
        });
    }

    fn cancel_size_active(&self) {
        let Some(active) = self.size_active.borrow_mut().take() else {
            return;
        };
        if let Some(started) = &active.started {
            self.size_service.cancel(started);
            crate::log_verbose!(
                "folder-size",
                "cancel {} (token {})",
                active.model.identity.root.path.display(),
                started.token
            );
        }
        self.size_gen.set(self.size_gen.get() + 1);
    }

    fn refresh_size_tooltip(&self, cell: &gtk::Widget) {
        let presentation = {
            let active = self.size_active.borrow();
            active
                .as_ref()
                .map(|active| (active.model.text(), active.keyboard))
        };
        let Some((text, keyboard)) = presentation else {
            return;
        };
        cell.update_property(&[gtk::accessible::Property::Description(&text)]);
        if let Some(label) = cell.last_child().and_downcast::<gtk::Label>() {
            label.update_property(&[gtk::accessible::Property::Description(&text)]);
        }
        if !keyboard {
            cell.trigger_tooltip_query();
        }
    }

    fn ensure_size_poll(self: &Rc<Self>) {
        if self.size_poll.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source =
            glib::timeout_add_local(Duration::from_millis(size::DEMAND_POLL_MS), move || {
                let Some(view) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                view.poll_size()
            });
        *self.size_poll.borrow_mut() = Some(source);
    }

    fn poll_size(self: &Rc<Self>) -> glib::ControlFlow {
        let Some(mut active) = self.size_active.borrow_mut().take() else {
            *self.size_poll.borrow_mut() = None;
            return glib::ControlFlow::Break;
        };
        if active.generation != self.size_gen.get() {
            *self.size_poll.borrow_mut() = None;
            return glib::ControlFlow::Break;
        }
        let cell = active.cell.clone();
        let mut changed = false;
        let mut keep = true;
        let event = active.started.as_ref().and_then(size::Started::try_next);
        if let Some(event) = event {
            let bound_token = active.started.as_ref().map(|started| started.token);
            if bound_token != Some(event.token) {
                crate::log!("folder-size", "rejected a result with another token");
                keep = false;
            }
            if keep {
                let binding = self.cell_bindings.borrow().get(&cell).cloned();
                match binding {
                    Some(binding)
                        if active.model.matches(
                            &binding.root,
                            &binding.name,
                            &binding.revision,
                        ) =>
                    {
                        let now = self.size_service.now_ms();
                        let age = now.saturating_sub(event.completed_at_ms);
                        let label = result_label(&event.result);
                        active.model.finish(event.result, age, event.cached);
                        crate::log_verbose!(
                            "folder-size",
                            "result {label} for {} (age {age} ms, cached {})",
                            active.model.identity.root.path.display(),
                            event.cached
                        );
                        active.started = None;
                        changed = true;
                    }
                    _ => {
                        crate::log_verbose!(
                            "folder-size",
                            "rejected a late result for a cell that no longer binds this folder"
                        );
                        keep = false;
                    }
                }
            }
        }
        if keep && let Some(started) = active.started.as_ref() {
            let (progress, version) = started.progress();
            if version != active.last_progress_version {
                let now = self.size_service.now_ms();
                if active.progress_throttle.should_show(now) {
                    active.last_progress_version = version;
                    active.model.set_progress(progress);
                    crate::log_verbose!(
                        "folder-size",
                        "progress for {}: {} files, {} folders",
                        active.model.identity.root.path.display(),
                        progress.files,
                        progress.folders
                    );
                    changed = true;
                }
            }
        }
        let has_demand = keep && active.started.is_some();
        if keep {
            *self.size_active.borrow_mut() = Some(active);
        }
        if changed {
            self.refresh_size_tooltip(&cell);
        }
        if has_demand {
            glib::ControlFlow::Continue
        } else {
            *self.size_poll.borrow_mut() = None;
            glib::ControlFlow::Break
        }
    }

    fn schedule_size_retry(self: &Rc<Self>, cell: &gtk::Widget) {
        let Some(generation) = self
            .size_active
            .borrow()
            .as_ref()
            .filter(|active| active.cell == *cell)
            .map(|active| active.generation)
        else {
            return;
        };
        let weak = Rc::downgrade(self);
        let cell = cell.clone();
        glib::timeout_add_local_once(Duration::from_millis(150), move || {
            if let Some(view) = weak.upgrade() {
                view.retry_size(&cell, generation);
            }
        });
    }

    fn retry_size(self: &Rc<Self>, cell: &gtk::Widget, generation: u64) {
        let pending = self.size_active.borrow().as_ref().is_some_and(|active| {
            active.cell == *cell && active.generation == generation && active.retry_pending
        });
        if !pending {
            return;
        }
        let keyboard = self
            .size_active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.keyboard);
        let Some(binding) = self.cell_bindings.borrow().get(cell).cloned() else {
            return;
        };
        self.start_size(cell, &binding, keyboard);
        self.refresh_size_tooltip(cell);
    }

    fn cancel_size_all(&self) {
        *self.size_hover.borrow_mut() = None;
        self.size_hover_delay.borrow_mut().leave();
        self.size_hover_gen.set(self.size_hover_gen.get() + 1);
        self.cancel_size_active();
        self.size_gen.set(self.size_gen.get() + 1);
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
        let indexed = self.index.borrow().get(name.as_os_str()).copied();
        if let Some(position) = indexed
            && let Some(info) = self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
            && info.name().as_os_str() == name.as_os_str()
        {
            return Some(info);
        }
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
        // The icon widget is built in prepare and handed to GTK's own
        // `GtkDragIcon` in drag-begin. This cell keeps it alive until the drag
        // ends, so its theme colours and badge survive the whole drag.
        let icon_widget: Rc<RefCell<Option<gtk::Widget>>> = Rc::new(RefCell::new(None));
        {
            let icon_widget = icon_widget.clone();
            let scale = scale.clone();
            source.connect_prepare(move |_, _, _| {
                dnd::prepare_drag();
                icon_widget.borrow_mut().take();
                let view = weak.upgrade()?;
                if view.tab_admin() {
                    crate::log!("dnd", "dragging out is off in an administrator tab");
                    return None;
                }
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
                *icon_widget.borrow_mut() = Some(widget);
                crate::log!(
                    "dnd",
                    "prepare: {} item(s), first {}",
                    items.len(),
                    items
                        .first()
                        .map(|item| item.path.display().to_string())
                        .unwrap_or_default()
                );
                let content = dnd::content_for(&paths);
                if content.is_none() {
                    icon_widget.borrow_mut().take();
                }
                content
            });
        }
        {
            let scale = scale.clone();
            let icon_widget = icon_widget.clone();
            source.connect_drag_begin(move |_, drag| {
                crate::log!(
                    "dnd",
                    "drag-begin: actions {}, selected {}, modifiers {}",
                    dnd::actions_text(drag.actions()),
                    dnd::action_text(dnd::unique_action(drag.selected_action())),
                    dnd::current_modifiers_text()
                );
                drag.connect_selected_action_notify(|drag| {
                    crate::log_verbose!(
                        "dnd",
                        "drag selected-action changed: {} (actions {})",
                        dnd::action_text(dnd::unique_action(drag.selected_action())),
                        dnd::actions_text(drag.actions())
                    );
                });
                let Some(widget) = icon_widget.borrow().clone() else {
                    return;
                };
                let drag_icon = gtk::DragIcon::for_drag(drag);
                drag_icon.set_child(Some(&widget));
                let hot = scaled(ICON_GRID, scale.get()) / 2;
                drag.set_hotspot(hot, hot);
            });
        }
        let weak_end = Rc::downgrade(self);
        let end_icon = icon_widget.clone();
        source.connect_drag_end(move |_, drag, delete_data| {
            let modifiers = dnd::current_modifiers_text();
            dnd::clear_internal_drag();
            end_icon.borrow_mut().take();
            gtk::DragIcon::for_drag(drag).set_child(None::<&gtk::Widget>);
            // GTK4 sets `delete_data` when a drop succeeds with MOVE, and
            // Firefox/Zen reports MOVE on every Wayland drop. Honouring it
            // would destroy sources on a copy drag; the drop side moves.
            crate::log!(
                "dnd",
                "drag-end: selected {:?}, action {}, delete_data={} (ignored), modifiers {}",
                drag.selected_action(),
                dnd::action_text(dnd::unique_action(drag.selected_action())),
                delete_data,
                modifiers
            );
            if let Some(view) = weak_end.upgrade()
                && let Some(process) = view.process.upgrade()
            {
                process.drag_finished(view.window);
            }
        });
        let weak_cancel = Rc::downgrade(self);
        let cancel_icon = icon_widget;
        source.connect_drag_cancel(move |_, drag, _| {
            dnd::clear_internal_drag();
            cancel_icon.borrow_mut().take();
            gtk::DragIcon::for_drag(drag).set_child(None::<&gtk::Widget>);
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
            if view.tab_admin() {
                return None;
            }
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

    /// Every cell's bounds in the viewport root's coordinates. `compute_bounds`
    /// accounts for the grid's scroll transform, so the same rectangles work
    /// for a single tab and for a split tile.
    fn cell_rects(&self) -> Vec<dnd::DropRect> {
        let mut rects = Vec::new();
        let mut child = self.grid.first_child();
        while let Some(widget) = child {
            if let Some(bounds) = widget.compute_bounds(&self.body) {
                rects.push(dnd::DropRect {
                    x: f64::from(bounds.x()),
                    y: f64::from(bounds.y()),
                    width: f64::from(bounds.width()),
                    height: f64::from(bounds.height()),
                });
            }
            child = widget.next_sibling();
        }
        rects
    }

    fn attach_background_drop_target(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        let folder: Rc<dyn Fn() -> Option<PathBuf>> = Rc::new(move || {
            let view = weak.upgrade()?;
            if view.tab_admin() {
                return None;
            }
            view.current.borrow().clone()
        });
        // The viewport root owns the background, not the grid: the grid's own
        // allocation ends at its content, which leaves the padding and the
        // space below a short listing without a target. A cell still owns its
        // own point, so a file cell's refusal never becomes a background
        // transfer.
        let weak_rects = Rc::downgrade(self);
        let mask: Rc<dyn Fn(f64, f64) -> bool> = Rc::new(move |x, y| {
            let Some(view) = weak_rects.upgrade() else {
                return false;
            };
            dnd::background_owns(x, y, &view.cell_rects())
        });
        dnd::attach_folder_target(
            &self.body,
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
        crate::log_verbose!(
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
                crate::log_verbose!(
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
        if let Some(position) = self.index.borrow().get(name.as_os_str()).copied()
            && self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
                .is_some_and(|info| info.name().as_os_str() == name.as_os_str())
        {
            return Some(position);
        }
        (0..self.selection.n_items()).find(|position| {
            self.selection
                .item(*position)
                .and_downcast::<gio::FileInfo>()
                .is_some_and(|info| info.name().as_os_str() == name.as_os_str())
        })
    }

    fn finished(&self) {
        let list = self.dir_list.borrow().clone();
        if let Some(error) = list.error() {
            self.show_error(&error.to_string());
            return;
        }
        self.loaded.set(true);
        crate::log!(
            "folder",
            "listing complete: {} items ({} visible) in {} ms",
            list.n_items(),
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
                    // A live listing change is a known change to this root's
                    // entries; invalidate its total conservatively.
                    if let Some(folder) = view.current.borrow().clone() {
                        size::invalidate_path(&folder);
                    }
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
            self.cursor.borrow_mut().reset();
            self.update_all_label_modes();
            return;
        }
        if total > 0 && names.len() == total as usize {
            self.selection.select_all();
        } else {
            self.selection.unselect_all();
            let index = self.index.borrow();
            if !index.is_empty() {
                for name in &names {
                    if let Some(position) = index.get(name.as_os_str()) {
                        self.selection.select_item(*position, false);
                    }
                }
                drop(index);
            } else {
                drop(index);
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
        }
        // Applying a passive/model selection must not reset an explicit cursor.
        // Keep it by name while the file is still listed; a removed item falls
        // back to the first selected item in visible order (never a name-sorted
        // set's arbitrary first name).
        let fallback = self.first_selected_name();
        let mut cursor = self.cursor.borrow_mut();
        cursor.reapply(|listed| self.is_listed(listed), fallback);
        drop(cursor);
        self.update_all_label_modes();
    }

    fn first_selected_name(&self) -> Option<OsString> {
        let wanted = self.wanted.borrow();
        (0..self.selection.n_items()).find_map(|position| {
            let info = self
                .selection
                .item(position)
                .and_downcast::<gio::FileInfo>()?;
            wanted
                .iter()
                .any(|name| name.as_os_str() == info.name().as_os_str())
                .then(|| info.name().into_os_string())
        })
    }

    fn is_listed(&self, name: &std::ffi::OsStr) -> bool {
        if self.index.borrow().contains_key(name) {
            return true;
        }
        (0..self.selection.n_items()).any(|position| {
            self.selection
                .item(position)
                .and_downcast::<gio::FileInfo>()
                .is_some_and(|info| info.name().as_os_str() == name)
        })
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
        self.status.set_status(crate::folder::selection_status(
            files,
            folders,
            &self.selection_entries(),
        ));
        let tooltip = self
            .wanted
            .borrow()
            .first()
            .and_then(|name| self.info_for_name(name))
            .map(|info| details_for(&info));
        self.grid.set_tooltip_text(tooltip.as_deref());
        // Keep keyboard/assistive queries reachable even with no selection;
        // the handler only claims them when a focused item resolves.
        self.grid.set_has_tooltip(true);
    }

    fn selection_entries(&self) -> Vec<SelectionEntry> {
        self.wanted
            .borrow()
            .iter()
            .map(|name| {
                let info = self.info_for_name(name);
                SelectionEntry {
                    name: info
                        .as_ref()
                        .map(|info| info.display_name().to_string())
                        .unwrap_or_else(|| name.to_string_lossy().into_owned()),
                    is_folder: info
                        .as_ref()
                        .is_some_and(|info| info.file_type() == gio::FileType::Directory),
                    size: info.as_ref().and_then(|info| {
                        (info.file_type() != gio::FileType::Directory && info.size() >= 0)
                            .then_some(info.size() as u64)
                    }),
                }
            })
            .collect()
    }

    fn update_label_mode(&self, cell: &gtk::Widget, name: &OsString) {
        let Some(label) = cell.last_child().and_downcast::<gtk::Label>() else {
            return;
        };
        let selected = self
            .wanted
            .borrow()
            .iter()
            .any(|candidate| candidate.as_os_str() == name.as_os_str());
        let (lines, ellipsize) = label_mode(selected);
        label.set_lines(lines);
        label.set_ellipsize(ellipsize);
    }

    fn update_all_label_modes(&self) {
        let cells = self.cell_positions.borrow().clone();
        for (cell, position) in cells {
            if let Some(name) = self.name_at(position) {
                self.update_label_mode(&cell, &name);
            }
        }
    }

    fn key_pressed(
        self: &Rc<Self>,
        key: gdk::Key,
        modifiers: gdk::ModifierType,
    ) -> glib::Propagation {
        // A keyboard/assistive request cancels on dismissal like the pointer
        // path; GTK 4.22 emits no tooltip query from a bare key press.
        let keyboard_demand = self
            .size_active
            .borrow()
            .as_ref()
            .is_some_and(|active| active.keyboard);
        if key == gdk::Key::Escape && keyboard_demand {
            self.cancel_size_active();
            return glib::Propagation::Proceed;
        }
        let count = self.selection.n_items();
        if count == 0 {
            return glib::Propagation::Proceed;
        }
        let position = self
            .cursor_position()
            .or_else(|| self.selected_position())
            .unwrap_or(0)
            .min(count - 1);
        let columns = self.column_count().max(1);
        // Page moves use the visible viewport, not the grid's full scrollable
        // height, and the realised row pitch, which grows when a selected name
        // wraps to several lines.
        let rows = visible_page_rows(
            self.scroller.vadjustment().page_size(),
            self.measured_row_height(),
        );
        let move_key = match key {
            gdk::Key::Left => Some(GridKey::Left),
            gdk::Key::Right => Some(GridKey::Right),
            gdk::Key::Up => Some(GridKey::Up),
            gdk::Key::Down => Some(GridKey::Down),
            gdk::Key::Home => Some(GridKey::Home),
            gdk::Key::End => Some(GridKey::End),
            gdk::Key::Page_Up => Some(GridKey::PageUp),
            gdk::Key::Page_Down => Some(GridKey::PageDown),
            _ => None,
        };
        if let Some(key) = move_key {
            let Some(target) = keyboard_move(Some(position), columns, count, key, rows) else {
                return glib::Propagation::Stop;
            };
            let Some(name) = self.name_at(target) else {
                return glib::Propagation::Stop;
            };
            // Set the explicit keyboard focus before the synchronous model
            // refresh reaches apply_selection; it is retained by name.
            self.cursor.borrow_mut().set(name.clone());
            match nav_mode(
                modifiers.contains(gdk::ModifierType::CONTROL_MASK),
                modifiers.contains(gdk::ModifierType::SHIFT_MASK),
            ) {
                NavMode::Plain => {
                    if let Some(process) = self.process.upgrade() {
                        process.file_click_tab(self.window, self.tab, ClickRequest::Plain(name));
                    }
                }
                NavMode::Extend => {
                    // Only a range operation needs the visible order. A plain
                    // arrow reads its target entry directly.
                    let order = self.listed_names();
                    if let Some(process) = self.process.upgrade() {
                        process.file_click_tab(
                            self.window,
                            self.tab,
                            ClickRequest::Shift { name, order },
                        );
                    }
                }
                NavMode::Focus => {}
            }
            self.scroll_to_item(target);
            self.grid.grab_focus();
            return glib::Propagation::Stop;
        }
        if grid_owns_enter(key, modifiers) {
            self.open_position(position);
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn cursor_position(&self) -> Option<u32> {
        let name = self
            .cursor
            .borrow()
            .name()
            .map(std::ffi::OsStr::to_os_string)?;
        self.position_of_name(&name)
    }

    /// The realised row pitch. A selected multi-line label can make the first
    /// row taller; the distance between row tops is then the correct page step.
    fn measured_row_height(&self) -> f64 {
        let cells = self.cell_positions.borrow();
        let mut rows: Vec<(i32, f64)> = Vec::new();
        for (cell, _) in cells.iter() {
            if let Some(bounds) = cell.compute_bounds(&self.grid) {
                let top = bounds.y().round() as i32;
                if !rows.iter().any(|(candidate, _)| *candidate == top) {
                    rows.push((top, f64::from(bounds.height())));
                }
            }
        }
        rows.sort_by_key(|(top, _)| *top);
        if rows.len() >= 2 {
            (f64::from(rows[1].0 - rows[0].0)).max(1.0)
        } else {
            rows.first()
                .map(|(_, height)| (*height).max(1.0))
                .unwrap_or_else(|| f64::from(scaled(CELL_H, self.scale.get())).max(1.0))
        }
    }

    fn selected_position(&self) -> Option<u32> {
        let wanted = self.wanted.borrow();
        (0..self.selection.n_items()).find(|position| {
            self.name_at(*position).is_some_and(|name| {
                wanted
                    .iter()
                    .any(|candidate| candidate.as_os_str() == name.as_os_str())
            })
        })
    }

    /// The real number of columns in the visible grid. GTK exposes only the
    /// cap, so measure the realised first row; the arithmetic fallback is used
    /// only before GTK has allocated any cell.
    fn column_count(&self) -> u32 {
        let cells = self.cell_positions.borrow();
        let mut rows: Vec<(i32, Vec<i32>)> = Vec::new();
        for (cell, _) in cells.iter() {
            if let Some(bounds) = cell.compute_bounds(&self.grid) {
                let y = bounds.y().round() as i32;
                let x = bounds.x().round() as i32;
                if let Some((_, xs)) = rows.iter_mut().find(|(row, _)| *row == y) {
                    xs.push(x);
                } else {
                    rows.push((y, vec![x]));
                }
            }
        }
        let Some((_, xs)) = rows.into_iter().min_by_key(|(y, _)| *y) else {
            return max_columns(
                self.scroller.hadjustment().page_size() as i32,
                self.scale.get(),
            )
            .max(1) as u32;
        };
        let mut xs = xs;
        xs.sort_unstable();
        xs.dedup();
        (xs.len() as u32).max(1)
    }

    fn request_thumbnail(
        self: &Rc<Self>,
        info: &gio::FileInfo,
        cell: &gtk::Widget,
        item: &gtk::ListItem,
        scale: f64,
    ) {
        self.cancel_thumbnail_for(cell);
        if self.tab_admin() {
            return;
        }
        let Some(kind) = thumb_kind(info) else {
            return;
        };
        let Some(folder) = self.current.borrow().clone() else {
            return;
        };
        let name = info.name().into_os_string();
        let path = folder.join(&name);
        let revision = revision_from_info(info);
        let device = f64::from(cell.scale_factor().max(1));
        let edge = (f64::from(scaled(ICON_GRID, scale)) * device)
            .round()
            .max(64.0) as i32;
        let target = core_preview::render_plan(edge, edge, 1.0).size;
        let key = ThumbKey {
            path: path.clone(),
            revision: revision.clone(),
            width: target.width,
            height: target.height,
            kind,
        };
        crate::log_verbose!(
            "thumb",
            "request {:?} {}x{} for {}",
            kind,
            target.width,
            target.height,
            path.display()
        );
        let Some(image) = cell.first_child().and_downcast::<gtk::Image>() else {
            return;
        };
        if let Some(texture) = thumbs::cached(&key) {
            image.set_paintable(Some(&texture));
            return;
        }
        let candidates = if kind == ThumbKind::Video {
            core_preview::thumbnail_candidates_unchecked(info)
        } else {
            Vec::new()
        };
        if kind == ThumbKind::Video && candidates.is_empty() {
            return;
        }
        let token = {
            let mut thumbnailer = self.thumbnails.borrow_mut();
            thumbnailer.request(key, path, candidates)
        };
        let Ok((token, _scheduled)) = token else {
            return;
        };
        self.thumb_bindings.borrow_mut().insert(
            token,
            ThumbBinding {
                cell: cell.downgrade(),
                item: item.downgrade(),
                name,
                revision,
            },
        );
        self.ensure_thumb_poll();
    }

    fn cancel_thumbnail_for(&self, cell: &gtk::Widget) {
        let mut remove = Vec::new();
        for (token, binding) in self.thumb_bindings.borrow().iter() {
            if binding
                .cell
                .upgrade()
                .is_some_and(|candidate| candidate == *cell)
            {
                remove.push(*token);
            }
        }
        for token in remove {
            self.thumbnails.borrow().cancel(token);
            self.thumb_bindings.borrow_mut().remove(&token);
        }
    }

    fn cancel_all_thumbnails(&self) {
        let tokens: Vec<u64> = self.thumb_bindings.borrow().keys().copied().collect();
        for token in tokens {
            self.thumbnails.borrow().cancel(token);
        }
        self.thumb_bindings.borrow_mut().clear();
    }

    fn ensure_thumb_poll(self: &Rc<Self>) {
        if self.thumb_poll.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(Duration::from_millis(THUMB_POLL_MS), move || {
            let Some(view) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            view.poll_thumbnails()
        });
        *self.thumb_poll.borrow_mut() = Some(source);
    }

    fn poll_thumbnails(self: &Rc<Self>) -> glib::ControlFlow {
        while let Some(event) = self.thumbnails.borrow().try_next() {
            let Some(binding) = self.thumb_bindings.borrow_mut().remove(&event.token) else {
                continue;
            };
            if event.key.revision != binding.revision {
                continue;
            }
            let Some(item) = binding.item.upgrade() else {
                continue;
            };
            let Some(info) = item.item().and_downcast::<gio::FileInfo>() else {
                continue;
            };
            if !thumbnail_binding_matches(
                binding.name.as_os_str(),
                &binding.revision,
                info.name().as_os_str(),
                &revision_from_info(&info),
            ) {
                continue;
            }
            let Some(cell) = binding.cell.upgrade() else {
                continue;
            };
            let Some(image) = cell.first_child().and_downcast::<gtk::Image>() else {
                continue;
            };
            if let Some(failure) = &event.failure {
                crate::log_verbose!("thumb", "no thumbnail for {:?}: {failure}", binding.name);
            }
            if let Some(image_data) = event.image {
                let key = event.key.clone();
                thumbs::insert(key.clone(), &image_data);
                if let Some(texture) = thumbs::cached(&key) {
                    image.set_paintable(Some(&texture));
                }
            }
        }
        // Queue pressure is temporary: retry visible deferred demand whenever
        // a poll runs, without requiring an unbind/rebind. Pending work is the
        // live/deferred token state, not a separately decremented counter.
        self.thumbnails.borrow().retry_deferred();
        if self.thumbnails.borrow().has_work() {
            glib::ControlFlow::Continue
        } else {
            // A stopped worker or a cancelled batch cannot leave the view
            // polling forever. Any leftover binding has no live token.
            self.thumb_bindings.borrow_mut().clear();
            *self.thumb_poll.borrow_mut() = None;
            glib::ControlFlow::Break
        }
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

impl Drop for FolderView {
    fn drop(&mut self) {
        if let Some(active) = self.size_active.get_mut().take()
            && let Some(started) = &active.started
        {
            self.size_service.cancel(started);
        }
    }
}

fn modified_text(info: &gio::FileInfo) -> String {
    info.modification_date_time()
        .and_then(|date| date.format("%Y-%m-%d %H:%M").ok())
        .map(|date| date.to_string())
        .unwrap_or_else(|| "Unavailable".to_string())
}

fn result_label(result: &fm_core::folder_size::ScanResult) -> &'static str {
    use fm_core::folder_size::{RootState, ScanResult};
    match result {
        ScanResult::Sized(size) if size.is_partial() => "partial",
        ScanResult::Sized(_) => "complete",
        ScanResult::Cancelled => "cancelled",
        ScanResult::Unavailable(RootState::Remote) => "remote",
        ScanResult::Unavailable(RootState::Symlink) => "symlink root",
        ScanResult::Unavailable(_) => "unavailable",
    }
}

fn grid_owns_enter(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    // The grid handles keys in capture. Modified Enter must continue to the
    // window's configured shortcuts, including Alt+Enter for Properties.
    let keyboard_modifiers = gdk::ModifierType::SHIFT_MASK
        | gdk::ModifierType::CONTROL_MASK
        | gdk::ModifierType::ALT_MASK
        | gdk::ModifierType::SUPER_MASK
        | gdk::ModifierType::HYPER_MASK
        | gdk::ModifierType::META_MASK;
    matches!(key, gdk::Key::Return | gdk::Key::KP_Enter)
        && !modifiers.intersects(keyboard_modifiers)
}

fn label_mode(selected: bool) -> (i32, gtk::pango::EllipsizeMode) {
    if selected {
        (0, gtk::pango::EllipsizeMode::None)
    } else {
        (3, gtk::pango::EllipsizeMode::End)
    }
}

/// A recycled GTK cell may be rebound to A then B while A's thumbnail is still
/// in flight. Apply a result only when both the exact name and revision still
/// match the binding that requested it.
fn thumbnail_binding_matches(
    binding_name: &std::ffi::OsStr,
    binding_revision: &FileRevision,
    current_name: &std::ffi::OsStr,
    current_revision: &FileRevision,
) -> bool {
    binding_name == current_name && binding_revision == current_revision
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
    // long unbroken run. The width belongs to the cell; no character-count
    // truncation hides an ordinary name (Kyle, Nov 3).
    label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    label.set_lines(3);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // One character is a width floor, not a text limit: the label is
    // allocated the cell's width and wraps/ellipsizes there, so a long
    // unbroken name cannot widen the grid column or hide ordinary names.
    label.set_width_chars(1);
    label.set_max_width_chars(1);
    cell.append(&icon);
    cell.append(&label);
    cell
}

fn show_type_icon(image: &gtk::Image, info: &gio::FileInfo) {
    image.set_paintable(None::<&gdk::Paintable>);
    if info.file_type() == gio::FileType::Directory {
        image.set_paintable(Some(&icons::svg(MockupIcon::Folder)));
    } else if let Some(icon) = info.icon() {
        image.set_from_gicon(&icon);
    } else {
        image.set_icon_name(Some("text-x-generic"));
    }
}

fn revision_from_info(info: &gio::FileInfo) -> FileRevision {
    FileRevision {
        device: u64::from(info.attribute_uint32("unix::device")),
        inode: info.attribute_uint64("unix::inode"),
        size: info.size().max(0) as u64,
        modified_sec: info.attribute_uint64("time::modified") as i64,
        modified_nsec: (info.attribute_uint32("time::modified-usec") as i64) * 1_000,
        changed_sec: 0,
        changed_nsec: 0,
        etag: info
            .attribute_string("etag::value")
            .map(|value| value.to_string())
            .filter(|value| !value.is_empty()),
    }
}

fn thumb_kind(info: &gio::FileInfo) -> Option<ThumbKind> {
    if info.file_type() == gio::FileType::Directory {
        return None;
    }
    let mime = info.content_type().unwrap_or_default();
    if mime == "application/pdf" {
        return Some(ThumbKind::Pdf);
    }
    if mime.starts_with("image/") {
        return Some(ThumbKind::Image);
    }
    if mime.starts_with("video/") {
        return Some(ThumbKind::Video);
    }
    let extension = info
        .name()
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())?;
    if extension == "pdf" {
        Some(ThumbKind::Pdf)
    } else if matches!(
        extension.as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "avif" | "jxl"
    ) {
        Some(ThumbKind::Image)
    } else if matches!(
        extension.as_str(),
        "mp4" | "m4v" | "webm" | "mkv" | "mov" | "avi" | "ogv" | "ogg"
    ) {
        Some(ThumbKind::Video)
    } else {
        None
    }
}

fn details_for(info: &gio::FileInfo) -> String {
    let folder = info.file_type() == gio::FileType::Directory;
    let kind = if folder {
        "Folder".to_string()
    } else {
        info.content_type()
            .map(|mime| mime.to_string())
            .unwrap_or_else(|| "File".to_string())
    };
    let modified = info
        .modification_date_time()
        .and_then(|date| date.format("%Y-%m-%d %H:%M").ok())
        .map(|date| date.to_string());
    if folder {
        return format!(
            "{}\nType: Folder\nSize: Folder\nModified: {}",
            info.display_name(),
            modified.as_deref().unwrap_or("Unavailable")
        );
    }
    let size = (info.size() >= 0).then_some(info.size() as u64);
    details_text(&info.display_name(), &kind, size, modified.as_deref())
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
    fn a_listing_disagrees_only_when_a_changed_name_differs_from_the_disk() {
        // Renamed away but still listed: the stalled-queue case.
        assert!(listing_disagrees(&[(true, false), (false, true)]));
        assert!(listing_disagrees(&[(true, false)]));
        assert!(listing_disagrees(&[(false, true)]));
        // A short-lived file that is gone and unlisted, and a listed file that
        // exists: both agree.
        assert!(!listing_disagrees(&[(false, false), (true, true)]));
        assert!(!listing_disagrees(&[]));
    }

    #[test]
    fn modified_enter_reaches_the_window_shortcuts() {
        for key in [gdk::Key::Return, gdk::Key::KP_Enter] {
            assert!(grid_owns_enter(key, gdk::ModifierType::empty()));
            assert!(grid_owns_enter(key, gdk::ModifierType::LOCK_MASK));
            assert!(!grid_owns_enter(key, gdk::ModifierType::ALT_MASK));
            assert!(!grid_owns_enter(key, gdk::ModifierType::CONTROL_MASK));
            assert!(!grid_owns_enter(key, gdk::ModifierType::SHIFT_MASK));
        }
    }

    #[test]
    fn the_column_cap_follows_the_viewport_and_never_is_g_maxuint() {
        assert_eq!(max_columns(1544, 1.0), 13);
        assert_eq!(max_columns(695, 1.0), 6);
        assert_eq!(max_columns(0, 1.0), 2);
        assert!(max_columns(1544, 0.5) >= 2 * 11);
        assert!(max_columns(100_000, 1.0) <= 64);
    }

    #[test]
    fn selected_names_get_more_lines_and_a_recycled_binding_rejects_old_data() {
        assert_eq!(label_mode(true), (0, gtk::pango::EllipsizeMode::None));
        assert_eq!(label_mode(false), (3, gtk::pango::EllipsizeMode::End));

        let revision = FileRevision {
            device: 1,
            inode: 2,
            size: 3,
            modified_sec: 4,
            modified_nsec: 0,
            changed_sec: 0,
            changed_nsec: 0,
            etag: None,
        };
        assert!(thumbnail_binding_matches(
            std::ffi::OsStr::new("a.png"),
            &revision,
            std::ffi::OsStr::new("a.png"),
            &revision
        ));
        assert!(!thumbnail_binding_matches(
            std::ffi::OsStr::new("a.png"),
            &revision,
            std::ffi::OsStr::new("b.png"),
            &revision
        ));
        let mut newer = revision.clone();
        newer.size = 4;
        assert!(!thumbnail_binding_matches(
            std::ffi::OsStr::new("a.png"),
            &revision,
            std::ffi::OsStr::new("a.png"),
            &newer
        ));
    }
}
