// SPDX-License-Identifier: GPL-3.0-or-later
//! Process state: the one config, the workspace model, the session, the one
//! stylesheet and the open window UIs.
//!
//! Every model operation returns events; `apply_events` turns each event into
//! one UI update through [`crate::events::touch`]. A GTK window's activation
//! focuses its model window, and every header click or hotkey in a window
//! focuses that window before dispatching.

use adw::prelude::*;
use fm_core::command::{Caller, CheckedCommand, Effect, Invocation, Outcome, Registry, Value};
use fm_core::config::{self, Change, ConfigFile};
use fm_core::session::{self, SessionProblem};
use fm_core::workspace::{Event, WindowId, Workspace};
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::commands;
use crate::folder::ClickRequest;
use crate::frame::WindowUi;
use crate::look::Look;
use crate::open::{OpenDecision, decision};
use crate::save::{Debounce, SAVE_DELAY_MS, Saver};

pub struct Process {
    application: adw::Application,
    pub config: Rc<RefCell<ConfigFile>>,
    pub look: Rc<RefCell<Look>>,
    workspace: RefCell<Workspace>,
    windows: RefCell<BTreeMap<WindowId, Rc<WindowUi>>>,
    session_path: Option<PathBuf>,
    saver: Saver,
    save_source: RefCell<Option<glib::SourceId>>,
    debounce: Cell<Debounce>,
    preserve_bad: Cell<bool>,
}

impl Process {
    /// Build the process state and restore the session. Called on the first
    /// activation or open, once per process.
    pub fn start(application: &adw::Application) -> Rc<Self> {
        let path = config::default_config_path().unwrap_or_else(|| {
            crate::log!(
                "config",
                "no config path (HOME is unset); using a relative file and defaults"
            );
            PathBuf::from("gimbal-files-config.toml")
        });
        if path.is_file() {
            crate::log!("config", "reading {}", path.display());
        } else {
            crate::log!(
                "config",
                "{} does not exist; defaults apply",
                path.display()
            );
        }
        let (config_file, problems) = ConfigFile::load(&path);
        for problem in problems {
            crate::log!("config", "{problem}");
        }
        let config = config_file.config().clone();
        let scale = config.look.base * config.look.scale;
        let opacity = config.look.opacity;
        let font = Some(config.look.font.clone()).filter(|font| !font.is_empty());
        crate::log!(
            "config",
            "scale {:.2} (base {:.2}), opacity {:.2}, font {:?}, left collapsed {} width {}, right collapsed {} width {}",
            config.look.scale,
            config.look.base,
            config.look.opacity,
            config.look.font,
            config.sidebars.left.collapsed,
            config.sidebars.left.width,
            config.sidebars.right.collapsed,
            config.sidebars.right.width,
        );

        let look = Look::new(scale, opacity, font);
        let workspace = Workspace::with_home(config.home.clone());
        let process = Rc::new(Self {
            application: application.clone(),
            config: Rc::new(RefCell::new(config_file)),
            look: Rc::new(RefCell::new(look)),
            workspace: RefCell::new(workspace),
            windows: RefCell::new(BTreeMap::new()),
            session_path: session::default_session_path(),
            saver: Saver::new(),
            save_source: RefCell::new(None),
            debounce: Cell::new(Debounce::new(SAVE_DELAY_MS)),
            preserve_bad: Cell::new(false),
        });
        process.restore_session();
        process
    }

