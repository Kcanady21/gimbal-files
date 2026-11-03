// SPDX-License-Identifier: GPL-3.0-or-later
//! One window's UI: P2g's frame (header strip, sidebars, content) with P2h's
//! header and content placeholder. Every widget reads the model through the
//! process and sends commands back through it.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use adw::prelude::*;
use fm_core::command::{Caller, Invocation};
use fm_core::config::{Change, Section, SectionState, Side};
use fm_core::preview::Selection;
use fm_core::workspace::{Window, WindowId};

use crate::app::Process;
use crate::commands;
use crate::content::Content;
use crate::dnd;
use crate::header::Header;
use crate::layout;
use crate::look::blur::Blur;
use crate::look::motion;
use crate::look::tokens::*;
use crate::preview::PreviewPanel;
use crate::sidebar::Sidebar;
use crate::trash::TrashBrowser;

pub struct WindowUi {
    pub id: WindowId,
    process: Weak<Process>,
    window: adw::ApplicationWindow,
    header: Rc<Header>,
    blur: RefCell<Blur>,
    content_frame: gtk::Box,
    content: Rc<Content>,
    sidebar: Rc<Sidebar>,
    preview: Rc<PreviewPanel>,

    /// Left sidebar | (content | preview): two nested horizontal Paneds.
    outer: gtk::Paned,
    inner: gtk::Paned,

    left_root: gtk::Box,
    left_host: gtk::Overlay,
    left_clip: gtk::ScrolledWindow,
    left_outline: gtk::Box,
    left_button: gtk::Button,
    left_collapsed: Cell<bool>,
    left_anim: RefCell<Option<SideAnim>>,
    left_gen: Rc<Cell<u64>>,

    right_root: gtk::Box,
    right_host: gtk::Overlay,
    right_clip: gtk::ScrolledWindow,
    right_outline: gtk::Box,
    right_button: gtk::Button,
    right_collapsed: Cell<bool>,
    right_anim: RefCell<Option<SideAnim>>,
    right_gen: Rc<Cell<u64>>,

    right_dirty: Cell<bool>,
    left_width: Cell<f64>,
    right_width: Cell<f64>,
    pending_left: Cell<Option<i32>>,
    pending_right: Cell<Option<i32>>,
    expected_left: Cell<i32>,
    expected_right: Cell<i32>,
    /// True while a rename or new-folder popover owns a text entry.
    editing: Cell<bool>,
    /// The one Trash dialog owned by this window. Opening it again presents it.
    trash: RefCell<Option<Rc<TrashBrowser>>>,
}

struct SideAnim {
    generation: u64,
    end: Cell<i32>,
    expanded: Cell<i32>,
    current: Cell<i32>,
    scale: f64,
    _animation: adw::TimedAnimation,
}

