// SPDX-License-Identifier: GPL-3.0-or-later
//! The header, bound to the workspace model.
//!
//! The layout, sizes and behaviours are P1i-P1l's header: nav buttons, the
//! crossfading search pill, the pill strip in a `GtkFixed` with drag reorder,
//! the accordion overflow list, the stretching path bar and the window
//! buttons. Every control dispatches a command through the window's command
//! path; the model's events rebuild what is on screen.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::time::Duration;

use adw::prelude::*;
use fm_core::command::{Caller, Invocation};
use fm_core::config::{Config, SortBy, SortOrder, SortRule};
use fm_core::workspace::{Group, Window, WindowId};
use gtk::{gdk, glib, pango};

use crate::dnd;
use crate::frame::WindowUi;
use crate::look::motion;
use crate::look::tokens::*;
use crate::platform;
const FADE_MS: u32 = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PillView {
    pub index: usize,
    pub label: String,
    pub title: String,
    pub is_group: bool,
    pub tabs: usize,
    pub active: bool,
    pub selected: bool,
    /// The pill shows the administrator red and lock when any tab in it is an
    /// administrator tab.
    pub admin: bool,
    /// The folder a drop on this pill goes into: the group's focused tab when
    /// it contains one, else its first tab.
    pub folder: PathBuf,
}

#[derive(Clone, Debug)]
pub struct HeaderView {
    pub pills: Vec<PillView>,
    pub tab_count: usize,
    pub folder: PathBuf,
    pub can_back: bool,
    pub can_forward: bool,
    pub can_up: bool,
}

impl HeaderView {
    pub fn any_selected(&self) -> bool {
        self.pills.iter().any(|pill| pill.selected)
    }

    /// The number of pills selected, as the Split View entry asks.
    pub fn selected_pills(&self) -> usize {
        self.pills.iter().filter(|pill| pill.selected).count()
    }

    pub fn active_index(&self) -> usize {
        self.pills.iter().position(|pill| pill.active).unwrap_or(0)
    }
}

/// What a left click on an accordion row does. Ctrl and Shift keep the menu
/// open so several rows can be selected in one visit; a plain click focuses
/// the row and closes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccordionClick {
    Focus,
    Toggle,
    Range,
}

impl AccordionClick {
    pub fn keeps_menu_open(self) -> bool {
        !matches!(self, Self::Focus)
    }
}

pub fn accordion_click(modifiers: gdk::ModifierType) -> AccordionClick {
    if modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        AccordionClick::Toggle
    } else if modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
        AccordionClick::Range
    } else {
        AccordionClick::Focus
    }
}

/// The header keeps one accordion while it stays collapsed. Only a change
/// between collapsed and expanded replaces it.
pub fn accordion_is_reused(collapsed_before: bool, collapsed_after: bool) -> bool {
    collapsed_before && collapsed_after
}

/// A right click on an unselected row first makes it the only selection, as
/// file managers do. A selected row keeps the selection it is part of.
pub fn row_right_click_selects_only(view: &HeaderView, index: usize) -> bool {
    view.pills.get(index).is_some_and(|pill| !pill.selected)
}

/// The accordion list may grow to twelve token-sized rows or to the space the
/// popover has below the header, whichever is smaller.
pub fn accordion_scroll_cap(scale: f64, window_height: i32, header_bottom: i32) -> i32 {
    let token_cap = scaled(PIN_ROW_H, scale) * 12;
    let below = (window_height - header_bottom).max(0);
    token_cap.min(below)
}

struct AccordionRow {
    row: gtk::Widget,
    choose: gtk::Button,
}

struct Accordion {
    pill: gtk::CenterBox,
    popover: gtk::Popover,
    menu: gtk::Box,
    label: gtk::Label,
    scroller: gtk::ScrolledWindow,
    rows: RefCell<Vec<AccordionRow>>,
}

/// A single tab's pill shows its folder's name; a split group shows its title
/// and tab count ("Group N · K tabs", or the chosen name).
pub fn pill_label(group: &Group) -> String {
    if group.is_split() {
        return group.label();
    }
    let Some(tab) = group.tabs.first() else {
        return group.title();
    };
    match tab.folder.file_name() {
        Some(name) if !name.is_empty() => name.to_string_lossy().into_owned(),
        _ => tab.folder.display().to_string(),
    }
}

pub fn view(window: &Window) -> HeaderView {
    let focused_group = window.focused_group().map(|group| group.id);
    let pills = window
        .pills
        .iter()
        .enumerate()
        .filter_map(|(index, group_id)| {
            let group = window.groups.get(group_id)?;
            let folder = group
                .tab(window.focused_tab)
                .or_else(|| group.tabs.first())
                .map(|tab| tab.folder.clone())
                .unwrap_or_default();
            Some(PillView {
                index,
                label: pill_label(group),
                title: group.title(),
                is_group: group.is_split(),
                tabs: group.tabs.len(),
                active: focused_group == Some(*group_id),
                selected: window
                    .selection
                    .selected
                    .get(index)
                    .copied()
                    .unwrap_or(false),
                admin: group.tabs.iter().any(|tab| tab.admin),
                folder,
            })
        })
        .collect();
    let tab = window.tab(window.focused_tab);
    let folder = tab.map(|tab| tab.folder.clone()).unwrap_or_else(|| {
        window
            .groups
            .values()
            .flat_map(|group| group.tabs.first())
            .map(|tab| tab.folder.clone())
            .next()
            .unwrap_or_else(|| PathBuf::from("/"))
    });
    let can_back = tab.is_some_and(|tab| !tab.back.is_empty());
    let can_forward = tab.is_some_and(|tab| !tab.forward.is_empty());
    let can_up = folder != Path::new("/")
        && folder
            .parent()
            .is_some_and(|parent| parent != folder.as_path());
    HeaderView {
        pills,
        tab_count: window.tab_count(),
        folder,
        can_back,
        can_forward,
        can_up,
    }
}

#[derive(Clone, Copy)]
struct DragState {
    index: usize,
    start_x: f64,
    start_y: f64,
    base: f64,
    to: usize,
}

type AgentWidgets = (gtk::Switch, gtk::Label, Rc<Cell<bool>>);

struct UpdateWidgets {
    switch: gtk::Switch,
    status: gtk::Label,
    check: gtk::Button,
    releases: gtk::Button,
    syncing: Rc<Cell<bool>>,
}

struct FileManager1Widgets {
    switch: gtk::Switch,
    status: gtk::Label,
    syncing: Rc<Cell<bool>>,
}

pub struct Header {
    window_id: WindowId,
    handle: gtk::WindowHandle,
    strip: gtk::Fixed,
    pills: RefCell<Vec<gtk::Widget>>,
    /// The collapsed strip's one pill and its popover. Kept across a refresh
    /// that leaves the header collapsed, so an open list can be refilled in
    /// place instead of destroyed under the pointer.
    accordion: RefCell<Option<Rc<Accordion>>>,
    last_labels: RefCell<Vec<String>>,
    last_groups: RefCell<Vec<fm_core::workspace::GroupId>>,
    drag: Cell<Option<DragState>>,
    available: Cell<i32>,
    path: gtk::Box,
    path_entry: gtk::Entry,
    menu_button: gtk::Button,
    back: gtk::Button,
    forward: gtk::Button,
    up: gtk::Button,
    refresh: gtk::Button,
    search_icon: gtk::Button,
    search: gtk::Revealer,
    search_field: gtk::Box,
    search_entry: gtk::Entry,
    search_hint: gtk::Label,
    trash: gtk::Button,
    float: gtk::Button,
    minimize: Option<gtk::Button>,
    close: gtk::Button,
    scale: Cell<f64>,
    overflow: Cell<bool>,
    search_open: Cell<bool>,
    /// Set while `close_search` clears the entry programmatically, so the
    /// changed handler does not mistake it for the user clearing search.
    search_programmatic: Cell<bool>,
    animate_group: Cell<Option<usize>>,
    /// Non-zero while an `adw::AlertDialog` owns the keyboard.
    dialogs: Cell<u32>,
    /// The open Settings switch and status label, so every window shows the
    /// shared listener state. The flag guards programmatic switch updates.
    agent_widgets: RefCell<Option<AgentWidgets>>,
    /// The open Settings menu's Updates switch, status and buttons.
    update_widgets: RefCell<Option<UpdateWidgets>>,
    /// The open Settings menu's Integration switch and status.
    filemanager1_widgets: RefCell<Option<FileManager1Widgets>>,
    /// The open Settings menu's Sort rows. The header owns them while the menu
    /// is open; the rows' click handlers hold only weak references.
    sort_rows: RefCell<Option<Rc<SortRows>>>,
    ui: RefCell<Weak<WindowUi>>,
}

impl Header {
    pub fn new(window_id: WindowId, scale: f64) -> Rc<Self> {
        let row = hbox(&["gl-header"]);
        row.set_hexpand(true);

        let menu_button = icon_button("open-menu-symbolic", "nav");
        row.append(&menu_button);
        let back = icon_button("go-previous-symbolic", "nav");
        let forward = icon_button("go-next-symbolic", "nav");
        let up = icon_button("go-up-symbolic", "nav");
        row.append(&back);
        row.append(&forward);
        row.append(&up);

        let search_icon = icon_button("system-search-symbolic", "searchbtn");
        let field = hbox(&["gl-field", "gl-search-field"]);
        field.set_valign(gtk::Align::Center);
        let glyph = gtk::Image::from_icon_name("system-search-symbolic");
        glyph.add_css_class("gl-search-glyph");
        field.append(&glyph);
        let search_entry = gtk::Entry::new();
        search_entry.set_placeholder_text(Some("Search"));
        field.append(&search_entry);
        let search_hint = gtk::Label::new(Some("Press Enter to search"));
        search_hint.add_css_class("gl-search-hint");
        search_hint.set_visible(false);
        field.append(&search_hint);
        let search = gtk::Revealer::new();
        // Crossfade, not SlideRight: a slide translates the field left by
        // `width - child_width` and the revealer does not clip (P1l item 2).
        search.set_transition_type(gtk::RevealerTransitionType::Crossfade);
        search.set_transition_duration(motion::ms(200));
        search.set_child(Some(&field));
        field.set_visible(false);
        let search_box = hbox(&["gl-search"]);
        search_box.set_valign(gtk::Align::Center);
        search_box.append(&search_icon);
        search_box.append(&search);
        row.append(&search_box);

        let strip = gtk::Fixed::new();
        strip.add_css_class("gl-strip");
        strip.set_valign(gtk::Align::Center);
        row.append(&strip);

        let path = hbox(&["gl-field", "gl-path"]);
        path.set_valign(gtk::Align::Center);
        path.set_hexpand(true);
        let path_entry = gtk::Entry::new();
        gtk::prelude::EntryExt::set_alignment(&path_entry, 0.0);
        path_entry.set_hexpand(true);
        path_entry.set_placeholder_text(Some("Path"));
        path.append(&path_entry);
        let refresh = icon_button("view-refresh-symbolic", "");
        path.append(&refresh);
        row.append(&path);

        let trash = gtk::Button::new();
        trash.add_css_class("gl-trash");
        trash.set_child(Some(&gtk::Image::from_icon_name("user-trash-symbolic")));
        trash.set_valign(gtk::Align::Center);
        row.append(&trash);

        let controls = platform::controls_here();
        let float = crate::look::icons::button(crate::look::icons::MockupIcon::Float);
        float.add_css_class("winbtn");
        float.add_css_class("float");
        float.set_valign(gtk::Align::Center);
        row.append(&float);
        let minimize = controls.minimize.then(|| {
            let button = icon_button("window-minimize-symbolic", "winbtn");
            row.append(&button);
            button
        });
        let close = crate::look::icons::button(crate::look::icons::MockupIcon::Close);
        close.add_css_class("winbtn");
        close.add_css_class("close");
        close.set_valign(gtk::Align::Center);
        row.append(&close);

        // A zero-minimum External scroller lets the header shrink while the
        // path and pills re-flow; the row still receives the viewport width.
        let scroller = gtk::ScrolledWindow::new();
        scroller.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        scroller.set_min_content_width(0);
        scroller.set_propagate_natural_width(false);
        scroller.set_has_frame(false);
        scroller.set_child(Some(&row));
        let handle = gtk::WindowHandle::builder().child(&scroller).build();

        let header = Rc::new(Self {
            window_id,
            handle,
            strip,
            pills: RefCell::new(Vec::new()),
            accordion: RefCell::new(None),
            last_labels: RefCell::new(Vec::new()),
            last_groups: RefCell::new(Vec::new()),
            drag: Cell::new(None),
            available: Cell::new(DEFAULT_W),
            path,
            path_entry,
            menu_button,
            back,
            forward,
            up,
            refresh,
            search_icon,
            search,
            search_field: field,
            search_entry,
            search_hint,
            trash,
            float,
            minimize,
            close,
            scale: Cell::new(scale),
            overflow: Cell::new(false),
            search_open: Cell::new(false),
            search_programmatic: Cell::new(false),
            animate_group: Cell::new(None),
            dialogs: Cell::new(0),
            agent_widgets: RefCell::new(None),
            update_widgets: RefCell::new(None),
            filemanager1_widgets: RefCell::new(None),
            sort_rows: RefCell::new(None),
            ui: RefCell::new(Weak::new()),
        });
        let weak = Rc::downgrade(&header);
        scroller
            .hadjustment()
            .connect_notify_local(Some("page-size"), move |adjustment, _| {
                let width = adjustment.page_size() as i32;
                if let Some(header) = weak.upgrade()
                    && width > 0
                    && width != header.available.get()
                {
                    header.available.set(width);
                    header.relayout();
                }
            });
        header
    }

