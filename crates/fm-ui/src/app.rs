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
use fm_core::preview::Selection;
use fm_core::session::{self, SessionProblem};
use fm_core::workspace::{Event, Heights, TabId, WindowId, Workspace};
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
use crate::preview::CloseGuard;
use crate::save::{Debounce, SAVE_DELAY_MS, Saver};
use crate::trash::{PageRoute, back_consumes_history, route_on_trash_page};

pub struct Process {
    application: Option<adw::Application>,
    pub config: Rc<RefCell<ConfigFile>>,
    pub look: Option<Rc<Look>>,
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
    quit: RefCell<QuitCoordinator>,
    /// The local automation listener and its bounded job history. The Settings
    /// switch is the only path that changes it; agents cannot.
    pub(crate) agent: RefCell<crate::agent::AgentRuntime>,
    agent_poll: RefCell<Option<glib::SourceId>>,
    /// The file-job id the handler just queued, read by the socket bridge.
    command_job: Cell<Option<u64>>,
    /// A bridge job token reserved before an asynchronous handler (clipboard
    /// paste) starts. The handler binds it once the file-op worker accepts.
    reserved_job: RefCell<Option<String>>,
    /// Authorization for exactly the handler dispatch now running. The bridge
    /// sets it from the accepted generation, never from job history.
    pending_authorization: RefCell<Option<crate::agent::JobAuthorization>>,
    /// Socket runs parked on the bounded filesystem pool.
    pending_runs: RefCell<HashMap<u64, crate::agent::PendingRun>>,
    next_work_id: Cell<u64>,
    /// The filesystem worker's resolved `nav.go` target, consumed by the real
    /// command handler instead of a GTK-thread `exists()` call.
    pending_nav_target: RefCell<Option<(PathBuf, Option<OsString>)>>,
    /// The first real model failure in the current command dispatch.
    last_failure: RefCell<Option<String>>,
    /// The injectable clipboard reader. Production uses the GTK clipboard;
    /// tests can pause or fail a read without touching Kyle's clipboard.
    clipboard_reader: Rc<dyn crate::fileops::clipboard::Reader>,
    /// Headless bridge tests skip GTK window construction and session saves.
    headless: bool,
    /// Test seam for the Trash page's contextual refusal. Production always
    /// reads the real window's page state.
    #[cfg(test)]
    trash_pages: RefCell<BTreeSet<WindowId>>,
}

/// One active Quit request. Every window with an unsaved buffer or an active
/// save is rechecked before the application may leave. Each request carries an
/// id so a callback from a cancelled or superseded attempt is a no-op.
#[derive(Default)]
struct QuitCoordinator {
    active: Option<u64>,
    next_request: u64,
    handled: BTreeSet<(WindowId, fm_core::preview::EditorIdentity)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuitStep {
    Inactive,
    Finish,
    Prompt { window: WindowId },
}

impl QuitCoordinator {
    fn begin(&mut self) -> Option<u64> {
        if self.active.is_some() {
            return None;
        }
        self.next_request = self.next_request.wrapping_add(1);
        self.active = Some(self.next_request);
        self.handled.clear();
        Some(self.next_request)
    }

    fn cancel(&mut self, request: u64) {
        if self.active == Some(request) {
            self.active = None;
            self.handled.clear();
        }
    }

    fn is_active(&self, request: u64) -> bool {
        self.active == Some(request)
    }

    /// Pick the next window that needs an answer. Saving windows are always
    /// prompted so their continuation binds to the in-flight job. A handled
    /// document is asked again only after its buffer identity changes.
    fn next(
        &mut self,
        request: u64,
        windows: &[(WindowId, Option<fm_core::preview::EditorActivity>)],
    ) -> QuitStep {
        if !self.is_active(request) {
            return QuitStep::Inactive;
        }
        if let Some((id, activity)) = windows.iter().find_map(|(id, activity)| {
            activity
                .as_ref()
                .filter(|activity| activity.saving)
                .map(|activity| (*id, activity))
        }) {
            self.handled.insert((id, activity.identity.clone()));
            return QuitStep::Prompt { window: id };
        }
        if let Some((id, activity)) = windows.iter().find_map(|(id, activity)| {
            activity
                .as_ref()
                .filter(|activity| {
                    activity.dirty && !self.handled.contains(&(*id, activity.identity.clone()))
                })
                .map(|activity| (*id, activity))
        }) {
            self.handled.insert((id, activity.identity.clone()));
            return QuitStep::Prompt { window: id };
        }
        // A handled document that is still dirty was not actually resolved;
        // ask again rather than quitting over it.
        if let Some((id, activity)) = windows.iter().find_map(|(id, activity)| {
            activity
                .as_ref()
                .filter(|activity| activity.dirty)
                .map(|activity| (*id, activity))
        }) {
            self.handled.insert((id, activity.identity.clone()));
            return QuitStep::Prompt { window: id };
        }
        QuitStep::Finish
    }
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
        let process_tag = format!(
            "{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        );
        let process = Rc::new(Self {
            application: Some(application.clone()),
            config: Rc::new(RefCell::new(config_file)),
            look: Some(look),
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
            quit: RefCell::new(QuitCoordinator::default()),
            agent: RefCell::new(crate::agent::AgentRuntime::new(process_tag)),
            agent_poll: RefCell::new(None),
            command_job: Cell::new(None),
            reserved_job: RefCell::new(None),
            pending_authorization: RefCell::new(None),
            pending_runs: RefCell::new(HashMap::new()),
            next_work_id: Cell::new(0),
            pending_nav_target: RefCell::new(None),
            last_failure: RefCell::new(None),
            clipboard_reader: Rc::new(crate::fileops::clipboard::SystemReader),
            headless: false,
            #[cfg(test)]
            trash_pages: RefCell::new(BTreeSet::new()),
        });
        let fileops = FileOps::new(&process);
        *process.fileops.borrow_mut() = Some(fileops);
        process.restore_session();
        crate::agent::start_if_enabled(&process);
        let weak = Rc::downgrade(&process);
        let poll = glib::timeout_add_local(crate::agent::POLL, move || match weak.upgrade() {
            Some(process) => crate::agent::poll(&process),
            None => glib::ControlFlow::Break,
        });
        *process.agent_poll.borrow_mut() = Some(poll);
        process
    }