impl WindowUi {
    pub fn new(process: &Rc<Process>, application: &adw::Application, id: WindowId) -> Rc<Self> {
        let config = process.config.borrow().config().clone();
        let scale = process.scale();
        let opacity = process.opacity();
        let left_config = config.sidebars.left;
        let right_config = config.sidebars.right;

        let window = adw::ApplicationWindow::builder()
            .application(application)
            .title("Gimbal Files")
            .build();
        window.set_default_size(
            scaled(f64::from(DEFAULT_W), scale),
            scaled(f64::from(DEFAULT_H), scale),
        );
        window.add_css_class("gimbal-look");
        // Observe real key events on the window. GTK clears the drag source
        // widget's controllers when a drag begins, so the tracker sits here.
        dnd::install_modifier_tracking(window.upcast_ref::<gtk::Window>());

        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.set_content(Some(&root));

        let header = Header::new(id, scale);
        root.append(header.widget());

        let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
        frame.add_css_class("gl-frame");
        frame.set_vexpand(true);
        root.append(&frame);

        let outer = gtk::Paned::new(gtk::Orientation::Horizontal);
        frame.append(&outer);
        outer.set_resize_start_child(false);
        outer.set_resize_end_child(true);
        outer.set_shrink_start_child(false);
        outer.set_shrink_end_child(true);

        let inner = gtk::Paned::new(gtk::Orientation::Horizontal);
        outer.set_end_child(Some(&inner));
        inner.set_resize_start_child(true);
        inner.set_resize_end_child(false);
        inner.set_shrink_start_child(true);
        inner.set_shrink_end_child(false);

        // The focused tab's folder in the spike's grid; a split group keeps
        // P2h's placeholder until P2j.
        let content = Content::new(process, id, scale);

        let (left_root, left_button, _left_title) =
            panel("Pinned Folders", "sidebar-show-symbolic", true);
        let sidebar = Sidebar::new(process, id, scale);
        left_root.append(sidebar.widget());
        let (left_host, left_clip, left_outline) = sidebar_host(&left_root, &left_button, true);
        let (right_root, right_button, right_title) =
            panel("File Name", "sidebar-show-right-symbolic", false);
        let preview = PreviewPanel::new(process, id, scale, &right_title);
        right_root.append(preview.widget());
        let (right_host, right_clip, right_outline) =
            sidebar_host(&right_root, &right_button, false);

        outer.set_start_child(Some(&left_host));
        inner.set_start_child(Some(content.widget()));
        inner.set_end_child(Some(&right_host));

        let blur = Blur::attach(window.upcast_ref::<gtk::Window>(), scale, opacity);

        let ui = Rc::new(Self {
            id,
            process: Rc::downgrade(process),
            window,
            header: header.clone(),
            blur: RefCell::new(blur),
            content_frame: frame,
            content,
            sidebar,
            preview,
            outer: outer.clone(),
            inner: inner.clone(),
            left_root,
            left_host,
            left_clip,
            left_outline,
            left_button,
            left_collapsed: Cell::new(left_config.collapsed),
            left_anim: RefCell::new(None),
            left_gen: Rc::new(Cell::new(0)),
            right_root,
            right_host,
            right_clip,
            right_outline,
            right_button,
            right_collapsed: Cell::new(right_config.collapsed),
            right_anim: RefCell::new(None),
            right_gen: Rc::new(Cell::new(0)),
            right_dirty: Cell::new(true),
            left_width: Cell::new(f64::from(left_config.width)),
            right_width: Cell::new(f64::from(right_config.width)),
            pending_left: Cell::new(None),
            pending_right: Cell::new(None),
            expected_left: Cell::new(0),
            expected_right: Cell::new(0),
            editing: Cell::new(false),
            trash: RefCell::new(None),
        });
        header.wire(&ui);
        ui.wire();
        ui.apply_scale();
        ui.update_content();
        crate::log!(
            "window",
            "built model window {} ({}×{})",
            id,
            ui.window.width(),
            ui.window.height()
        );
        ui
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    /// Both `RootExt` and `WidgetExt` provide `display`, so name the one the
    /// clipboard needs once here.
    pub fn display(&self) -> gtk::gdk::Display {
        gtk::prelude::WidgetExt::display(&self.window)
    }

    pub fn header(&self) -> &Rc<Header> {
        &self.header
    }

    pub fn preview(&self) -> &Rc<PreviewPanel> {
        &self.preview
    }

    pub fn home(&self) -> PathBuf {
        self.process
            .upgrade()
            .map(|process| process.home())
            .unwrap_or_else(|| PathBuf::from("/"))
    }

    pub fn workspace_window(&self) -> Option<Window> {
        self.process
            .upgrade()
            .and_then(|process| process.window_snapshot(self.id))
    }

    pub fn process_upgrade(&self) -> Option<Rc<Process>> {
        self.process.upgrade()
    }

    pub fn dispatch(&self, caller: Caller, invocation: Invocation) {
        if let Some(process) = self.process.upgrade() {
            process.dispatch_for_window(self.id, caller, invocation);
        }
    }

    pub fn focus_and_dispatch(&self, index: usize, invocation: Invocation) {
        if let Some(process) = self.process.upgrade() {
            process.focus_pill_and_dispatch(self.id, index, invocation);
        }
    }

    pub fn pill_click(&self, index: usize, modifiers: gtk::gdk::ModifierType) {
        if let Some(process) = self.process.upgrade() {
            process.pill_click(self.id, index, modifiers);
        }
    }

    pub fn focus_pill(&self, index: usize) {
        if let Some(process) = self.process.upgrade() {
            process.focus_pill(self.id, index);
        }
    }

    pub fn clear_pill_selection(&self) {
        if let Some(process) = self.process.upgrade() {
            process.clear_pill_selection(self.id);
        }
    }

    pub fn close_pill(self: &Rc<Self>, index: usize) {
        let Some(window) = self.workspace_window() else {
            return;
        };
        let Some(group_id) = window.pills.get(index).copied() else {
            return;
        };
        let Some(group) = window.groups.get(&group_id) else {
            return;
        };
        if group.is_split() {
            self.header.confirm_group(self, index, group.tabs.len());
        } else {
            self.focus_and_dispatch(index, Invocation::new("tab.close", Vec::new()));
        }
    }

    pub fn confirm_and_close_group(self: &Rc<Self>, index: usize, tabs: usize) {
        self.header.confirm_group(self, index, tabs);
    }

    pub fn focus_path(&self) {
        self.header.focus_path();
    }

    pub fn open_search(&self, query: Option<&str>) {
        self.header.open_search(query);
    }

    pub fn refresh_header(self: &Rc<Self>, fade: bool) {
        self.header.rebuild(self, fade);
    }

    pub fn update_content(&self) {
        if self.content.trash_active() {
            // Do not present the hidden folder as the current view. The header
            // says Trash and the sidebar has no current-folder highlight.
            self.sidebar.set_current_folder(None);
            self.preview.apply_model_selection(Selection::Trash);
            return;
        }
        let Some(window) = self.workspace_window() else {
            self.content.show_placeholder("No window");
            self.sidebar.set_current_folder(None);
            self.preview.apply_model_selection(Selection::None);
            return;
        };
        let folder = window.tab(window.focused_tab).map(|tab| tab.folder.clone());
        self.sidebar.set_current_folder(folder.as_deref());
        let selection = window
            .tab(window.focused_tab)
            .map(|tab| Selection::from_names(&tab.folder, &tab.selection.names))
            .unwrap_or(Selection::None);
        self.preview.apply_model_selection(selection);
        self.content.show(&window);
    }

    pub fn arm_anchor_restore(&self) {
        self.content.arm_anchor_restore();
    }

    pub fn clear_anchor_restore(&self) {
        self.content.clear_anchor_restore();
    }

    pub fn listed_names(&self) -> Vec<std::ffi::OsString> {
        self.content.listed_names()
    }

    pub fn focused_folder(&self) -> Option<PathBuf> {
        self.workspace_window()
            .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
    }

    /// A dialog owns the keyboard; hotkeys are ignored while it is up.
    pub fn begin_dialog(&self) {
        self.header.begin_dialog();
    }

    pub fn end_dialog(&self) {
        self.header.end_dialog();
    }

    pub fn set_editing(&self, editing: bool) {
        self.editing.set(editing);
    }

    /// Whether a text entry in this window has focus: the path bar, the search
    /// field, or a rename/new-folder popover.
    pub fn text_entry_focused(&self) -> bool {
        self.editing.get() || self.header.text_focused()
    }

    /// A text preview owns the editing keys while its text view has focus.
    pub fn preview_text_focused(&self) -> bool {
        self.preview.text_focused()
    }

    /// Close after a preview guard has finished. The guard flag prevents the
    /// same close from asking twice.
    pub fn close_now(&self) {
        self.preview.allow_close();
        self.dispatch(Caller::Ui, Invocation::new("window.close", Vec::new()));
    }

    pub fn fileop_started(&self) {
        if !self.content.trash_active() {
            self.content.begin_fileop();
        }
    }

    pub fn fileop_progress(&self, view: Option<crate::fileops::ProgressView>) {
        if !self.content.trash_active() {
            self.content.set_progress(view);
        }
    }

    pub fn show_summary(&self, text: &str, failures: Option<String>) {
        self.content.show_summary(text, failures);
    }

    pub fn show_nothing_to_undo(&self) {
        self.content.show_nothing_to_undo();
    }

    pub fn reload_folder(&self) {
        self.content.reload();
    }

    /// Open the one Trash page, or present the page this window already owns.
    /// The page lives in the central stack; opening it never creates a tab,
    /// model window or folder-history entry.
    pub fn open_trash(self: &Rc<Self>) {
        let action = crate::trash::open_action(self.trash.borrow().is_some());
        match action {
            crate::trash::OpenAction::ShowExisting => {
                self.content.show_trash();
                self.refresh_header(false);
                crate::log!(
                    "trash",
                    "presented the existing Trash page for window {}",
                    self.id
                );
            }
            crate::trash::OpenAction::CreatePage => {
                let browser = TrashBrowser::new(self);
                self.content.add_trash_page(browser.widget());
                *self.trash.borrow_mut() = Some(browser);
                self.content.show_trash();
                self.refresh_header(false);
                crate::log!("trash", "opened the Trash page for window {}", self.id);
            }
        }
        self.update_content();
    }

    /// Return to the unchanged folder or split page. Any Empty Trash
    /// confirmation this page owned is cancelled for its exact job.
    pub fn leave_trash(self: &Rc<Self>) -> bool {
        if !crate::trash::leaves_trash_page(
            self.content.trash_active(),
            crate::trash::RefreshCause::Navigation,
        ) {
            return false;
        }
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.leave_page();
        }
        self.content.hide_trash();
        self.refresh_header(false);
        self.update_content();
        crate::log!("trash", "returned to files for window {}", self.id);
        true
    }