    pub fn widget(&self) -> &gtk::WindowHandle {
        &self.handle
    }

    /// Sync the open Settings switch and status label with the shared runtime.
    pub fn refresh_agent_status(&self, status: &crate::agent::AgentStatus) {
        let widgets = self
            .agent_widgets
            .borrow()
            .as_ref()
            .map(|(switch, label, guard)| (switch.clone(), label.clone(), guard.clone()));
        if let Some((switch, label, guard)) = widgets {
            guard.set(true);
            switch.set_active(status.state == crate::agent::AgentState::On);
            guard.set(false);
            label.set_text(&agent_status_text(status));
        }
    }

    /// Sync the open Settings menu's Updates rows with the process state.
    pub fn refresh_update_status(&self, status: &crate::app::update::UpdateStatus) {
        let widgets = self.update_widgets.borrow().as_ref().map(|widgets| {
            (
                widgets.switch.clone(),
                widgets.status.clone(),
                widgets.check.clone(),
                widgets.releases.clone(),
                widgets.syncing.clone(),
            )
        });
        let Some((switch, label, check, releases, syncing)) = widgets else {
            return;
        };
        syncing.set(true);
        switch.set_active(status.check_enabled);
        syncing.set(false);
        let development = status.kind == fm_core::update::InstallKind::Development;
        switch.set_sensitive(!development);
        check.set_sensitive(!development && status.check_enabled && !status.checking);
        releases.set_visible(status.releases_visible());
        label.set_text(&status.status_text());
    }

    /// Sync the open Settings menu's Integration switch and status line.
    pub fn refresh_filemanager1_status(&self, status: &crate::filemanager1::Status) {
        let widgets = self.filemanager1_widgets.borrow().as_ref().map(|widgets| {
            (
                widgets.switch.clone(),
                widgets.status.clone(),
                widgets.syncing.clone(),
            )
        });
        let Some((switch, label, syncing)) = widgets else {
            return;
        };
        syncing.set(true);
        switch.set_active(status.enabled);
        syncing.set(false);
        label.set_text(&status.text());
    }

    /// Sync the open Settings menu's Sort rows with the config, after any sort
    /// change: a menu click, a hotkey or another window.
    pub fn refresh_sort_menu(&self) {
        let rows = self.sort_rows.borrow().clone();
        let ui = self.ui.borrow().upgrade();
        if let (Some(rows), Some(ui)) = (rows, ui) {
            rows.refresh(&ui);
        }
    }

    pub fn dialogs_open(&self) -> bool {
        self.dialogs.get() > 0
    }

    pub fn begin_dialog(&self) {
        self.dialogs.set(self.dialogs.get() + 1);
    }

    pub fn end_dialog(&self) {
        self.dialogs.set(self.dialogs.get().saturating_sub(1));
    }

    /// The path bar and search field are the window's own text entries. The
    /// rename and new-folder entries report through `WindowUi::editing`.
    pub fn text_focused(&self) -> bool {
        self.path_entry.has_focus() || self.search_entry.has_focus()
    }

    pub fn search_entry_focused(&self) -> bool {
        self.search_entry.has_focus()
    }

    /// The results page is about to open; its own status replaces the
    /// pre-submission hint.
    pub fn hide_search_hint(&self) {
        self.search_hint.set_visible(false);
    }

    /// The real header entry, used by widget-level regressions that exercise
    /// the same activate signal as Enter without synthesising desktop input.
    #[cfg(test)]
    pub(crate) fn search_entry_for_tests(&self) -> &gtk::Entry {
        &self.search_entry
    }

    #[cfg(test)]
    pub(crate) fn search_open_for_tests(&self) -> bool {
        self.search_open.get()
    }

    #[cfg(test)]
    pub(crate) fn search_hint_visible_for_tests(&self) -> bool {
        self.search_hint.get_visible()
    }

    fn window_ui(&self) -> Option<Rc<WindowUi>> {
        self.ui.borrow().upgrade()
    }

    pub fn set_scale(self: &Rc<Self>, ui: &Rc<WindowUi>, scale: f64) {
        self.scale.set(scale);
        self.path.set_width_request(scaled(PATH_MIN, scale));
        self.rebuild(ui, false);
    }

    pub fn rebuild(self: &Rc<Self>, ui: &Rc<WindowUi>, fade: bool) {
        let Some(window) = ui.workspace_window() else {
            return;
        };
        let mut view = view(&window);
        let trash = ui.trash_active();
        if trash {
            // The hidden folder is not the current view. Back remains useful
            // as Back to Files; path, Up and Forward are unsupported here.
            view.folder = PathBuf::from("Trash");
            view.can_back = true;
            view.can_forward = false;
            view.can_up = false;
        }
        let collapsed_before = self.overflow.get();
        let collapsed = self.update_layout(&view);

        let groups: Vec<_> = window.pills.clone();
        let previous = self.last_groups.borrow().clone();
        let fade_index = if fade {
            groups
                .iter()
                .zip(previous.iter())
                .position(|(new, old)| new != old)
                .or_else(|| (groups.len() != previous.len()).then_some(view.active_index()))
                .or(Some(view.active_index()))
        } else {
            None
        };
        self.animate_group.set(fade_index);
        *self.last_groups.borrow_mut() = groups;
        *self.last_labels.borrow_mut() = view.pills.iter().map(|p| p.label.clone()).collect();

        let reused = self.accordion.borrow().clone();
        if accordion_is_reused(collapsed_before, collapsed)
            && let Some(accordion) = reused
        {
            // The strip still holds this pill. Refill its rows instead of
            // building another accordion, so an open popover (and the row the
            // click came from) survives the selection change.
            self.fill_accordion(&accordion, &view);
            self.update_text(&view, trash);
            self.animate_group.set(None);
            crate::log!(
                "header",
                "window {} refilled the accordion {:?} (active {}, overflow {})",
                self.window_id,
                self.last_labels.borrow(),
                view.active_index(),
                collapsed
            );
            return;
        }

        // Leaving the collapsed strip drops the old accordion before its pill
        // leaves the tree, so the pill's destroy can unparent its popover once.
        if !collapsed {
            self.accordion.borrow_mut().take();
        }
        while let Some(child) = self.strip.first_child() {
            self.strip.remove(&child);
        }
        self.pills.borrow_mut().clear();

        let widgets: Vec<gtk::Widget> = if collapsed {
            vec![self.make_accordion(&view)]
        } else {
            view.pills.iter().map(|pill| self.make_pill(pill)).collect()
        };
        let pitch = self.pitch();
        for (index, widget) in widgets.into_iter().enumerate() {
            self.strip.put(&widget, index as f64 * pitch, 0.0);
            self.pills.borrow_mut().push(widget);
        }
        self.update_text(&view, trash);
        self.animate_group.set(None);
        crate::log_verbose!(
            "header",
            "window {} pills {:?} (active {}, overflow {})",
            self.window_id,
            self.last_labels.borrow(),
            view.active_index(),
            collapsed
        );
    }

    fn pitch(&self) -> f64 {
        (scaled(PILL_W, self.scale.get()) + scaled(GAP_L, self.scale.get())) as f64
    }

    fn update_layout(&self, view: &HeaderView) -> bool {
        let scale = self.scale.get();
        let searching = self.search_open.get() || self.search_field.get_visible();
        let search = if searching {
            ((SEARCH_W - (ICON_SEARCH + 2.0 * GAP_S)) * scale).round() as i32
        } else {
            0
        };
        let desktop = if platform::controls_here().minimize {
            ((ICON_WINDOW + 2.0 * GAP_S + GAP_L) * scale).round() as i32
        } else {
            0
        };
        let fit = crate::look::css::header_fits(
            self.available.get() - search - desktop,
            view.pills.len(),
            scale,
        );
        self.path.set_width_request(scaled(PATH_MIN, scale));
        self.overflow.set(!fit.fits && view.tab_count > 1);
        self.overflow.get()
    }

    fn relayout(self: &Rc<Self>) {
        let Some(ui) = self.ui.borrow().upgrade() else {
            return;
        };
        let Some(window) = ui.workspace_window() else {
            return;
        };
        let view = view(&window);
        let trash = ui.trash_active();
        let was = self.overflow.get();
        let now = self.update_layout(&view);
        if was != now {
            self.rebuild(&ui, false);
        } else {
            self.update_text(&view, trash);
        }
    }

    fn update_text(&self, view: &HeaderView, trash: bool) {
        let shown = if trash {
            "Trash".to_string()
        } else {
            view.folder.display().to_string()
        };
        if trash {
            self.path.remove_css_class("invalid");
            self.path_entry.set_text(&shown);
        } else if !self.path_entry.has_focus() && !self.path.has_css_class("invalid") {
            self.path_entry.set_text(&shown);
        }
        self.path_entry.set_editable(!trash);
        self.path_entry.set_can_focus(!trash);
        self.back
            .set_tooltip_text(Some(if trash { "Back to Files" } else { "Back" }));
        self.back
            .set_sensitive(if trash { true } else { view.can_back });
        self.forward.set_sensitive(view.can_forward && !trash);
        self.up.set_sensitive(view.can_up && !trash);
    }

    pub fn dismiss_empty_search(self: &Rc<Self>) {
        if self.search_open.get() && self.search_entry.text().is_empty() {
            if let Some(ui) = self.window_ui() {
                ui.close_search();
            } else {
                self.close_search();
            }
        }
    }

    pub fn focus_path(&self) {
        if self
            .ui
            .borrow()
            .upgrade()
            .is_some_and(|ui| ui.trash_active())
        {
            return;
        }
        self.path_entry.grab_focus();
        self.path_entry.select_region(0, -1);
    }

