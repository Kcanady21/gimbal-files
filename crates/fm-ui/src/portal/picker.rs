// SPDX-License-Identifier: GPL-3.0-or-later
//! The Open dialog itself.
//!
//! A picker is an ordinary model window in the workspace — one group, one tab,
//! one folder — with a small view of its own instead of `WindowUi`: a nav row,
//! the main app's `Sidebar` and `FolderView`, a status line, and a footer. The
//! model gives it navigation, history, selection and sorting for free; the
//! registry entry below is how the rest of the app knows the window is read-only.
//!
//! The portal service ([`super::service`]) hands each call a [`Request`]. The
//! picker answers it exactly once: accept, cancel, Close from the portal, or a
//! window destroyed some other way.
//!
//! What the dialog can do follows the call's options (`P2an`):
//!
//! - **one file** by default, or **several** with `multiple`, in the grid's
//!   order rather than the order they were clicked;
//! - **folders only** with `directory`: files are filtered out of the grid the
//!   way hidden files are, and Open takes the selected folder — or, with
//!   nothing selected, the folder you are in;
//! - **file-type filters** in a dropdown at the left of the footer, matched the
//!   way `GtkFileFilter` matches ([`super::filter`]);
//! - the application's **choices** as a row of checkboxes and dropdowns;
//! - **search**, in the nav row and on Ctrl+F, scoped to the folder the dialog
//!   is showing, with the same rules applied to its results in the worker.
//!
//! It stays what F1 made it: read-only. Nothing here writes a file, the config,
//! or a session, and no agent can reach it.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use fm_core::command::{Caller, CheckedCommand, Invocation, Outcome};
use fm_core::config::{Change, SortRule};
use fm_core::preview::Selection;
use fm_core::search::SearchResultFilter;
use fm_core::workspace::{TabId, Window, WindowId};

use super::filter::{SearchFilter, ViewRules};
use super::service::{self, Call, DialogHost, Pick, Request};
use crate::app::Process;
use crate::folder::{ClickRequest, FolderView, GridFilter, StatusLine};
use crate::look::blur::Blur;
use crate::look::motion;
use crate::look::tokens::*;
use crate::search::{SearchPage, SearchSurface, SearchView};
use crate::sidebar::Sidebar;

thread_local! {
    /// The pickers in this process, keyed by their model window. Pending
    /// entries are options waiting for `build_window` to reach them.
    static PICKERS: RefCell<BTreeMap<WindowId, Entry>> = const { RefCell::new(BTreeMap::new()) };
}

enum Entry {
    /// The options are boxed: they hold a whole parsed `Call`, and the picker
    /// half of this enum is one pointer.
    Pending(Box<PickerOptions>),
    Live(Rc<Picker>),
}

/// What `build_window` needs to construct one picker: the call and the request
/// that must be answered.
pub struct PickerOptions {
    pub request: Rc<Request>,
    pub call: Call,
    /// The application hold taken for this dialog. It is released when the
    /// request is answered (or when the options are dropped, if the window was
    /// never built), never before: the process must outlive its dialog.
    pub hold: gio::ApplicationHoldGuard,
}

/// Park a picker's options for the model window `id`, before `apply_events`
/// builds it.
pub(crate) fn set_pending(id: WindowId, options: PickerOptions) {
    PICKERS.with(|map| {
        map.borrow_mut()
            .insert(id, Entry::Pending(Box::new(options)));
    });
}

/// Take the parked options exactly once, so a rebuild cannot reuse them.
pub(crate) fn take_pending(id: WindowId) -> Option<PickerOptions> {
    PICKERS.with(|map| {
        let mut map = map.borrow_mut();
        if !matches!(map.get(&id), Some(Entry::Pending(_))) {
            return None;
        }
        match map.remove(&id) {
            Some(Entry::Pending(options)) => Some(*options),
            _ => None,
        }
    })
}

/// Whether `id` is a live picker window.
pub(crate) fn is_live(id: WindowId) -> bool {
    PICKERS.with(|map| matches!(map.borrow().get(&id), Some(Entry::Live(_))))
}

/// The picker whose model window is `id`.
pub(crate) fn get(id: WindowId) -> Option<Rc<Picker>> {
    PICKERS.with(|map| match map.borrow().get(&id) {
        Some(Entry::Live(picker)) => Some(picker.clone()),
        _ => None,
    })
}

/// Drop one picker and close its window. Returns whether `id` was a picker.
pub(crate) fn forget(id: WindowId) -> bool {
    let picker = PICKERS.with(|map| map.borrow_mut().remove(&id));
    match picker {
        Some(Entry::Live(picker)) => {
            picker.close_now();
            true
        }
        Some(Entry::Pending(_)) | None => false,
    }
}

/// The names this picker's grid lists, for Select All.
pub(crate) fn listed_names(id: WindowId) -> Option<Vec<OsString>> {
    get(id).map(|picker| picker.folder.listed_names())
}

/// Follow a model change: back/forward/up, the address, the grid, Open and the
/// status line.
pub(crate) fn refresh(process: &Rc<Process>, id: WindowId) {
    let (Some(picker), Some(window)) = (get(id), process.window_snapshot(id)) else {
        return;
    };
    picker.show(process, &window);
}

/// A double-click or Enter on a grid row: a folder is entered, a file is
/// accepted — and in folders-only mode a file is never accepted, because the
/// dialog has no answer for one.
pub(crate) fn open_path(process: &Rc<Process>, id: WindowId, path: PathBuf) {
    let Some(picker) = get(id) else {
        return;
    };
    // A folder row is entered in both modes: picking a folder is Open's job,
    // not a double-click's, as in any file dialog.
    if path.is_dir() {
        process.nav_go_path(id, path);
        return;
    }
    picker.open_row(&path);
}

/// Build one picker for a model window that already exists.
pub(crate) fn build(
    process: &Rc<Process>,
    application: &adw::Application,
    id: WindowId,
    options: PickerOptions,
) -> Rc<Picker> {
    let picker = Picker::new(process, application, id, options);
    PICKERS.with(|map| {
        map.borrow_mut().insert(id, Entry::Live(picker.clone()));
    });
    picker.present();
    picker
}

/// Sort and the hidden-files switch, applied to this dialog's grid only. The
/// picker never writes `config.toml`.
pub(crate) fn apply_setting(
    process: &Rc<Process>,
    id: WindowId,
    command: &CheckedCommand,
) -> Outcome {
    let Some(picker) = get(id) else {
        return Outcome::Failed(format!("{} has no picker window", command.name()));
    };
    match command.name() {
        "view.set-sort" => {
            let folder = process
                .window_snapshot(id)
                .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()));
            let Some(folder) = folder else {
                return Outcome::Failed("the dialog has no folder".to_string());
            };
            let change = {
                let config = process.config.borrow();
                fm_core::config::change_for(command, config.config(), &folder)
            };
            match change {
                Some(Change::SetSort { by, order, .. }) => {
                    picker.folder.set_sort(SortRule {
                        by,
                        order,
                        folders_first: true,
                    });
                    crate::log!(
                        "portal",
                        "dialog sorted by {} {}",
                        by.as_str(),
                        order.as_str()
                    );
                    Outcome::Done
                }
                _ => Outcome::Failed("view.set-sort has no sort rule".to_string()),
            }
        }
        "view.clear-folder-sort" => {
            let rule = process.config.borrow().config().sort.global();
            picker.folder.set_sort(rule);
            Outcome::Done
        }
        "view.set-hidden" => {
            let Some(fm_core::command::Value::Bool(show)) = command.get("enabled") else {
                return Outcome::Failed("view.set-hidden needs a boolean".to_string());
            };
            picker.set_hidden(process, *show);
            Outcome::Done
        }
        "view.toggle-hidden" => {
            let next = !picker.hidden.get();
            picker.set_hidden(process, next);
            Outcome::Done
        }
        _ => Outcome::Failed(format!(
            "{} is not available in the file chooser",
            command.name()
        )),
    }
}

// ---------------------------------------------------------------------------
// What Open will answer: the pure half
// ---------------------------------------------------------------------------

/// What Open would answer, or why it cannot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accept {
    /// The paths to answer with, in the order they are reported.
    Pick(Vec<PathBuf>),
    /// Open is disabled, and this is what the status line says instead of the
    /// folder's counts.
    Refused(&'static str),
}