    /// A headless process for bridge tests: no GTK windows, no session file
    /// and an injected clipboard reader. Production routing, the registry and
    /// the real file-op worker remain the same.
    #[cfg(test)]
    pub(crate) fn headless(
        config_path: PathBuf,
        home: PathBuf,
        clipboard_reader: Rc<dyn crate::fileops::clipboard::Reader>,
    ) -> Rc<Self> {
        let (config_file, _problems) = ConfigFile::load(&config_path);
        let process = Rc::new(Self {
            application: None,
            config: Rc::new(RefCell::new(config_file)),
            look: None,
            workspace: RefCell::new(Workspace::with_home(home)),
            windows: RefCell::new(BTreeMap::new()),
            session_path: None,
            saver: Saver::new(),
            save_source: RefCell::new(None),
            debounce: Cell::new(Debounce::new(SAVE_DELAY_MS)),
            preserve_bad: Cell::new(false),
            fileops: RefCell::new(None),
            fresh_home: Cell::new(false),
            suppress_header: Cell::new(false),
            pending_trash_delete: RefCell::new(None),
            quit: RefCell::new(QuitCoordinator::default()),
            agent: RefCell::new(crate::agent::AgentRuntime::new("test".to_string())),
            agent_poll: RefCell::new(None),
            command_job: Cell::new(None),
            reserved_job: RefCell::new(None),
            pending_authorization: RefCell::new(None),
            pending_runs: RefCell::new(HashMap::new()),
            next_work_id: Cell::new(0),
            pending_nav_target: RefCell::new(None),
            last_failure: RefCell::new(None),
            clipboard_reader,
            headless: true,
            trash_pages: RefCell::new(BTreeSet::new()),
        });
        let fileops = FileOps::new_manual(&process);
        *process.fileops.borrow_mut() = Some(fileops);
        process
    }

    #[cfg(test)]
    pub(crate) fn seed_window(self: &Rc<Self>, folder: PathBuf) -> WindowId {
        let events = self
            .workspace
            .borrow_mut()
            .window_new(Some(folder))
            .expect("test window");
        let id = events
            .iter()
            .find_map(|event| match event {
                Event::WindowOpened(id) => Some(*id),
                _ => None,
            })
            .expect("window id");
        self.apply_events(&events);
        id
    }

    #[cfg(test)]
    pub(crate) fn focus_window_for_tests(self: &Rc<Self>, id: WindowId) {
        let events = self.workspace.borrow_mut().focus_window(id).expect("focus");
        self.apply_events(&events);
    }

    #[cfg(test)]
    pub(crate) fn mark_trash_for_tests(&self, id: WindowId) {
        self.trash_pages.borrow_mut().insert(id);
    }

    #[cfg(test)]
    pub(crate) fn close_window_for_tests(self: &Rc<Self>, id: WindowId) {
        let _ = self.workspace.borrow_mut().focus_window(id);
        let result = self.workspace.borrow_mut().window_close();
        if let Ok(events) = result {
            self.apply_events(&events);
        }
    }

    #[cfg(test)]
    pub(crate) fn install_fileops_with_pause(self: &Rc<Self>, pause: crate::fileops::WorkerPause) {
        *self.fileops.borrow_mut() = Some(FileOps::with_pause(self, pause));
    }

    /// The config file this process reads and saves.
    pub fn config_path(&self) -> PathBuf {
        self.config.borrow().path().to_path_buf()
    }

    pub fn agent_status(&self) -> crate::agent::AgentStatus {
        crate::agent::status(self)
    }

    #[cfg(test)]
    fn trash_active_for_test(&self, window: WindowId) -> bool {
        self.trash_pages.borrow().contains(&window)
    }

    #[cfg(not(test))]
    fn trash_active_for_test(&self, _window: WindowId) -> bool {
        false
    }

    /// Whether a dirty/saving preview or close prompt needs a human answer.
    /// Socket clients cannot answer it in V1.
    pub fn user_prompt_pending(&self) -> bool {
        prompt_pending(
            self.windows
                .borrow()
                .values()
                .map(|ui| (ui.header().dialogs_open(), ui.preview().activity())),
        )
    }

    /// Push the shared listener state into every open Settings submenu.
    pub fn refresh_agent_status_all(&self) {
        let status = self.agent_status();
        for ui in self.windows.borrow().values() {
            ui.header().refresh_agent_status(&status);
        }
    }

    pub fn set_agent_enabled(self: &Rc<Self>, enabled: bool) -> Outcome {
        crate::agent::set_enabled(self, enabled)
    }

    pub fn clear_command_job(&self) {
        self.command_job.set(None);
    }

    pub fn take_command_job(&self) -> Option<u64> {
        self.command_job.take()
    }

    pub(crate) fn set_reserved_job(&self, token: Option<String>) {
        *self.reserved_job.borrow_mut() = token;
    }

    pub(crate) fn take_reserved_job(&self) -> Option<String> {
        self.reserved_job.borrow_mut().take()
    }