    pub fn open_search(self: &Rc<Self>, query: Option<&str>) {
        if self.search_open.get() {
            self.search_entry.grab_focus();
            if let Some(query) = query {
                // GtkEditable::set_text clears the old text before inserting
                // the new one and can emit `changed` with an empty string in
                // between. That is not the user clearing the search: suppress
                // the handler so it cannot cancel the session this command
                // just submitted.
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
        self.search.set_reveal_child(true);
        if let Some(query) = query {
            self.search_programmatic.set(true);
            self.search_entry.set_text(query);
            self.search_programmatic.set(false);
            self.search_entry.set_position(-1);
        }
        self.search_entry.grab_focus();
        let weak = self.search_entry.downgrade();
        glib::timeout_add_local_once(Duration::from_millis(50), move || {
            if let Some(entry) = weak.upgrade() {
                entry.grab_focus();
            }
        });
        self.relayout();
        crate::log!("search", "opened");
    }

    pub fn close_search(self: &Rc<Self>) {
        if !self.search_open.get() {
            return;
        }
        self.search_open.set(false);
        self.search_hint.set_visible(false);
        self.search_programmatic.set(true);
        self.search_entry.set_text("");
        self.search_programmatic.set(false);
        self.search.set_reveal_child(false);
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(
            Duration::from_millis((motion::ms(200) + 10).into()),
            move || {
                if let Some(header) = weak.upgrade()
                    && !header.search_open.get()
                {
                    header.search_field.set_visible(false);
                    header.search_icon.set_visible(true);
                    header.relayout();
                }
            },
        );
        self.relayout();
        crate::log!("search", "closed");
    }

    pub fn escape(self: &Rc<Self>, ui: &Rc<WindowUi>) -> glib::Propagation {
        if self.dialogs_open() {
            return glib::Propagation::Proceed;
        }
        if self.search_open.get() {
            ui.close_search();
            return glib::Propagation::Stop;
        }
        if let Some(window) = ui.workspace_window()
            && window.selection.selected.iter().any(|selected| *selected)
        {
            ui.clear_pill_selection();
            return glib::Propagation::Stop;
        }
        if ui.trash_active() {
            ui.dispatch(Caller::Ui, Invocation::new("nav.back", Vec::new()));
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn make_pill(self: &Rc<Self>, view: &PillView) -> gtk::Widget {
        let index = view.index;
        let pill = gtk::CenterBox::new();
        pill.add_css_class("gl-pill");
        pill.add_css_class(if view.is_group { "group" } else { "tab" });
        pill.set_valign(gtk::Align::Center);
        if view.active {
            pill.add_css_class("active");
        }
        if view.selected {
            pill.add_css_class("selected");
        }
        if view.admin {
            pill.add_css_class("admin");
        }
        let name = gtk::Label::builder()
            .label(&view.label)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(48)
            .build();
        if view.admin {
            let center = gtk::Box::new(
                gtk::Orientation::Horizontal,
                scaled(GAP_S, self.scale.get()),
            );
            center.append(&gtk::Image::from_icon_name(
                crate::look::icons::ADMIN_LOCK_ICON,
            ));
            center.append(&name);
            pill.set_center_widget(Some(&center));
        } else {
            pill.set_center_widget(Some(&name));
        }

        let close = gtk::Button::new();
        close.add_css_class("gl-pill-close");
        close.set_valign(gtk::Align::Center);
        close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
        let header_close = self.clone();
        close.connect_clicked(move |_| {
            if let Some(ui) = header_close.window_ui() {
                ui.close_pill(index);
            }
        });
        pill.set_end_widget(Some(&close));

        let click = gtk::GestureClick::new();
        click.set_button(1);
        let header_click = self.clone();
        click.connect_released(move |gesture, _, _, _| {
            let modifiers = gesture
                .last_event(None)
                .map(|event| event.modifier_state())
                .unwrap_or_else(gdk::ModifierType::empty);
            if let Some(ui) = header_click.window_ui() {
                ui.pill_click(index, modifiers);
            }
        });
        pill.add_controller(click);

        let right = gtk::GestureClick::new();
        right.set_button(3);
        let header_right = self.clone();
        right.connect_pressed(move |gesture, _, _, _| {
            let Some(anchor) = gesture.widget() else {
                return;
            };
            let Some(ui) = header_right.window_ui() else {
                return;
            };
            let Some(window) = ui.workspace_window() else {
                return;
            };
            let current = crate::header::view(&window);
            header_right.open_pill_menu(&ui, index, &anchor, &current, None);
        });
        pill.add_controller(right);

        let drag = gtk::GestureDrag::new();
        let header_begin = self.clone();
        drag.connect_drag_begin(move |gesture, _, _| {
            if let Some((x, y)) = pointer_pos(gesture) {
                header_begin.drag_begin(index, x, y);
            }
        });
        let header_update = self.clone();
        drag.connect_drag_update(move |gesture, _, _| {
            let Some((x, y)) = pointer_pos(gesture) else {
                return;
            };
            let Some(state) = header_update.drag.get() else {
                return;
            };
            let dx = x - state.start_x;
            let dy = y - state.start_y;
            // Claim once the surface distance from the press passes the drag
            // threshold. Never on press: that would deny the click gesture.
            if let Some(pill) = gesture.widget()
                && pill.drag_check_threshold(0, 0, dx.round() as i32, dy.round() as i32)
                && gesture.set_state(gtk::EventSequenceState::Claimed)
            {
                crate::log!("drag", "claimed the sequence at dx {dx:.1}");
            }
            header_update.drag_move(x);
        });
        let header_end = self.clone();
        drag.connect_drag_end(move |gesture, _, _| {
            if let Some((x, _)) = pointer_pos(gesture) {
                header_end.drag_move(x);
            }
            header_end.drag_end();
        });
        let header_cancel = self.clone();
        drag.connect_cancel(move |_, _| {
            crate::log!("drag", "cancel");
            header_cancel.drag_cancel();
        });
        pill.add_controller(drag);

        if let Some(ui) = self.window_ui()
            && let Some(process) = ui.process_upgrade()
        {
            let folder = view.folder.clone();
            dnd::attach_folder_target(
                &pill,
                Rc::downgrade(&process),
                self.window_id,
                Rc::new(move || Some(folder.clone())),
                None,
                None,
            );
            let spring = process.clone();
            let window = self.window_id;
            dnd::spring_load(
                &pill,
                Rc::new(move || spring.spring_focus_pill(window, index)),
            );
        }

        let wrapper = pill_revealer(&pill);
        if self.animate_group.get() == Some(index) {
            wrapper.set_reveal_child(false);
            let weak = wrapper.downgrade();
            glib::timeout_add_local_once(Duration::from_millis(20), move || {
                if let Some(wrapper) = weak.upgrade() {
                    wrapper.set_reveal_child(true);
                }
            });
        }
        wrapper.upcast()
    }

    fn make_accordion(self: &Rc<Self>, view: &HeaderView) -> gtk::Widget {
        let pill = gtk::CenterBox::new();
        pill.add_css_class("gl-pill");
        let label = gtk::Label::new(Some(&format!("{} tabs", view.tab_count)));
        pill.set_center_widget(Some(&label));
        let popover = gtk::Popover::new();
        popover.set_parent(&pill);
        let menu = vbox(&["gl-menu"]);
        let scroller = gtk::ScrolledWindow::new();
        scroller.add_css_class("gl-accordion-scroll");
        scroller.set_policy(gtk::PolicyType::Never, gtk::PolicyType::Automatic);
        scroller.set_propagate_natural_height(true);
        scroller.set_propagate_natural_width(true);
        // A classic scrollbar that takes its own width: the default overlay
        // scrollbar was drawn over each row's close button (Kyle, Nov 8). With
        // the natural width propagated, the popover widens to fit it.
        scroller.set_overlay_scrolling(false);
        scroller.set_child(Some(&menu));
        popover.set_child(Some(&scroller));

        let wrapper = pill_revealer(&pill);
        let accordion = Rc::new(Accordion {
            pill: pill.clone(),
            popover: popover.clone(),
            menu,
            label,
            scroller,
            rows: RefCell::new(Vec::new()),
        });
        *self.accordion.borrow_mut() = Some(accordion.clone());
        self.fill_accordion(&accordion, view);

        // The pill owns this popover for its whole life; only the pill's
        // destroy unparents it. Showing a popover with no parent realizes it
        // outside any window, which crashed GTK (Nov 8), so both openers check.
        // The closures hold weak references: a strong one would make the
        // pill/popover/widget cycle keep itself alive after an expand.
        let weak_header = Rc::downgrade(self);
        let weak_accordion = Rc::downgrade(&accordion);
        let motion = gtk::EventControllerMotion::new();
        let weak_header_motion = weak_header.clone();
        let weak_accordion_motion = weak_accordion.clone();
        motion.connect_enter(move |_, _, _| {
            if let (Some(header), Some(accordion)) = (
                weak_header_motion.upgrade(),
                weak_accordion_motion.upgrade(),
            ) && accordion.popover.parent().is_some()
            {
                header.open_accordion(&accordion);
            }
        });
        pill.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let weak_header_click = weak_header.clone();
        let weak_accordion_click = weak_accordion.clone();
        click.connect_released(move |_, _, _, _| {
            if let (Some(header), Some(accordion)) =
                (weak_header_click.upgrade(), weak_accordion_click.upgrade())
                && accordion.popover.parent().is_some()
            {
                header.open_accordion(&accordion);
            }
        });
        pill.add_controller(click);

        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak_header_key = weak_header;
        let weak_accordion_key = weak_accordion;
        key.connect_key_pressed(move |_, keyval, _, _| {
            if let (Some(header), Some(accordion)) =
                (weak_header_key.upgrade(), weak_accordion_key.upgrade())
            {
                header.accordion_key(&accordion, keyval)
            } else {
                glib::Propagation::Proceed
            }
        });
        accordion.scroller.add_controller(key);

        let weak_map = Rc::downgrade(&accordion);
        popover.connect_map(move |_| {
            let weak = weak_map.clone();
            glib::idle_add_local_once(move || {
                if let Some(accordion) = weak.upgrade()
                    && accordion.popover.is_visible()
                {
                    scroll_active_into_view(&accordion);
                }
            });
        });

        let weak_popover = popover.downgrade();
        pill.connect_destroy(move |_| {
            if let Some(popover) = weak_popover.upgrade() {
                popover.popdown();
                popover.unparent();
            }
        });
        wrapper.upcast()
    }

    /// Show the accordion with the largest content height this window can use.
    /// The cap is computed at open time, so a resized window does not need a
    /// rebuild to get a fitting list.
    fn open_accordion(self: &Rc<Self>, accordion: &Rc<Accordion>) {
        self.apply_accordion_cap(accordion);
        crate::popup::show(&accordion.popover);
    }

    fn apply_accordion_cap(self: &Rc<Self>, accordion: &Accordion) {
        let scale = self.scale.get();
        let (window_height, header_bottom) = match self.window_ui() {
            Some(ui) => {
                let window = ui.window();
                let height = if window.height() > 0 {
                    window.height()
                } else {
                    scaled(f64::from(DEFAULT_H), scale)
                };
                let bottom = accordion
                    .pill
                    .compute_bounds(window)
                    .map(|bounds| (bounds.y() + bounds.height()).round() as i32)
                    .unwrap_or_else(|| scaled(HEADER_H, scale));
                (height, bottom)
            }
            None => (scaled(f64::from(DEFAULT_H), scale), scaled(HEADER_H, scale)),
        };
        let cap = accordion_scroll_cap(scale, window_height, header_bottom);
        accordion.scroller.set_max_content_height(cap.max(1));
        // With an Automatic policy GTK measured the popover before deciding
        // on a scrollbar, then took the scrollbar's width out of the rows and
        // clipped their close buttons (Kyle, Nov 8). Decide here instead: the
        // rows keep their natural width, and a needed scrollbar is counted in
        // the popover's width up front.
        let (_, rows_width, _, _) = accordion.menu.measure(gtk::Orientation::Horizontal, -1);
        let (_, rows_height, _, _) = accordion
            .menu
            .measure(gtk::Orientation::Vertical, rows_width);
        let vertical = if rows_height > cap {
            gtk::PolicyType::Always
        } else {
            gtk::PolicyType::Never
        };
        accordion
            .scroller
            .set_policy(gtk::PolicyType::Never, vertical);
        accordion.scroller.set_min_content_width(rows_width);
    }

    fn activate_accordion_row(
        self: &Rc<Self>,
        index: usize,
        modifiers: gdk::ModifierType,
        popover: &gtk::Popover,
    ) {
        let Some(ui) = self.window_ui() else {
            return;
        };
        if accordion_click(modifiers).keeps_menu_open() {
            // Ctrl and Shift take the same path as a header pill's modified
            // click; the accordion stays open while the selection changes.
            ui.pill_click(index, modifiers);
        } else {
            popover.popdown();
            // The modifier wrapper's plain branch is the same model call as
            // focus_pill(); use it so the row keeps the pre-existing path.
            ui.focus_pill(index);
        }
    }

    fn accordion_key(
        self: &Rc<Self>,
        accordion: &Rc<Accordion>,
        keyval: gdk::Key,
    ) -> glib::Propagation {
        let rows = accordion.rows.borrow();
        if rows.is_empty() {
            return glib::Propagation::Proceed;
        }
        let focused = rows.iter().position(|row| row.choose.has_focus());
        let active = rows.iter().position(|row| row.row.has_css_class("active"));
        match keyval {
            gdk::Key::Up | gdk::Key::Down => {
                let current = focused.or(active).unwrap_or(0);
                let target = if keyval == gdk::Key::Up {
                    current.saturating_sub(1)
                } else {
                    (current + 1).min(rows.len() - 1)
                };
                let button = rows[target].choose.clone();
                drop(rows);
                button.grab_focus();
                scroll_row_into_view(accordion, target);
                glib::Propagation::Stop
            }
            // Enter on a row button is a plain click; on a row's ✕ leave the
            // button's own activation alone. Space toggles exactly like Ctrl.
            gdk::Key::Return | gdk::Key::KP_Enter => {
                let Some(index) = focused else {
                    return glib::Propagation::Proceed;
                };
                drop(rows);
                self.activate_accordion_row(index, gdk::ModifierType::empty(), &accordion.popover);
                glib::Propagation::Stop
            }
            gdk::Key::space => {
                let Some(index) = focused else {
                    return glib::Propagation::Proceed;
                };
                drop(rows);
                self.activate_accordion_row(
                    index,
                    gdk::ModifierType::CONTROL_MASK,
                    &accordion.popover,
                );
                glib::Propagation::Stop
            }
            gdk::Key::Escape => {
                drop(rows);
                accordion.popover.popdown();
                glib::Propagation::Stop
            }
            _ => glib::Propagation::Proceed,
        }
    }

    fn accordion_row_right_click(
        self: &Rc<Self>,
        accordion: &Rc<Accordion>,
        index: usize,
        anchor: &gtk::Widget,
    ) {
        let Some(ui) = self.window_ui() else {
            return;
        };
        let Some(window) = ui.workspace_window() else {
            return;
        };
        let current = view(&window);
        if current.pills.get(index).is_none() {
            return;
        }
        if row_right_click_selects_only(&current, index) {
            // pill_click() rebuilds the header, which recreates the rows. Open
            // the menu from the new row after that refill, never from the one
            // whose gesture is running.
            ui.pill_click(index, gdk::ModifierType::empty());
            let weak_header = Rc::downgrade(self);
            let weak_accordion = Rc::downgrade(accordion);
            glib::idle_add_local_once(move || {
                let (Some(header), Some(accordion)) =
                    (weak_header.upgrade(), weak_accordion.upgrade())
                else {
                    return;
                };
                let current_accordion = header.accordion.borrow().clone();
                if !current_accordion.is_some_and(|current| Rc::ptr_eq(&current, &accordion))
                    || accordion.popover.parent().is_none()
                {
                    return;
                }
                let Some(ui) = header.window_ui() else {
                    return;
                };
                let Some(window) = ui.workspace_window() else {
                    return;
                };
                let current = view(&window);
                let anchor = accordion
                    .rows
                    .borrow()
                    .get(index)
                    .map(|row| row.row.clone());
                let Some(anchor) = anchor else {
                    return;
                };
                header.open_pill_menu(
                    &ui,
                    index,
                    &anchor,
                    &current,
                    Some(accordion.popover.clone()),
                );
            });
        } else {
            self.open_pill_menu(
                &ui,
                index,
                anchor,
                &current,
                Some(accordion.popover.clone()),
            );
        }
    }

    fn fill_accordion(self: &Rc<Self>, accordion: &Rc<Accordion>, view: &HeaderView) {
        let focused = accordion
            .rows
            .borrow()
            .iter()
            .position(|row| row.choose.has_focus());
        let scroll = accordion.scroller.vadjustment().value();
        let open = accordion.popover.is_visible();
        while let Some(child) = accordion.menu.first_child() {
            accordion.menu.remove(&child);
        }
        accordion
            .label
            .set_label(&format!("{} tabs", view.tab_count));

        let weak_header = Rc::downgrade(self);
        let weak_accordion = Rc::downgrade(accordion);
        let mut rows = Vec::new();
        for pill in &view.pills {
            let row = hbox(&["gl-accordion-row"]);
            if pill.active {
                row.add_css_class("active");
            }
            if pill.selected {
                row.add_css_class("selected");
            }
            if pill.admin {
                row.add_css_class("admin");
            }
            let index = pill.index;
            let choose = gtk::Button::new();
            choose.add_css_class("gl-menu-item");
            choose.set_hexpand(true);
            let label = gtk::Label::builder()
                .label(&pill.label)
                .halign(gtk::Align::Start)
                .hexpand(true)
                .build();
            if pill.admin {
                let content = gtk::Box::new(
                    gtk::Orientation::Horizontal,
                    scaled(GAP_S, self.scale.get()),
                );
                content.append(&gtk::Image::from_icon_name(
                    crate::look::icons::ADMIN_LOCK_ICON,
                ));
                content.append(&label);
                choose.set_child(Some(&content));
            } else {
                choose.set_child(Some(&label));
            }

            // A row's own click gesture reads the real modifiers. Claiming the
            // press keeps the button from also emitting a modifier-blind
            // `clicked`; the keyboard path calls the same action directly.
            let click = gtk::GestureClick::new();
            click.set_button(1);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(|gesture, _, _, _| {
                let _ = gesture.set_state(gtk::EventSequenceState::Claimed);
            });
            let weak_header_click = weak_header.clone();
            let weak_accordion_click = weak_accordion.clone();
            click.connect_released(move |gesture, _, _, _| {
                let modifiers = gesture
                    .last_event(None)
                    .map(|event| event.modifier_state())
                    .unwrap_or_else(gdk::ModifierType::empty);
                if let (Some(header), Some(accordion)) =
                    (weak_header_click.upgrade(), weak_accordion_click.upgrade())
                {
                    header.activate_accordion_row(index, modifiers, &accordion.popover);
                }
            });
            choose.add_controller(click);

            let right = gtk::GestureClick::new();
            right.set_button(3);
            let weak_header_right = weak_header.clone();
            let weak_accordion_right = weak_accordion.clone();
            right.connect_pressed(move |gesture, _, _, _| {
                let Some(anchor) = gesture.widget() else {
                    return;
                };
                if let (Some(header), Some(accordion)) =
                    (weak_header_right.upgrade(), weak_accordion_right.upgrade())
                {
                    header.accordion_row_right_click(&accordion, index, &anchor);
                }
            });
            row.add_controller(right);

            row.update_state(&[gtk::accessible::State::Selected(Some(pill.selected))]);
            choose.update_state(&[gtk::accessible::State::Selected(Some(pill.selected))]);
            row.append(&choose);
            let close = gtk::Button::new();
            close.add_css_class("gl-pill-close");
            close.set_valign(gtk::Align::Center);
            close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
            let weak_header_close = weak_header.clone();
            let weak_accordion_close = weak_accordion.clone();
            close.connect_clicked(move |_| {
                if let Some(accordion) = weak_accordion_close.upgrade() {
                    accordion.popover.popdown();
                }
                if let Some(header) = weak_header_close.upgrade()
                    && let Some(ui) = header.window_ui()
                {
                    ui.close_pill(index);
                }
            });
            row.append(&close);
            rows.push(AccordionRow {
                row: row.clone().upcast(),
                choose,
            });
            accordion.menu.append(&row);
        }
        *accordion.rows.borrow_mut() = rows;

        if open {
            if let Some(index) = focused
                && let Some(row) = accordion.rows.borrow().get(index)
            {
                row.choose.grab_focus();
            }
            // The rows were rebuilt, so restore the old top after the new
            // content has been measured. This keeps the list still while the
            // user Ctrl-clicks several rows.
            let adjustment = accordion.scroller.vadjustment();
            adjustment.set_value(scroll);
            glib::idle_add_local_once(move || {
                if adjustment.upper() > adjustment.page_size() {
                    adjustment.set_value(scroll.min(adjustment.upper() - adjustment.page_size()));
                }
            });
        }
    }

    fn open_pill_menu(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        index: usize,
        anchor: &gtk::Widget,
        view: &HeaderView,
        accordion: Option<gtk::Popover>,
    ) {
        let Some(pill) = view.pills.get(index).cloned() else {
            return;
        };
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let menu = vbox(&["gl-menu"]);

        let ui_split = ui.clone();
        menu_item_extra(
            &popover,
            &menu,
            "Split View",
            view.selected_pills() >= 2,
            accordion.clone(),
            move || {
                ui_split.dispatch(Caller::Ui, Invocation::new("group.split", Vec::new()));
            },
        );
        let ui_close = ui.clone();
        menu_item_extra(
            &popover,
            &menu,
            "Close",
            true,
            accordion.clone(),
            move || {
                ui_close.focus_and_dispatch(index, Invocation::new("tab.close", Vec::new()));
            },
        );
        let ui_selected = ui.clone();
        menu_item_extra(
            &popover,
            &menu,
            "Close Selected Tabs",
            view.any_selected(),
            accordion.clone(),
            move || {
                ui_selected.dispatch(
                    Caller::Ui,
                    Invocation::new("tab.close-selected", Vec::new()),
                );
            },
        );
        let ui_window = ui.clone();
        menu_item_extra(
            &popover,
            &menu,
            "Open in New Window",
            true,
            accordion.clone(),
            move || {
                ui_window.focus_and_dispatch(
                    index,
                    Invocation::new("tab.open-in-new-window", Vec::new()),
                );
            },
        );

        if pill.is_group {
            let header_rename = self.clone();
            let ui_rename = ui.clone();
            let anchor_rename = anchor.clone();
            let title = pill.title.clone();
            let accordion_rename = accordion.clone();
            menu_item(&popover, &menu, "Rename Group…", true, move || {
                header_rename.open_rename_popover(
                    &ui_rename,
                    index,
                    &anchor_rename,
                    &title,
                    accordion_rename.clone(),
                );
            });
            let ui_group_close = ui.clone();
            let count = pill.tabs;
            menu_item_extra(
                &popover,
                &menu,
                "Close Group",
                true,
                accordion.clone(),
                move || {
                    ui_group_close.confirm_and_close_group(index, count);
                },
            );
        }

        if view.pills.len() > 1 {
            let header_add = self.clone();
            let ui_add = ui.clone();
            let anchor_add = anchor.clone();
            let current = view.clone();
            let accordion_add = accordion.clone();
            menu_item(&popover, &menu, "Add to Group", true, move || {
                header_add.open_add_group_menu(
                    &ui_add,
                    index,
                    &anchor_add,
                    &current,
                    accordion_add.clone(),
                );
            });
        }

        popover.set_child(Some(&menu));
        close_on_closed(&popover);
        crate::popup::show(&popover);
    }

    fn open_add_group_menu(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        source: usize,
        anchor: &gtk::Widget,
        view: &HeaderView,
        accordion: Option<gtk::Popover>,
    ) {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let menu = vbox(&["gl-menu"]);
        for pill in &view.pills {
            if pill.index == source {
                continue;
            }
            let ui_target = ui.clone();
            let target = pill.index;
            let label = pill.label.clone();
            menu_item_extra(
                &popover,
                &menu,
                &label,
                true,
                accordion.clone(),
                move || {
                    ui_target.focus_and_dispatch(
                        source,
                        Invocation::new(
                            "group.add-tab",
                            vec![("group".to_string(), target.to_string())],
                        ),
                    );
                },
            );
        }
        if menu.first_child().is_none() {
            menu.append(
                &gtk::Label::builder()
                    .label("No other group")
                    .css_classes(["gl-menu-item"])
                    .build(),
            );
        }
        popover.set_child(Some(&menu));
        close_on_closed(&popover);
        crate::popup::show(&popover);
    }

    fn open_rename_popover(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        index: usize,
        anchor: &gtk::Widget,
        title: &str,
        accordion: Option<gtk::Popover>,
    ) {
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let box_ = vbox(&["gl-menu"]);
        let entry = gtk::Entry::new();
        entry.set_text(title);
        entry.set_placeholder_text(Some("Group name"));
        box_.append(&entry);
        let row = hbox(&[]);
        let cancel = gtk::Button::with_label("Cancel");
        cancel.add_css_class("gl-menu-item");
        let popover_cancel = popover.clone();
        cancel.connect_clicked(move |_| take_down(&popover_cancel));
        let rename = gtk::Button::with_label("Rename");
        rename.add_css_class("gl-menu-item");
        let ui_rename = ui.clone();
        let popover_rename = popover.clone();
        let entry_rename = entry.clone();
        let accordion_rename = accordion.clone();
        rename.connect_clicked(move |_| {
            let name = entry_rename.text().to_string();
            take_down(&popover_rename);
            if let Some(accordion) = &accordion_rename {
                accordion.popdown();
            }
            ui_rename.focus_and_dispatch(
                index,
                Invocation::new("group.rename", vec![("name".to_string(), name)]),
            );
        });
        row.append(&cancel);
        row.append(&rename);
        box_.append(&row);
        popover.set_child(Some(&box_));
        ui.set_editing(true);
        let ui_closed = ui.clone();
        popover.connect_closed(move |popover| {
            ui_closed.set_editing(false);
            if popover.parent().is_some() {
                popover.unparent();
            }
        });
        crate::popup::show(&popover);
        entry.grab_focus();
        entry.select_region(0, -1);
        let weak_entry = entry.downgrade();
        let ui_enter = ui.clone();
        let popover_enter = popover.clone();
        let accordion_enter = accordion;
        entry.connect_activate(move |_| {
            let Some(entry) = weak_entry.upgrade() else {
                return;
            };
            let name = entry.text().to_string();
            take_down(&popover_enter);
            if let Some(accordion) = &accordion_enter {
                accordion.popdown();
            }
            ui_enter.focus_and_dispatch(
                index,
                Invocation::new("group.rename", vec![("name".to_string(), name)]),
            );
        });
    }

    pub fn confirm_group(self: &Rc<Self>, ui: &Rc<WindowUi>, index: usize, tabs: usize) {
        let dialog = adw::AlertDialog::new(
            Some(&format!("Close this group and its {tabs} tabs?")),
            None,
        );
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("close", "Close");
        dialog.set_response_appearance("close", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        let ui_action = ui.clone();
        dialog.connect_response(None, move |_, response| {
            if response == "close" {
                ui_action.focus_and_dispatch(index, Invocation::new("group.close", Vec::new()));
            }
        });
        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            if let Some(header) = weak.upgrade() {
                header.dialogs.set(header.dialogs.get().saturating_sub(1));
            }
        });
        self.dialogs.set(self.dialogs.get() + 1);
        dialog.choose(Some(ui.window()), None::<&gtk::gio::Cancellable>, |_| {});
    }

    fn path_enter(self: &Rc<Self>, ui: &Rc<WindowUi>) {
        if ui.trash_active() {
            self.path_entry.set_text("Trash");
            return;
        }
        let raw = self.path_entry.text().to_string();
        let text = raw.trim();
        if text.is_empty() {
            return;
        }
        let home = ui.home();
        let mut target = expand_home(text, &home);
        if !target.is_absolute() {
            let base = ui
                .workspace_window()
                .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
                .unwrap_or(home);
            target = base.join(target);
        }
        if !target.exists() {
            self.path.add_css_class("invalid");
            crate::log!("path", "{} does not exist", target.display());
            return;
        }
        self.path.remove_css_class("invalid");
        let shown = if target.is_file() {
            target
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| target.clone())
        } else {
            target.clone()
        };
        ui.dispatch(
            Caller::Ui,
            Invocation::new(
                "nav.go",
                vec![("path".to_string(), target.to_string_lossy().into_owned())],
            ),
        );
        self.path_entry.set_text(&shown.display().to_string());
    }

    fn sync_path(&self, ui: &Rc<WindowUi>) {
        if self.path.has_css_class("invalid") {
            return;
        }
        if ui.trash_active() {
            self.path_entry.set_text("Trash");
            return;
        }
        if let Some(window) = ui.workspace_window() {
            self.path_entry
                .set_text(&view(&window).folder.display().to_string());
        }
    }

    fn drag_begin(self: &Rc<Self>, index: usize, pointer_x: f64, pointer_y: f64) {
        let base = index as f64 * self.pitch();
        self.drag.set(Some(DragState {
            index,
            start_x: pointer_x,
            start_y: pointer_y,
            base,
            to: index,
        }));
        // Clone the handle, then drop the `pills` borrow before touching GTK.
        let pill = self.pills.borrow().get(index).cloned();
        if let Some(pill) = pill {
            // Raise with insert_after; never unparent during a drag.
            pill.insert_after(&self.strip, self.strip.last_child().as_ref());
            self.strip.move_(&pill, base, 0.0);
        }
    }

    fn drag_move(self: &Rc<Self>, pointer_x: f64) {
        let Some(drag) = self.drag.get() else {
            return;
        };
        let count = self.pills.borrow().len();
        let layout = crate::look::css::drag_layout(
            drag.index,
            pointer_x - drag.start_x,
            self.pitch(),
            count,
        );
        let dragged = self.pills.borrow().get(drag.index).cloned();
        if let Some(pill) = dragged {
            self.strip.move_(&pill, drag.base + layout.offset, 0.0);
        }
        if layout.to != drag.to {
            let pills: Vec<gtk::Widget> = self.pills.borrow().clone();
            for (index, slide) in layout.slides.iter().enumerate() {
                if index != drag.index
                    && let Some(pill) = pills.get(index)
                {
                    set_slide(pill, *slide);
                }
            }
        }
        self.drag.set(Some(DragState {
            to: layout.to,
            ..drag
        }));
    }

    fn drag_end(self: &Rc<Self>) {
        let Some(drag) = self.drag.take() else {
            return;
        };
        // Rebuild after the gesture's handler returns: it removes the widget
        // the gesture fired on.
        let weak_ui = self.ui.borrow().clone();
        glib::idle_add_local_once(move || {
            let Some(ui) = weak_ui.upgrade() else {
                return;
            };
            if drag.to != drag.index {
                ui.dispatch(
                    Caller::Ui,
                    Invocation::new(
                        "tab.move",
                        vec![
                            ("from".to_string(), drag.index.to_string()),
                            ("to".to_string(), drag.to.to_string()),
                        ],
                    ),
                );
                crate::log!("tabs", "moved pill {} to {}", drag.index, drag.to);
            } else {
                ui.refresh_header(false);
            }
        });
    }

    fn drag_cancel(self: &Rc<Self>) {
        self.drag.set(None);
        let weak = self.ui.borrow().clone();
        glib::idle_add_local_once(move || {
            if let Some(ui) = weak.upgrade() {
                ui.refresh_header(false);
            }
        });
    }

    pub fn wire(self: &Rc<Self>, ui: &Rc<WindowUi>) {
        *self.ui.borrow_mut() = Rc::downgrade(ui);

        let header_menu = self.clone();
        let anchor = self.menu_button.clone();
        self.menu_button.connect_clicked(move |_| {
            if let Some(ui) = header_menu.window_ui() {
                open_main_menu(&ui, &anchor);
            }
        });

        for (button, command) in [
            (&self.back, "nav.back"),
            (&self.forward, "nav.forward"),
            (&self.up, "nav.up"),
            (&self.refresh, "nav.refresh"),
        ] {
            let header = self.clone();
            let command = command.to_string();
            button.connect_clicked(move |_| {
                if let Some(ui) = header.window_ui() {
                    ui.dispatch(Caller::Ui, Invocation::new(command.clone(), Vec::new()));
                }
            });
        }

        let header_search = self.clone();
        self.search_icon.connect_clicked(move |_| {
            if let Some(ui) = header_search.window_ui() {
                ui.dispatch(Caller::Ui, Invocation::new("search.open", Vec::new()));
            }
        });
        let header_activate = self.clone();
        self.search_entry.connect_activate(move |entry| {
            // The registry keeps its nonempty Text rule. Enter on an empty
            // entry is not a search and must not be turned into one here.
            if entry.text().is_empty() {
                return;
            }
            if let Some(ui) = header_activate.window_ui() {
                ui.dispatch(
                    Caller::Ui,
                    Invocation::new(
                        "search.open",
                        vec![("query".to_string(), entry.text().to_string())],
                    ),
                );
            }
        });
        let header_typed = self.clone();
        self.search_entry.connect_changed(move |entry| {
            crate::log!("search", "typed {:?}", entry.text());
            let search_visible = header_typed
                .window_ui()
                .is_some_and(|ui| ui.search_active());
            header_typed
                .search_hint
                .set_visible(!entry.text().is_empty() && !search_visible);
            if entry.text().is_empty()
                && header_typed.search_open.get()
                && !header_typed.search_programmatic.get()
                && let Some(ui) = header_typed.window_ui()
            {
                ui.clear_search();
            }
        });
        let header_focus = self.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            if header_focus.search_open.get() && header_focus.search_entry.text().is_empty() {
                if let Some(ui) = header_focus.window_ui() {
                    ui.close_search();
                } else {
                    header_focus.close_search();
                }
            }
        });
        self.search_entry.add_controller(focus);

        let header_path = self.clone();
        self.path_entry.connect_activate(move |_| {
            if let Some(ui) = header_path.window_ui() {
                header_path.path_enter(&ui);
            }
        });
        let header_changed = self.clone();
        self.path_entry.connect_changed(move |_| {
            header_changed.path.remove_css_class("invalid");
        });
        let header_path_leave = self.clone();
        let path_focus = gtk::EventControllerFocus::new();
        path_focus.connect_leave(move |_| {
            if let Some(ui) = header_path_leave.window_ui() {
                header_path_leave.sync_path(&ui);
            }
        });
        self.path_entry.add_controller(path_focus);

        let header_trash = self.clone();
        self.trash.connect_clicked(move |_| {
            if let Some(ui) = header_trash.window_ui() {
                let command = if ui.trash_active() {
                    "nav.back"
                } else {
                    "trash.open"
                };
                ui.dispatch(Caller::Ui, Invocation::new(command, Vec::new()));
            }
        });
        if let Some(process) = ui.process_upgrade() {
            dnd::attach_trash_target(&self.trash, Rc::downgrade(&process), self.window_id);
        }

        let window = ui.window().clone();
        let float_action = platform::controls_here().float;
        self.float.connect_clicked(move |_| {
            platform::toggle_float(window.upcast_ref(), float_action);
        });
        if let Some(minimize) = &self.minimize {
            let window = ui.window().clone();
            minimize.connect_clicked(move |_| window.minimize());
        }
        let header_close = self.clone();
        self.close.connect_clicked(move |_| {
            if let Some(ui) = header_close.window_ui() {
                ui.dispatch(Caller::Ui, Invocation::new("window.close", Vec::new()));
            }
        });
    }
}