/// The dialog picks files or folders, one or several, as the call asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    pub multiple: bool,
    pub folders_only: bool,
}

impl Mode {
    /// The mode as the log names it.
    pub fn label(self) -> &'static str {
        match (self.folders_only, self.multiple) {
            (false, false) => "files",
            (false, true) => "files+multiple",
            (true, false) => "folders",
            (true, true) => "folders+multiple",
        }
    }
}

/// Nothing chosen where one is needed, or too many chosen where one is allowed.
pub const ONE_FILE: &str = "Select one file";
/// The same in folders-only mode.
pub const ONE_FOLDER: &str = "Select one folder";
/// Nothing chosen where any number is welcome.
pub const ANY_FILE: &str = "Select at least one file";
/// A folder in a file picker's selection.
pub const NO_FOLDERS: &str = "Folders can't be opened here";
/// A file in a folder picker's selection.
pub const NO_FILES: &str = "Only folders can be selected here";
/// A file the active filter does not show: typed into the address field, or a
/// search result from before the filter changed.
pub const FILTERED: &str = "That file isn't shown by the current filter";
/// A chosen name that is neither a folder nor a regular file any more.
pub const GONE: &str = "That item is no longer there";

/// What Open answers, from the dialog's mode, the folder it is showing, the
/// names its grid lists and the model's selection.
///
/// `uris` follows `listed`, the grid's own order, not the order the items were
/// clicked in: a list the user can read off the screen is easier to check than
/// a history of clicks. Nautilus's rules are the model for the folder cases —
/// `mode_can_accept_files` and `mode_can_accept_current_directory`
/// (`nautilus-file-chooser.c:85-160`) — and a folder picker with nothing chosen
/// takes the folder you are in for the same reason.
///
/// A selected name the grid no longer lists is ignored rather than answered: it
/// is hidden by Ctrl+H or by the filter, and a dialog must not answer with what
/// it is not showing.
pub fn accept_for(
    mode: Mode,
    folder: &Path,
    listed: &[OsString],
    selected: &BTreeSet<OsString>,
) -> Accept {
    let chosen: Vec<PathBuf> = listed
        .iter()
        .filter(|name| selected.contains(*name))
        .map(|name| folder.join(name))
        .collect();
    if chosen.is_empty() {
        return if mode.folders_only {
            Accept::Pick(vec![folder.to_path_buf()])
        } else if mode.multiple {
            Accept::Refused(ANY_FILE)
        } else {
            Accept::Refused(ONE_FILE)
        };
    }
    let folders: Vec<PathBuf> = chosen
        .iter()
        .filter(|path| path.is_dir())
        .cloned()
        .collect();
    let files: Vec<PathBuf> = chosen
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .collect();
    if mode.folders_only {
        if folders.len() != chosen.len() {
            return Accept::Refused(NO_FILES);
        }
        if !mode.multiple && folders.len() > 1 {
            return Accept::Refused(ONE_FOLDER);
        }
        return Accept::Pick(folders);
    }
    if !folders.is_empty() {
        return Accept::Refused(NO_FOLDERS);
    }
    if files.len() != chosen.len() {
        // Something in the way is a socket, a device or a name that was removed
        // after the listing was read.
        return Accept::Refused(GONE);
    }
    if !mode.multiple && files.len() > 1 {
        return Accept::Refused(ONE_FILE);
    }
    Accept::Pick(files)
}

/// What the status line says about that answer. In folders-only mode it always
/// says what Open will pick, because Open is always ready; elsewhere a refusal
/// explains itself and anything else leaves the grid's counts alone.
pub fn status_for(mode: Mode, folder: &Path, accept: &Accept) -> Option<String> {
    match accept {
        Accept::Refused(reason) => Some((*reason).to_string()),
        Accept::Pick(paths) if mode.folders_only => Some(match paths.as_slice() {
            [] => "Will select this folder".to_string(),
            [one] if one == folder => "Will select this folder".to_string(),
            [one] => format!(
                "Will select {:?}",
                one.file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| one.display().to_string())
            ),
            many => format!("Will select {} folders", many.len()),
        }),
        Accept::Pick(_) => None,
    }
}

// ---------------------------------------------------------------------------
// The dialog
// ---------------------------------------------------------------------------

/// One of the application's extra choices, with the widget that holds its value.
enum ChoiceControl {
    /// A choice with no options: `"true"` / `"false"`.
    Check {
        id: String,
        button: gtk::CheckButton,
    },
    /// A choice with options: the option's id, in the order they were sent.
    Options {
        id: String,
        drop: gtk::DropDown,
        ids: Vec<String>,
    },
}

pub(crate) struct Picker {
    id: WindowId,
    process: Weak<Process>,
    window: adw::ApplicationWindow,
    /// The window's nav row and the address entry.
    path: gtk::Box,
    path_entry: gtk::Entry,
    back: gtk::Button,
    forward: gtk::Button,
    up: gtk::Button,
    /// The search button, field and entry, after the address bar.
    search_icon: gtk::Button,
    search_revealer: gtk::Revealer,
    search_field: gtk::Box,
    search_entry: gtk::Entry,
    sidebar: Rc<Sidebar>,
    /// The grid, or the results page over it.
    stack: gtk::Stack,
    folder: Rc<FolderView>,
    /// The results page, built on the first query: see [`Self::page`].
    search_page: RefCell<Option<Rc<SearchPage>>>,
    /// The dialog's scale, kept so the page can be built later at the same size.
    scale: f64,
    status: Rc<StatusLine>,
    /// The footer, so the status line can go away while search holds the page.
    status_row: gtk::Box,
    /// The file-type filter dropdown, and the row of the app's own choices.
    filters: Option<gtk::DropDown>,
    choices: Vec<ChoiceControl>,
    open: gtk::Button,
    /// The folder and tab the grid is showing, so a refresh only reloads when
    /// the model's folder actually changed.
    displayed: RefCell<Option<(TabId, PathBuf)>>,
    /// This dialog's hidden-files switch, independent of `config.toml`.
    hidden: Cell<bool>,
    /// The call, and the mode and rules it asked for.
    call: Call,
    /// What the grid and the search worker keep, read by both on every decision.
    rules: Rc<RefCell<ViewRules>>,
    /// The grid's extra filter, re-supplied whenever `rules` changes.
    grid_filter: GridFilter,
    /// The results page is the visible page, and its entry is open.
    search_showing: Cell<bool>,
    search_open: Cell<bool>,
    /// Set while the entry's text is replaced by code, so the `changed` handler
    /// does not read it as the user clearing search.
    search_programmatic: Cell<bool>,
    request: RefCell<Option<Rc<Request>>>,
    blur: RefCell<Option<Blur>>,
}

