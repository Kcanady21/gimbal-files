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
use fm_core::fileops::{Decisions, Plan, Report, Request, TrashIdentity, TrashUri};
use fm_core::session::{self, SessionProblem};
use fm_core::workspace::{Event, Heights, TabId, WindowId, Workspace};
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::commands;
use crate::dnd::{self, DropIntent};
use crate::fileops::{self, FileOps, ProgressView, clipboard, dialogs, menu as menus, progress};
use crate::folder::ClickRequest;
use crate::frame::WindowUi;
use crate::look::Look;
use crate::open::{OpenDecision, decision};
use crate::save::{Debounce, SAVE_DELAY_MS, Saver};
use crate::trash::{PageRoute, back_consumes_history, route_on_trash_page};

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
    fileops: RefCell<Option<Rc<FileOps>>>,
    /// The session was empty, so `restore_session` opened a home tab that no
    /// one asked for. The first command-line open may reuse it (P2l item 5).
    fresh_home: Cell<bool>,
    /// A spring-loaded tab focuses the model without rebuilding the header,
    /// which would unparent the pill the drag is over.
    suppress_header: Cell<bool>,
    /// The typed Trash identities frozen when the item-menu Delete Permanently
    /// was clicked. It is taken once, before the worker starts.
    pending_trash_delete: RefCell<Option<Vec<TrashIdentity>>>,
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
            fileops: RefCell::new(None),
            fresh_home: Cell::new(false),
            suppress_header: Cell::new(false),
            pending_trash_delete: RefCell::new(None),
        });
        let fileops = FileOps::new(&process);
        *process.fileops.borrow_mut() = Some(fileops);
        process.restore_session();
        process
    }

    pub fn fileops(&self) -> Option<Rc<FileOps>> {
        self.fileops.borrow().clone()
    }

    /// Freeze the full typed Trash identities selected in the item menu. This
    /// happens before the registry dispatch and before any asynchronous
    /// planning starts.
    pub fn stage_trash_delete(&self, items: Vec<TrashIdentity>) {
        crate::log!(
            "fileops",
            "staged {} selected Trash identity(ies) for permanent deletion",
            items.len()
        );
        *self.pending_trash_delete.borrow_mut() = Some(items);
    }

    /// Take the click-time snapshot exactly once and require its deduplicated
    /// URIs to equal the registry's validated `items` set. A later browser
    /// selection or monitor refresh therefore cannot change the deletion set.
    fn take_trash_delete_for(
        &self,
        command: &CheckedCommand,
    ) -> Result<Vec<TrashIdentity>, String> {
        let Some(Value::TrashItems(requested)) = command.get("items") else {
            return Err("the selected Trash items are missing".to_string());
        };
        let snapshot = self
            .pending_trash_delete
            .borrow_mut()
            .take()
            .unwrap_or_default();
        let mut unique: Vec<TrashIdentity> = Vec::new();
        for identity in snapshot {
            if let Some(existing) = unique
                .iter()
                .find(|candidate| candidate.uri() == identity.uri())
            {
                if existing != &identity {
                    return Err("the selected Trash items are inconsistent".to_string());
                }
                continue;
            }
            unique.push(identity);
        }
        let requested: HashSet<&str> = requested.iter().map(TrashUri::as_str).collect();
        let snapshot: HashSet<&str> = unique.iter().map(|item| item.uri().as_str()).collect();
        if requested != snapshot || unique.is_empty() {
            return Err("the Trash selection changed before deletion".to_string());
        }
        Ok(unique)
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
        // A missing session file means this is a fresh start; its automatic
        // home tab may be reused by the first command-line open.
        let fresh = self
            .session_path
            .as_ref()
            .is_none_or(|path| !path.is_file());
        self.fresh_home.set(fresh);
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

    /// A tab, group or sidebar destination was chosen. That is explicit
    /// navigation, unlike a job or monitor refresh, so Trash steps aside.
    fn leave_trash_for_navigation(self: &Rc<Self>, window: WindowId) {
        if let Some(ui) = self.window(window) {
            ui.leave_trash();
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
        self.leave_trash_for_navigation(window);
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
        self.leave_trash_for_navigation(window);
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().pill_click(index);
        self.finish(result);
    }

    /// Focus one split tile's tab, used by every click that lands in a tile.
    pub fn focus_tab(self: &Rc<Self>, window: WindowId, tab: TabId) {
        self.leave_trash_for_navigation(window);
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().focus_tab(tab);
        self.finish(result);
    }

    /// Focus a tile and then run a command against that tab (a tile's ✕, or
    /// its Open in New Window).
    pub fn focus_tab_and_dispatch(
        self: &Rc<Self>,
        window: WindowId,
        tab: TabId,
        invocation: Invocation,
    ) {
        self.leave_trash_for_navigation(window);
        self.focus_model_window(window);
        let focus = self.workspace.borrow_mut().focus_tab(tab);
        let mut handler = commands::UiHandler {
            caller: Caller::Ui,
            window,
            process: Rc::downgrade(self),
        };
        let outcome = Registry::new().dispatch(Caller::Ui, &invocation, &mut handler);
        if let Outcome::Error(error) = outcome {
            crate::log!("command", "{error}");
            if let Ok(events) = focus {
                self.apply_events(&events);
            }
        }
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
        if !paths.is_empty()
            && let Some(window) = self.focused_window_id()
        {
            self.leave_trash_for_navigation(window);
        }
        let result = self.workspace.borrow_mut().open_request(paths);
        self.finish(result);
    }

    pub fn clear_fresh_home(&self) {
        self.fresh_home.set(false);
    }

    fn focused_window_id(&self) -> Option<WindowId> {
        self.workspace.borrow().focused_window
    }

    /// Command-line arguments: a folder is a tab; files in one folder share a
    /// tab with all of them selected (P2l item 5).
    pub fn open_arguments(self: &Rc<Self>, paths: &[PathBuf], reuse_fresh: bool) {
        let groups = crate::cli::group_arguments(paths, &|path| path.is_dir());
        if groups.is_empty() {
            return;
        }
        let mut reuse = reuse_fresh && self.fresh_home.replace(false);
        for group in groups {
            let Some(window) = self.focused_window_id() else {
                return;
            };
            match group {
                crate::cli::Argument::Folder(folder) => {
                    crate::log!("launch", "argument folder {}", folder.display());
                    if reuse {
                        self.nav_go_path(window, folder);
                    } else {
                        self.open_request(&[folder]);
                    }
                }
                crate::cli::Argument::Files { folder, names } => {
                    crate::log!(
                        "launch",
                        "argument files in {}: {:?}",
                        folder.display(),
                        names
                    );
                    if reuse {
                        self.nav_go_select(window, folder.clone(), &names);
                    } else {
                        self.open_request(std::slice::from_ref(&folder));
                    }
                    let Some(window) = self.focused_window_id() else {
                        return;
                    };
                    self.selection_all(window, &names);
                    crate::log!(
                        "launch",
                        "command line selected {} file(s) in {}: {:?}",
                        names.len(),
                        folder.display(),
                        names
                    );
                }
            }
            reuse = false;
        }
        if let Some(window) = self.focused_window_id() {
            let tabs = self
                .window_snapshot(window)
                .map(|window| window.tab_count())
                .unwrap_or(0);
            let selected = self
                .window_snapshot(window)
                .and_then(|window| {
                    window
                        .tab(window.focused_tab)
                        .map(|tab| tab.selection.names.len())
                })
                .unwrap_or(0);
            crate::log!(
                "launch",
                "command line done: {tabs} tab(s), {selected} selected in the focused tab"
            );
        }
    }

    pub fn tab_folder(&self, window: WindowId, tab: TabId) -> Option<PathBuf> {
        self.window_snapshot(window)
            .and_then(|window| window.tab(tab).map(|tab| tab.folder.clone()))
    }

    /// 700 ms over a header pill focuses its tab without rebuilding the header
    /// (which would remove the pill under the drag).
    pub fn spring_focus_pill(self: &Rc<Self>, window: WindowId, index: usize) {
        self.leave_trash_for_navigation(window);
        self.suppress_header.set(true);
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().pill_click(index);
        self.finish(result);
        self.suppress_header.set(false);
        if let Some(ui) = self.window(window) {
            ui.update_content();
        }
        crate::log!("dnd", "spring-loaded header pill {index}");
    }

    /// The same for a split tile's pill.
    pub fn spring_focus_tab(self: &Rc<Self>, window: WindowId, tab: TabId) {
        self.leave_trash_for_navigation(window);
        self.suppress_header.set(true);
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().focus_tab(tab);
        self.finish(result);
        self.suppress_header.set(false);
        if let Some(ui) = self.window(window) {
            ui.update_content();
        }
        crate::log!("dnd", "spring-loaded tile {tab}");
    }

    /// A drag ended: the header can follow the spring-loaded focus again.
    pub fn drag_finished(self: &Rc<Self>, window: WindowId) {
        if let Some(ui) = self.window(window) {
            ui.refresh_header(false);
            ui.update_content();
        }
    }

    pub fn show_drop_reason(&self, window: WindowId, reason: &str) {
        crate::log!("dnd", "refused: {reason}");
        if let Some(ui) = self.window(window) {
            ui.show_summary(&format!("Cannot drop: {reason}"), None);
        }
    }

    /// Submit a folder drop with the action frozen at the drop callback. The
    /// negotiation, not a later filesystem query, chose Copy or Move; the
    /// worker owns the conflict dialog, progress, Cancel and undo.
    pub fn drop_on_folder(
        self: &Rc<Self>,
        window: WindowId,
        target: PathBuf,
        paths: Vec<PathBuf>,
        intent: DropIntent,
    ) {
        if paths.is_empty() {
            return;
        }
        if let Err(reason) = dnd::validate_folder_drop(&target, &paths) {
            self.show_drop_reason(window, &reason);
            return;
        }
        let count = paths.len();
        let action = intent.action();
        let operation =
            dnd::operation_for_action(&dnd::Target::Grid(target.clone()), &paths, action);
        let (request, chosen) = match operation {
            Ok(dnd::Operation::Copy { to }) => (Request::Copy { paths, to }, dnd::DropAction::Copy),
            Ok(dnd::Operation::Move { to }) => (Request::Move { paths, to }, dnd::DropAction::Move),
            Ok(_) => return,
            Err(reason) => {
                self.show_drop_reason(window, &reason);
                return;
            }
        };
        if let Some(fileops) = self.fileops() {
            let id = fileops.submit(window, request, false);
            crate::log!(
                "dnd",
                "drop {count} item(s) into {}: source {}, selected {}, target preference {}, filesystem {}, modifiers {}, final {} -> job {id}",
                target.display(),
                intent.source_text(),
                dnd::action_text(intent.selected()),
                dnd::action_text(intent.requested()),
                dnd::filesystem_text(intent.filesystem()),
                intent.modifiers_text(),
                dnd::action_text(Some(chosen))
            );
        }
    }

    /// The Trash button: `file.trash` for every dropped path, modifiers
    /// ignored.
    pub fn drop_on_trash(self: &Rc<Self>, window: WindowId, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let count = paths.len();
        if let Some(fileops) = self.fileops() {
            let id = fileops.submit(window, Request::Trash { paths }, false);
            crate::log!("dnd", "drop {count} item(s) on the Trash -> job {id}");
        }
    }

    /// Pin dropped folders, one `pins.add` per folder. Files are filtered by
    /// the caller before this point.
    pub fn pin_folders(self: &Rc<Self>, window: WindowId, folders: Vec<PathBuf>) {
        for folder in folders {
            let Some(path) = folder.to_str() else {
                crate::log!(
                    "dnd",
                    "cannot pin the non-UTF-8 folder {}",
                    folder.display()
                );
                continue;
            };
            crate::log!("dnd", "pin {}", folder.display());
            self.dispatch_for_window(
                window,
                Caller::Ui,
                Invocation::new("pins.add", vec![("path".to_string(), path.to_string())]),
            );
        }
    }

    /// A grid click in one split tile: focus that tab first, then select, and
    /// apply both operations' events as one update.
    pub fn file_click_tab(
        self: &Rc<Self>,
        window: WindowId,
        tab: Option<TabId>,
        request: ClickRequest,
    ) {
        self.focus_model_window(window);
        let mut events = Vec::new();
        {
            let mut workspace = self.workspace.borrow_mut();
            if let Some(tab) = tab {
                match workspace.focus_tab(tab) {
                    Ok(mut focus) => events.append(&mut focus),
                    Err(error) => {
                        crate::log!("workspace", "{error}");
                        return;
                    }
                }
            }
            let result = match request {
                ClickRequest::Plain(name) => workspace.file_click(name),
                ClickRequest::Ctrl(name) => workspace.file_ctrl_click(name),
                ClickRequest::Shift { name, order } => workspace.file_shift_click(name, &order),
            };
            match result {
                Ok(mut result) => events.append(&mut result),
                Err(error) => crate::log!("workspace", "{error}"),
            }
        }
        if events.is_empty() {
            return;
        }
        self.apply_events(&events);
        self.schedule_save();
    }

    pub fn selection_all(self: &Rc<Self>, window: WindowId, names: &[OsString]) {
        if let Some(ui) = self.window(window)
            && ui.trash_active()
        {
            ui.trash_select_all();
            return;
        }
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().selection_all(names);
        self.finish(result);
    }

    pub fn selection_none(self: &Rc<Self>, window: WindowId) {
        if let Some(ui) = self.window(window)
            && ui.trash_active()
        {
            ui.trash_select_none();
            return;
        }
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().selection_none();
        self.finish(result);
    }

    /// A tile drop, after the UI has hit-tested it. The model decides whether
    /// the move is refused.
    pub fn apply_tile_drop(
        self: &Rc<Self>,
        window: WindowId,
        from: TabId,
        drop: fm_core::workspace::Drop,
    ) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().apply_tile_drop(from, drop);
        self.finish(result);
    }

    /// The column border's new share, written once when its drag ends.
    pub fn set_width_share(self: &Rc<Self>, window: WindowId, permille: u16) {
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().set_width_share(permille);
        self.finish(result);
    }

    /// One column's hand heights, written once when its border drag ends.
    pub fn set_column_heights(self: &Rc<Self>, window: WindowId, column: usize, heights: Heights) {
        self.focus_model_window(window);
        let result = self
            .workspace
            .borrow_mut()
            .set_column_heights(column, heights);
        self.finish(result);
    }

    /// Record one tile's anchor. `fm-core` records the focused tab only, so an
    /// unfocused tile is focused for the one update and the old focus is put
    /// back without touching the UI.
    pub fn record_scroll_anchor_for(
        self: &Rc<Self>,
        window: WindowId,
        tab: Option<TabId>,
        name: OsString,
    ) {
        self.focus_model_window(window);
        let mut events = Vec::new();
        let mut hidden = false;
        {
            let mut workspace = self.workspace.borrow_mut();
            let previous = workspace.focused().map(|window| window.focused_tab);
            if let Some(tab) = tab
                && previous != Some(tab)
            {
                match workspace.focus_tab(tab) {
                    Ok(_) => hidden = true,
                    Err(error) => crate::log!("workspace", "{error}"),
                }
            }
            match workspace.record_scroll_anchor(Some(name)) {
                Ok(mut result) => {
                    if !hidden {
                        events.append(&mut result);
                    }
                }
                Err(error) => crate::log!("workspace", "{error}"),
            }
            if hidden && let Some(previous) = previous {
                let _ = workspace.focus_tab(previous);
            }
        }
        if !events.is_empty() {
            self.apply_events(&events);
        }
        self.schedule_save();
    }

    /// Navigate the focused tab to `path` without going through a string
    /// command, so a non-UTF-8 folder name survives.
    pub fn nav_go_path(self: &Rc<Self>, window: WindowId, path: PathBuf) {
        self.leave_trash_for_navigation(window);
        self.focus_model_window(window);
        let result = self.workspace.borrow_mut().nav_go(path, None);
        self.finish(result);
    }

    /// The command line's file form: `nav_go(folder, select)` picks the first
    /// name, then `selection_all` adds the rest of the group.
    fn nav_go_select(self: &Rc<Self>, window: WindowId, path: PathBuf, names: &[OsString]) {
        self.leave_trash_for_navigation(window);
        self.focus_model_window(window);
        let result = self
            .workspace
            .borrow_mut()
            .nav_go(path, names.first().cloned());
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

    pub fn selected_paths(&self, window: WindowId) -> Vec<PathBuf> {
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
            if !self.suppress_header.get() {
                ui.refresh_header(fade);
            }
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
            Change::SetSort { folder, .. } => {
                let windows: Vec<Rc<WindowUi>> = self.windows.borrow().values().cloned().collect();
                for ui in windows {
                    ui.sort_changed(folder.as_deref());
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

        // The visible page owns these keys and actions. Nothing here may fall
        // back to the hidden folder's selection or current folder.
        if let Some(ui) = self.window(window) {
            match route_on_trash_page(command.name(), ui.trash_active()) {
                PageRoute::Files => {}
                PageRoute::SelectAll => {
                    ui.trash_select_all();
                    return Outcome::Done;
                }
                PageRoute::SelectNone => {
                    ui.trash_select_none();
                    return Outcome::Done;
                }
                PageRoute::RefreshTrash => {
                    ui.trash_refresh();
                    crate::log!("trash", "nav.refresh reloaded the Trash page");
                    return Outcome::Done;
                }
                PageRoute::BackToFiles => {
                    ui.leave_trash();
                    return Outcome::Done;
                }
                PageRoute::LeaveTrash => {
                    // An explicit destination was requested. Leave the
                    // transient page, then let this command navigate normally.
                    ui.leave_trash();
                }
                PageRoute::Refuse(message) => {
                    ui.trash_status(message);
                    crate::log!("trash", "refused {message}");
                    return Outcome::Done;
                }
            }
        }

        match command.name() {
            "file.cut" => {
                self.put_on_clipboard(window, caller, command, clipboard::ClipboardMode::Cut);
                return Outcome::Done;
            }
            "file.copy-to-clipboard" => {
                self.put_on_clipboard(window, caller, command, clipboard::ClipboardMode::Copy);
                return Outcome::Done;
            }
            "file.paste" => {
                self.paste(window, caller, command);
                return Outcome::Done;
            }
            "app.disk-usage" => {
                let path = argument_path(command, "path").or_else(|| self.current_folder(window));
                if let Some(path) = path {
                    self.launch_disk_usage(&path);
                }
                return Outcome::Done;
            }
            _ => {}
        }

        if command.effect() == Effect::Files
            && let Some(outcome) = self.run_file_command(window, caller, command)
        {
            return outcome;
        }

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
                let trash_active = self.window(window).is_some_and(|ui| ui.trash_active());
                if !back_consumes_history(trash_active) {
                    if let Some(ui) = self.window(window) {
                        ui.leave_trash();
                    }
                    return Outcome::Done;
                }
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
            "trash.open" => {
                if let Some(ui) = self.window(window) {
                    ui.open_trash();
                }
            }
            "preview.edit" | "preview.discard" | "preview.save" | "preview.save-as" => {
                crate::log!("command", "{} not built yet", command.name())
            }
            _ => crate::log!("command", "{} not built yet", command.name()),
        }
        Outcome::Done
    }

    // The arms share one "fill only when the argument is absent" shape; a
    // match guard would hide which context argument each command gets.
    #[allow(clippy::collapsible_match)]
    fn filled_invocation(
        &self,
        window: WindowId,
        caller: Caller,
        command: &CheckedCommand,
    ) -> Result<Invocation, String> {
        let mut arguments: Vec<(String, String)> = Vec::new();
        for argument in command.arguments() {
            match &argument.value {
                Value::Paths(paths) => {
                    for path in paths {
                        arguments.push((argument.name.to_string(), utf8_path(path)?));
                    }
                }
                Value::TrashItems(items) => {
                    for item in items {
                        arguments.push((argument.name.to_string(), item.as_str().to_string()));
                    }
                }
                value => arguments.push((argument.name.to_string(), value.to_string())),
            }
        }

        if matches!(caller, Caller::Ui | Caller::Hotkey) {
            match command.name() {
                "file.trash"
                | "file.copy"
                | "file.move"
                | "file.cut"
                | "file.copy-to-clipboard" => {
                    if command.get("paths").is_none() {
                        for path in self.selected_paths(window) {
                            arguments.push(("paths".to_string(), utf8_path(&path)?));
                        }
                    }
                }
                "file.rename" => {
                    if command.get("path").is_none()
                        && let Some(path) = self.selected_paths(window).first()
                    {
                        arguments.push(("path".to_string(), utf8_path(path)?));
                    }
                }
                "file.new-folder" | "file.paste" => {
                    if command.get("in").is_none()
                        && let Some(folder) = self.current_folder(window)
                    {
                        arguments.push(("in".to_string(), utf8_path(&folder)?));
                    }
                }
                _ => {}
            }
        }

        Ok(Invocation::new(command.name(), arguments))
    }

    fn request_from_command(
        &self,
        window: WindowId,
        caller: Caller,
        command: &CheckedCommand,
    ) -> Result<Option<Request>, String> {
        let invocation = self.filled_invocation(window, caller, command)?;
        let checked = Registry::new()
            .validate(caller, &invocation)
            .map_err(|error| error.to_string())?;
        fm_core::fileops::request_for(&checked).map_err(|error| error.to_string())
    }

    fn run_file_command(
        self: &Rc<Self>,
        window: WindowId,
        caller: Caller,
        command: &CheckedCommand,
    ) -> Option<Outcome> {
        let request = if command.name() == "trash.delete-permanently" {
            match self.take_trash_delete_for(command) {
                Ok(items) => Request::DeleteSelected { items },
                Err(error) => {
                    crate::log!("fileops", "{}: {error}", command.name());
                    if let Some(ui) = self.window(window) {
                        ui.trash_status(&format!("Could not delete permanently: {error}"));
                    }
                    return Some(Outcome::Done);
                }
            }
        } else {
            match self.request_from_command(window, caller, command) {
                Ok(Some(request)) => request,
                Ok(None) => return None,
                Err(error) => {
                    crate::log!("fileops", "{}: {error}", command.name());
                    return Some(Outcome::Done);
                }
            }
        };

        if command.dry_run() {
            if matches!(
                request,
                Request::EmptyTrash | Request::Restore { .. } | Request::DeleteSelected { .. }
            ) && let Some(fileops) = self.fileops()
            {
                let id = fileops.submit_dry_run(window, request);
                crate::log!("fileops", "{} dry run queued as job {id}", command.name());
                return Some(Outcome::Done);
            }
            let lines = self
                .fileops()
                .map(|fileops| fileops.plan_lines(&request))
                .unwrap_or_default();
            crate::log!(
                "fileops",
                "{} dry run: {}",
                command.name(),
                lines.join(" | ")
            );
            return Some(Outcome::DryRun(lines));
        }

        if let Some(fileops) = self.fileops() {
            let id = fileops.submit(window, request, false);
            crate::log!("fileops", "{} queued as job {id}", command.name());
        }
        Some(Outcome::Done)
    }

    fn put_on_clipboard(
        self: &Rc<Self>,
        window: WindowId,
        caller: Caller,
        command: &CheckedCommand,
        mode: clipboard::ClipboardMode,
    ) {
        let paths = argument_paths(command, "paths")
            .filter(|paths| !paths.is_empty())
            .unwrap_or_else(|| {
                if matches!(caller, Caller::Ui | Caller::Hotkey) {
                    self.selected_paths(window)
                } else {
                    Vec::new()
                }
            });
        if paths.is_empty() {
            crate::log!("fileops", "{} has nothing selected", command.name());
            return;
        }
        let Some(ui) = self.window(window) else {
            return;
        };
        match clipboard::write(&ui.display(), mode, &paths) {
            Ok(()) => crate::log!(
                "fileops",
                "{} put {} item(s) on the clipboard",
                mode.as_str(),
                paths.len()
            ),
            Err(error) => crate::log!("fileops", "clipboard: {error}"),
        }
    }

    fn paste(self: &Rc<Self>, window: WindowId, caller: Caller, command: &CheckedCommand) {
        let folder = argument_path(command, "in").or_else(|| {
            if matches!(caller, Caller::Ui | Caller::Hotkey) {
                self.current_folder(window)
            } else {
                None
            }
        });
        let Some(folder) = folder else {
            crate::log!("fileops", "file.paste has no folder");
            return;
        };
        let dry_run = command.dry_run();
        let Some(ui) = self.window(window) else {
            return;
        };
        let weak = Rc::downgrade(self);
        clipboard::read(&ui.display(), move |result| {
            let Some(process) = weak.upgrade() else {
                return;
            };
            let payload = match result {
                Ok(payload) => payload,
                Err(error) => {
                    crate::log!("fileops", "paste: {error}");
                    return;
                }
            };
            let paths: Vec<PathBuf> = payload
                .uris
                .iter()
                .filter_map(|uri| gio::File::for_uri(uri).path())
                .collect();
            if paths.is_empty() {
                crate::log!("fileops", "paste: the clipboard has no local files");
                return;
            }
            let request = match payload.mode {
                clipboard::ClipboardMode::Cut => Request::Move {
                    paths,
                    to: folder.clone(),
                },
                clipboard::ClipboardMode::Copy => Request::Copy {
                    paths,
                    to: folder.clone(),
                },
            };
            if dry_run {
                let lines = process
                    .fileops()
                    .map(|fileops| fileops.plan_lines(&request))
                    .unwrap_or_default();
                crate::log!("fileops", "paste dry run: {}", lines.join(" | "));
                return;
            }
            if let Some(fileops) = process.fileops() {
                let id = fileops.submit(window, request, clipboard::clear_after_cut(payload.mode));
                crate::log!("fileops", "paste ({}): job {id}", payload.mode.as_str());
            }
        });
    }

    fn current_folder(&self, window: WindowId) -> Option<PathBuf> {
        self.window_snapshot(window)
            .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
    }

    pub fn clipboard_has_files(&self, window: WindowId) -> bool {
        self.window(window)
            .is_some_and(|ui| clipboard::display_has_files(&ui.display()))
    }

    /// A right press on a grid cell. The tile is focused first; an unselected
    /// item becomes the selection before the menu opens.
    pub fn right_click_item(
        self: &Rc<Self>,
        window: WindowId,
        tab: Option<TabId>,
        name: OsString,
        anchor: &gtk::Widget,
    ) {
        self.focus_model_window(window);
        let selected = self.window_snapshot(window).is_some_and(|window| {
            let tab = tab
                .and_then(|tab| window.tab(tab))
                .or_else(|| window.tab(window.focused_tab));
            tab.is_some_and(|tab| tab.selection.names.contains(&name))
        });
        if selected {
            if let Some(tab) = tab {
                let result = self.workspace.borrow_mut().focus_tab(tab);
                self.finish(result);
            }
        } else {
            self.file_click_tab(window, tab, ClickRequest::Plain(name));
        }

        let paths = self.selected_paths(window);
        let items: Vec<fileops::ItemInfo> = paths
            .iter()
            .map(|path| fileops::ItemInfo {
                path: path.clone(),
                is_dir: path.is_dir(),
            })
            .collect();
        if let Some(ui) = self.window(window) {
            dialogs::show_menu(&ui, anchor, &menus::item_menu(&items), None);
        }
    }

    /// A right press on empty grid space.
    pub fn right_click_background(
        self: &Rc<Self>,
        window: WindowId,
        tab: Option<TabId>,
        folder: PathBuf,
        anchor: &gtk::Widget,
        at: (f64, f64),
    ) {
        self.focus_model_window(window);
        if let Some(tab) = tab {
            let result = self.workspace.borrow_mut().focus_tab(tab);
            self.finish(result);
        }
        let fileops = self.fileops();
        let undo = fileops.as_ref().and_then(|fileops| fileops.undo_summary());
        let baobab = fileops
            .as_ref()
            .is_some_and(|fileops| fileops.baobab_available());
        let has_files = self.clipboard_has_files(window);
        let items = menus::background_menu(&folder, undo.as_deref(), baobab, has_files);
        if let Some(ui) = self.window(window) {
            // The anchor is the whole grid scroller: point at the click, or
            // the popover lands at the scroller's edge, outside the window
            // (Claude's P2k review).
            dialogs::show_menu(&ui, anchor, &items, Some(at));
        }
    }

    pub fn fileop_started(&self, window: WindowId) {
        if let Some(ui) = self.window(window) {
            ui.fileop_started();
            ui.trash_job_started();
        }
    }

    pub fn fileop_progress(&self, window: WindowId, view: Option<ProgressView>) {
        if let Some(ui) = self.window(window) {
            ui.fileop_progress(view.clone());
            ui.trash_progress(view);
        }
    }

    pub fn fileop_dry_run(&self, window: WindowId, request: &Request, lines: &[String]) {
        crate::log!("fileops", "dry run: {}", lines.join(" | "));
        if let Some(ui) = self.window(window) {
            ui.fileop_progress(None);
            ui.trash_dry_run(request, lines);
        }
    }

    pub fn fileop_empty_confirmation(&self, window: WindowId, id: u64, plan: Plan) {
        if let Some(ui) = self.window(window)
            && ui.trash_visible()
        {
            ui.confirm_empty_trash(id, &plan);
        } else if let Some(fileops) = self.fileops() {
            crate::log!(
                "fileops",
                "no visible Trash page for the permanent-delete confirmation; cancelling job {id}"
            );
            fileops.cancel_job(id);
        }
    }

    pub fn fileop_finished(
        &self,
        window: WindowId,
        request: &Request,
        plan: &Plan,
        report: &Report,
    ) {
        let summary = if *request == Request::Undo {
            progress::undo_status(plan, report)
        } else {
            progress::summary(request, report)
        };
        let failures = progress::failure_lines(report);
        let details = (!failures.is_empty()).then(|| failures.join("\n"));
        crate::log!("fileops", "summary: {summary}");
        for line in report.lines() {
            crate::log!("fileops", "item: {line}");
        }
        if let Some(ui) = self.window(window) {
            ui.show_summary(&summary, details.clone());
            ui.trash_job_finished(request, &summary, details);
        }
    }

    pub fn fileop_refused(&self, window: WindowId, request: &Request, message: &str) {
        crate::log!("fileops", "refused: {message}");
        if let Some(ui) = self.window(window) {
            if matches!(request, Request::Undo) && message.contains("nothing to undo") {
                ui.show_nothing_to_undo();
            } else {
                ui.show_summary(&format!("{}: {message}", request.summary()), None);
            }
            ui.trash_refused(request, message);
        }
    }

    pub fn fileop_clear_clipboard(&self, window: WindowId) {
        if let Some(ui) = self.window(window) {
            clipboard::clear(&ui.display());
            crate::log!("fileops", "cleared the clipboard after a cut paste");
        }
    }

    pub fn fileop_conflicts(
        &self,
        window: WindowId,
        flow: fileops::conflict::ConflictFlow,
        done: Box<dyn Fn(Decisions)>,
    ) {
        if let Some(ui) = self.window(window) {
            dialogs::show_conflicts(&ui, flow, Rc::from(done));
        }
    }

    fn launch_disk_usage(&self, path: &Path) {
        let Some(executable) = menus::find_on_path("baobab", std::env::var_os("PATH").as_deref())
        else {
            crate::log!("disk-usage", "baobab is not on PATH");
            return;
        };
        match std::process::Command::new(executable).arg(path).spawn() {
            Ok(_) => crate::log!("disk-usage", "launched baobab for {}", path.display()),
            Err(error) => crate::log!(
                "disk-usage",
                "could not launch baobab for {}: {error}",
                path.display()
            ),
        }
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

fn utf8_path(path: &Path) -> Result<String, String> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        format!(
            "a command argument cannot hold the non-UTF-8 name {}",
            path.display()
        )
    })
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