fn pointer_pos(gesture: &gtk::GestureDrag) -> Option<(f64, f64)> {
    gesture.last_event(None)?.position()
}

fn pill_revealer(pill: &impl IsA<gtk::Widget>) -> gtk::Revealer {
    let revealer = gtk::Revealer::new();
    revealer.add_css_class("gl-pill-wrap");
    revealer.set_transition_type(gtk::RevealerTransitionType::Crossfade);
    revealer.set_transition_duration(motion::ms(FADE_MS));
    revealer.set_child(Some(pill));
    revealer.set_reveal_child(true);
    revealer
}

fn open_main_menu(ui: &Rc<WindowUi>, anchor: &gtk::Button) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let menu = vbox(&["gl-menu"]);
    let ui_new = ui.clone();
    menu_item(&popover, &menu, "New Window", true, move || {
        ui_new.dispatch(Caller::Ui, Invocation::new("window.new", Vec::new()));
    });
    let ui_settings = ui.clone();
    let settings_anchor = anchor.clone();
    menu_item(&popover, &menu, "Settings", true, move || {
        // Open the submenu after this click and the closing main menu are
        // done, never from inside the popover being torn down (see popup.rs).
        let ui = ui_settings.clone();
        let anchor = settings_anchor.clone();
        gtk::glib::idle_add_local_once(move || open_settings_menu(&ui, &anchor));
    });
    let ui_quit = ui.clone();
    menu_item(&popover, &menu, "Quit", true, move || {
        ui_quit.dispatch(Caller::Ui, Invocation::new("app.quit", Vec::new()));
    });
    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    crate::popup::show(&popover);
}