impl Picker {
    fn new(
        process: &Rc<Process>,
        application: &adw::Application,
        id: WindowId,
        options: PickerOptions,
    ) -> Rc<Self> {
        let scale = process.scale();
        let opacity = process.opacity();
        let PickerOptions {
            request,
            call,
            hold,
        } = options;
        let mode = Mode {
            multiple: call.options.multiple,
            folders_only: call.options.directory,
        };
        let rules = Rc::new(RefCell::new(ViewRules {
            folders_only: mode.folders_only,
            filter: call.options.start_filter().cloned(),
            show_hidden: process.show_hidden(),
        }));
        let grid_filter: GridFilter = {
            let rules = rules.clone();
            Rc::new(move |info: &gio::FileInfo| rules.borrow().keeps_info(info))
        };

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title(call.title())
            .build();
        window.set_default_size(
            scaled(f64::from(PICKER_W), scale),
            scaled(f64::from(PICKER_H), scale),
        );
        window.add_css_class("gimbal-look");
        // The dialog is modal to the calling application through the portal's
        // parent window; Gimbal's own process has nothing else to keep usable.

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.set_content(Some(&root));

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        row.add_css_class("gl-header");
        row.set_hexpand(true);
        let back = icon_button("go-previous-symbolic");
        let forward = icon_button("go-next-symbolic");
        let up = icon_button("go-up-symbolic");
        row.append(&back);
        row.append(&forward);
        row.append(&up);

        let path = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        path.add_css_class("gl-field");
        path.add_css_class("gl-path");
        path.set_valign(gtk::Align::Center);
        path.set_hexpand(true);
        let path_entry = gtk::Entry::new();
        gtk::prelude::EntryExt::set_alignment(&path_entry, 0.0);
        path_entry.set_hexpand(true);
        path_entry.set_placeholder_text(Some("Path"));
        path.append(&path_entry);
        let refresh = icon_button("view-refresh-symbolic");
        path.append(&refresh);
        row.append(&path);

        // Search, after the address bar, with the main header's own classes so
        // the two look like one control. The button stands where the field will
        // slide in, as it does in the header.
        let search_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_box.add_css_class("gl-search");
        search_box.set_valign(gtk::Align::Center);
        let search_icon = icon_button("system-search-symbolic");
        search_icon.add_css_class("searchbtn");
        let search_field = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        search_field.add_css_class("gl-field");
        search_field.add_css_class("gl-search-field");
        search_field.set_valign(gtk::Align::Center);
        let glyph = gtk::Image::from_icon_name("system-search-symbolic");
        glyph.add_css_class("gl-search-glyph");
        search_field.append(&glyph);
        let search_entry = gtk::Entry::new();
        search_entry.set_placeholder_text(Some("Search"));
        search_entry.set_hexpand(true);
        search_field.append(&search_entry);
        let search_revealer = gtk::Revealer::new();
        // Crossfade, as the header chose: a slide moves the field by its own
        // width and the revealer does not clip.
        search_revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        search_revealer.set_transition_duration(motion::ms(200));
        search_revealer.set_child(Some(&search_field));
        search_field.set_visible(false);
        search_box.append(&search_icon);
        search_box.append(&search_revealer);
        row.append(&search_box);

        let close = crate::look::icons::button(crate::look::icons::MockupIcon::Close);
        close.add_css_class("winbtn");
        close.add_css_class("close");
        close.set_valign(gtk::Align::Center);
        row.append(&close);

        let handle = gtk::WindowHandle::builder().child(&row).build();
        root.append(&handle);

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("gl-frame");
        frame.set_vexpand(true);
        root.append(&frame);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        frame.append(&paned);
        paned.set_resize_start_child(false);
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(true);

        let sidebar = Sidebar::new(process, id, scale);
        paned.set_start_child(Some(sidebar.widget()));
        // The main window's own sidebar width, so the dialog looks like it.
        let left_width = process.config.borrow().config().sidebars.left.width;
        paned.set_position(scaled(f64::from(left_width), scale));

        let status = StatusLine::new();
        let folder = FolderView::new(process, id, status.clone(), scale, None, None);
        // The results page is not built here: a dialog nobody searches should
        // not carry one. `page` builds it on the first query and parents it into
        // this stack, which is why the "search" child does not exist yet.
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(motion::ms(250));
        stack.add_named(folder.widget(), Some("folder"));
        stack.set_visible_child_name("folder");
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.set_vexpand(true);
        right.append(&stack);
        right.append(status.widget());
        paned.set_end_child(Some(&right));

        // The footer: the app's filter and its choices on the left, Cancel and
        // Open on the right. Both halves are one row, so a narrow dialog squeezes
        // the dropdowns rather than clipping the buttons.
        let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        footer.add_css_class("gl-action-row");
        footer.set_spacing(scaled(GAP_S, scale));
        let filters = build_filter_dropdown(&call, &footer);
        let choices = build_choices(&call, &footer, scale);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        footer.append(&spacer);
        let cancel = action_button("Cancel", false);
        let open = action_button(call.accept_label(), true);
        footer.append(&cancel);
        footer.append(&open);
        root.append(&footer);

        let picker = Rc::new(Self {
            id,
            process: Rc::downgrade(process),
            window: window.clone(),
            path,
            path_entry: path_entry.clone(),
            back: back.clone(),
            forward: forward.clone(),
            up: up.clone(),
            search_icon: search_icon.clone(),
            search_revealer: search_revealer.clone(),
            search_field: search_field.clone(),
            search_entry: search_entry.clone(),
            sidebar,
            stack,
            folder: folder.clone(),
            search_page: RefCell::new(None),
            scale,
            status: status.clone(),
            status_row: status.widget().clone(),
            filters,
            choices,
            open: open.clone(),
            displayed: RefCell::new(None),
            hidden: Cell::new(process.show_hidden()),
            call: call.clone(),
            rules,
            grid_filter,
            search_showing: Cell::new(false),
            search_open: Cell::new(false),
            search_programmatic: Cell::new(false),
            request: RefCell::new(Some(request.clone())),
            blur: RefCell::new(None),
        });
        // The grid's rows follow the call's rules from the first listing.
        picker.apply_grid_filter();

        // The request's completion closes this window and releases the
        // application hold. Installed before Present so a Close from the
        // portal in the same tick still finds it.
        let weak_process = Rc::downgrade(process);
        request.set_on_done(move || {
            // Dropping the guard is the release (`gio`'s hold is RAII).
            drop(hold);
            if let Some(process) = weak_process.upgrade() {
                process.close_picker_window(id);
            }
        });

        // Every path out of the dialog answers once.
        window.connect_close_request({
            let weak = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = weak.upgrade() {
                    picker.cancel();
                }
                glib::Propagation::Stop
            }
        });
        window.connect_destroy({
            let weak = Rc::downgrade(&picker);
            move |_| {
                // A destroy from any other route (the model closing the
                // window, the app shutting down) still answers the call.
                if let Some(picker) = weak.upgrade() {
                    picker.cancel();
                }
            }
        });