    fn restore_session(self: &Rc<Self>) {
        let home = self.home();
        let (workspace, problems) = match &self.session_path {
            Some(path) => session::load(path, &home, Path::is_dir),
            None => {
                crate::log!("session", "no session path (HOME and XDG_STATE_HOME unset)");
                (Workspace::with_home(home), Vec::new())
            }
        };
        for problem in &problems {
            crate::log!("session", "{problem}");
        }
        if problems.iter().any(|problem| {
            matches!(
                problem,
                SessionProblem::Parse { .. } | SessionProblem::UnsupportedVersion { .. }
            )
        }) {
            self.preserve_bad.set(true);
        }
        let empty = workspace.windows.is_empty();
        *self.workspace.borrow_mut() = workspace;
        crate::log!(
            "session",
            "restored {} window(s)",
            self.workspace.borrow().windows.len()
        );

        if empty {
            self.open_request(&[]);
        } else {
            let ids: Vec<WindowId> = self
                .workspace
                .borrow()
                .windows
                .iter()
                .map(|window| window.id)
                .collect();
            for id in ids {
                self.build_window(id);
            }
            if let Some(focused) = self.workspace.borrow().focused_window
                && let Some(ui) = self.window(focused)
            {
                ui.present();
            }
        }
        // Persist the restored (and repaired) state once, so a bad file's
        // copy is made even when no one interacts.
        self.schedule_save();
    }

    pub fn home(&self) -> PathBuf {
        self.config.borrow().config().home.clone()
    }

    pub fn scale(&self) -> f64 {
        let config = self.config.borrow();
        config.config().look.base * config.config().look.scale
    }

    pub fn opacity(&self) -> f64 {
        self.config.borrow().config().look.opacity
    }

    pub fn hotkeys(&self) -> BTreeMap<String, fm_core::config::Hotkey> {
        self.config.borrow().config().hotkeys.clone()
    }

    pub fn window(&self, id: WindowId) -> Option<Rc<WindowUi>> {
        self.windows.borrow().get(&id).cloned()
    }

    pub fn window_snapshot(&self, id: WindowId) -> Option<fm_core::workspace::Window> {
        self.workspace
            .borrow()
            .windows
            .iter()
            .find(|window| window.id == id)
            .cloned()
    }

    pub fn forget_window(&self, id: WindowId) {
        self.windows.borrow_mut().remove(&id);
    }

    fn build_window(self: &Rc<Self>, id: WindowId) {
        if self.windows.borrow().contains_key(&id) {
            return;
        }
        let ui = WindowUi::new(self, &self.application, id);
        self.windows.borrow_mut().insert(id, ui.clone());
        ui.present();
        crate::log!("window", "opened model window {id}");
    }

    fn destroy_window(&self, id: WindowId) {
        if let Some(ui) = self.windows.borrow_mut().remove(&id) {
            ui.window().destroy();
            crate::log!("window", "closed model window {id}");
        }
    }

    /// A GTK window became active: its model window is now the focused one.
    pub fn activate_window(self: &Rc<Self>, id: WindowId) {
        let result = self.workspace.borrow_mut().focus_window(id);
        self.finish(result);
    }

    /// Focus the window before a header click or hotkey runs in it.
    fn focus_model_window(self: &Rc<Self>, id: WindowId) {
        let result = self.workspace.borrow_mut().focus_window(id);
        if let Ok(events) = result
            && !events.is_empty()
        {
            self.apply_events(&events);
        }
    }

    /// The one command path. Validation happens in `Registry`; the handler
    /// knows which window and caller ran it.
    pub fn dispatch_for_window(
        self: &Rc<Self>,
        window: WindowId,
        caller: Caller,
        invocation: Invocation,
    ) {
        self.focus_model_window(window);
        let mut handler = commands::UiHandler {
            caller,
            window,
            process: Rc::downgrade(self),
        };
        let outcome = Registry::new().dispatch(caller, &invocation, &mut handler);
        match outcome {
            Outcome::Error(error) => crate::log!("command", "{error}"),
            Outcome::DryRun(lines) => crate::log!(
                "command",
                "{} dry run: {} lines",
                invocation.name(),
                lines.len()
            ),
            Outcome::Done => {}
        }
    }