/// The Settings submenu. The switch is the only path to `agent.set-enabled`
/// and it dispatches as `Caller::Ui`; the command's registry row refuses every
/// other caller before a handler or config write.
fn open_settings_menu(ui: &Rc<WindowUi>, anchor: &gtk::Button) {
    let popover = gtk::Popover::new();
    popover.set_parent(anchor);
    let menu = vbox(&["gl-menu"]);

    let heading = gtk::Label::builder()
        .label("Settings")
        .halign(gtk::Align::Start)
        .build();
    heading.add_css_class("gl-menu-item");
    menu.append(&heading);

    let sort_heading = gtk::Label::builder()
        .label("Sort")
        .halign(gtk::Align::Start)
        .build();
    sort_heading.add_css_class("gl-menu-item");
    menu.append(&sort_heading);

    let keys: Vec<(SortBy, SortRow)> = [SortBy::Name, SortBy::Modified, SortBy::Size, SortBy::Type]
        .into_iter()
        .map(|by| (by, sort_row(&menu)))
        .collect();
    let orders: Vec<(SortOrder, SortRow)> = [SortOrder::Ascending, SortOrder::Descending]
        .into_iter()
        .map(|order| (order, sort_row(&menu)))
        .collect();
    let folder = sort_row(&menu);
    let sort_rows = Rc::new(SortRows {
        keys,
        orders,
        folder,
    });
    sort_rows.refresh(ui);
    *ui.header().sort_rows.borrow_mut() = Some(sort_rows.clone());
    {
        let header = Rc::downgrade(ui.header());
        let rows = Rc::downgrade(&sort_rows);
        popover.connect_closed(move |_| {
            let (Some(header), Some(rows)) = (header.upgrade(), rows.upgrade()) else {
                return;
            };
            let mut slot = header.sort_rows.borrow_mut();
            if slot.as_ref().is_some_and(|open| Rc::ptr_eq(open, &rows)) {
                *slot = None;
            }
        });
    }

    for (by, row) in &sort_rows.keys {
        let ui_key = ui.clone();
        let rows = Rc::downgrade(&sort_rows);
        let by = *by;
        row.button.connect_clicked(move |_| {
            let state = sort_state_now(&ui_key);
            ui_key.dispatch(Caller::Ui, sort_key_action(&state, by).invocation());
            if let Some(rows) = rows.upgrade() {
                rows.refresh(&ui_key);
            }
        });
    }
    for (order, row) in &sort_rows.orders {
        let ui_order = ui.clone();
        let rows = Rc::downgrade(&sort_rows);
        let order = *order;
        row.button.connect_clicked(move |_| {
            let state = sort_state_now(&ui_order);
            ui_order.dispatch(Caller::Ui, sort_order_action(&state, order).invocation());
            if let Some(rows) = rows.upgrade() {
                rows.refresh(&ui_order);
            }
        });
    }
    {
        let ui_folder = ui.clone();
        let rows = Rc::downgrade(&sort_rows);
        sort_rows.folder.button.connect_clicked(move |_| {
            let state = sort_state_now(&ui_folder);
            if let Some(action) = sort_folder_toggle_action(&state) {
                ui_folder.dispatch(Caller::Ui, action.invocation());
            }
            if let Some(rows) = rows.upgrade() {
                rows.refresh(&ui_folder);
            }
        });
    }

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("gl-menu-item");
    let label = gtk::Label::builder()
        .label("Allow local automation")
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    let switch = gtk::Switch::new();
    switch.update_property(&[gtk::accessible::Property::Label("Allow local automation")]);
    switch.update_property(&[gtk::accessible::Property::Description(
        "Allows local scripts and AI agents to control Gimbal.",
    )]);
    let status = ui
        .process_upgrade()
        .map(|process| process.agent_status())
        .unwrap_or(crate::agent::AgentStatus {
            state: crate::agent::AgentState::Off,
            detail: None,
            path: None,
            generation: 0,
            instance: None,
        });
    let syncing = Rc::new(Cell::new(false));
    syncing.set(true);
    switch.set_active(status.state == crate::agent::AgentState::On);
    syncing.set(false);
    row.append(&label);
    row.append(&switch);
    menu.append(&row);

    let explanation = gtk::Label::builder()
        .label("Allows local scripts and AI agents to control Gimbal. Off is the default.")
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    explanation.add_css_class("dim-label");
    menu.append(&explanation);

    let status_label = gtk::Label::builder()
        .label(agent_status_text(&status))
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    status_label.add_css_class("dim-label");
    menu.append(&status_label);
    *ui.header().agent_widgets.borrow_mut() =
        Some((switch.clone(), status_label.clone(), syncing.clone()));

    let ui_switch = ui.clone();
    let switch_widget = switch.clone();
    let switch_syncing = syncing.clone();
    switch.connect_active_notify(move |_| {
        if switch_syncing.get() {
            return;
        }
        let requested = switch_widget.is_active();
        ui_switch.dispatch(
            Caller::Ui,
            Invocation::new(
                "agent.set-enabled",
                vec![("enabled".to_string(), requested.to_string())],
            ),
        );
        if let Some(process) = ui_switch.process_upgrade() {
            let status = process.agent_status();
            ui_switch.header().refresh_agent_status(&status);
            // The dispatch already refreshed every window; this also covers a
            // second open submenu in another window.
            process.refresh_agent_status_all();
            let actual = status.state == crate::agent::AgentState::On;
            if actual != requested {
                switch_syncing.set(true);
                switch_widget.set_active(actual);
                switch_syncing.set(false);
            }
        }
    });

    let update_status = ui
        .process_upgrade()
        .map(|process| crate::app::update::status(&process))
        .unwrap_or(crate::app::update::UpdateStatus {
            kind: fm_core::update::InstallKind::Development,
            check_enabled: true,
            checking: false,
            result: None,
        });

    let updates_heading = gtk::Label::builder()
        .label(fm_core::update::text::UPDATES)
        .halign(gtk::Align::Start)
        .build();
    updates_heading.add_css_class("gl-menu-item");
    menu.append(&updates_heading);

    let version_label = gtk::Label::builder()
        .label(fm_core::update::text::version_label())
        .halign(gtk::Align::Start)
        .build();
    version_label.add_css_class("gl-menu-item");
    menu.append(&version_label);

    let update_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    update_row.add_css_class("gl-menu-item");
    let update_label = gtk::Label::builder()
        .label(fm_core::update::text::CHECK_FOR_UPDATES)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    let update_switch = gtk::Switch::new();
    update_switch.update_property(&[gtk::accessible::Property::Label(
        fm_core::update::text::CHECK_FOR_UPDATES,
    )]);
    update_row.append(&update_label);
    update_row.append(&update_switch);
    menu.append(&update_row);

    let update_syncing = Rc::new(Cell::new(false));
    let check_now = gtk::Button::with_label(fm_core::update::text::CHECK_NOW);
    check_now.add_css_class("gl-menu-item");
    menu.append(&check_now);
    let releases = gtk::Button::with_label(fm_core::update::text::OPEN_RELEASES);
    releases.add_css_class("gl-menu-item");
    releases.set_visible(false);
    menu.append(&releases);

    let update_status_label = gtk::Label::builder()
        .label(update_status.status_text())
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    update_status_label.add_css_class("dim-label");
    menu.append(&update_status_label);

    *ui.header().update_widgets.borrow_mut() = Some(UpdateWidgets {
        switch: update_switch.clone(),
        status: update_status_label,
        check: check_now.clone(),
        releases: releases.clone(),
        syncing: update_syncing.clone(),
    });
    ui.header().refresh_update_status(&update_status);

    let ui_update_switch = ui.clone();
    let switch_widget = update_switch.clone();
    let switch_syncing = update_syncing.clone();
    update_switch.connect_active_notify(move |_| {
        if switch_syncing.get() {
            return;
        }
        let requested = switch_widget.is_active();
        ui_update_switch.dispatch(
            Caller::Ui,
            crate::app::update::set_enabled_invocation(requested),
        );
        if let Some(process) = ui_update_switch.process_upgrade() {
            let status = crate::app::update::status(&process);
            ui_update_switch.header().refresh_update_status(&status);
            if status.check_enabled != requested {
                switch_syncing.set(true);
                switch_widget.set_active(status.check_enabled);
                switch_syncing.set(false);
            }
        }
    });

    let ui_check = ui.clone();
    check_now.connect_clicked(move |_| {
        ui_check.dispatch(Caller::Ui, crate::app::update::check_now_invocation());
    });

    let ui_releases = ui.clone();
    releases.connect_clicked(move |_| {
        let launcher = gtk::UriLauncher::new(fm_core::update::RELEASES_PAGE);
        launcher.launch(
            Some(ui_releases.window()),
            None::<&gtk::gio::Cancellable>,
            |result| {
                if let Err(error) = result {
                    crate::log!("update", "could not open the releases page: {error}");
                }
            },
        );
    });

    let integration_status = crate::filemanager1::status();

    let integration_heading = gtk::Label::builder()
        .label(crate::filemanager1::text::INTEGRATION)
        .halign(gtk::Align::Start)
        .build();
    integration_heading.add_css_class("gl-menu-item");
    menu.append(&integration_heading);

    let integration_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    integration_row.add_css_class("gl-menu-item");
    let integration_label = gtk::Label::builder()
        .label(crate::filemanager1::text::SHOW_IN_FOLDER)
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    let integration_switch = gtk::Switch::new();
    integration_switch.update_property(&[gtk::accessible::Property::Label(
        crate::filemanager1::text::SHOW_IN_FOLDER,
    )]);
    integration_switch.update_property(&[gtk::accessible::Property::Description(
        crate::filemanager1::text::SUBTITLE,
    )]);
    integration_row.append(&integration_label);
    integration_row.append(&integration_switch);
    menu.append(&integration_row);

    let integration_subtitle = gtk::Label::builder()
        .label(crate::filemanager1::text::SUBTITLE)
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    integration_subtitle.add_css_class("dim-label");
    menu.append(&integration_subtitle);

    let integration_status_label = gtk::Label::builder()
        .label(integration_status.text())
        .wrap(true)
        .xalign(0.0)
        .halign(gtk::Align::Start)
        .build();
    integration_status_label.add_css_class("dim-label");
    menu.append(&integration_status_label);

    // Report a bug: both actions are local. The report command writes a file
    // the user reads and attaches; nothing is sent anywhere.
    let report_row = gtk::Box::new(gtk::Orientation::Vertical, 6);
    report_row.add_css_class("gl-menu-item");
    let report_label = gtk::Label::builder()
        .label(fm_core::logfile::text::REPORT_A_BUG)
        .halign(gtk::Align::Start)
        .build();
    let report_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let open_log = gtk::Button::with_label(fm_core::logfile::text::OPEN_LOG_FOLDER);
    open_log.update_property(&[gtk::accessible::Property::Label(
        fm_core::logfile::text::OPEN_LOG_FOLDER,
    )]);
    let copy_report = gtk::Button::with_label(fm_core::logfile::text::COPY_REPORT_COMMAND);
    copy_report.update_property(&[gtk::accessible::Property::Label(
        fm_core::logfile::text::COPY_REPORT_COMMAND,
    )]);
    report_buttons.append(&open_log);
    report_buttons.append(&copy_report);
    report_row.append(&report_label);
    report_row.append(&report_buttons);
    menu.append(&report_row);

    let ui_open_log = ui.clone();
    open_log.connect_clicked(move |_| {
        let Some(directory) = fm_core::logfile::default_log_dir() else {
            return;
        };
        let _ = std::fs::create_dir_all(&directory);
        let uri = gtk::gio::File::for_path(&directory).uri();
        gtk::UriLauncher::new(&uri).launch(
            Some(ui_open_log.window()),
            None::<&gtk::gio::Cancellable>,
            |result| {
                if let Err(error) = result {
                    crate::log!("logfile", "could not open the log folder: {error}");
                }
            },
        );
    });

    let ui_copy_report = ui.clone();
    copy_report.connect_clicked(move |_| {
        ui_copy_report
            .window()
            .clipboard()
            .set_text(fm_core::logfile::REPORT_COMMAND);
    });

    let integration_syncing = Rc::new(Cell::new(false));
    *ui.header().filemanager1_widgets.borrow_mut() = Some(FileManager1Widgets {
        switch: integration_switch.clone(),
        status: integration_status_label.clone(),
        syncing: integration_syncing.clone(),
    });
    ui.header().refresh_filemanager1_status(&integration_status);

    let ui_integration = ui.clone();
    let integration_switch_widget = integration_switch.clone();
    let integration_syncing_flag = integration_syncing.clone();
    integration_switch.connect_active_notify(move |_| {
        if integration_syncing_flag.get() {
            return;
        }
        let requested = integration_switch_widget.is_active();
        ui_integration.dispatch(
            Caller::Ui,
            Invocation::new(
                "integration.set-show-in-folder",
                vec![("enabled".to_string(), requested.to_string())],
            ),
        );
        if let Some(process) = ui_integration.process_upgrade() {
            let status = crate::filemanager1::status();
            ui_integration.header().refresh_filemanager1_status(&status);
            process.refresh_filemanager1_status_all();
            if status.enabled != requested {
                integration_syncing_flag.set(true);
                integration_switch_widget.set_active(status.enabled);
                integration_syncing_flag.set(false);
            }
        }
    });

    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    crate::popup::show(&popover);
}