    pub(crate) fn set_pending_authorization(&self, auth: Option<crate::agent::JobAuthorization>) {
        *self.pending_authorization.borrow_mut() = auth;
    }

    pub(crate) fn take_pending_authorization(&self) -> Option<crate::agent::JobAuthorization> {
        self.pending_authorization.borrow_mut().take()
    }

    pub(crate) fn next_work_id(&self) -> u64 {
        let id = self.next_work_id.get().wrapping_add(1);
        self.next_work_id.set(id);
        id
    }

    pub(crate) fn store_pending_run(&self, pending: crate::agent::PendingRun) {
        self.pending_runs
            .borrow_mut()
            .insert(pending.work_id, pending);
    }

    pub(crate) fn take_pending_run(&self, work_id: u64) -> Option<crate::agent::PendingRun> {
        self.pending_runs.borrow_mut().remove(&work_id)
    }

    pub(crate) fn clear_agent_pending(&self) {
        self.pending_runs.borrow_mut().clear();
        self.pending_authorization.borrow_mut().take();
        self.pending_nav_target.borrow_mut().take();
        self.reserved_job.borrow_mut().take();
    }

    pub(crate) fn set_pending_nav_target(&self, target: Option<(PathBuf, Option<OsString>)>) {
        *self.pending_nav_target.borrow_mut() = target;
    }

    fn take_pending_nav_target(&self) -> Option<(PathBuf, Option<OsString>)> {
        self.pending_nav_target.borrow_mut().take()
    }

    pub(crate) fn clipboard_reader(&self) -> Rc<dyn crate::fileops::clipboard::Reader> {
        self.clipboard_reader.clone()
    }

    /// Every model window, cloned for a read-only agent projection.
    pub fn windows_snapshot(&self) -> Vec<fm_core::workspace::Window> {
        self.workspace.borrow().windows.clone()
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
        if self.headless {
            return;
        }
        let Some(application) = &self.application else {
            return;
        };
        let ui = WindowUi::new(self, application, id);
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

    /// Whether the exact window/tab pair still exists and belongs together.
    /// Used by the socket bridge after asynchronous evidence returns.
    pub(crate) fn execution_target_valid(&self, target: crate::agent::ExecutionTarget) -> bool {
        self.window_snapshot(target.window)
            .and_then(|window| window.tab(target.tab).map(|_| ()))
            .is_some()
    }

    /// Bind a validated socket target immediately before its effect. This is
    /// intentionally not the UI's pre-validation focus wrapper: callers run
    /// registry validation first, and a refused command never reaches here.
    /// It also does not leave the Trash page; `handle_command` still decides
    /// that per command.
    pub(crate) fn bind_execution_target(
        self: &Rc<Self>,
        target: crate::agent::ExecutionTarget,
    ) -> bool {
        let focused_window = self.workspace.borrow_mut().focus_window(target.window);
        match focused_window {
            Ok(events) => {
                if !events.is_empty() {
                    self.apply_events(&events);
                }
            }
            Err(_) => return false,
        }
        let focused_tab = self.workspace.borrow_mut().focus_tab(target.tab);
        match focused_tab {
            Ok(events) => {
                if !events.is_empty() {
                    self.apply_events(&events);
                }
                true
            }
            Err(_) => false,
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
            Outcome::Failed(message) => crate::log!("command", "{}: {message}", invocation.name()),
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

    pub(crate) fn focused_window_id(&self) -> Option<WindowId> {
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
            let id = fileops.submit(window, request, false, None);
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
            let id = fileops.submit(window, Request::Trash { paths }, false, None);
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
        // A pointer or keyboard selection is explicit user intent, even when
        // it re-selects the item already selected after Save As. Apply it
        // after the model refresh so a saved preview override is released.
        self.sync_preview_user(window);
        self.schedule_save();
    }

    fn sync_preview_user(&self, window: WindowId) {
        let Some(ui) = self.window(window) else {
            return;
        };
        let selection = self
            .window_snapshot(window)
            .and_then(|window| {
                window
                    .tab(window.focused_tab)
                    .map(|tab| Selection::from_names(&tab.folder, &tab.selection.names))
            })
            .unwrap_or(Selection::None);
        ui.preview().set_selection_user(selection);
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
        self.sync_preview_user(window);
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
        self.sync_preview_user(window);
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
        self.sync_preview_user(window);
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
                    let _ = self.launch_file(&path);
                }
            }
            OpenDecision::Missing => {
                crate::log!("open", "{} is gone; nothing to open", path.display())
            }
        }
    }