    pub fn trash_active(&self) -> bool {
        self.content.trash_active()
    }

    pub fn trash_visible(&self) -> bool {
        self.content.trash_active() && self.trash.borrow().is_some()
    }

    pub fn trash_status(&self, text: &str) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.set_status(text);
        }
    }

    pub fn trash_select_all(&self) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.select_all();
        }
    }

    pub fn trash_select_none(&self) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.select_none();
        }
    }

    pub fn trash_refresh(&self) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.refresh_now();
        }
    }

    pub fn trash_closed(&self) {
        let browser = self.trash.borrow_mut().take();
        if let Some(browser) = browser {
            browser.leave_page();
        }
        crate::log!("trash", "closed the Trash page for window {}", self.id);
    }

    pub fn trash_job_started(&self) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.job_started();
        }
    }

    pub fn trash_progress(&self, view: Option<crate::fileops::ProgressView>) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.set_progress(view);
        }
    }

    pub fn trash_dry_run(&self, request: &fm_core::fileops::Request, lines: &[String]) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.show_dry_run(request, lines);
        }
    }

    pub fn trash_job_finished(
        &self,
        request: &fm_core::fileops::Request,
        summary: &str,
        details: Option<String>,
    ) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.job_finished(request, summary, details);
        }
    }

    pub fn trash_refused(&self, request: &fm_core::fileops::Request, message: &str) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.refused(request, message);
        }
    }

    pub fn confirm_empty_trash(self: &Rc<Self>, id: u64, plan: &fm_core::fileops::Plan) {
        let browser = self.trash.borrow().clone();
        if let Some(browser) = browser {
            browser.confirm_empty(self, id, plan);
        }
    }

    pub fn reload_sidebar(&self) {
        self.sidebar.reload_from_config();
    }

    pub fn set_section(&self, section: Section, state: SectionState) {
        self.sidebar.set_section(section, state);
    }

    pub fn sort_changed(&self, folder: Option<&std::path::Path>) {
        self.content.refresh_sort(folder);
    }

    pub fn cancel_split_drag(&self) -> bool {
        self.content.cancel_drag()
    }

    pub fn push_recent(&self, folder: &std::path::Path) {
        self.sidebar.push_recent(folder);
    }

    fn scale(&self) -> f64 {
        self.process
            .upgrade()
            .map(|process| process.scale())
            .unwrap_or(1.0)
    }

    pub fn set_opacity(&self, opacity: f64) {
        self.blur.borrow_mut().set_opacity(opacity);
    }

    pub fn apply_scale(self: &Rc<Self>) {
        let scale = self.scale();
        self.blur.borrow_mut().set_scale(scale);
        self.header.set_scale(self, scale);
        self.content.apply_scale(scale);
        self.sidebar.set_scale(scale);
        self.preview.apply_scale(scale);
        let px = |token: f64| scaled(token, scale);
        if !self.animating(true) {
            self.left_root.set_width_request(px(SIDEBAR_MIN));
        }
        if !self.animating(false) {
            self.right_root.set_width_request(px(PREVIEW_MIN));
        }
        let strip = layout::strip_width(scale);
        self.left_host.set_width_request(strip);
        self.right_host.set_width_request(strip);
        let inset = layout::inset(scale);
        self.left_host.set_margin_start(inset);
        self.left_host.set_margin_bottom(inset);
        self.right_host.set_margin_end(inset);
        self.right_host.set_margin_bottom(inset);
        let top = layout::panel_button_inset(scale);
        self.left_button.set_margin_top(top);
        self.right_button.set_margin_top(top);
        for left in [true, false] {
            if self.animating(left) {
                continue;
            }
            let collapsed = self.collapsed(left);
            let p = if collapsed { 1.0 } else { 0.0 };
            let h = layout::button_inset(scale, left, p);
            if left {
                self.left_button.set_margin_start(h);
            } else {
                self.right_button.set_margin_end(h);
            }
            self.clip(left).set_visible(!collapsed);
            self.outline(left).set_visible(false);
            self.outline(left).set_opacity(1.0);
        }
        self.apply_positions(true);
        let (w, _) = self.body_widths();
        crate::log!(
            "layout",
            "scale {scale:.2}: left {}, content {}, preview {} (inset {inset})",
            w.left,
            w.content,
            w.preview
        );
    }

    pub fn set_saved_sidebar_width(self: &Rc<Self>, side: Side, width: u32) {
        let design = f64::from(width);
        if side == Side::Left {
            self.left_width.set(design);
        } else {
            self.right_width.set(design);
        }
        self.apply_positions(true);
    }

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.outer
            .connect_notify_local(Some("max-position"), move |_, _| {
                if let Some(ui) = weak.upgrade()
                    && ui.outer.max_position() > 0
                {
                    ui.apply_positions(false);
                }
            });
        let weak = Rc::downgrade(self);
        self.inner
            .connect_notify_local(Some("max-position"), move |_, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.on_inner_max_position();
                }
            });
        let weak = Rc::downgrade(self);
        self.outer
            .connect_notify_local(Some("position"), move |paned, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.on_outer_position(paned);
                }
            });
        let weak = Rc::downgrade(self);
        self.inner
            .connect_notify_local(Some("position"), move |paned, _| {
                if let Some(ui) = weak.upgrade() {
                    ui.on_inner_position(paned);
                }
            });

        for (button, left) in [(&self.left_button, true), (&self.right_button, false)] {
            let ui = self.clone();
            let side = if left { "left" } else { "right" };
            button.connect_clicked(move |_| {
                ui.dispatch(
                    Caller::Ui,
                    Invocation::new(
                        "view.toggle-sidebar",
                        vec![("side".to_string(), side.to_string())],
                    ),
                );
            });
        }

        // GtkPaned has no drag-end signal. A capture-phase legacy controller
        // sees the button release without claiming the handle's own gesture.
        let release = gtk::EventControllerLegacy::new();
        release.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        release.connect_event(move |_, event| {
            if event.event_type() == gtk::gdk::EventType::ButtonRelease
                && let Some(ui) = weak.upgrade()
            {
                ui.flush_widths();
            }
            gtk::glib::Propagation::Proceed
        });
        self.window.add_controller(release);

        // A drag that ends by losing the window keeps what it reached; an
        // activation also focuses this window's model window.
        let weak = Rc::downgrade(self);
        self.window
            .connect_notify_local(Some("is-active"), move |window, _| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if window.is_active() {
                    if let Some(process) = ui.process.upgrade() {
                        process.activate_window(ui.id);
                    }
                } else {
                    ui.flush_widths();
                    ui.cancel_split_drag();
                }
            });

        // Closing the GTK window runs the one command path. The model's
        // WindowClosed event is what destroys the widget.
        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.dispatch(Caller::Ui, Invocation::new("window.close", Vec::new()));
            }
            gtk::glib::Propagation::Stop
        });

        let weak = Rc::downgrade(self);
        self.window.connect_destroy(move |_| {
            if let Some(ui) = weak.upgrade() {
                ui.trash_closed();
                if let Some(process) = ui.process.upgrade() {
                    process.forget_window(ui.id);
                }
            }
        });

        // Escape closes an empty search (and then clears a pill selection).
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Bubble);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            if key != gtk::gdk::Key::Escape {
                return gtk::glib::Propagation::Proceed;
            }
            if let Some(ui) = weak.upgrade() {
                if ui.cancel_split_drag() {
                    return gtk::glib::Propagation::Stop;
                }
                ui.header.escape(&ui)
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        self.window.add_controller(keys);

        // A click on the body closes an open, empty search: the body is not
        // focusable, so the entry's focus-leave never fires.
        let click = gtk::GestureClick::new();
        click.set_button(1);
        let header = self.header.clone();
        click.connect_released(move |_, _, _, _| header.dismiss_empty_search());
        self.content_frame.add_controller(click);

        commands::install_hotkeys(self, self.window.upcast_ref::<gtk::Window>());
    }

    fn on_inner_max_position(&self) {
        if self.animating(false) {
            return;
        }
        if self.right_dirty.get() && self.inner.max_position() > 0 {
            self.right_dirty.set(false);
            let (w, _) = self.body_widths();
            let pos = self.right_position(w.preview);
            self.expected_right.set(pos);
            if self.inner.position() != pos {
                self.inner.set_position(pos);
            }
        }
    }

    fn on_outer_position(&self, paned: &gtk::Paned) {
        let position = paned.position();
        let visible = layout::left_from_position(position, self.scale());
        if self.animating(true) {
            self.expected_left.set(position);
            return;
        }
        if self.left_collapsed.get() {
            let target = layout::left_position(self.body_widths().0.left, self.scale());
            if position != target {
                paned.set_position(target);
            }
        } else {
            let min = layout::left_position(scaled(SIDEBAR_MIN, self.scale()), self.scale());
            if position < min {
                self.expected_left.set(min);
                if position != min {
                    paned.set_position(min);
                }
                return;
            }
            let implied = layout::left_position(self.body_widths().0.left, self.scale());
            if position != self.expected_left.get() && position != implied {
                self.left_width.set(visible as f64 / self.scale());
                self.pending_left.set(Some(visible));
                self.expected_left.set(position);
            }
        }
    }

    fn on_inner_position(&self, paned: &gtk::Paned) {
        let position = paned.position();
        if self.animating(false) {
            self.expected_right.set(position);
            return;
        }
        if self.right_collapsed.get() {
            let target = self.right_position(self.body_widths().0.preview);
            if position != target {
                paned.set_position(target);
            }
        } else {
            if self.inner.width() > 0 {
                let max = self.right_position(scaled(PREVIEW_MIN, self.scale()));
                if position > max {
                    self.expected_right.set(max);
                    if position != max {
                        paned.set_position(max);
                    }
                    return;
                }
            }
            let implied = self.right_position(self.body_widths().0.preview);
            if !self.right_dirty.get()
                && position != self.expected_right.get()
                && position != implied
            {
                let width =
                    layout::preview_from_position(self.inner.width(), position, self.scale());
                self.right_width.set(width as f64 / self.scale());
                self.pending_right.set(Some(width));
                self.expected_right.set(position);
            }
        }
    }

    fn flush_widths(&self) {
        let Some(process) = self.process.upgrade() else {
            return;
        };
        let scale = self.scale();
        for left in [true, false] {
            let pending = if left {
                self.pending_left.take()
            } else {
                self.pending_right.take()
            };
            let Some(visible) = pending else {
                continue;
            };
            let min = if left { SIDEBAR_MIN } else { PREVIEW_MIN };
            let design = ((visible as f64) / scale).round().max(min) as u32;
            let side = if left { Side::Left } else { Side::Right };
            let applied = process.config.borrow_mut().apply(Change::SetSidebarWidth {
                side,
                width: design,
            });
            match applied {
                Ok(()) => crate::log!(
                    "sidebar",
                    "saved {} width {design} design px ({visible} visible)",
                    side.as_str()
                ),
                Err(error) => crate::log!("config", "{error}"),
            }
        }
    }

    fn anim_slot(&self, left: bool) -> &RefCell<Option<SideAnim>> {
        if left {
            &self.left_anim
        } else {
            &self.right_anim
        }
    }

    fn gen_cell(&self, left: bool) -> Rc<Cell<u64>> {
        if left {
            self.left_gen.clone()
        } else {
            self.right_gen.clone()
        }
    }

    fn animating(&self, left: bool) -> bool {
        self.anim_slot(left).borrow().is_some()
    }

    fn collapsed(&self, left: bool) -> bool {
        if left {
            self.left_collapsed.get()
        } else {
            self.right_collapsed.get()
        }
    }

    fn set_collapsed(&self, left: bool, value: bool) {
        if left {
            self.left_collapsed.set(value);
        } else {
            self.right_collapsed.set(value);
            self.right_dirty.set(true);
        }
    }

    fn panel_root(&self, left: bool) -> gtk::Box {
        if left {
            self.left_root.clone()
        } else {
            self.right_root.clone()
        }
    }

    fn clip(&self, left: bool) -> gtk::ScrolledWindow {
        if left {
            self.left_clip.clone()
        } else {
            self.right_clip.clone()
        }
    }

    fn outline(&self, left: bool) -> gtk::Box {
        if left {
            self.left_outline.clone()
        } else {
            self.right_outline.clone()
        }
    }

    fn current_width(&self, left: bool) -> i32 {
        if left {
            layout::left_from_position(self.outer.position(), self.scale())
        } else {
            layout::preview_from_position(self.inner.width(), self.inner.position(), self.scale())
        }
    }

    fn expanded_width(&self, left: bool) -> i32 {
        let (w, _) = layout::body_widths(
            self.window.width(),
            self.scale(),
            self.left_width.get(),
            self.right_width.get(),
            if left {
                false
            } else {
                self.left_collapsed.get()
            },
            if left {
                self.right_collapsed.get()
            } else {
                false
            },
        );
        if left { w.left } else { w.preview }
    }

    fn anchor_clip(&self, left: bool) {
        let adjustment = self.clip(left).hadjustment();
        let anchor = if left {
            0.0
        } else {
            (adjustment.upper() - adjustment.page_size()).max(0.0)
        };
        if (adjustment.value() - anchor).abs() > 0.5 {
            adjustment.set_value(anchor);
        }
    }

    fn frame_sidebar(&self, left: bool, width: i32) {
        let (generation, expanded) = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            let Some(anim) = borrowed.as_ref() else {
                return;
            };
            (anim.generation, anim.expanded.get())
        };
        if self.gen_cell(left).get() != generation {
            return;
        }
        let width = width.max(1);
        if let Some(anim) = self.anim_slot(left).borrow().as_ref() {
            anim.current.set(width);
        }
        let scale = self.scale();
        let progress = layout::collapse_progress(width, expanded, scale);
        if left {
            let position = layout::left_position(width, scale);
            self.expected_left.set(position);
            if self.outer.position() != position {
                self.outer.set_position(position);
            }
        } else {
            let position = layout::preview_position(self.inner.width(), width, scale).max(0);
            self.expected_right.set(position);
            if self.inner.position() != position {
                self.inner.set_position(position);
            }
        }
        let inset = layout::button_inset(scale, left, progress);
        if left {
            self.left_button.set_margin_start(inset);
        } else {
            self.right_button.set_margin_end(inset);
        }
        self.outline(left)
            .set_opacity(layout::outline_opacity(progress));
        self.anchor_clip(left);
    }

    fn begin_motion(self: &Rc<Self>, left: bool, start: i32, end: i32, expanded: i32) {
        let generation = {
            let cell = self.gen_cell(left);
            let next = cell.get() + 1;
            cell.set(next);
            next
        };
        let gen_cell = self.gen_cell(left);
        let weak = Rc::downgrade(self);
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if gen_cell.get() != generation {
                return;
            }
            let width = layout::motion_width(start, end, v);
            if let Some(ui) = weak.upgrade() {
                ui.frame_sidebar(left, width);
            }
        });
        let duration = if start == end { 1 } else { motion::ms(200) };
        let animation = adw::TimedAnimation::new(&self.window, 0.0, 1.0, duration, target);
        let done_cell = self.gen_cell(left);
        let done_weak = Rc::downgrade(self);
        animation.connect_done(move |_| {
            if done_cell.get() != generation {
                return;
            }
            if let Some(ui) = done_weak.upgrade() {
                ui.finish_sidebar(left);
            }
        });
        let scale = self.scale();
        *self.anim_slot(left).borrow_mut() = Some(SideAnim {
            generation,
            end: Cell::new(end),
            expanded: Cell::new(expanded),
            current: Cell::new(start),
            scale,
            _animation: animation.clone(),
        });
        animation.play();
    }

    pub fn toggle_sidebar_to(self: &Rc<Self>, side: Side, target_collapsed: bool) {
        let left = side == Side::Left;
        let running = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            borrowed
                .as_ref()
                .map(|anim| (anim.current.get(), anim.expanded.get()))
        };
        self.set_collapsed(left, target_collapsed);
        let scale = self.scale();
        let strip = layout::strip_width(scale);
        let start = match running {
            Some((current, _)) => current,
            None => self.current_width(left).max(1),
        };
        let end = if target_collapsed {
            strip
        } else {
            self.expanded_width(left).max(strip)
        };
        let expanded = match running {
            Some((_, expanded)) => expanded,
            None if target_collapsed => start.max(end),
            None => end,
        };
        if running.is_none() {
            self.panel_root(left).set_width_request(expanded);
        }
        self.panel_root(left).add_css_class("gl-anim");
        self.clip(left).set_visible(true);
        self.outline(left).set_visible(true);
        self.outline(left)
            .set_opacity(layout::outline_opacity(layout::collapse_progress(
                start, expanded, scale,
            )));
        let action = if target_collapsed {
            "collapsing"
        } else {
            "expanding"
        };
        crate::log!(
            "sidebar",
            "{action} {} sidebar from {start} to {end} (panel {expanded})",
            if left { "left" } else { "preview" }
        );
        self.begin_motion(left, start, end, expanded);
    }

    fn finish_sidebar(self: &Rc<Self>, left: bool) {
        let anim = self.anim_slot(left).borrow_mut().take();
        let Some(anim) = anim else {
            return;
        };
        if self.gen_cell(left).get() != anim.generation {
            return;
        }
        let collapsed = self.collapsed(left);
        let scale = self.scale();
        let root = self.panel_root(left);
        root.remove_css_class("gl-anim");
        root.set_width_request(if left {
            scaled(SIDEBAR_MIN, scale)
        } else {
            scaled(PREVIEW_MIN, scale)
        });
        let outline = self.outline(left);
        outline.set_visible(false);
        outline.set_opacity(1.0);
        self.clip(left).set_visible(!collapsed);
        let progress = if collapsed { 1.0 } else { 0.0 };
        let inset = layout::button_inset(scale, left, progress);
        if left {
            self.left_button.set_margin_start(inset);
        } else {
            self.right_button.set_margin_end(inset);
        }
        crate::log!(
            "sidebar",
            "{} {} sidebar (panel {})",
            if collapsed { "collapsed" } else { "expanded" },
            if left { "left" } else { "preview" },
            root.width()
        );
        self.apply_positions(true);
    }

    fn retarget_side(self: &Rc<Self>, left: bool, end: i32) {
        let (current, old_end, expanded, old_scale) = {
            let slot = self.anim_slot(left);
            let borrowed = slot.borrow();
            let Some(anim) = borrowed.as_ref() else {
                return;
            };
            (
                anim.current.get(),
                anim.end.get(),
                anim.expanded.get(),
                anim.scale,
            )
        };
        let scale = self.scale();
        if end == old_end && (scale - old_scale).abs() < 1e-9 {
            return;
        }
        let new_expanded = if self.collapsed(left) {
            (expanded as f64 * scale / old_scale).round() as i32
        } else {
            end.max(current)
        };
        self.panel_root(left).set_width_request(new_expanded);
        self.begin_motion(left, current, end, new_expanded);
    }

    fn body_widths(&self) -> (layout::BodyWidths, i32) {
        layout::body_widths(
            self.window.width(),
            self.scale(),
            self.left_width.get(),
            self.right_width.get(),
            self.left_collapsed.get(),
            self.right_collapsed.get(),
        )
    }

    fn right_position(&self, preview: i32) -> i32 {
        layout::preview_position(self.inner.width(), preview, self.scale())
    }

    fn apply_positions(self: &Rc<Self>, apply_preview: bool) {
        let (w, min_w) = self.body_widths();
        self.window.set_size_request(min_w, -1);
        if self.animating(true) {
            self.retarget_side(true, w.left);
        } else {
            let left = layout::left_position(w.left, self.scale());
            self.expected_left.set(left);
            if self.outer.position() != left {
                self.outer.set_position(left);
            }
        }
        if self.animating(false) {
            self.retarget_side(false, w.preview);
        } else if apply_preview && self.inner.width() > 0 {
            let pos = self.right_position(w.preview);
            self.expected_right.set(pos);
            self.right_dirty.set(false);
            if self.inner.position() != pos {
                self.inner.set_position(pos);
            }
        } else {
            self.right_dirty.set(true);
        }
    }
}