fn agent_status_text(status: &crate::agent::AgentStatus) -> String {
    let mut text = match status.state {
        crate::agent::AgentState::Off => "Off".to_string(),
        crate::agent::AgentState::On => match &status.path {
            Some(path) => format!("On — {}", path.display()),
            None => "On".to_string(),
        },
        crate::agent::AgentState::Unavailable => "Unavailable".to_string(),
    };
    if let Some(detail) = &status.detail {
        text.push_str(" — ");
        text.push_str(detail);
    }
    text
}

/// Which sort the Settings rows are showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortScope {
    Global,
    Folder,
}

impl SortScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Folder => "folder",
        }
    }
}

/// A click in the Sort section, before it becomes a command invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SortMenuAction {
    SetSort {
        by: SortBy,
        order: SortOrder,
        scope: SortScope,
    },
    ClearFolderSort,
}

impl SortMenuAction {
    pub fn invocation(&self) -> Invocation {
        match self {
            Self::SetSort { by, order, scope } => Invocation::new(
                "view.set-sort",
                vec![
                    ("by".to_string(), by.as_str().to_string()),
                    ("order".to_string(), order.as_str().to_string()),
                    ("scope".to_string(), scope.as_str().to_string()),
                ],
            ),
            Self::ClearFolderSort => Invocation::new("view.clear-folder-sort", Vec::new()),
        }
    }
}