        connect_nav(&picker, &back, "nav.back");
        connect_nav(&picker, &forward, "nav.forward");
        connect_nav(&picker, &up, "nav.up");
        refresh.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.dispatch(Invocation::new("nav.refresh", Vec::new()));
                }
            }
        });
        close.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.cancel();
                }
            }
        });
        path_entry.connect_activate({
            let picker = Rc::downgrade(&picker);
            move |entry| {
                if let Some(picker) = picker.upgrade() {
                    picker.path_entered(entry);
                }
            }
        });
        cancel.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.cancel();
                }
            }
        });
        open.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.accept();
                }
            }
        });

        // The dropdown, once it holds its starting filter: a change re-filters
        // the grid, drops what the new filter hides, and restarts a query that
        // is on screen.
        if let Some(drop) = picker.filters.clone() {
            let weak = Rc::downgrade(&picker);
            drop.connect_selected_notify(move |drop| {
                let Some(picker) = weak.upgrade() else {
                    return;
                };
                picker.filter_changed(drop.selected() as usize);
            });
        }

        // The search button opens the entry with no query; Enter in the entry
        // asks for one, exactly as the main header does.
        search_icon.connect_clicked({
            let picker = Rc::downgrade(&picker);
            move |_| {
                if let Some(picker) = picker.upgrade() {
                    picker.dispatch(Invocation::new("search.open", Vec::new()));
                }
            }
        });
        search_entry.connect_activate({
            let picker = Rc::downgrade(&picker);
            move |entry| {
                let text = entry.text().trim().to_string();
                if text.is_empty() {
                    // Enter on an empty entry is not a search.
                    return;
                }
                if let Some(picker) = picker.upgrade() {
                    picker.dispatch(Invocation::new(
                        "search.open",
                        vec![("query".to_string(), text)],
                    ));
                }
            }
        });
        search_entry.connect_changed({
            let picker = Rc::downgrade(&picker);
            move |entry| {
                let Some(picker) = picker.upgrade() else {
                    return;
                };
                if entry.text().is_empty()
                    && picker.search_open.get()
                    && !picker.search_programmatic.get()
                    && let Some(process) = picker.process.upgrade()
                {
                    // The query is gone; the entry stays open for another.
                    process.cancel_search(picker.id);
                }
            }
        });

        // Escape closes search first and the dialog second. The navigation
        // shortcuts work whenever neither text entry has focus, so Ctrl+A in a
        // typed path still selects the typed text. The main app's configurable
        // hotkeys are not installed: no file-effect key exists in the dialog.
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(&picker);
        key.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(picker) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if key == gtk::gdk::Key::Escape {
                if picker.search_open.get() {
                    // The first Escape leaves search; the dialog survives it.
                    if let Some(process) = picker.process.upgrade() {
                        process.close_search(picker.id);
                    }
                    return glib::Propagation::Stop;
                }
                picker.cancel();
                return glib::Propagation::Stop;
            }
            // A text entry keeps its editing keys (Ctrl+A/C/X/V/Z, cursor
            // movement, Delete), the main window's rule
            // (`fileops::keys::entry_owns_key`). The dialog's shortcuts —
            // Ctrl+F, Ctrl+H, Ctrl+L, F5 — still work while you type, so
            // Ctrl+H on the results page re-asks the query.
            let typing = entry_focused(&picker.path_entry) || entry_focused(&picker.search_entry);
            if typing && crate::fileops::keys::entry_owns_key(key, modifiers) {
                return glib::Propagation::Proceed;
            }
            match picker_shortcut(key, modifiers) {
                Some(name) => {
                    crate::log!(
                        "portal",
                        "dialog key {name} (search {}, typing {typing})",
                        if picker.search_open.get() {
                            "open"
                        } else {
                            "closed"
                        }
                    );
                    picker.dispatch(Invocation::new(name, Vec::new()));
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        window.add_controller(key);

        *picker.blur.borrow_mut() = Some(Blur::attach(
            window.upcast_ref::<gtk::Window>(),
            scale,
            opacity,
        ));

        // Draw the first state now; the model's WindowOpened event already
        // ran `refresh` before this view existed.
        if let Some(window) = process.window_snapshot(id) {
            picker.show(process, &window);
        }
        picker
    }

    fn present(&self) {
        self.window.present();
        // Focus the grid, not the address entry: the dialog opens ready to
        // receive arrow keys and Enter.
        self.folder.grab_grid_focus();
    }

    /// Destroy the GTK window. The request is answered by `destroy` if nothing
    /// has answered it yet.
    fn close_now(&self) {
        self.window.destroy();
    }

    fn dispatch(&self, invocation: Invocation) {
        if let Some(process) = self.process.upgrade() {
            process.dispatch_for_window(self.id, Caller::Ui, invocation);
        }
    }

    /// The mode the call asked for.
    fn mode(&self) -> Mode {
        Mode {
            multiple: self.call.options.multiple,
            folders_only: self.call.options.directory,
        }
    }

    /// Follow the model: history buttons, the address, the grid, Open and the
    /// status line.
    fn show(&self, process: &Rc<Process>, window: &Window) {
        let view = crate::header::view(window);
        self.back.set_sensitive(view.can_back);
        self.forward.set_sensitive(view.can_forward);
        self.up.set_sensitive(view.can_up);
        if !entry_focused(&self.path_entry) {
            let text = view.folder.display().to_string();
            if self.path_entry.text() != text {
                // A navigation the user asked for wins over a typed path that
                // did not exist.
                self.path.remove_css_class("invalid");
                self.path_entry.set_text(&text);
            }
        }
        let Some(tab) = window.tab(window.focused_tab) else {
            return;
        };
        let changed = self
            .displayed
            .borrow()
            .as_ref()
            .is_none_or(|(tab_id, folder)| *tab_id != tab.id || *folder != tab.folder);
        if changed {
            // A local sort survives while the folder stays; a new folder starts
            // from its own configured rule.
            let rule: SortRule = process.config.borrow().config().sort_for(&tab.folder);
            self.folder
                .set_folder(&tab.folder, rule, tab.scroll_anchor.clone(), true, false);
            *self.displayed.borrow_mut() = Some((tab.id, tab.folder.clone()));
        }
        self.folder.set_selection(&tab.selection);
        let accept = self.accept_now(process);
        self.open.set_sensitive(matches!(accept, Accept::Pick(_)));
        self.show_status(&tab.folder, &accept);
        self.sidebar.set_current_folder(Some(&view.folder));
    }

    /// What Open would answer for the model's current state.
    fn accept_now(&self, process: &Rc<Process>) -> Accept {
        let Some(window) = process.window_snapshot(self.id) else {
            return Accept::Refused(ONE_FILE);
        };
        let Some(tab) = window.tab(window.focused_tab) else {
            return Accept::Refused(ONE_FILE);
        };
        accept_for(
            self.mode(),
            &tab.folder,
            &self.folder.listed_names(),
            &tab.selection.names,
        )
    }

    /// Put the dialog's own words on the status line, or leave the grid's
    /// counts there.
    fn show_status(&self, folder: &Path, accept: &Accept) {
        if let Some(text) = status_for(self.mode(), folder, accept) {
            self.status.set_counts(text);
        }
    }

    /// Re-run the grid's filter over every listed name after the rules changed.
    /// The predicate reads `rules` itself, so re-supplying it is only the nudge
    /// GTK needs to ask again.
    fn apply_grid_filter(&self) {
        self.folder.set_grid_filter(Some(self.grid_filter.clone()));
    }

    /// The active filter, or `None` when nothing filters files out.
    fn active_filter(&self) -> Option<super::filter::Filter> {
        self.rules.borrow().filter.clone()
    }

    /// A different filter from the dropdown: re-filter the grid, drop what it
    /// hides from the model's selection, and start the query again if one is on
    /// screen (`P2an`'s rule for a filter change).
    fn filter_changed(&self, index: usize) {
        let Some(filter) = self.call.options.filters.get(index).cloned() else {
            return;
        };
        let name = filter.name.clone();
        self.rules.borrow_mut().filter = Some(filter);
        self.apply_grid_filter();
        crate::log!("portal", "dialog filter is now {name:?}");
        if let Some(process) = self.process.upgrade() {
            self.drop_hidden_selection(&process);
            self.restart_search(&process);
        }
    }

    /// The hidden-files switch in this dialog only. `Process::apply_show_hidden`
    /// would flip every grid in the process, so a second open dialog would
    /// change too (Claude's review): this touches only this dialog's grid and
    /// selection.
    fn set_hidden(&self, process: &Rc<Process>, show: bool) {
        if self.hidden.replace(show) == show {
            return;
        }
        self.rules.borrow_mut().show_hidden = show;
        self.folder.set_show_hidden(show);
        self.apply_grid_filter();
        // Nothing invisible may stay selected: Open would answer with a file
        // the user cannot see.
        self.drop_hidden_selection(process);
        if self.search_showing.get() {
            // The worker's filter holds the switch too, so the results page has
            // to be asked again.
            if let Some(page) = self.existing_page() {
                page.set_hidden_included(show);
            }
            self.restart_search(process);
        }
        crate::log!(
            "portal",
            "dialog {} hidden files",
            if show { "shows" } else { "hides" }
        );
    }

    /// Drop from the model every selected name the grid no longer lists.
    ///
    /// The main app does this with `Workspace::selection_drop_hidden`, which
    /// walks every tab of every window; a dialog has one folder, so it can ask
    /// its own grid what survived and toggle off the rest. Nothing may be
    /// accepted that the user cannot see.
    fn drop_hidden_selection(&self, process: &Rc<Process>) {
        let Some(window) = process.window_snapshot(self.id) else {
            return;
        };
        let Some(tab) = window.tab(window.focused_tab) else {
            return;
        };
        if tab.selection.names.is_empty() {
            return;
        }
        let visible: HashSet<OsString> = self.folder.listed_names().into_iter().collect();
        let gone: Vec<OsString> = tab
            .selection
            .names
            .iter()
            .filter(|name| !visible.contains(*name))
            .cloned()
            .collect();
        if gone.is_empty() {
            return;
        }
        for name in gone {
            // A Ctrl-click on a selected name takes it out of the selection, and
            // the model event that follows refreshes Open and the status line.
            process.file_click_tab(self.id, Some(tab.id), ClickRequest::Ctrl(name));
        }
    }

    /// Start the query on screen again, with the dialog's present rules.
    fn restart_search(&self, process: &Rc<Process>) {
        if self.search_showing.get() {
            process.requery_search(self.id);
        }
    }

    pub(crate) fn reload(&self) {
        self.folder.reload();
    }

    pub(crate) fn focus_path(&self) {
        self.path.remove_css_class("invalid");
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    /// The address bar's Enter. A path to a file is accepted; a directory is
    /// navigated to; anything else marks the field invalid.
    fn path_entered(&self, entry: &gtk::Entry) {
        let Some(process) = self.process.upgrade() else {
            return;
        };
        let text = entry.text().to_string();
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let home = process.home();
        let mut target = expand_home(text, &home);
        if !target.is_absolute() {
            let base = process
                .window_snapshot(self.id)
                .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
                .unwrap_or_else(|| home.clone());
            target = base.join(target);
        }
        if !target.exists() {
            self.path.add_css_class("invalid");
            crate::log!("portal", "{} does not exist", target.display());
            return;
        }
        self.path.remove_css_class("invalid");
        if target.is_file() {
            self.open_row(&target);
            return;
        }
        self.dispatch(Invocation::new(
            "nav.go",
            vec![("path".to_string(), target.to_string_lossy().into_owned())],
        ));
        self.path_entry.set_text(&target.display().to_string());
    }

    /// A row the user activated, or a file they typed: a folder picker has no
    /// answer for a file, and a file picker answers with this one file however
    /// many happen to be selected — a double-click means "this one".
    fn open_row(&self, path: &Path) {
        self.accept_path(path);
    }

    /// Answer with one explicit path, if the dialog's rules admit it.
    fn accept_path(&self, path: &Path) {
        if self.process.upgrade().is_none() {
            // The process is on its way out; nobody is left to answer.
            return;
        }
        match self.refusal(path) {
            Some(message) => self.refuse(message),
            None => self.answer(self.pick(vec![path.to_path_buf()])),
        }
    }

    /// Why one explicit file path may not be the answer, or `None` when it may.
    fn refusal(&self, path: &Path) -> Option<&'static str> {
        if self.mode().folders_only {
            return Some(NO_FILES);
        }
        if !path.is_file() {
            crate::log!("portal", "{} is not a file to open", path.display());
            return Some(GONE);
        }
        // `path` is a regular file here, so only the filter can refuse it.
        (!self.admits(path)).then_some(FILTERED)
    }

    /// Whether the dialog's own rules let it answer with this path: not a
    /// folder in a folder picker, and past the active filter in a file picker.
    ///
    /// A grid row and a search result have both been through these rules
    /// already — the worker applies them before it ranks — so this is the last
    /// check, not the first: it catches a row that changed while the dialog was
    /// open and a result from a query the filter has since replaced.
    fn admits(&self, path: &Path) -> bool {
        let rules = self.rules.borrow().clone();
        let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
            return path.is_dir() && rules.folders_only;
        };
        if rules.folders_only {
            return path.is_dir();
        }
        if path.is_dir() {
            // Folders are never an answer to a file picker, and a filter never
            // hides one, so this is not the filter's business.
            return false;
        }
        rules
            .filter
            .as_ref()
            .is_none_or(|filter| filter.matches_name(&name))
    }

    /// Say why nothing was accepted, on the status line. The line is written the
    /// same way the standing hint is — never as a held summary — so the next
    /// selection change takes it back without a timer.
    fn refuse(&self, message: &str) {
        self.status.set_counts(message.to_string());
        crate::log!("portal", "accept refused: {message}");
    }

    /// The `Pick` a set of paths answers with: the filter the app gets back,
    /// and what its own choices hold now.
    fn pick(&self, paths: Vec<PathBuf>) -> Pick {
        Pick {
            paths,
            // Whenever the call sent filters, the answer names the one in use,
            // as the bytes it arrived in. A `current_filter` that arrived alone
            // is a filter too, and is reported the same way (Nautilus puts it in
            // the chooser's list and reports that: `nautilus-portal.c:553-560`,
            // `270-273`).
            current_filter: self.active_filter().map(|filter| filter.raw().clone()),
            choices: self
                .choices
                .iter()
                .map(|control| match control {
                    ChoiceControl::Check { id, button } => (
                        id.clone(),
                        if button.is_active() { "true" } else { "false" }.to_string(),
                    ),
                    ChoiceControl::Options { id, drop, ids } => (
                        id.clone(),
                        ids.get(drop.selected() as usize)
                            .cloned()
                            .unwrap_or_default(),
                    ),
                })
                .collect(),
        }
    }

    fn answer(&self, pick: Pick) {
        let filter = self.active_filter().map(|filter| filter.name);
        crate::log!(
            "portal",
            "{}",
            pick.log_line(self.mode().label(), filter.as_deref())
        );
        let Some(request) = self.request.borrow_mut().take() else {
            crate::log!("portal", "the dialog had no request to answer");
            return;
        };
        request.accept(pick);
    }

    /// Answer the call with what Open picks under the dialog's mode.
    pub(crate) fn accept(&self) -> Outcome {
        let Some(process) = self.process.upgrade() else {
            return Outcome::Failed("the file chooser has no process".to_string());
        };
        match self.accept_now(&process) {
            Accept::Pick(paths) => {
                let pick = self.pick(paths);
                self.answer(pick);
                Outcome::Done
            }
            Accept::Refused(message) => {
                self.refuse(message);
                Outcome::Failed(message.to_string())
            }
        }
    }

    /// Answer "user cancelled", unless the call was already answered.
    pub(crate) fn cancel(&self) -> Outcome {
        let Some(request) = self.request.borrow_mut().take() else {
            // The window is still closing after its answer; the second path
            // (click then destroy, or Esc then close) must stay quiet.
            return Outcome::Done;
        };
        request.complete(service::RESPONSE_CANCELLED, service::empty_results());
        Outcome::Done
    }

    /// The results page, built the first time a query needs one and parented
    /// into the stack beside the grid. A dialog nobody searches never builds
    /// it: that is the brief's rule that search costs nothing until it is used,
    /// and [`SearchPage::new`] is ten widgets and a `ListStore`.
    ///
    /// `None` only when the process is already gone, in which case every caller
    /// has nothing left to update.
    fn page(&self) -> Option<Rc<SearchPage>> {
        if let Some(page) = self.search_page.borrow().as_ref() {
            return Some(page.clone());
        }
        let process = self.process.upgrade()?;
        let page = SearchPage::new(&process, self.id, self.scale);
        self.stack.add_named(page.widget(), Some("search"));
        *self.search_page.borrow_mut() = Some(page.clone());
        crate::log!("search", "dialog {} built its results page", self.id);
        Some(page)
    }

    /// The results page if one exists, for the paths that must not create it:
    /// a revoke or a status update for a page that was never built has no work.
    fn existing_page(&self) -> Option<Rc<SearchPage>> {
        self.search_page.borrow().clone()
    }

    /// The results page over the grid, or the grid again.
    fn show_search_page(&self, showing: bool) {
        self.search_showing.set(showing);
        self.stack
            .set_visible_child_name(if showing { "search" } else { "folder" });
        // The page carries its own status line, so the dialog's goes quiet
        // rather than showing counts for a folder the user cannot see.
        self.status_row.set_visible(!showing);
    }

    /// Reveal the search field and focus it, with `query` typed if there is one.
    fn reveal_search(&self, query: Option<&str>) {
        if self.search_open.get() {
            crate::log!(
                "search",
                "dialog search already open (field visible {}, revealed {})",
                self.search_field.is_visible(),
                self.search_revealer.reveals_child()
            );
            self.search_entry.grab_focus();
            if let Some(query) = query {
                // `set_text` emits `changed` with the field empty on its way
                // through, which is not the user clearing the search.
                self.search_programmatic.set(true);
                self.search_entry.set_text(query);
                self.search_programmatic.set(false);
                self.search_entry.set_position(-1);
            }
            return;
        }
        self.search_field.set_visible(true);
        self.search_open.set(true);
        self.search_icon.set_visible(false);
        self.search_revealer.set_reveal_child(true);
        if let Some(query) = query {
            self.search_programmatic.set(true);
            self.search_entry.set_text(query);
            self.search_programmatic.set(false);
            self.search_entry.set_position(-1);
        }
        self.search_entry.grab_focus();
        // The revealed field cannot take focus until the transition has a child
        // to give it: the header waits the same short while.
        let weak = self.search_entry.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
            if let Some(entry) = weak.upgrade() {
                entry.grab_focus();
            }
        });
        crate::log!("search", "dialog search opened");
    }

    /// Put the search field away. The results page is the caller's business.
    fn conceal_search(&self) {
        if !self.search_open.get() {
            return;
        }
        self.search_open.set(false);
        self.search_programmatic.set(true);
        self.search_entry.set_text("");
        self.search_programmatic.set(false);
        self.search_revealer.set_reveal_child(false);
        let field = self.search_field.downgrade();
        let icon = self.search_icon.downgrade();
        let revealer = self.search_revealer.downgrade();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis((motion::ms(200) + 10).into()),
            move || {
                // Ctrl+F again before the slide finished: the field is open
                // again and must stay (the main header's same check).
                if let (Some(field), Some(icon), Some(revealer)) =
                    (field.upgrade(), icon.upgrade(), revealer.upgrade())
                    && !revealer.reveals_child()
                {
                    field.set_visible(false);
                    icon.set_visible(true);
                    crate::log!("search", "dialog search field put away");
                }
            },
        );
        crate::log!("search", "dialog search closed");
    }
}