    /// The one launch path, shared by `file.open` and a double-click. The
    /// command bridge reports the real error; a double-click only logs it.
    pub fn launch_file(&self, path: &Path) -> Result<(), String> {
        let uri = gio::File::for_path(path).uri().to_string();
        match gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>) {
            Ok(()) => {
                crate::log!("open", "launched {}", path.display());
                Ok(())
            }
            Err(error) => {
                crate::log!("open", "could not launch {}: {error}", path.display());
                Err(format!("could not launch {}: {error}", path.display()))
            }
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

    pub fn quit_now(self: &Rc<Self>) {
        self.save_now(true);
        // Owned Poppler children must be killed/reaped while their workers
        // still exist; setting a flag after `application.quit` is too late.
        crate::folder::size::shutdown();
        crate::pdf::shutdown();
        if let Some(application) = &self.application {
            application.quit();
        }
    }

    /// Quit rechecks every current model window before the application leaves.
    /// One request is active at a time; Cancel aborts the whole request. A
    /// window that becomes dirty during another window's question, or is
    /// created dirty meanwhile, is still asked before `quit_now`.
    pub fn request_quit(self: &Rc<Self>) {
        let request = match self.quit.borrow_mut().begin() {
            Some(request) => request,
            None => {
                crate::log!("quit", "a quit request is already waiting for an answer");
                return;
            }
        };
        self.advance_quit(request);
    }

    fn window_activities(&self) -> Vec<(WindowId, Option<fm_core::preview::EditorActivity>)> {
        self.windows
            .borrow()
            .iter()
            .map(|(id, ui)| (*id, ui.preview().activity()))
            .collect()
    }

    fn advance_quit(self: &Rc<Self>, request: u64) {
        let step = self
            .quit
            .borrow_mut()
            .next(request, &self.window_activities());
        match step {
            QuitStep::Inactive => {
                crate::log!("quit", "a stale quit continuation was ignored");
            }
            QuitStep::Finish => {
                // A callback from a cancelled or superseded request never
                // reaches the final quit.
                if self.quit.borrow().is_active(request) {
                    self.quit.borrow_mut().cancel(request);
                    self.quit_now();
                }
            }
            QuitStep::Prompt { window } => {
                let Some(ui) = self.window(window) else {
                    // The window disappeared while the request was active.
                    self.advance_quit(request);
                    return;
                };
                let process = Rc::downgrade(self);
                let proceed: Rc<dyn Fn()> = Rc::new(move || {
                    if let Some(process) = process.upgrade() {
                        process.advance_quit(request);
                    }
                });
                let process = Rc::downgrade(self);
                let cancel: Rc<dyn Fn()> = Rc::new(move || {
                    if let Some(process) = process.upgrade() {
                        process.cancel_quit(request);
                    }
                });
                match ui.preview().guard_close(proceed, cancel) {
                    CloseGuard::Waiting => {}
                    CloseGuard::Free => {
                        // The preview stopped needing attention between the
                        // check and the dialog. Rescan every window instead of
                        // quitting over a stale answer.
                        self.advance_quit(request);
                    }
                    CloseGuard::Busy => {
                        // An ordinary window-close dialog already owns this
                        // panel. Leave it intact and cancel this Quit attempt;
                        // a later Quit starts fresh once that close resolves.
                        crate::log!(
                            "quit",
                            "window {window} already owns a close dialog; quit cancelled"
                        );
                        self.cancel_quit(request);
                    }
                }
            }
        }
    }

    fn cancel_quit(&self, request: u64) {
        if self.quit.borrow().is_active(request) {
            crate::log!("quit", "the pending quit was cancelled");
            self.quit.borrow_mut().cancel(request);
        }
    }

    fn finish(
        self: &Rc<Self>,
        result: Result<Vec<Event>, fm_core::workspace::WorkspaceError>,
    ) -> Outcome {
        match result {
            Ok(events) => {
                if !events.is_empty() {
                    self.apply_events(&events);
                    self.schedule_save();
                }
                Outcome::Done
            }
            Err(error) => {
                crate::log!("workspace", "{error}");
                let message = error.to_string();
                *self.last_failure.borrow_mut() = Some(message.clone());
                Outcome::Failed(message)
            }
        }
    }

    /// Model events drive the UI: one function per event kind.
    pub fn apply_events(self: &Rc<Self>, events: &[Event]) {
        // Headless tests have no widgets to reborrow the workspace during a
        // refresh. Enforce the same reentrant delivery contract there.
        #[cfg(test)]
        assert!(
            self.workspace.try_borrow_mut().is_ok(),
            "release the workspace borrow before delivering model events"
        );
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
        self.quit_now();
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
        if let Some(ui) = self.window(id) {
            ui.preview().release_saved_override();
        }
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
            ui.preview().release_saved_override();
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
        // The one Settings key with a runtime side effect. The registry has
        // already refused every caller but Ui before this handler is reached.
        if command.name() == "agent.set-enabled" {
            let Some(Value::Bool(enabled)) = command.get("enabled") else {
                return Outcome::Failed("agent.set-enabled needs a boolean".to_string());
            };
            let outcome = self.set_agent_enabled(*enabled);
            self.refresh_agent_status_all();
            return outcome;
        }

        let current = self
            .window_snapshot(window)
            .and_then(|window| window.tab(window.focused_tab).map(|tab| tab.folder.clone()))
            .unwrap_or_else(|| self.home());
        let change = {
            let config = self.config.borrow();
            config::change_for(command, config.config(), &current)
        };
        let Some(change) = change else {
            return Outcome::Failed(format!(
                "{}: no config change could be made",
                command.name()
            ));
        };
        let applied = self.config.borrow_mut().apply(change.clone());
        match applied {
            Ok(()) => {
                crate::log!("command", "{} applied", command.name());
                self.after_change(window, &change);
                Outcome::Done
            }
            Err(error) => {
                crate::log!("config", "{error}");
                Outcome::Failed(error.to_string())
            }
        }
    }

    fn after_change(self: &Rc<Self>, window: WindowId, change: &Change) {
        match change {
            Change::SetScale(_) => self.apply_scale_all(),
            Change::SetOpacity(opacity) => {
                if let Some(look) = &self.look {
                    look.set_opacity(*opacity);
                }
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
        if let Some(look) = &self.look {
            look.set_scale(scale);
        }
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
        self.last_failure.borrow_mut().take();
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
                    // Keep the visible explanation, but never report a
                    // synchronous refusal as a successful completion.
                    ui.trash_status(message);
                    crate::log!("trash", "refused {message}");
                    return Outcome::Failed(format!("refused: {message}"));
                }
            }
        }
        // Headless bridge tests have no GTK page; the same route still makes
        // the contextual refusal a typed failure.
        if self.trash_active_for_test(window)
            && let PageRoute::Refuse(message) = route_on_trash_page(command.name(), true)
        {
            crate::log!("trash", "refused {message}");
            return Outcome::Failed(format!("refused: {message}"));
        }

        match command.name() {
            "file.cut" => {
                return self.put_on_clipboard(
                    window,
                    caller,
                    command,
                    clipboard::ClipboardMode::Cut,
                );
            }
            "file.copy-to-clipboard" => {
                return self.put_on_clipboard(
                    window,
                    caller,
                    command,
                    clipboard::ClipboardMode::Copy,
                );
            }
            "file.paste" => {
                self.paste(window, caller, command);
                return Outcome::Done;
            }
            "app.disk-usage" => {
                let path = argument_path(command, "path").or_else(|| self.current_folder(window));
                let Some(path) = path else {
                    return Outcome::Failed("app.disk-usage needs a folder".to_string());
                };
                return match self.launch_disk_usage(&path) {
                    Ok(()) => Outcome::Done,
                    Err(error) => Outcome::Failed(error),
                };
            }
            _ => {}
        }

        if command.name().starts_with("preview.") {
            return self.preview_command(window, command);
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
                return Outcome::Failed("file.open has nothing selected".to_string());
            }
            let mut failures = Vec::new();
            for path in paths {
                if let Err(error) = self.launch_file(&path) {
                    failures.push(error);
                }
            }
            return if failures.is_empty() {
                Outcome::Done
            } else {
                Outcome::Failed(failures.join("; "))
            };
        }
        match command.effect() {
            Effect::Settings => return self.settings_change(window, command),
            Effect::View => {}
            _ => {
                return Outcome::Failed(format!(
                    "{} is not implemented by this version",
                    command.name()
                ));
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
                self.request_quit();
            }
            "window.new" => {
                let result = self.workspace.borrow_mut().window_new(path);
                self.finish(result);
            }
            "window.close" => {
                if let Some(ui) = self.window(window)
                    && !ui.preview().close_allowed()
                {
                    let close_ui = Rc::downgrade(&ui);
                    let proceed: Rc<dyn Fn()> = Rc::new(move || {
                        if let Some(ui) = close_ui.upgrade() {
                            ui.close_now();
                        }
                    });
                    let cancel: Rc<dyn Fn()> = Rc::new(|| {});
                    match ui.preview().guard_close(proceed, cancel) {
                        // An existing dialog owns this close; a repeated
                        // request must never fall through and close the model.
                        CloseGuard::Waiting | CloseGuard::Busy => return Outcome::Done,
                        CloseGuard::Free => {}
                    }
                }
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
                let target = self.take_pending_nav_target().or_else(|| {
                    let path = path?;
                    if path.is_dir() {
                        Some((path, None))
                    } else if path.is_file() {
                        let parent = path
                            .parent()
                            .map(Path::to_path_buf)
                            .unwrap_or_else(|| PathBuf::from("/"));
                        let name = path.file_name().map(std::ffi::OsString::from);
                        Some((parent, name))
                    } else {
                        None
                    }
                });
                let Some((folder, select)) = target else {
                    return Outcome::Failed("the navigation target does not exist".to_string());
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
            _ => {
                return Outcome::Failed(format!(
                    "{} is not implemented by this version",
                    command.name()
                ));
            }
        }
        match self.last_failure.borrow_mut().take() {
            Some(message) => Outcome::Failed(message),
            None => Outcome::Done,
        }
    }

    fn preview_command(self: &Rc<Self>, window: WindowId, command: &CheckedCommand) -> Outcome {
        let Some(ui) = self.window(window) else {
            return Outcome::Done;
        };
        match command.name() {
            "preview.edit" => ui.preview().begin_edit(),
            "preview.discard" => ui.preview().discard(),
            "preview.save" => {
                if command.dry_run() {
                    return Outcome::DryRun(ui.preview().save_plan(None));
                }
                ui.preview().save();
            }
            "preview.save-as" => {
                let Some(path) = argument_path(command, "path") else {
                    crate::log!("preview", "preview.save-as needs a path");
                    return Outcome::Done;
                };
                if command.dry_run() {
                    return Outcome::DryRun(ui.preview().save_plan(Some(&path)));
                }
                ui.preview().save_as(path);
            }
            _ => return Outcome::Done,
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
                    return Some(Outcome::Failed(error));
                }
            }
        } else {
            match self.request_from_command(window, caller, command) {
                Ok(Some(request)) => request,
                Ok(None) => return None,
                Err(error) => {
                    crate::log!("fileops", "{}: {error}", command.name());
                    return Some(Outcome::Failed(error));
                }
            }
        };