    /// Focus a pill and then run a command against it. The pill-click events
    /// are applied together with the command's, so the header is rebuilt once
    /// and the button that started this is not removed mid-signal.
    pub fn focus_pill_and_dispatch(
        self: &Rc<Self>,
        window: WindowId,
        index: usize,
        invocation: Invocation,
    ) {
        self.focus_model_window(window);
        let click = self.workspace.borrow_mut().pill_click(index);
        let mut handler = commands::UiHandler {
            caller: Caller::Ui,
            window,
            process: Rc::downgrade(self),
        };
        let outcome = Registry::new().dispatch(Caller::Ui, &invocation, &mut handler);
        if let Outcome::Error(error) = outcome {
            crate::log!("command", "{error}");
            if let Ok(events) = click {
                self.apply_events(&events);
            }
        }
    }

    pub fn pill_click(
        self: &Rc<Self>,
        window: WindowId,
        index: usize,
        modifiers: gtk::gdk::ModifierType,
    ) {
        self.focus_model_window(window);
        let result = {
            let mut workspace = self.workspace.borrow_mut();
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                workspace.pill_ctrl_click(index)
            } else if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                workspace.pill_shift_click(index)
            } else {
                workspace.pill_click(index)
            }
        };
        self.finish(result);
    }

    pub fn focus_pill(self: &Rc<Self>, window: WindowId, index: usize) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().pill_click(index);
        self.finish(result);
    }

    pub fn clear_pill_selection(self: &Rc<Self>, window: WindowId) {
        let index = self
            .window_snapshot(window)
            .and_then(|window| window.focused_pill_index());
        if let Some(index) = index {
            self.focus_pill(window, index);
        }
    }

    pub fn open_request(self: &Rc<Self>, paths: &[PathBuf]) {
        crate::log!("launch", "open request: {paths:?}");
        let result = self.workspace.borrow_mut().open_request(paths);
        self.finish(result);
    }

    /// A grid click, after the view has collected the visible order.
    pub fn file_click(self: &Rc<Self>, window: WindowId, request: ClickRequest) {
        self.focus_model_window(window);
        let result = {
            let mut workspace = self.workspace.borrow_mut();
            match request {
                ClickRequest::Plain(name) => workspace.file_click(name),
                ClickRequest::Ctrl(name) => workspace.file_ctrl_click(name),
                ClickRequest::Shift { name, order } => workspace.file_shift_click(name, &order),
            }
        };
        self.finish(result);
    }

    pub fn selection_all(self: &Rc<Self>, window: WindowId, names: &[OsString]) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().selection_all(names);
        self.finish(result);
    }

    pub fn selection_none(self: &Rc<Self>, window: WindowId) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().selection_none();
        self.finish(result);
    }

    pub fn record_scroll_anchor(self: &Rc<Self>, window: WindowId, name: OsString) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().record_scroll_anchor(Some(name));
        self.finish(result);
    }

    /// Navigate the focused tab to `path` without going through a string
    /// command, so a non-UTF-8 folder name survives.
    pub fn nav_go_path(self: &Rc<Self>, window: WindowId, path: PathBuf) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().nav_go(path, None);
        self.finish(result);
    }

    /// Open a grid item: a folder navigates, a file launches. The decision is
    /// pure; the only filesystem calls are `is_dir`/`is_file`, which follow a
    /// symlink to its target. A UTF-8 path goes through the named commands;
    /// a non-UTF-8 one uses the model and launch code directly.
    pub fn open_path(self: &Rc<Self>, window: WindowId, path: PathBuf) {
        match decision(path.is_dir(), path.is_file()) {
            OpenDecision::Navigate => {
                if let Some(path) = path.to_str() {
                    self.dispatch_for_window(
                        window,
                        Caller::Ui,
                        Invocation::new("nav.go", vec![("path".to_string(), path.to_string())]),
                    );
                } else {
                    self.nav_go_path(window, path);
                }
            }
            OpenDecision::Launch => {
                if let Some(path) = path.to_str() {
                    self.dispatch_for_window(
                        window,
                        Caller::Ui,
                        Invocation::new("file.open", vec![("paths".to_string(), path.to_string())]),
                    );
                } else {
                    self.launch_file(&path);
                }
            }
            OpenDecision::Missing => {
                crate::log!("open", "{} is gone; nothing to open", path.display())
            }
        }
    }

    /// The one launch path, shared by `file.open` and a double-click. A failure
    /// is logged, never a panic.
    pub fn launch_file(&self, path: &Path) {
        let uri = gio::File::for_path(path).uri().to_string();
        match gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
            Ok(()) => crate::log!("open", "launched {}", path.display()),
            Err(error) => crate::log!("open", "could not launch {}: {error}", path.display()),
        }
    }

    fn selected_paths(&self, window: WindowId) -> Vec<PathBuf> {
        let Some(window) = self.window_snapshot(window) else {
            return Vec::new();
        };
        let Some(tab) = window.tab(window.focused_tab) else {
            return Vec::new();
        };
        tab.selection
            .names
            .iter()
            .map(|name| tab.folder.join(name))
            .collect()
    }

    pub fn quit(self: &Rc<Self>) {
        self.save_now(true);
        self.application.quit();
    }

    fn finish(self: &Rc<Self>, result: Result<Vec<Event>, fm_core::workspace::WorkspaceError>) {
        match result {
            Ok(events) => {
                if events.is_empty() {
                    return;
                }
                self.apply_events(&events);
                self.schedule_save();
            }
            Err(error) => crate::log!("workspace", "{error}"),
        }
    }

    /// Model events drive the UI: one function per event kind.
    pub fn apply_events(self: &Rc<Self>, events: &[Event]) {
        for event in events {
            crate::log!("model", "{event:?}");
            self.on_event(event);
        }
    }

    fn on_event(self: &Rc<Self>, event: &Event) {
        match event {
            Event::WindowOpened(id) => self.on_window_opened(*id),
            Event::WindowClosed(id) => self.on_window_closed(*id),
            Event::WindowFocused(id) => self.on_window_focused(*id),
            Event::LastWindowClosed => self.on_last_window_closed(),
            Event::TabAdded { window, .. } => self.on_tab_added(*window),
            Event::TabClosed { window, .. } => self.on_tab_closed(*window),
            Event::TabMoved { window, .. } => self.on_tab_moved(*window),
            Event::GroupFormed { window, .. } => self.on_group_formed(*window),
            Event::GroupDissolved { window, .. } => self.on_group_dissolved(*window),
            Event::GroupRenamed { window, .. } => self.on_group_renamed(*window),
            Event::FocusChanged { window, .. } => self.on_focus_changed(*window),
            Event::PillMoved { window, .. } => self.on_pill_moved(*window),
            Event::LayoutChanged { window, .. } => self.on_layout_changed(*window),
            Event::SelectionChanged { window } => self.on_selection_changed(*window),
            Event::Navigated { window, folder, .. } => self.on_navigated(*window, folder),
            Event::ScrollAnchorChanged { window, .. } => self.on_scroll_anchor_changed(*window),
        }
    }

    fn on_window_opened(self: &Rc<Self>, id: WindowId) {
        self.build_window(id);
    }

    fn on_window_closed(self: &Rc<Self>, id: WindowId) {
        self.destroy_window(id);
    }

    fn on_window_focused(self: &Rc<Self>, id: WindowId) {
        // Do not present here: presenting from inside an is-active notify
        // ping-pongs between two windows. New windows are presented when they
        // are built, and the session's focused window is presented last.
        crate::log!("focus", "model window {id} is now the focused one");
    }

    fn on_last_window_closed(self: &Rc<Self>) {
        self.quit();
    }

    fn on_tab_added(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_tab_closed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_tab_moved(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_group_formed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, true);
    }

    fn on_group_dissolved(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, true);
    }

    fn on_group_renamed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_focus_changed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_pill_moved(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_layout_changed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_selection_changed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn on_navigated(self: &Rc<Self>, id: WindowId, folder: &Path) {
        if let Some(ui) = self.window(id) {
            ui.push_recent(folder);
        }
        self.refresh(id, false);
    }

    fn on_scroll_anchor_changed(self: &Rc<Self>, id: WindowId) {
        self.refresh(id, false);
    }

    fn refresh(self: &Rc<Self>, id: WindowId, fade: bool) {
        if let Some(ui) = self.window(id) {
            ui.refresh_header(fade);
            ui.update_content();
        }
    }

    fn settings_change(self: &Rc<Self>, window: WindowId, command: &CheckedCommand) -> Outcome {
        let current = self
            .window_snapshot(window)
            .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
            .unwrap_or_else(|| self.home());
        let change = {
            let config = self.config.borrow();
            config::change_for(command, config.config(), &current)
        };
        let Some(change) = change else {
            crate::log!(
                "command",
                "{}: no config change could be made",
                command.name()
            );
            return Outcome::Done;
        };
        let applied = self.config.borrow_mut().apply(change.clone());
        match &applied {
            Ok(()) => crate::log!("command", "{} applied", command.name()),
            Err(error) => crate::log!("config", "{error}"),
        }
        self.after_change(window, &change);
        Outcome::Done
    }

    fn after_change(self: &Rc<Self>, window: WindowId, change: &Change) {
        match change {
            Change::SetScale(_) => self.apply_scale_all(),
            Change::SetOpacity(opacity) => {
                self.look.borrow_mut().set_opacity(*opacity);
                for ui in self.windows.borrow().values() {
                    ui.set_opacity(*opacity);
                }
                crate::log!("opacity", "setting {opacity:.2}: blur follows live");
            }
            Change::SetSidebarCollapsed { side, collapsed } => {
                if let Some(ui) = self.window(window) {
                    ui.toggle_sidebar_to(*side, *collapsed);
                }
            }
            Change::SetSidebarWidth { side, width } => {
                if let Some(ui) = self.window(window) {
                    ui.set_saved_sidebar_width(*side, *width);
                }
                crate::log!("sidebar", "saved {} width {width} design px", side.as_str());
            }
            Change::SetSection { section, state } => {
                for ui in self.windows.borrow().values() {
                    ui.set_section(*section, *state);
                }
                crate::log!(
                    "sidebar",
                    "{} section is {}",
                    section.as_str(),
                    state.as_str()
                );
            }
            Change::AddPin(_) | Change::RemovePin(_) | Change::MovePin { .. } => {
                let windows: Vec<Rc<WindowUi>> = self.windows.borrow().values().cloned().collect();
                for ui in windows {
                    ui.reload_sidebar();
                }
                crate::log!("pins", "the sidebars follow the config");
            }
            Change::SetSort { .. } => {
                let windows: Vec<Rc<WindowUi>> = self.windows.borrow().values().cloned().collect();
                for ui in windows {
                    ui.sort_changed();
                }
            }
            other => crate::log!("command", "change applied: {other:?}"),
        }
    }

    fn apply_scale_all(self: &Rc<Self>) {
        let scale = self.scale();
        self.look.borrow_mut().set_scale(scale);
        let windows: Vec<Rc<WindowUi>> = self.windows.borrow().values().cloned().collect();
        for ui in windows {
            ui.apply_scale();
        }
    }

    pub fn handle_command(
        self: &Rc<Self>,
        window: WindowId,
        caller: Caller,
        command: &CheckedCommand,
    ) -> Outcome {
        crate::log!(
            "command",
            "{} via {caller} ({})",
            command.name(),
            command.effect()
        );
        if command.name() == "file.open" {
            let paths = argument_paths(command, "paths")
                .filter(|paths| !paths.is_empty())
                .unwrap_or_else(|| self.selected_paths(window));
            if paths.is_empty() {
                crate::log!("open", "file.open has nothing selected");
            } else {
                for path in paths {
                    self.launch_file(&path);
                }
            }
            return Outcome::Done;
        }
        match command.effect() {
            Effect::Settings => return self.settings_change(window, command),
            Effect::View => {}
            _ => {
                crate::log!("command", "{} not built yet", command.name());
                return Outcome::Done;
            }
        }

        let path = argument_path(command, "path");
        let text = argument_text(command, "name").or_else(|| argument_text(command, "query"));
        let index = argument_index(command, "index");
        let group = argument_index(command, "group");
        let from = argument_index(command, "from");
        let to = argument_index(command, "to");

        match command.name() {
            "app.quit" => {
                self.quit();
            }
            "window.new" => {
                let result = self.workspace.borrow_mut().window_new(path);
                self.finish(result);
            }
            "window.close" => {
                self.save_now(false);
                let result = self.workspace.borrow_mut().window_close();
                self.finish(result);
            }
            "tab.new" => {
                let result = self.workspace.borrow_mut().tab_new(path);
                self.finish(result);
            }
            "tab.close" => {
                self.save_now(false);
                let result = self.workspace.borrow_mut().tab_close();
                self.finish(result);
            }
            "tab.close-selected" => {
                self.save_now(false);
                let result = self.workspace.borrow_mut().tab_close_selected();
                self.finish(result);
            }
            "tab.focus" => {
                let result = match index {
                    Some(index) => self.workspace.borrow_mut().tab_focus(index),
                    None => {
                        crate::log!("command", "tab.focus needs an index");
                        return Outcome::Done;
                    }
                };
                self.finish(result);
            }
            "tab.move" => {
                let result = match (from, to) {
                    (Some(from), Some(to)) => self.workspace.borrow_mut().tab_move(from, to),
                    _ => {
                        crate::log!("command", "tab.move needs from and to");
                        return Outcome::Done;
                    }
                };
                self.finish(result);
            }
            "tab.open-in-new-window" => {
                self.save_now(false);
                let result = self.workspace.borrow_mut().tab_open_in_new_window();
                self.finish(result);
            }
            "group.split" => {
                let result = self.workspace.borrow_mut().group_split();
                self.finish(result);
            }
            "group.close" => {
                self.save_now(false);
                let result = self.workspace.borrow_mut().group_close();
                self.finish(result);
            }
            "group.rename" => {
                let result = match text {
                    Some(name) => self.workspace.borrow_mut().group_rename(&name),
                    None => {
                        crate::log!("command", "group.rename needs a name");
                        return Outcome::Done;
                    }
                };
                self.finish(result);
            }
            "group.add-tab" => {
                let result = match group {
                    Some(group) => self.workspace.borrow_mut().group_add_tab(group),
                    None => {
                        crate::log!("command", "group.add-tab needs a group");
                        return Outcome::Done;
                    }
                };
                self.finish(result);
            }
            "nav.go" => {
                let Some(path) = path else {
                    crate::log!("command", "nav.go needs a path");
                    return Outcome::Done;
                };
                let (folder, select) = if path.is_dir() {
                    (path, None)
                } else if path.is_file() {
                    let parent = path
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from("/"));
                    let name = path.file_name().map(std::ffi::OsString::from);
                    (parent, name)
                } else {
                    crate::log!("path", "{} does not exist", path.display());
                    return Outcome::Done;
                };
                let result = self.workspace.borrow_mut().nav_go(folder, select);
                self.finish(result);
            }
            "nav.back" => {
                if let Some(ui) = self.window(window) {
                    ui.arm_anchor_restore();
                }
                let result = self.workspace.borrow_mut().nav_back();
                if result.as_ref().map_or(true, Vec::is_empty)
                    && let Some(ui) = self.window(window)
                {
                    ui.clear_anchor_restore();
                }
                self.finish(result);
            }
            "nav.forward" => {
                if let Some(ui) = self.window(window) {
                    ui.arm_anchor_restore();
                }
                let result = self.workspace.borrow_mut().nav_forward();
                if result.as_ref().map_or(true, Vec::is_empty)
                    && let Some(ui) = self.window(window)
                {
                    ui.clear_anchor_restore();
                }
                self.finish(result);
            }
            "nav.up" => {
                let result = self.workspace.borrow_mut().nav_up();
                self.finish(result);
            }
            "nav.refresh" => {
                if let Some(ui) = self.window(window) {
                    ui.reload_folder();
                    crate::log!("folder", "nav.refresh reloaded the listing");
                }
            }
            "nav.focus-path" => {
                if let Some(ui) = self.window(window) {
                    ui.focus_path();
                }
            }
            "search.open" => {
                if let Some(ui) = self.window(window) {
                    ui.open_search(text.as_deref());
                }
            }
            "selection.all" => {
                let names = self
                    .window(window)
                    .map(|ui| ui.listed_names())
                    .unwrap_or_default();
                self.selection_all(window, &names);
            }
            "selection.none" => {
                self.selection_none(window);
            }
            "trash.open" => crate::log!(
                "trash",
                "not built yet: the Trash view needs the folder view"
            ),
            "preview.edit" | "preview.discard" | "preview.save" | "preview.save-as" => {
                crate::log!("command", "{} not built yet", command.name())
            }
            _ => crate::log!("command", "{} not built yet", command.name()),
        }
        Outcome::Done
    }

    pub fn schedule_save(self: &Rc<Self>) {
        let now = crate::save::now_ms();
        let mut debounce = self.debounce.get();
        debounce.change(now);
        self.debounce.set(debounce);
        if let Some(source) = self.save_source.borrow_mut().take() {
            source.remove();
        }
        let remaining = debounce.remaining(now).max(1);
        let weak = Rc::downgrade(self);
        let source =
            glib::timeout_add_local_once(std::time::Duration::from_millis(remaining), move || {
                if let Some(process) = weak.upgrade() {
                    process.save_source.borrow_mut().take();
                    process.save_now(false);
                }
            });
        *self.save_source.borrow_mut() = Some(source);
    }

    pub fn save_now(self: &Rc<Self>, blocking: bool) {
        if let Some(source) = self.save_source.borrow_mut().take() {
            source.remove();
        }
        self.debounce.set(Debounce::new(SAVE_DELAY_MS));
        let Some(path) = self.session_path.clone() else {
            crate::log!("session", "no session path; the workspace is not saved");
            return;
        };
        let preserve_bad = self.preserve_bad.replace(false);
        let workspace = self.workspace.borrow().clone();
        // A blocking save is also the quit barrier: even an empty workspace
        // sends a task so the writer drains earlier queued saves before the
        // process ends. A normal empty save stays a no-op.
        if workspace.windows.is_empty() && !preserve_bad && !blocking {
            return;
        }
        if blocking {
            self.saver.save_now(path, workspace, preserve_bad);
        } else {
            self.saver.save(path, workspace, preserve_bad);
        }
    }

    /// A hotkey in this window is ignored while one of its dialogs is up.
    pub fn hotkeys_blocked(&self, window: WindowId) -> bool {
        self.window(window)
            .is_some_and(|ui| ui.header().dialogs_open())
    }
}

fn argument_path(command: &CheckedCommand, name: &str) -> Option<PathBuf> {
    match command.get(name) {
        Some(Value::Path(path)) => Some(path.clone()),
        _ => None,
    }
}

fn argument_paths(command: &CheckedCommand, name: &str) -> Option<Vec<PathBuf>> {
    match command.get(name) {
        Some(Value::Paths(paths)) => Some(paths.clone()),
        _ => None,
    }
}

fn argument_text(command: &CheckedCommand, name: &str) -> Option<String> {
    match command.get(name) {
        Some(Value::Text(text)) => Some(text.clone()),
        Some(Value::Choice(choice)) => Some((*choice).to_string()),
        _ => None,
    }
}

fn argument_index(command: &CheckedCommand, name: &str) -> Option<usize> {
    match command.get(name) {
        Some(Value::Int(value)) => usize::try_from(*value).ok(),
        _ => None,
    }
}