impl SearchSurface for Picker {
    fn search_active(&self) -> bool {
        self.search_showing.get()
    }

    fn open_search(&self, query: Option<&str>) {
        self.reveal_search(query);
    }

    fn begin_search(&self, view: &SearchView) {
        let Some(page) = self.page() else {
            return;
        };
        // The page says what its results include, and this dialog drops
        // dotfiles while Ctrl+H is off.
        page.set_hidden_included(self.hidden.get());
        self.reveal_search(Some(&view.query));
        self.show_search_page(true);
        page.begin(view);
    }

    fn search_changes(&self, changes: &[fm_core::search::SearchChange]) {
        if let Some(page) = self.existing_page() {
            page.apply_changes(changes);
        }
    }

    fn search_status(&self, view: &SearchView) {
        if let Some(page) = self.existing_page() {
            page.status(view);
        }
    }

    fn search_notice(&self, message: &str) {
        if let Some(page) = self.existing_page() {
            page.show_notice(message);
        }
    }

    fn search_revoked(&self) {
        if let Some(page) = self.existing_page() {
            page.reset();
        }
        self.show_search_page(false);
        self.conceal_search();
        self.folder.grab_grid_focus();
    }

    fn search_cleared(&self) {
        if let Some(page) = self.existing_page() {
            page.reset();
        }
        self.show_search_page(false);
    }

