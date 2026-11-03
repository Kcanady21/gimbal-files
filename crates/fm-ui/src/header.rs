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

pub struct Header {
    window_id: WindowId,
    handle: gtk::WindowHandle,
    strip: gtk::Fixed,
    pills: RefCell<Vec<gtk::Widget>>,
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
    trash: gtk::Button,
    float: gtk::Button,
    minimize: Option<gtk::Button>,
    close: gtk::Button,
    scale: Cell<f64>,
    overflow: Cell<bool>,
    search_open: Cell<bool>,
    animate_group: Cell<Option<usize>>,
    /// Non-zero while an `adw::AlertDialog` owns the keyboard.
    dialogs: Cell<u32>,
    /// The open Settings switch and status label, so every window shows the
    /// shared listener state. The flag guards programmatic switch updates.
    agent_widgets: RefCell<Option<AgentWidgets>>,
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
            trash,
            float,
            minimize,
            close,
            scale: Cell::new(scale),
            overflow: Cell::new(false),
            search_open: Cell::new(false),
            animate_group: Cell::new(None),
            dialogs: Cell::new(0),
            agent_widgets: RefCell::new(None),
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
        crate::log!(
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
            self.close_search();
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
                self.search_entry.set_text(query);
                self.search_entry.set_position(-1);
            }
            return;
        }
        self.search_field.set_visible(true);
        self.search_open.set(true);
        self.search_icon.set_visible(false);
        self.search.set_reveal_child(true);
        if let Some(query) = query {
            self.search_entry.set_text(query);
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
        self.search_entry.set_text("");
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
            self.close_search();
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
        let name = gtk::Label::builder()
            .label(&view.label)
            .ellipsize(pango::EllipsizeMode::End)
            .max_width_chars(48)
            .build();
        pill.set_center_widget(Some(&name));

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
            header_right.open_pill_menu(&ui, index, &anchor, &current);
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
        self.fill_accordion(&popover, &menu, &label, view);
        popover.set_child(Some(&menu));
        let motion = gtk::EventControllerMotion::new();
        let weak = popover.downgrade();
        motion.connect_enter(move |_, _, _| {
            if let Some(popover) = weak.upgrade() {
                popover.popup();
            }
        });
        pill.add_controller(motion);
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let weak = popover.downgrade();
        click.connect_released(move |_, _, _, _| {
            if let Some(popover) = weak.upgrade() {
                popover.popup();
            }
        });
        pill.add_controller(click);
        let weak = popover.downgrade();
        pill.connect_destroy(move |_| {
            if let Some(popover) = weak.upgrade() {
                popover.popdown();
                popover.unparent();
            }
        });
        pill_revealer(&pill).upcast()
    }

    fn fill_accordion(
        self: &Rc<Self>,
        popover: &gtk::Popover,
        menu: &gtk::Box,
        label: &gtk::Label,
        view: &HeaderView,
    ) {
        while let Some(child) = menu.first_child() {
            menu.remove(&child);
        }
        label.set_label(&format!("{} tabs", view.tab_count));
        for pill in &view.pills {
            let row = hbox(&["gl-accordion-row"]);
            let choose = gtk::Button::new();
            choose.add_css_class("gl-menu-item");
            choose.set_hexpand(true);
            choose.set_child(Some(
                &gtk::Label::builder()
                    .label(&pill.label)
                    .halign(gtk::Align::Start)
                    .hexpand(true)
                    .build(),
            ));
            let header_choose = self.clone();
            let popover_choose = popover.clone();
            let index = pill.index;
            choose.connect_clicked(move |_| {
                take_down(&popover_choose);
                if let Some(ui) = header_choose.window_ui() {
                    ui.focus_pill(index);
                }
            });
            row.append(&choose);
            let close = gtk::Button::new();
            close.add_css_class("gl-pill-close");
            close.set_valign(gtk::Align::Center);
            close.set_child(Some(&gtk::Image::from_icon_name("window-close-symbolic")));
            let header_close = self.clone();
            let popover_close = popover.clone();
            close.connect_clicked(move |_| {
                take_down(&popover_close);
                if let Some(ui) = header_close.window_ui() {
                    ui.close_pill(index);
                }
            });
            row.append(&close);
            menu.append(&row);
        }
    }

    fn open_pill_menu(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        index: usize,
        anchor: &gtk::Widget,
        view: &HeaderView,
    ) {
        let Some(pill) = view.pills.get(index).cloned() else {
            return;
        };
        let popover = gtk::Popover::new();
        popover.set_parent(anchor);
        let menu = vbox(&["gl-menu"]);

        let ui_split = ui.clone();
        menu_item(
            &popover,
            &menu,
            "Split View",
            view.selected_pills() >= 2,
            move || {
                ui_split.dispatch(Caller::Ui, Invocation::new("group.split", Vec::new()));
            },
        );
        let ui_close = ui.clone();
        menu_item(&popover, &menu, "Close", true, move || {
            ui_close.focus_and_dispatch(index, Invocation::new("tab.close", Vec::new()));
        });
        let ui_selected = ui.clone();
        menu_item(
            &popover,
            &menu,
            "Close Selected Tabs",
            view.any_selected(),
            move || {
                ui_selected.dispatch(
                    Caller::Ui,
                    Invocation::new("tab.close-selected", Vec::new()),
                );
            },
        );
        let ui_window = ui.clone();
        menu_item(&popover, &menu, "Open in New Window", true, move || {
            ui_window
                .focus_and_dispatch(index, Invocation::new("tab.open-in-new-window", Vec::new()));
        });

        if pill.is_group {
            let header_rename = self.clone();
            let ui_rename = ui.clone();
            let anchor_rename = anchor.clone();
            let title = pill.title.clone();
            menu_item(&popover, &menu, "Rename Group…", true, move || {
                header_rename.open_rename_popover(&ui_rename, index, &anchor_rename, &title);
            });
            let ui_group_close = ui.clone();
            let count = pill.tabs;
            menu_item(&popover, &menu, "Close Group", true, move || {
                ui_group_close.confirm_and_close_group(index, count);
            });
        }

        if view.pills.len() > 1 {
            let header_add = self.clone();
            let ui_add = ui.clone();
            let anchor_add = anchor.clone();
            let current = view.clone();
            menu_item(&popover, &menu, "Add to Group", true, move || {
                header_add.open_add_group_menu(&ui_add, index, &anchor_add, &current);
            });
        }

        popover.set_child(Some(&menu));
        close_on_closed(&popover);
        popover.popup();
    }

    fn open_add_group_menu(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        source: usize,
        anchor: &gtk::Widget,
        view: &HeaderView,
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
            menu_item(&popover, &menu, &label, true, move || {
                ui_target.focus_and_dispatch(
                    source,
                    Invocation::new(
                        "group.add-tab",
                        vec![("group".to_string(), target.to_string())],
                    ),
                );
            });
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
        popover.popup();
    }

    fn open_rename_popover(
        self: &Rc<Self>,
        ui: &Rc<WindowUi>,
        index: usize,
        anchor: &gtk::Widget,
        title: &str,
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
        rename.connect_clicked(move |_| {
            let name = entry_rename.text().to_string();
            take_down(&popover_rename);
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
        popover.popup();
        entry.grab_focus();
        entry.select_region(0, -1);
        let weak_entry = entry.downgrade();
        let ui_enter = ui.clone();
        let popover_enter = popover.clone();
        entry.connect_activate(move |_| {
            let Some(entry) = weak_entry.upgrade() else {
                return;
            };
            let name = entry.text().to_string();
            take_down(&popover_enter);
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
        self.search_entry.connect_changed(|entry| {
            crate::log!("search", "typed {:?}", entry.text());
        });
        let header_focus = self.clone();
        let focus = gtk::EventControllerFocus::new();
        focus.connect_leave(move |_| {
            if header_focus.search_open.get() && header_focus.search_entry.text().is_empty() {
                header_focus.close_search();
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
        open_settings_menu(&ui_settings, &settings_anchor);
    });
    let ui_quit = ui.clone();
    menu_item(&popover, &menu, "Quit", true, move || {
        ui_quit.dispatch(Caller::Ui, Invocation::new("app.quit", Vec::new()));
    });
    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    popover.popup();
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

    popover.set_child(Some(&menu));
    close_on_closed(&popover);
    popover.popup();
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

fn menu_item(
    popover: &gtk::Popover,
    menu: &gtk::Box,
    text: &str,
    enabled: bool,
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
        action();
    });
    menu.append(&item);
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
}