fn rule() -> gtk::Box {
    let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    rule.add_css_class("gl-rule");
    rule
}

fn title_row(title: &str, icon: &str, button_first: bool) -> (gtk::Box, gtk::Button, gtk::Label) {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let label = gtk::Label::new(Some(title));
    label.add_css_class("gl-title");
    label.set_hexpand(true);
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    slot.add_css_class("gl-icon-slot");
    let button = icon_button(icon);
    if button_first {
        row.append(&slot);
    }
    row.append(&label);
    if !button_first {
        row.append(&slot);
    }
    (row, button, label)
}

fn icon_button(icon: &str) -> gtk::Button {
    let button = gtk::Button::new();
    button.set_child(Some(&gtk::Image::from_icon_name(icon)));
    button.add_css_class("gl-icon-button");
    button
}

fn panel(title: &str, icon: &str, button_first: bool) -> (gtk::Box, gtk::Button, gtk::Label) {
    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("gl-panel");
    root.set_vexpand(true);
    let (row, button, label) = title_row(title, icon, button_first);
    root.append(&row);
    root.append(&rule());
    (root, button, label)
}

fn sidebar_host(
    panel: &gtk::Box,
    button: &gtk::Button,
    left: bool,
) -> (gtk::Overlay, gtk::ScrolledWindow, gtk::Box) {
    let host = gtk::Overlay::new();
    let clip = gtk::ScrolledWindow::new();
    clip.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
    clip.set_propagate_natural_width(false);
    clip.set_child(Some(panel));
    let outline = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    outline.add_css_class("gl-panel-outline");
    outline.set_hexpand(true);
    outline.set_vexpand(true);
    outline.set_halign(gtk::Align::Fill);
    outline.set_valign(gtk::Align::Fill);
    outline.set_can_target(false);
    outline.set_visible(false);
    button.set_valign(gtk::Align::Start);
    button.set_halign(if left {
        gtk::Align::Start
    } else {
        gtk::Align::End
    });
    host.set_child(Some(&clip));
    host.add_overlay(&outline);
    host.add_overlay(button);
    let adjustment = clip.hadjustment();
    let pin = move |adjustment: &gtk::Adjustment| {
        let anchor = if left {
            0.0
        } else {
            (adjustment.upper() - adjustment.page_size()).max(0.0)
        };
        if (adjustment.value() - anchor).abs() > 0.5 {
            adjustment.set_value(anchor);
        }
    };
    adjustment.connect_changed(pin);
    adjustment.connect_value_changed(pin);
    (host, clip, outline)
}