    fn search_message(&self, text: &str) {
        self.status.set_counts(text.to_string());
    }

    fn preview_result(&self, _selection: &Selection) {
        // A dialog has no preview panel: the path is the whole point, and the
        // row's own tooltip on the results page says where it is.
    }

    fn search_filter(&self) -> Option<Arc<dyn SearchResultFilter>> {
        let rules = self.rules.borrow().clone();
        // Nothing to exclude is the cheap answer, and the honest one: the
        // worker then does exactly what it does for the main app.
        if !rules.folders_only && rules.filter.is_none() && rules.show_hidden {
            return None;
        }
        Some(Arc::new(SearchFilter(rules)))
    }

    fn consume_file_result(&self, path: &Path) -> Option<Result<(), &'static str>> {
        // A dialog always owns a file result: it either answers with the file or
        // says why it will not, on the results page the user is looking at (the
        // dialog's own status line is hidden under it). Nothing navigates.
        self.process.upgrade()?;
        Some(match self.refusal(path) {
            Some(message) => {
                crate::log!("portal", "accept refused: {message}");
                Err(message)
            }
            None => {
                self.answer(self.pick(vec![path.to_path_buf()]));
                Ok(())
            }
        })
    }
}

/// The footer's filter dropdown: the app's filters in the order it sent them,
/// starting on the selected one. It is there only when the call had filters,
/// and no "All files" entry is added — the application decides its own list, as
/// neither GTK's nor Nautilus's backends do.
fn build_filter_dropdown(call: &Call, footer: &gtk::Box) -> Option<gtk::DropDown> {
    if !call.options.shows_filters() {
        return None;
    }
    let names: Vec<&str> = call
        .options
        .filters
        .iter()
        .map(|filter| filter.name.as_str())
        .collect();
    let drop = gtk::DropDown::from_strings(&names);
    // No `gl-menu-item` here: that class is `all: unset` for the dialog's text
    // buttons, and a dropdown needs its own frame and arrow to read as one.
    drop.set_valign(gtk::Align::Center);
    drop.set_tooltip_text(Some("Filter by file type"));
    drop.set_hexpand(false);
    // The entry the call named, or the first: `current_filter` is matched as a
    // whole `(sa(us))` value when the list is read, so this is already the right
    // row (`OpenOptions::current_filter`).
    drop.set_selected(call.options.current_filter.unwrap_or(0) as u32);
    footer.append(&drop);
    Some(drop)
}

/// The footer's row of the application's own choices: a checkbox for a choice
/// with no options, a dropdown of the options for one that has them. A malformed
/// choice never reaches here — `parse_open_options` skips it with a log line.
fn build_choices(call: &Call, footer: &gtk::Box, scale: f64) -> Vec<ChoiceControl> {
    let mut controls = Vec::new();
    for choice in &call.options.choices {
        if choice.is_check() {
            let button = gtk::CheckButton::with_label(&choice.label);
            button.set_active(choice.starts_checked());
            button.set_valign(gtk::Align::Center);
            button.update_property(&[gtk::accessible::Property::Label(&choice.id)]);
            footer.append(&button);
            controls.push(ChoiceControl::Check {
                id: choice.id.clone(),
                button,
            });
            continue;
        }
        let holder = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale));
        holder.set_valign(gtk::Align::Center);
        let label = gtk::Label::new(Some(&choice.label));
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        holder.append(&label);
        let names: Vec<&str> = choice
            .options
            .iter()
            .map(|(_, label)| label.as_str())
            .collect();
        let drop = gtk::DropDown::from_strings(&names);
        drop.set_valign(gtk::Align::Center);
        drop.set_selected(choice.start_option() as u32);
        drop.set_tooltip_text(Some(&choice.id));
        holder.append(&drop);
        footer.append(&holder);
        controls.push(ChoiceControl::Options {
            id: choice.id.clone(),
            drop,
            ids: choice.options.iter().map(|(id, _)| id.clone()).collect(),
        });
    }
    controls
}

/// Whether the caret is in this entry. A `GtkEntry` hands its focus to the
/// `GtkText` inside it (`gtk/gtkentry.c:426-431`), so `has_focus()` on the
/// entry itself is always false; the entry and every other ancestor of the
/// focus widget carry `FOCUS_WITHIN` instead (`gtk/gtkwindow.c:5280-5310`).
fn entry_focused(entry: &gtk::Entry) -> bool {
    entry.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN)
}

fn connect_nav(picker: &Rc<Picker>, button: &gtk::Button, name: &'static str) {
    let weak = Rc::downgrade(picker);
    button.connect_clicked(move |_| {
        if let Some(picker) = weak.upgrade()
            && let Some(process) = picker.process.upgrade()
        {
            process.dispatch_for_window(picker.id, Caller::Ui, Invocation::new(name, Vec::new()));
        }
    });
}

/// The dialog's own shortcuts: the navigation half of the main app's defaults,
/// Ctrl+H, and Ctrl+F for search (`fm-core/src/config.rs:645`). Every one is in
/// the picker allowlist, so a key can never reach a file effect.
fn picker_shortcut(key: gtk::gdk::Key, modifiers: gtk::gdk::ModifierType) -> Option<&'static str> {
    let control = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
    let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);
    let name = match key {
        gtk::gdk::Key::a if control => "selection.all",
        gtk::gdk::Key::f if control => "search.open",
        gtk::gdk::Key::h if control => "view.toggle-hidden",
        gtk::gdk::Key::l if control => "nav.focus-path",
        gtk::gdk::Key::F5 => "nav.refresh",
        gtk::gdk::Key::Left if alt => "nav.back",
        gtk::gdk::Key::Right if alt => "nav.forward",
        gtk::gdk::Key::Up if alt => "nav.up",
        _ => return None,
    };
    Some(name)
}

fn icon_button(icon_name: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    button.add_css_class("nav");
    button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
    button
}

fn action_button(text: &str, suggested: bool) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-menu-item");
    if suggested {
        button.add_css_class("suggested-action");
    }
    let label = gtk::Label::new(Some(text));
    label.set_use_underline(true);
    button.set_child(Some(&label));
    button
}

fn expand_home(text: &str, home: &Path) -> PathBuf {
    if text == "~" {
        home.to_path_buf()
    } else if let Some(rest) = text.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(text)
    }
}

/// The picker process's dialog host. Created once per process.
pub struct PickerHost {
    process: Rc<Process>,
    application: adw::Application,
}

impl PickerHost {
    pub fn new(process: &Rc<Process>, application: &adw::Application) -> Rc<Self> {
        Rc::new(Self {
            process: process.clone(),
            application: application.clone(),
        })
    }
}