/// The rule and scope the Sort rows display. It always comes from the loaded
/// config, never from widget state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SortMenuState {
    pub rule: SortRule,
    pub scope: SortScope,
    pub folder_available: bool,
    pub folder_label: String,
}

pub fn sort_menu_state(config: &Config, folder: Option<&Path>) -> SortMenuState {
    let Some(folder) = folder else {
        return SortMenuState {
            rule: config.sort.global(),
            scope: SortScope::Global,
            folder_available: false,
            folder_label: "Only this folder".to_string(),
        };
    };

    SortMenuState {
        rule: config.sort_for(folder),
        scope: if config.has_folder_sort(folder) {
            SortScope::Folder
        } else {
            SortScope::Global
        },
        folder_available: true,
        folder_label: sort_folder_label(folder),
    }
}

fn sort_folder_label(folder: &Path) -> String {
    match folder.file_name() {
        Some(name) if !name.is_empty() => format!("Only for {}", name.to_string_lossy()),
        _ => "Only this folder".to_string(),
    }
}

pub fn sort_key_label(by: SortBy) -> &'static str {
    match by {
        SortBy::Name => "Name",
        SortBy::Modified => "Date modified",
        SortBy::Size => "Size",
        SortBy::Type => "Type",
    }
}

pub fn sort_order_label(by: SortBy, order: SortOrder) -> &'static str {
    match (by, order) {
        (SortBy::Name | SortBy::Type, SortOrder::Ascending) => "A → Z",
        (SortBy::Name | SortBy::Type, SortOrder::Descending) => "Z → A",
        (SortBy::Modified, SortOrder::Ascending) => "Oldest first",
        (SortBy::Modified, SortOrder::Descending) => "Newest first",
        (SortBy::Size, SortOrder::Ascending) => "Smallest first",
        (SortBy::Size, SortOrder::Descending) => "Largest first",
    }
}

/// Choosing a key keeps the order currently shown and writes to the checked
/// scope.
pub fn sort_key_action(state: &SortMenuState, by: SortBy) -> SortMenuAction {
    SortMenuAction::SetSort {
        by,
        order: state.rule.order,
        scope: state.scope,
    }
}

/// Choosing an order keeps the key currently shown and writes to the checked
/// scope.
pub fn sort_order_action(state: &SortMenuState, order: SortOrder) -> SortMenuAction {
    SortMenuAction::SetSort {
        by: state.rule.by,
        order,
        scope: state.scope,
    }
}

/// The "Only this folder" check. Checking creates an override with the rule
/// currently shown; unchecking removes it. A view without a folder cannot.
pub fn sort_folder_toggle_action(state: &SortMenuState) -> Option<SortMenuAction> {
    if !state.folder_available {
        return None;
    }
    Some(match state.scope {
        SortScope::Folder => SortMenuAction::ClearFolderSort,
        SortScope::Global => SortMenuAction::SetSort {
            by: state.rule.by,
            order: state.rule.order,
            scope: SortScope::Folder,
        },
    })
}

fn sort_row_text(checked: bool, text: &str) -> String {
    if checked {
        format!("✓ {text}")
    } else {
        format!("   {text}")
    }
}

struct SortRow {
    button: gtk::Button,
    label: gtk::Label,
}

struct SortRows {
    keys: Vec<(SortBy, SortRow)>,
    orders: Vec<(SortOrder, SortRow)>,
    folder: SortRow,
}

impl SortRows {
    fn refresh(&self, ui: &Rc<WindowUi>) {
        let state = sort_state_now(ui);
        for (by, row) in &self.keys {
            row.label
                .set_text(&sort_row_text(state.rule.by == *by, sort_key_label(*by)));
        }
        for (order, row) in &self.orders {
            row.label.set_text(&sort_row_text(
                state.rule.order == *order,
                sort_order_label(state.rule.by, *order),
            ));
        }
        self.folder.label.set_text(&sort_row_text(
            state.scope == SortScope::Folder,
            &state.folder_label,
        ));
        self.folder.button.set_sensitive(state.folder_available);
        self.folder
            .button
            .set_tooltip_text(if state.folder_available {
                None
            } else {
                Some("This view has no folder, so sort settings apply to every folder.")
            });
    }
}

fn sort_state_now(ui: &Rc<WindowUi>) -> SortMenuState {
    let folder = focused_sort_folder(ui);
    let Some(process) = ui.process_upgrade() else {
        return sort_menu_state(&Config::default(), None);
    };
    let config = process.config.borrow();
    sort_menu_state(config.config(), folder.as_deref())
}

fn focused_sort_folder(ui: &Rc<WindowUi>) -> Option<PathBuf> {
    if ui.trash_active() || ui.search_active() {
        return None;
    }
    ui.workspace_window()
        .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
}

fn sort_row(menu: &gtk::Box) -> SortRow {
    let button = gtk::Button::new();
    button.add_css_class("gl-menu-item");
    let label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    button.set_child(Some(&label));
    menu.append(&button);
    SortRow { button, label }
}

fn menu_item(
    popover: &gtk::Popover,
    menu: &gtk::Box,
    text: &str,
    enabled: bool,
    action: impl Fn() + 'static,
) {
    menu_item_extra(popover, menu, text, enabled, None, action);
}

/// A menu item that can also close the accordion under an inner menu. The
/// inner menu is unparented first, so the action never runs against a widget
/// whose anchor has already gone away.
fn menu_item_extra(
    popover: &gtk::Popover,
    menu: &gtk::Box,
    text: &str,
    enabled: bool,
    accordion: Option<gtk::Popover>,
    action: impl Fn() + 'static,
) {
    let item = gtk::Button::new();
    item.add_css_class("gl-menu-item");
    item.set_sensitive(enabled);
    item.set_child(Some(
        &gtk::Label::builder()
            .label(text)
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build(),
    ));
    let popover = popover.clone();
    item.connect_clicked(move |_| {
        take_down(&popover);
        if let Some(accordion) = &accordion {
            accordion.popdown();
        }
        action();
    });
    menu.append(&item);
}

/// Focus the active row and put it inside the scroll viewport. Called from
/// the popover's map handler after the list has been allocated.
fn scroll_active_into_view(accordion: &Rc<Accordion>) {
    let rows = accordion.rows.borrow();
    let Some(index) = rows
        .iter()
        .position(|row| row.row.has_css_class("active"))
        .or_else(|| (!rows.is_empty()).then_some(0))
    else {
        return;
    };
    let button = rows[index].choose.clone();
    drop(rows);
    button.grab_focus();
    scroll_row_into_view(accordion, index);
}