        if command.dry_run() {
            // Every plan is built on the worker. The UI only logs the eventual
            // result through `fileop_dry_run`; GTK never plans a file request.
            if let Some(fileops) = self.fileops() {
                let id = fileops.submit_dry_run(window, request, None);
                self.command_job.set(Some(id));
                crate::log!("fileops", "{} dry run queued as job {id}", command.name());
                return Some(Outcome::Done);
            }
            return Some(Outcome::Failed(
                "the file-operation worker is unavailable".to_string(),
            ));
        }

        if let Some(fileops) = self.fileops() {
            if !fileops.can_accept() {
                return Some(Outcome::Failed(
                    "too many file jobs are already waiting".to_string(),
                ));
            }
            let authorization = self.take_pending_authorization();
            let id = fileops.submit(window, request, false, authorization);
            self.command_job.set(Some(id));
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
    ) -> Outcome {
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
            return Outcome::Failed(format!("{} has nothing selected", command.name()));
        }
        let Some(ui) = self.window(window) else {
            return Outcome::Failed("the target window is gone".to_string());
        };
        match clipboard::write(&ui.display(), mode, &paths) {
            Ok(()) => {
                crate::log!(
                    "fileops",
                    "{} put {} item(s) on the clipboard",
                    mode.as_str(),
                    paths.len()
                );
                Outcome::Done
            }
            Err(error) => {
                crate::log!("fileops", "clipboard: {error}");
                Outcome::Failed(error)
            }
        }
    }