impl DialogHost for PickerHost {
    fn open(&self, request: Rc<Request>, call: &Call) -> Result<(), String> {
        let home = self.process.home();
        let folder = service::starting_folder(call.options.current_folder.as_deref(), &home);
        crate::log!(
            "portal",
            "opening the dialog at {} for {:?} ({}{}, {} filter(s), {} choice(s))",
            folder.display(),
            call.app_id,
            if call.options.directory {
                "folders"
            } else {
                "files"
            },
            if call.options.multiple {
                "+multiple"
            } else {
                ""
            },
            call.options.filters.len(),
            call.options.choices.len()
        );
        let options = PickerOptions {
            request: request.clone(),
            call: call.clone(),
            hold: self.application.hold(),
        };
        // A refused window drops the options, which releases the hold.
        match self.process.open_picker_window(folder, options) {
            Some(_id) => Ok(()),
            None => Err("the model refused a new window".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::workspace::Workspace;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "gimbal-picker-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&path).expect("a scratch folder");
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One model window at `folder`, with nothing selected. Only the model is
    /// built: these tests never construct a GTK window.
    fn model_at(folder: &Path) -> (Workspace, Window) {
        let mut workspace = Workspace::with_home(PathBuf::from("/home/kyle"));
        let events = workspace
            .window_new(Some(folder.to_path_buf()))
            .expect("a window");
        let id = events
            .iter()
            .find_map(|event| match event {
                fm_core::workspace::Event::WindowOpened(id) => Some(*id),
                _ => None,
            })
            .expect("a WindowOpened event");
        let window = workspace
            .windows
            .iter()
            .find(|window| window.id == id)
            .cloned()
            .expect("the window");
        (workspace, window)
    }

    fn names(list: &[&str]) -> Vec<OsString> {
        list.iter().copied().map(OsString::from).collect()
    }

    fn selected(window: &Window) -> BTreeSet<OsString> {
        window
            .tab(window.focused_tab)
            .map(|tab| tab.selection.names.clone())
            .unwrap_or_default()
    }

    /// The listed order the grid would give for a folder: the names the scratch
    /// folder holds, as the tests write them.
    fn grid_order(folder: &Path, order: &[&str]) -> Vec<OsString> {
        let known: HashSet<OsString> = std::fs::read_dir(folder)
            .expect("a scratch folder")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name())
            .collect();
        let out = names(order);
        assert!(
            out.iter().all(|name| known.contains(name)),
            "a test listed a name the scratch folder does not hold"
        );
        out
    }

    #[test]
    fn one_selected_file_is_the_answer() {
        let scratch = Scratch::new("one");
        let folder = &scratch.0;
        std::fs::write(folder.join("photo.png"), b"x").expect("file");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("photo.png"))
            .expect("select");
        let window = &workspace.windows[0];
        let mode = Mode {
            multiple: false,
            folders_only: false,
        };
        let listed = grid_order(folder, &["photo.png"]);
        assert_eq!(
            accept_for(mode, folder, &listed, &selected(window)),
            Accept::Pick(vec![folder.join("photo.png")])
        );
    }

    #[test]
    fn two_files_a_folder_and_a_vanished_name_are_not_an_answer() {
        let scratch = Scratch::new("many");
        let folder = &scratch.0;
        std::fs::write(folder.join("a.txt"), b"x").expect("file");
        std::fs::write(folder.join("b.txt"), b"x").expect("file");
        std::fs::create_dir_all(folder.join("sub")).expect("directory");

        let (mut workspace, _) = model_at(folder);
        let mode = Mode {
            multiple: false,
            folders_only: false,
        };

        // A folder is not a file to open.
        workspace.file_click(OsString::from("sub")).expect("select");
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["sub"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(NO_FOLDERS)
        );

        // Two files are not one file.
        workspace
            .file_click(OsString::from("a.txt"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("b.txt"))
            .expect("extend");
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["a.txt", "b.txt"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(ONE_FILE)
        );

        // The listed name is gone by the time Open is pressed: choosing it
        // would answer with a URI that no longer exists.
        workspace
            .file_click(OsString::from("b.txt"))
            .expect("select");
        std::fs::remove_file(folder.join("b.txt")).expect("remove");
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["b.txt"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(GONE)
        );
        // The grid no longer lists `b.txt` either, so its answer is the empty one.
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["a.txt", "sub"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(ONE_FILE)
        );
    }

    #[test]
    fn no_selection_is_not_an_answer() {
        let scratch = Scratch::new("none");
        let (_, window) = model_at(&scratch.0);
        let mode = Mode {
            multiple: false,
            folders_only: false,
        };
        assert_eq!(
            accept_for(mode, &scratch.0, &[], &selected(&window)),
            Accept::Refused(ONE_FILE)
        );
    }

    #[test]
    fn multiple_mode_answers_every_file_in_grid_order() {
        let scratch = Scratch::new("multi");
        let folder = &scratch.0;
        for name in ["a.txt", "b.txt", "c.txt"] {
            std::fs::write(folder.join(name), b"x").expect("file");
        }
        let mode = Mode {
            multiple: true,
            folders_only: false,
        };
        let (mut workspace, _) = model_at(folder);
        // Clicked last-to-first: the answer still follows the grid.
        workspace
            .file_click(OsString::from("c.txt"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("b.txt"))
            .expect("extend");
        workspace
            .file_ctrl_click(OsString::from("a.txt"))
            .expect("extend");
        let window = &workspace.windows[0];
        let listed = grid_order(folder, &["a.txt", "b.txt", "c.txt"]);
        assert_eq!(
            accept_for(mode, folder, &listed, &selected(window)),
            Accept::Pick(vec![
                folder.join("a.txt"),
                folder.join("b.txt"),
                folder.join("c.txt")
            ]),
            "uris are in the grid's order, not the order they were clicked"
        );

        // A reversed listing is a reversed answer: the grid decides.
        let listed = grid_order(folder, &["c.txt", "b.txt", "a.txt"]);
        assert_eq!(
            accept_for(mode, folder, &listed, &selected(window)),
            Accept::Pick(vec![
                folder.join("c.txt"),
                folder.join("b.txt"),
                folder.join("a.txt")
            ])
        );
    }

    #[test]
    fn a_folder_in_the_selection_disables_open_in_both_file_modes() {
        let scratch = Scratch::new("mixed");
        let folder = &scratch.0;
        std::fs::write(folder.join("a.txt"), b"x").expect("file");
        std::fs::create_dir_all(folder.join("sub")).expect("directory");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("a.txt"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("sub"))
            .expect("extend");
        let window = &workspace.windows[0];
        let listed = grid_order(folder, &["sub", "a.txt"]);
        let selection = selected(window);
        for multiple in [false, true] {
            let mode = Mode {
                multiple,
                folders_only: false,
            };
            assert_eq!(
                accept_for(mode, folder, &listed, &selection),
                Accept::Refused(NO_FOLDERS),
                "a folder in a file picker's selection disables Open"
            );
            assert_eq!(
                status_for(mode, folder, &Accept::Refused(NO_FOLDERS)).as_deref(),
                Some(NO_FOLDERS),
                "and the status line says why"
            );
        }
    }

    #[test]
    fn a_single_mode_with_two_files_says_select_one_file() {
        let scratch = Scratch::new("two-one");
        let folder = &scratch.0;
        std::fs::write(folder.join("a.txt"), b"x").expect("file");
        std::fs::write(folder.join("b.txt"), b"x").expect("file");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("a.txt"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("b.txt"))
            .expect("extend");
        let mode = Mode {
            multiple: false,
            folders_only: false,
        };
        let listed = grid_order(folder, &["a.txt", "b.txt"]);
        let accept = accept_for(mode, folder, &listed, &selected(&workspace.windows[0]));
        assert_eq!(accept, Accept::Refused(ONE_FILE));
        assert_eq!(status_for(mode, folder, &accept).as_deref(), Some(ONE_FILE));
    }

    #[test]
    fn folders_only_mode_picks_a_selected_folder() {
        let scratch = Scratch::new("dirs");
        let folder = &scratch.0;
        std::fs::create_dir_all(folder.join("Pictures")).expect("directory");
        std::fs::create_dir_all(folder.join("Music")).expect("directory");
        std::fs::write(folder.join("photo.png"), b"x").expect("file");
        let mode = Mode {
            multiple: false,
            folders_only: true,
        };
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("Pictures"))
            .expect("select");
        let listed = grid_order(folder, &["Music", "Pictures"]);
        let accept = accept_for(mode, folder, &listed, &selected(&workspace.windows[0]));
        assert_eq!(accept, Accept::Pick(vec![folder.join("Pictures")]));
        assert_eq!(
            status_for(mode, folder, &accept).as_deref(),
            Some("Will select \"Pictures\""),
            "the status line says what Open will pick"
        );
    }

    #[test]
    fn folders_only_mode_with_nothing_selected_takes_the_current_folder() {
        let scratch = Scratch::new("current");
        let folder = &scratch.0;
        std::fs::create_dir_all(folder.join("Pictures")).expect("directory");
        let mode = Mode {
            multiple: false,
            folders_only: true,
        };
        let (_, window) = model_at(folder);
        let listed = grid_order(folder, &["Pictures"]);
        let accept = accept_for(mode, folder, &listed, &selected(&window));
        assert_eq!(accept, Accept::Pick(vec![folder.clone()]));
        assert_eq!(
            status_for(mode, folder, &accept).as_deref(),
            Some("Will select this folder")
        );
    }

    #[test]
    fn folders_only_mode_with_multiple_takes_every_selected_folder() {
        let scratch = Scratch::new("dirs-multi");
        let folder = &scratch.0;
        for name in ["one", "two", "three"] {
            std::fs::create_dir_all(folder.join(name)).expect("directory");
        }
        let mode = Mode {
            multiple: true,
            folders_only: true,
        };
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("three"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("one"))
            .expect("extend");
        let listed = grid_order(folder, &["one", "two", "three"]);
        let accept = accept_for(mode, folder, &listed, &selected(&workspace.windows[0]));
        assert_eq!(
            accept,
            Accept::Pick(vec![folder.join("one"), folder.join("three")]),
            "in the grid's order"
        );
        assert_eq!(
            status_for(mode, folder, &accept).as_deref(),
            Some("Will select 2 folders")
        );

        // Without `multiple`, two folders are too many.
        let one_only = Mode {
            multiple: false,
            folders_only: true,
        };
        assert_eq!(
            accept_for(one_only, folder, &listed, &selected(&workspace.windows[0])),
            Accept::Refused(ONE_FOLDER)
        );
    }

    #[test]
    fn a_file_in_folders_only_mode_and_a_vanished_name_are_refused() {
        let scratch = Scratch::new("refuse");
        let folder = &scratch.0;
        std::fs::create_dir_all(folder.join("Pictures")).expect("directory");
        std::fs::write(folder.join("photo.png"), b"x").expect("file");
        let mode = Mode {
            multiple: true,
            folders_only: true,
        };
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("Pictures"))
            .expect("select");
        workspace
            .file_ctrl_click(OsString::from("photo.png"))
            .expect("extend");
        let listed = grid_order(folder, &["Pictures", "photo.png"]);
        assert_eq!(
            accept_for(mode, folder, &listed, &selected(&workspace.windows[0])),
            Accept::Refused(NO_FILES),
            "a file in a folder picker's selection disables Open"
        );

        // A name that is neither folder nor file any more, in a file picker.
        let file_mode = Mode {
            multiple: true,
            folders_only: false,
        };
        std::fs::remove_file(folder.join("photo.png")).expect("remove");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_ctrl_click(OsString::from("photo.png"))
            .expect("select");
        assert_eq!(
            accept_for(
                file_mode,
                folder,
                &names(&["photo.png"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(GONE)
        );
    }

    #[test]
    fn a_symlink_to_a_file_is_openable_and_a_folder_listing_is_not_asked() {
        let scratch = Scratch::new("link");
        let folder = &scratch.0;
        std::fs::write(folder.join("real.txt"), b"x").expect("file");
        std::os::unix::fs::symlink(folder.join("real.txt"), folder.join("link.txt"))
            .expect("symlink");
        std::fs::create_dir_all(folder.join("real-dir")).expect("directory");
        std::os::unix::fs::symlink(folder.join("real-dir"), folder.join("link-dir"))
            .expect("symlink");
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("link.txt"))
            .expect("select");
        let mode = Mode {
            multiple: false,
            folders_only: false,
        };
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["link.txt"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Pick(vec![folder.join("link.txt")]),
            "a symlink to a regular file is a file to open"
        );

        // The same dialog asked about a link to a folder answers "folders can't
        // be opened here": `is_dir` follows the link, as the file managers do.
        let (mut workspace, _) = model_at(folder);
        workspace
            .file_click(OsString::from("link-dir"))
            .expect("select");
        assert_eq!(
            accept_for(
                mode,
                folder,
                &names(&["link-dir"]),
                &selected(&workspace.windows[0])
            ),
            Accept::Refused(NO_FOLDERS)
        );
    }

    #[test]
    fn the_grid_order_wins_even_when_the_selection_is_a_superset() {
        let scratch = Scratch::new("order");
        let folder = &scratch.0;
        for name in ["z.txt", "m.txt", "a.txt"] {
            std::fs::write(folder.join(name), b"x").expect("file");
        }
        let mode = Mode {
            multiple: true,
            folders_only: false,
        };
        let selection: BTreeSet<OsString> =
            names(&["a.txt", "m.txt", "z.txt"]).into_iter().collect();
        // The grid sorted a-m-z and lists only two of them: the third is hidden
        // by a filter, so it is not part of the answer.
        assert_eq!(
            accept_for(mode, folder, &names(&["z.txt", "a.txt"]), &selection),
            Accept::Pick(vec![folder.join("z.txt"), folder.join("a.txt")])
        );
    }

    #[test]
    fn every_shortcut_is_an_allowlisted_command_and_no_file_key_is_bound() {
        use gtk::gdk::{Key, ModifierType};

        let none = ModifierType::empty();
        let control = ModifierType::CONTROL_MASK;
        let alt = ModifierType::ALT_MASK;
        for (key, modifiers, expected) in [
            (Key::a, control, "selection.all"),
            (Key::f, control, "search.open"),
            (Key::h, control, "view.toggle-hidden"),
            (Key::l, control, "nav.focus-path"),
            (Key::F5, none, "nav.refresh"),
            (Key::Left, alt, "nav.back"),
            (Key::Right, alt, "nav.forward"),
            (Key::Up, alt, "nav.up"),
        ] {
            assert_eq!(picker_shortcut(key, modifiers), Some(expected));
            assert!(
                crate::app::picker_accepts(expected),
                "{expected} is bound but not allowed"
            );
        }

        // The main app's file keys are not bound at all in the dialog.
        for (key, modifiers) in [
            (Key::Delete, none),
            (Key::v, control),
            (Key::c, control),
            (Key::x, control),
            (Key::n, control),
            (Key::t, control),
            (Key::w, control),
        ] {
            assert_eq!(picker_shortcut(key, modifiers), None);
        }
        // A modifier-less letter is not a shortcut either, and neither is a
        // bare F that would type into the address field.
        assert_eq!(picker_shortcut(Key::a, none), None);
        assert_eq!(picker_shortcut(Key::f, none), None);
        assert_eq!(picker_shortcut(Key::F5, control), Some("nav.refresh"));
    }

    #[test]
    fn the_address_field_expands_home_and_keeps_other_paths() {
        let home = Path::new("/home/kyle");
        assert_eq!(expand_home("~", home), home);
        assert_eq!(expand_home("~/Pictures", home), home.join("Pictures"));
        assert_eq!(expand_home("/srv/data", home), PathBuf::from("/srv/data"));
        assert_eq!(expand_home("relative", home), PathBuf::from("relative"));
    }

    #[test]
    fn the_mode_labels_match_the_log_and_the_live_script() {
        for (mode, label) in [
            (
                Mode {
                    multiple: false,
                    folders_only: false,
                },
                "files",
            ),
            (
                Mode {
                    multiple: true,
                    folders_only: false,
                },
                "files+multiple",
            ),
            (
                Mode {
                    multiple: false,
                    folders_only: true,
                },
                "folders",
            ),
            (
                Mode {
                    multiple: true,
                    folders_only: true,
                },
                "folders+multiple",
            ),
        ] {
            assert_eq!(mode.label(), label);
        }
    }
}