fn scroll_row_into_view(accordion: &Accordion, index: usize) {
    let row = accordion
        .rows
        .borrow()
        .get(index)
        .map(|row| row.row.clone());
    let Some(row) = row else {
        return;
    };
    let Some(bounds) = row.compute_bounds(&accordion.menu) else {
        return;
    };
    let adjustment = accordion.scroller.vadjustment();
    let top = f64::from(bounds.y());
    let bottom = top + f64::from(bounds.height());
    let value = adjustment.value();
    let page = adjustment.page_size();
    if top < value {
        adjustment.set_value(top);
    } else if bottom > value + page {
        adjustment.set_value(bottom - page);
    }
}

fn take_down(popover: &gtk::Popover) {
    popover.popdown();
    if popover.parent().is_some() {
        popover.unparent();
    }
}

fn close_on_closed(popover: &gtk::Popover) {
    popover.connect_closed(|popover| {
        if popover.parent().is_some() {
            popover.unparent();
        }
    });
}

fn set_slide(pill: &gtk::Widget, offset: f64) {
    pill.remove_css_class("slide-left");
    pill.remove_css_class("slide-right");
    if offset < 0.0 {
        pill.add_css_class("slide-left");
    } else if offset > 0.0 {
        pill.add_css_class("slide-right");
    }
}

fn hbox(classes: &[&str]) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    for class in classes {
        box_.add_css_class(class);
    }
    box_
}

fn vbox(classes: &[&str]) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    for class in classes {
        box_.add_css_class(class);
    }
    box_
}

fn icon_button(icon_name: &str, extra: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.add_css_class("gl-icon-button");
    if !extra.is_empty() {
        button.add_css_class(extra);
    }
    button.set_child(Some(&gtk::Image::from_icon_name(icon_name)));
    button.set_valign(gtk::Align::Center);
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

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::workspace::Workspace;

    fn window_with(folders: &[&str]) -> fm_core::workspace::Workspace {
        let mut workspace = Workspace::with_home(PathBuf::from("/home/kyle"));
        workspace
            .window_new(Some(PathBuf::from(folders[0])))
            .unwrap();
        for folder in &folders[1..] {
            workspace.tab_new(Some(PathBuf::from(folder))).unwrap();
        }
        workspace
    }

    fn first(workspace: &Workspace) -> Window {
        workspace.windows[0].clone()
    }

    #[test]
    fn a_single_tab_shows_its_folder_name() {
        let mut workspace = window_with(&["/home/kyle", "/tmp/projects"]);
        assert_eq!(
            pill_label(first(&workspace).focused_group().unwrap()),
            "projects"
        );
        workspace.pill_click(0).unwrap();
        assert_eq!(
            pill_label(first(&workspace).focused_group().unwrap()),
            "kyle"
        );
    }

    #[test]
    fn a_split_group_shows_its_title_and_count_then_its_name() {
        let mut workspace = window_with(&["/tmp/a", "/tmp/b", "/tmp/c"]);
        workspace.pill_ctrl_click(0).unwrap();
        workspace.pill_ctrl_click(1).unwrap();
        workspace.group_split().unwrap();
        let group = first(&workspace).focused_group().unwrap().clone();
        assert_eq!(pill_label(&group), "Group 1 · 2 tabs");
        workspace.group_rename("Work").unwrap();
        let group = first(&workspace).focused_group().unwrap().clone();
        assert_eq!(pill_label(&group), "Work · 2 tabs");
    }

    #[test]
    fn the_view_marks_the_focused_pill_active_and_keeps_selection() {
        let mut workspace = window_with(&["/tmp/a", "/tmp/b", "/tmp/c"]);
        workspace.pill_ctrl_click(0).unwrap();
        workspace.pill_ctrl_click(2).unwrap();
        let window = first(&workspace);
        let view = view(&window);
        assert_eq!(view.pills.len(), 3);
        assert!(view.pills[2].active);
        assert!(view.pills[0].selected);
        assert!(view.pills[2].selected);
        assert!(!view.pills[1].selected);
        assert_eq!(view.tab_count, 3);
        assert_eq!(view.active_index(), 2);
        assert_eq!(view.selected_pills(), 2);
        assert!(view.can_up);
    }

    #[test]
    fn a_root_folder_cannot_go_up() {
        let workspace = window_with(&["/"]);
        let view = view(&first(&workspace));
        assert!(!view.can_up);
        assert!(!view.can_back);
        assert!(!view.can_forward);
    }

    fn downloads_example() -> (Config, SortMenuState) {
        let home = Path::new("/home/kyle");
        let mut config = Config::with_defaults(home, &Default::default());
        config.sort.by = SortBy::Name;
        config.sort.order = SortOrder::Ascending;
        config.sort.folders.push(fm_core::config::FolderSort {
            folder: home.join("Downloads"),
            by: SortBy::Modified,
            order: SortOrder::Descending,
        });
        let state = sort_menu_state(&config, Some(&home.join("Downloads")));
        (config, state)
    }

    #[test]
    fn the_downloads_example_shows_its_folder_rule_and_writes_to_it() {
        let (_config, state) = downloads_example();
        assert_eq!(
            state.rule,
            SortRule {
                by: SortBy::Modified,
                order: SortOrder::Descending,
                folders_first: true,
            }
        );
        assert_eq!(state.scope, SortScope::Folder);
        assert_eq!(state.folder_label, "Only for Downloads");
        assert_eq!(
            sort_order_label(SortBy::Modified, SortOrder::Descending),
            "Newest first"
        );

        assert_eq!(
            sort_key_action(&state, SortBy::Size).invocation(),
            Invocation::new(
                "view.set-sort",
                vec![
                    ("by".to_string(), "size".to_string()),
                    ("order".to_string(), "descending".to_string()),
                    ("scope".to_string(), "folder".to_string()),
                ],
            )
        );
        assert_eq!(
            sort_order_action(&state, SortOrder::Ascending).invocation(),
            Invocation::new(
                "view.set-sort",
                vec![
                    ("by".to_string(), "modified".to_string()),
                    ("order".to_string(), "ascending".to_string()),
                    ("scope".to_string(), "folder".to_string()),
                ],
            )
        );
        assert_eq!(
            sort_folder_toggle_action(&state)
                .expect("a folder can be unchecked")
                .invocation(),
            Invocation::new("view.clear-folder-sort", Vec::new())
        );
    }

    #[test]
    fn another_folder_shows_the_global_rule_and_checking_creates_an_override() {
        let (config, _) = downloads_example();
        let state = sort_menu_state(&config, Some(Path::new("/home/kyle/Documents")));
        assert_eq!(
            state.rule,
            SortRule {
                by: SortBy::Name,
                order: SortOrder::Ascending,
                folders_first: true,
            }
        );
        assert_eq!(state.scope, SortScope::Global);
        assert_eq!(state.folder_label, "Only for Documents");
        assert_eq!(
            sort_folder_toggle_action(&state).unwrap(),
            SortMenuAction::SetSort {
                by: SortBy::Name,
                order: SortOrder::Ascending,
                scope: SortScope::Folder,
            }
        );
        assert_eq!(
            sort_key_action(&state, SortBy::Type).invocation(),
            Invocation::new(
                "view.set-sort",
                vec![
                    ("by".to_string(), "type".to_string()),
                    ("order".to_string(), "ascending".to_string()),
                    ("scope".to_string(), "global".to_string()),
                ],
            )
        );
    }

    #[test]
    fn a_view_without_a_folder_shows_the_global_rule_and_cannot_check_the_row() {
        let (config, _) = downloads_example();
        let state = sort_menu_state(&config, None);
        assert_eq!(state.rule, config.sort.global());
        assert_eq!(state.scope, SortScope::Global);
        assert!(!state.folder_available);
        assert_eq!(state.folder_label, "Only this folder");
        assert_eq!(sort_folder_toggle_action(&state), None);
        assert_eq!(
            sort_key_action(&state, SortBy::Modified).invocation(),
            Invocation::new(
                "view.set-sort",
                vec![
                    ("by".to_string(), "modified".to_string()),
                    ("order".to_string(), "ascending".to_string()),
                    ("scope".to_string(), "global".to_string()),
                ],
            )
        );
    }

    #[test]
    fn check_marks_and_order_words_follow_the_rule() {
        assert_eq!(sort_row_text(true, "Name"), "✓ Name");
        assert_eq!(sort_row_text(false, "Name"), "   Name");
        assert_eq!(
            sort_order_label(SortBy::Name, SortOrder::Ascending),
            "A → Z"
        );
        assert_eq!(
            sort_order_label(SortBy::Size, SortOrder::Descending),
            "Largest first"
        );
        assert_eq!(
            sort_order_label(SortBy::Modified, SortOrder::Ascending),
            "Oldest first"
        );
    }

    #[test]
    fn an_accordion_is_reused_only_while_the_header_stays_collapsed() {
        assert!(accordion_is_reused(true, true));
        assert!(!accordion_is_reused(false, true));
        assert!(!accordion_is_reused(true, false));
        assert!(!accordion_is_reused(false, false));
    }

    #[test]
    fn ctrl_and_shift_clicks_keep_the_accordion_open_and_a_plain_click_closes_it() {
        assert_eq!(
            accordion_click(gdk::ModifierType::empty()),
            AccordionClick::Focus
        );
        assert_eq!(
            accordion_click(gdk::ModifierType::CONTROL_MASK),
            AccordionClick::Toggle
        );
        assert_eq!(
            accordion_click(gdk::ModifierType::SHIFT_MASK),
            AccordionClick::Range
        );
        assert!(!accordion_click(gdk::ModifierType::empty()).keeps_menu_open());
        assert!(accordion_click(gdk::ModifierType::CONTROL_MASK).keeps_menu_open());
        assert!(accordion_click(gdk::ModifierType::SHIFT_MASK).keeps_menu_open());
        // Ctrl wins when both are held, exactly as Process::pill_click does.
        assert_eq!(
            accordion_click(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::SHIFT_MASK),
            AccordionClick::Toggle
        );
    }

    #[test]
    fn right_clicking_an_unselected_row_selects_only_it() {
        let mut workspace = window_with(&["/tmp/a", "/tmp/b", "/tmp/c"]);
        workspace.pill_ctrl_click(0).unwrap();
        workspace.pill_ctrl_click(1).unwrap();
        let view = view(&first(&workspace));
        assert!(view.pills[0].selected);
        assert!(view.pills[1].selected);
        assert!(!view.pills[2].selected);

        assert!(!row_right_click_selects_only(&view, 0));
        assert!(!row_right_click_selects_only(&view, 1));
        assert!(row_right_click_selects_only(&view, 2));
        assert!(!row_right_click_selects_only(&view, 9));
    }

    #[test]
    fn the_accordion_cap_is_twelve_rows_or_the_space_below_the_header() {
        let twelve_rows = scaled(PIN_ROW_H, 1.0) * 12;
        assert_eq!(twelve_rows, 528);
        // A tall window is limited by the twelve-row token cap.
        assert_eq!(accordion_scroll_cap(1.0, 1440, 96), twelve_rows);
        // A short window is limited by the space below the header.
        assert_eq!(accordion_scroll_cap(1.0, 500, 96), 404);
        // The smaller of the two wins when they meet in the middle.
        assert_eq!(accordion_scroll_cap(1.0, 600, 200), 400);
        // The token cap scales with the look.
        assert_eq!(
            accordion_scroll_cap(2.0, 2000, 192),
            scaled(PIN_ROW_H, 2.0) * 12
        );
        assert_eq!(accordion_scroll_cap(1.0, 90, 96), 0);
    }
}