    fn paste(self: &Rc<Self>, window: WindowId, caller: Caller, command: &CheckedCommand) {
        let authorization = if caller == Caller::Agent {
            self.take_pending_authorization()
        } else {
            None
        };
        let reserved = if caller == Caller::Agent {
            self.take_reserved_job()
        } else {
            None
        };
        let fail = |message: String| {
            if let Some(token) = &reserved {
                crate::agent::fail_job(self, token, message.clone());
            }
            crate::log!("fileops", "paste: {message}");
        };

        let folder = argument_path(command, "in").or_else(|| {
            if matches!(caller, Caller::Ui | Caller::Hotkey) {
                self.current_folder(window)
            } else {
                None
            }
        });
        let Some(folder) = folder else {
            fail("file.paste has no folder".to_string());
            return;
        };
        if caller == Caller::Agent && authorization.is_none() {
            fail("local automation is off".to_string());
            return;
        }
        let dry_run = command.dry_run();
        // A headless bridge test injects a reader; production passes the real
        // window display. The paste effect targets the command's explicit
        // folder, not a focused tab.
        let display = self.window(window).map(|ui| ui.display());
        let reader = self.clipboard_reader();
        let weak = Rc::downgrade(self);
        reader.read(
            display.as_ref(),
            Box::new(move |result| {
                let Some(process) = weak.upgrade() else {
                    return;
                };
                let fail = |message: String| {
                    if let Some(token) = &reserved {
                        crate::agent::fail_job(&process, token, message.clone());
                    }
                    crate::log!("fileops", "paste: {message}");
                };
                // A callback from an earlier enable cycle has no authority,
                // even if it arrived after another On.
                if !crate::agent::clipboard_callback_allowed(&authorization) {
                    fail(
                        "local automation was turned off before the clipboard read finished"
                            .to_string(),
                    );
                    return;
                }
                let payload = match result {
                    Ok(payload) => payload,
                    Err(error) => {
                        fail(error);
                        return;
                    }
                };
                let paths: Vec<PathBuf> = payload
                    .uris
                    .iter()
                    .filter_map(|uri| gio::File::for_uri(uri).path())
                    .collect();
                if paths.is_empty() {
                    fail("the clipboard has no local files".to_string());
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
                let Some(fileops) = process.fileops() else {
                    fail("the file-operation worker is unavailable".to_string());
                    return;
                };
                if !fileops.can_accept() {
                    fail("too many file jobs are already waiting".to_string());
                    return;
                }
                if dry_run {
                    let id = fileops.submit_dry_run(window, request, authorization.clone());
                    if let Some(token) = &reserved {
                        let running = fileops.current_id() == Some(id);
                        crate::agent::attach_fileops(&process, token, id, running);
                    }
                    crate::log!("fileops", "paste dry run queued as job {id}");
                    return;
                }
                let id = fileops.submit(
                    window,
                    request,
                    clipboard::clear_after_cut(payload.mode),
                    authorization.clone(),
                );
                if let Some(token) = &reserved {
                    let running = fileops.current_id() == Some(id);
                    crate::agent::attach_fileops(&process, token, id, running);
                }
                crate::log!("fileops", "paste ({}): job {id}", payload.mode.as_str());
            }),
        );
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

    pub fn fileop_started(&self, window: WindowId, id: u64) {
        crate::agent::job_started(self, id);
        if let Some(ui) = self.window(window) {
            ui.fileop_started();
            ui.trash_job_started();
        }
    }

    pub fn fileop_progress(&self, window: WindowId, id: u64, view: Option<ProgressView>) {
        crate::agent::job_started(self, id);
        if let Some(ui) = self.window(window) {
            ui.fileop_progress(view.clone());
            ui.trash_progress(view);
        }
    }

    pub fn fileop_dry_run(&self, window: WindowId, id: u64, request: &Request, lines: &[String]) {
        crate::agent::job_dry_run(self, id, lines.to_vec());
        crate::log!("fileops", "dry run: {}", lines.join(" | "));
        if let Some(ui) = self.window(window) {
            ui.fileop_progress(None);
            ui.trash_dry_run(request, lines);
        }
    }

    /// A revoked agent job never reached execution. The socket result is
    /// terminal, while the UI simply stops showing a job it must not run.
    pub fn fileop_revoked(&self, window: WindowId, id: u64, request: &Request, message: &str) {
        crate::agent::job_revoked(self, id, message.to_string());
        crate::log!("fileops", "job {id} revoked before execution: {message}");
        if let Some(ui) = self.window(window) {
            ui.fileop_progress(None);
            ui.trash_progress(None);
            let _ = request;
        }
    }

    pub fn fileop_empty_confirmation(&self, window: WindowId, id: u64, plan: Plan) {
        crate::agent::job_waiting(self, id);
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
        id: u64,
        request: &Request,
        plan: &Plan,
        report: &Report,
    ) {
        // A known file operation can change any ancestor's total. Invalidate
        // conservatively; the next hover recalculates on demand.
        crate::folder::size::invalidate_all();
        let summary = if *request == Request::Undo {
            progress::undo_status(plan, report)
        } else {
            progress::summary(request, report)
        };
        let failures = progress::failure_lines(report);
        crate::agent::job_finished(
            self,
            id,
            summary.clone(),
            report.lines(),
            failures.clone(),
            report.cancelled,
        );
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

    pub fn fileop_refused(&self, window: WindowId, id: u64, request: &Request, message: &str) {
        crate::agent::job_failed(self, id, message.to_string());
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
        id: u64,
        flow: fileops::conflict::ConflictFlow,
        done: Box<dyn Fn(Decisions)>,
    ) {
        crate::agent::job_waiting(self, id);
        if let Some(ui) = self.window(window) {
            dialogs::show_conflicts(&ui, flow, Rc::from(done));
        }
    }

    fn launch_disk_usage(&self, path: &Path) -> Result<(), String> {
        let Some(executable) = menus::find_on_path("baobab", std::env::var_os("PATH").as_deref())
        else {
            return Err("the disk-usage analyser (baobab) is not on PATH".to_string());
        };
        match std::process::Command::new(executable).arg(path).spawn() {
            Ok(_) => {
                crate::log!("disk-usage", "launched baobab for {}", path.display());
                Ok(())
            }
            Err(error) => {
                let message = format!("could not launch baobab for {}: {error}", path.display());
                crate::log!("disk-usage", "{message}");
                Err(message)
            }
        }
    }

    pub fn schedule_save(self: &Rc<Self>) {
        if self.headless {
            return;
        }
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

/// A window needs a human answer when a dialog owns it, or when the preview
/// has unsaved edits or an active save. The preview close bypass flag is not a
/// prompt predicate: it starts false in every clean window.
pub fn prompt_pending(
    windows: impl IntoIterator<Item = (bool, Option<fm_core::preview::EditorActivity>)>,
) -> bool {
    windows.into_iter().any(|(dialog, activity)| {
        dialog || activity.is_some_and(|activity| activity.dirty || activity.saving)
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::{CloseGuard, close_guard_decision};
    use fm_core::preview::{
        EditorActivity, EditorIdentity, FileRevision, LineEnding, Loaded, PreviewContent,
        PreviewDocument, PreviewSession, SaveCompletion, Selection, TextDocument, TextEncoding,
        WriteAccess,
    };

    fn revision() -> FileRevision {
        FileRevision {
            device: 1,
            inode: 2,
            size: 3,
            modified_sec: 4,
            modified_nsec: 0,
            changed_sec: 4,
            changed_nsec: 0,
            etag: None,
        }
    }

    fn activity(editor: u64, buffer: u64, dirty: bool, saving: bool) -> EditorActivity {
        EditorActivity {
            identity: EditorIdentity {
                editor_id: editor,
                path: PathBuf::from("/tmp/a.txt"),
                revision: revision(),
                buffer_revision: buffer,
            },
            dirty,
            saving,
        }
    }

    #[test]
    fn clean_close_and_quit_need_no_human_prompt() {
        assert!(!prompt_pending([]));
        assert!(!prompt_pending([(false, None)]));
        assert!(!prompt_pending([(
            false,
            Some(activity(1, 2, false, false))
        )]));
    }

    #[test]
    fn dirty_or_saving_or_dialog_windows_block_close_and_quit() {
        assert!(prompt_pending([(false, Some(activity(1, 2, true, false)))]));
        assert!(prompt_pending([(false, Some(activity(1, 2, false, true)))]));
        assert!(prompt_pending([(true, None)]));
        assert!(!prompt_pending([
            (false, Some(activity(1, 2, false, false))),
            (false, None),
        ]));
    }

    fn window(number: u64) -> WindowId {
        WindowId::from_raw(number)
    }

    fn prompt(window: WindowId) -> QuitStep {
        QuitStep::Prompt { window }
    }

    fn preview_document(path: &str, text: &str) -> PreviewDocument {
        PreviewDocument {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            size: text.len() as u64,
            revision: revision(),
            access: WriteAccess {
                file_writable: true,
                parent_writable: true,
                reason: None,
            },
            content: PreviewContent::Text(TextDocument {
                text: text.to_string(),
                encoding: TextEncoding {
                    bom: false,
                    line_ending: Some(LineEnding::Lf),
                },
            }),
        }
    }

    fn dirty_preview(path: &str) -> PreviewSession {
        let mut session = PreviewSession::new();
        let ticket = session.select(Selection::file(path)).unwrap();
        session.load_finished(&ticket, Loaded::Document(preview_document(path, "A")));
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        session
    }

    #[test]
    fn quit_rechecks_a_clean_window_dirtied_during_another_prompt() {
        let (a, b) = (window(1), window(2));
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, None), (b, None)]),
            QuitStep::Finish
        );
        quit.cancel(request);

        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(
                request,
                &[(a, Some(activity(1, 1, true, false))), (b, None)]
            ),
            prompt(a)
        );
        // B was clean when the request started and is dirty by the time A's
        // question is answered.
        assert_eq!(
            quit.next(
                request,
                &[(a, None), (b, Some(activity(2, 1, true, false)))]
            ),
            prompt(b)
        );
        assert_eq!(
            quit.next(request, &[(a, None), (b, None)]),
            QuitStep::Finish
        );
        quit.cancel(request);
    }

    #[test]
    fn a_handled_window_edited_again_is_asked_again() {
        let a = window(1);
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 2, true, false)))]),
            prompt(a)
        );
        assert_eq!(quit.next(request, &[(a, None)]), QuitStep::Finish);
        quit.cancel(request);
    }

    #[test]
    fn a_newly_created_dirty_window_joins_a_pending_quit() {
        let (a, c) = (window(1), window(3));
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        assert_eq!(
            quit.next(
                request,
                &[
                    (a, Some(activity(1, 1, true, false))),
                    (c, Some(activity(3, 1, true, false))),
                ]
            ),
            prompt(c)
        );
        quit.cancel(request);
    }

    #[test]
    fn repeated_quit_and_cancel_are_scoped_to_one_request() {
        let a = window(1);
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert!(
            quit.begin().is_none(),
            "a second Quit must not start a dialog"
        );
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        quit.cancel(request);
        assert!(!quit.is_active(request));
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            QuitStep::Inactive
        );
        let later = quit.begin().expect("a later Quit starts fresh");
        assert_ne!(later, request);
        quit.cancel(later);
    }

    #[test]
    fn final_quit_requires_no_dirty_or_saving_window() {
        let a = window(1);
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        // Handled but still dirty: never finish silently.
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        // An active save is prompted so the continuation binds to that job.
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, true)))]),
            prompt(a)
        );
        // A clean editor is not a prompt.
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, false, false)))]),
            QuitStep::Finish
        );
        quit.cancel(request);
    }

    #[test]
    fn a_save_failure_cancels_the_whole_pending_quit() {
        let (a, b) = (window(1), window(2));
        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        // The panel calls the cancel closure when its save fails.
        quit.cancel(request);
        assert_eq!(
            quit.next(
                request,
                &[
                    (a, Some(activity(1, 1, true, false))),
                    (b, Some(activity(2, 1, true, false))),
                ]
            ),
            QuitStep::Inactive,
            "a cancelled request does not keep prompting"
        );
    }

    #[test]
    fn a_quit_does_not_take_over_an_existing_window_close_request() {
        let (a, b) = (window(1), window(2));
        let session = RefCell::new(dirty_preview("/tmp/a.txt"));
        assert_eq!(
            close_guard_decision(&session, false),
            CloseGuard::Waiting,
            "A's ordinary window close owns the panel"
        );

        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, session.borrow().activity()), (b, None)]),
            prompt(a)
        );
        // Quit reaches A, but must not overwrite A's dialog or continuation.
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Busy);
        quit.cancel(request);
        assert!(!quit.is_active(request));
        assert!(
            session.borrow().has_close(),
            "A's close request must remain intact"
        );
        assert!(session.borrow().is_dirty());
    }

    #[test]
    fn quit_works_again_after_an_existing_close_is_cancelled() {
        let (a, b) = (window(1), window(2));
        let session = RefCell::new(dirty_preview("/tmp/a.txt"));
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Waiting);
        session.borrow_mut().invalidate_close();

        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, session.borrow().activity()), (b, None)]),
            prompt(a)
        );
        assert_eq!(
            close_guard_decision(&session, false),
            CloseGuard::Waiting,
            "Quit may arm its own dialog once A's close is cancelled"
        );
        quit.cancel(request);
    }

    #[test]
    fn quit_works_again_after_an_existing_close_is_saved() {
        let (a, b) = (window(1), window(2));
        let session = RefCell::new(dirty_preview("/tmp/a.txt"));
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Waiting);
        let save = session.borrow_mut().begin_save_original().unwrap();
        session.borrow_mut().bind_close_to_save(&save);
        assert_eq!(
            session
                .borrow_mut()
                .save_finished(&save, preview_document("/tmp/a.txt", "saved")),
            SaveCompletion::Applied
        );
        assert!(session.borrow().has_close());
        assert!(session.borrow_mut().take_close().is_some());

        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, None), (b, None)]),
            QuitStep::Finish
        );
        quit.cancel(request);
    }

    #[test]
    fn quit_works_again_after_an_existing_close_is_discarded() {
        let (a, b) = (window(1), window(2));
        let session = RefCell::new(dirty_preview("/tmp/a.txt"));
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Waiting);
        let close = session.borrow().close_request().cloned().unwrap();
        assert!(session.borrow_mut().discard_preserving_close().is_some());
        assert!(session.borrow().close_ready(&close));
        assert!(session.borrow_mut().take_close().is_some());

        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, None), (b, None)]),
            QuitStep::Finish
        );
        quit.cancel(request);
    }

    #[test]
    fn a_window_close_save_failure_clears_its_continuation_and_allows_a_new_quit() {
        let (a, b) = (window(1), window(2));
        let session = RefCell::new(dirty_preview("/tmp/a.txt"));
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Waiting);
        let save = session.borrow_mut().begin_save_original().unwrap();
        session.borrow_mut().bind_close_to_save(&save);
        assert!(session.borrow_mut().save_failed(&save, "disk full"));
        assert!(!session.borrow().has_close());

        let mut quit = QuitCoordinator::default();
        let request = quit.begin().unwrap();
        assert_eq!(
            quit.next(request, &[(a, session.borrow().activity()), (b, None)]),
            prompt(a)
        );
        // A is still dirty, but its failed close no longer blocks a new Quit.
        assert_eq!(close_guard_decision(&session, false), CloseGuard::Waiting);
        quit.cancel(request);
    }

    #[test]
    fn stale_quit_continuations_cannot_finish_a_cancelled_or_new_request() {
        let a = window(1);
        let mut quit = QuitCoordinator::default();
        let first = quit.begin().unwrap();
        assert_eq!(
            quit.next(first, &[(a, Some(activity(1, 1, true, false)))]),
            prompt(a)
        );
        quit.cancel(first);
        assert_eq!(
            quit.next(first, &[(a, None)]),
            QuitStep::Inactive,
            "a cancelled request's continuation is a no-op"
        );

        let second = quit.begin().unwrap();
        assert_ne!(first, second);
        assert_eq!(
            quit.next(first, &[(a, None)]),
            QuitStep::Inactive,
            "the old request cannot finish the new one"
        );
        assert_eq!(quit.next(second, &[(a, None)]), QuitStep::Finish);
        quit.cancel(second);
    }
}
