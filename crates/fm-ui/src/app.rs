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
#[cfg(test)]
use crate::search::SearchView;
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
    /// command handler instead of a GTK-thread `exists()` call. Search
    /// activation adds the exact text the registry validated and the caller it
    /// is trusted for, so the payload can never replace a different command.
    pending_nav: RefCell<Option<PendingNav>>,
    /// The first real model failure in the current command dispatch.
    last_failure: RefCell<Option<String>>,
    /// Process-wide search sessions and the fixed two-worker pool.
    search: Rc<crate::search::SearchRuntime>,
    search_poll: RefCell<Option<glib::SourceId>>,
    /// True while the search timer's own callback is draining events, so a
    /// close from inside it cannot remove the source that is currently firing.
    search_in_poll: Cell<bool>,
    /// The injectable clipboard reader. Production uses the GTK clipboard;
    /// tests can pause or fail a read without touching Kyle's clipboard.
    clipboard_reader: Rc<dyn crate::fileops::clipboard::Reader>,
    /// Headless bridge tests skip GTK window construction and session saves.
    headless: bool,
    /// Test seam for the Trash page's contextual refusal. Production always
    /// reads the real window's page state.
    #[cfg(test)]
    trash_pages: RefCell<BTreeSet<WindowId>>,
    /// Test seam: how many `nav.go` dispatches consumed trusted native-path
    /// evidence after registry validation.
    #[cfg(test)]
    native_nav_evidence_uses: Cell<usize>,
}

/// Trusted out-of-band navigation evidence. `expected` and `caller` are set for
/// search activation: the handler only uses the payload when the registry
/// accepted exactly this text path for exactly this caller. The existing
/// worker evidence for the agent leaves both unset, preserving its behaviour.
struct PendingNav {
    expected: Option<String>,
    caller: Option<Caller>,
    folder: PathBuf,
    select: Option<OsString>,
}

impl PendingNav {
    fn matches(&self, caller: Caller, command: &CheckedCommand) -> bool {
        match (&self.expected, self.caller) {
            (Some(expected), Some(expected_caller)) => {
                expected_caller == caller
                    && matches!(command.get("path"), Some(Value::Path(path)) if path.to_str() == Some(expected.as_str()))
            }
            _ => true,
        }
    }
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
            pending_nav: RefCell::new(None),
            last_failure: RefCell::new(None),
            search: Rc::new(crate::search::SearchRuntime::production()),
            search_poll: RefCell::new(None),
            search_in_poll: Cell::new(false),
            clipboard_reader: Rc::new(crate::fileops::clipboard::SystemReader),
            headless: false,
            #[cfg(test)]
            trash_pages: RefCell::new(BTreeSet::new()),
            #[cfg(test)]
            native_nav_evidence_uses: Cell::new(0),
        });
        let fileops = FileOps::new(&process);
        *process.fileops.borrow_mut() = Some(fileops);
        let weak = Rc::downgrade(&process);
        process.search.set_wake(move || {
            if let Some(process) = weak.upgrade() {
                process.ensure_search_poll();
            }
        });
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
        Self::headless_with_search(
            config_path,
            home,
            clipboard_reader,
            Rc::new(crate::search::SearchRuntime::production()),
        )
    }

    #[cfg(test)]
    pub(crate) fn headless_with_search(
        config_path: PathBuf,
        home: PathBuf,
        clipboard_reader: Rc<dyn crate::fileops::clipboard::Reader>,
        search: Rc<crate::search::SearchRuntime>,
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
            pending_nav: RefCell::new(None),
            last_failure: RefCell::new(None),
            search,
            search_poll: RefCell::new(None),
            search_in_poll: Cell::new(false),
            clipboard_reader,
            headless: true,
            trash_pages: RefCell::new(BTreeSet::new()),
            native_nav_evidence_uses: Cell::new(0),
        });
        let fileops = FileOps::new_manual(&process);
        *process.fileops.borrow_mut() = Some(fileops);
        let weak = Rc::downgrade(&process);
        process.search.set_wake(move || {
            if let Some(process) = weak.upgrade() {
                process.ensure_search_poll();
            }
        });
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
        self.pending_nav.borrow_mut().take();
        self.reserved_job.borrow_mut().take();
    }

    pub(crate) fn set_pending_nav_target(&self, target: Option<(PathBuf, Option<OsString>)>) {
        *self.pending_nav.borrow_mut() = target.map(|(folder, select)| PendingNav {
            expected: None,
            caller: None,
            folder,
            select,
        });
    }

    /// Bind the exact native target to the text the registry is about to
    /// validate. Only trusted UI code calls this, immediately around the one
    /// `nav.go` dispatch.
    fn set_pending_nav_evidence(
        &self,
        text: String,
        folder: PathBuf,
        select: Option<OsString>,
        caller: Caller,
    ) {
        *self.pending_nav.borrow_mut() = Some(PendingNav {
            expected: Some(text),
            caller: Some(caller),
            folder,
            select,
        });
    }

    fn take_pending_nav(&self) -> Option<PendingNav> {
        self.pending_nav.borrow_mut().take()
    }

    #[cfg(test)]
    pub(crate) fn set_pending_nav_evidence_for_tests(
        &self,
        text: String,
        folder: PathBuf,
        select: Option<OsString>,
        caller: Caller,
    ) {
        self.set_pending_nav_evidence(text, folder, select, caller);
    }

    #[cfg(test)]
    pub(crate) fn native_nav_evidence_uses_for_tests(&self) -> usize {
        self.native_nav_evidence_uses.get()
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
        self.search.close(id);
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
        self.search.close(id);
        self.stop_search_poll_if_idle();
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
        let outcome = self.dispatch_with_outcome(window, caller, &invocation);
        log_outcome(&invocation, &outcome);
    }

    /// The same production path as [`Self::dispatch_for_window`], returning
    /// the registry's outcome for a caller that must react to acceptance.
    fn dispatch_with_outcome(
        self: &Rc<Self>,
        window: WindowId,
        caller: Caller,
        invocation: &Invocation,
    ) -> Outcome {
        self.focus_model_window(window);
        let mut handler = commands::UiHandler {
            caller,
            window,
            process: Rc::downgrade(self),
        };
        Registry::new().dispatch(caller, invocation, &mut handler)
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

    // -----------------------------------------------------------------------
    // Search routing
    // -----------------------------------------------------------------------

    /// The exact tab and folder a new search captures. Split tiles share one
    /// focused tab, so this is only ever the focused tile's folder.
    fn search_target(&self, window: WindowId) -> Option<(TabId, PathBuf)> {
        let window = self.window_snapshot(window)?;
        let tab = window.tab(window.focused_tab)?;
        Some((tab.id, tab.folder.clone()))
    }

    fn ensure_search_poll(self: &Rc<Self>) {
        if self.headless || self.search_poll.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(
            std::time::Duration::from_millis(crate::search::SEARCH_POLL_MS),
            move || {
                let Some(process) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                process.search_in_poll.set(true);
                let active = process.poll_search_all();
                process.search_in_poll.set(false);
                if active && process.search.needs_poll() {
                    glib::ControlFlow::Continue
                } else {
                    process.search_poll.borrow_mut().take();
                    glib::ControlFlow::Break
                }
            },
        );
        *self.search_poll.borrow_mut() = Some(source);
    }

    fn stop_search_poll_if_idle(&self) {
        if self.search_in_poll.get() {
            return;
        }
        if self.search.session_count() == 0
            && let Some(source) = self.search_poll.borrow_mut().take()
        {
            source.remove();
        }
    }

    fn poll_search_all(self: &Rc<Self>) -> bool {
        for window in self.search.sessions() {
            self.poll_search_window(window);
        }
        self.search.session_count() > 0
    }

    fn poll_search_window(self: &Rc<Self>, window: WindowId) {
        let tick = self.search.poll(window);
        if let Some(reason) = tick.revoked {
            // The completed or partial root disappeared. Revoke the session
            // (pending activation, batches and monitors are already dropped),
            // restore ordinary content and explain the removal.
            crate::log!(
                "search",
                "root lifetime revoked the page: {}",
                reason.label()
            );
            self.close_search(window);
            if let Some(ui) = self.window(window) {
                ui.show_summary(&format!("Search unavailable — {}.", reason.label()), None);
            }
            return;
        }
        let missing_root = tick.terminal
            && self.search.view(window).is_some_and(|view| {
                matches!(
                    view.phase,
                    crate::search::SearchPhase::Unavailable(fm_core::search::RootFailure::Missing)
                )
            });
        if missing_root {
            // The root disappeared: no late batch may revive the page.
            self.close_search(window);
            if let Some(ui) = self.window(window) {
                ui.show_summary("Search unavailable — the folder no longer exists.", None);
            }
            return;
        }
        if let Some(ui) = self.window(window) {
            if !tick.alive {
                ui.search_revoked();
            } else {
                // A failed root clears the retained rows; do not repaint a
                // batch from earlier in the same tick under an unavailable page.
                let unavailable = tick.terminal
                    && self.search.view(window).is_some_and(|view| {
                        matches!(view.phase, crate::search::SearchPhase::Unavailable(_))
                    });
                if !unavailable && !tick.changes.is_empty() {
                    ui.search_changes(&tick.changes);
                }
                if tick.changed
                    && let Some(view) = self.search.view(window)
                {
                    ui.search_status(&view);
                }
            }
        }
        if let Some(ready) = tick.activation {
            self.finish_search_activation(window, ready);
        }
    }

    /// Cancel and remove a window's session, then restore ordinary content.
    /// This is a transient view lifecycle operation, not a new command.
    pub fn close_search(self: &Rc<Self>, window: WindowId) {
        let closed = self.search.close(window);
        if let Some(ui) = self.window(window) {
            ui.search_revoked();
        }
        if closed {
            self.stop_search_poll_if_idle();
        }
    }

    /// Clearing the entry cancels the session but leaves the entry open for a
    /// new query. No new command is involved.
    pub fn cancel_search(self: &Rc<Self>, window: WindowId) {
        let closed = self.search.close(window);
        if let Some(ui) = self.window(window) {
            ui.search_cleared();
        }
        if closed {
            self.stop_search_poll_if_idle();
        }
    }

    fn restart_search(self: &Rc<Self>, window: WindowId) {
        let Some(result) = self.search.restart(window) else {
            self.close_search(window);
            return;
        };
        match result {
            Ok(view) => {
                if let Some(ui) = self.window(window) {
                    ui.begin_search(&view);
                }
                self.ensure_search_poll();
            }
            Err(message) => {
                if let Some(ui) = self.window(window) {
                    ui.show_summary(&message, None);
                }
                crate::log!("search", "{message}");
            }
        }
    }

    /// A selected result asks the existing preview for its exact path. It does
    /// not touch the model's hidden folder selection and never enables editing.
    pub(crate) fn search_preview(self: &Rc<Self>, window: WindowId, generation: u64, index: usize) {
        let Some(selection) = self.search_selection(window, generation, index) else {
            return;
        };
        self.search.clear_notice(window);
        if let Some(ui) = self.window(window) {
            if let Some(view) = self.search.view(window) {
                ui.search_status(&view);
            }
            ui.preview().set_selection_user(selection);
        }
    }

    fn search_selection(
        &self,
        window: WindowId,
        generation: u64,
        index: usize,
    ) -> Option<Selection> {
        let hit = self.search.result(window, generation, index)?;
        Some(Selection::file(hit.path))
    }

    /// Enter or double-click: start a bounded worker lookup for the captured
    /// window/tab/root/generation and exact native path. No GTK-thread metadata
    /// lookup occurs, and no navigation starts until the worker returns.
    pub(crate) fn search_activate(
        self: &Rc<Self>,
        window: WindowId,
        generation: u64,
        index: usize,
    ) {
        let Some(hit) = self.search.result(window, generation, index) else {
            return;
        };
        let Some(snapshot) = self.window_snapshot(window) else {
            self.close_search(window);
            return;
        };
        match self
            .search
            .begin_activation(window, generation, hit.path, &snapshot)
        {
            crate::search::ActivationStart::Started => {
                self.ensure_search_poll();
            }
            crate::search::ActivationStart::AlreadyPending => {}
            crate::search::ActivationStart::Busy(message) => {
                crate::log!("search", "{message}");
                self.search.notice(window, &message);
                if let Some(ui) = self.window(window)
                    && let Some(view) = self.search.view(window)
                {
                    ui.search_status(&view);
                }
            }
            crate::search::ActivationStart::Revoked => {
                if let Some(ui) = self.window(window) {
                    ui.search_revoked();
                }
                self.stop_search_poll_if_idle();
            }
        }
    }

    /// A worker finished the activation lookup. Revalidate the complete model
    /// target and session, then dispatch the existing `nav.go` through the real
    /// registry. The exact native target travels as one-shot typed evidence
    /// bound to the validated text and `Caller::Ui`.
    fn finish_search_activation(
        self: &Rc<Self>,
        window: WindowId,
        ready: crate::search::ActivationReady,
    ) {
        let path = ready.path.clone();
        let Some(snapshot) = self.window_snapshot(window) else {
            self.close_search(window);
            return;
        };
        if !self
            .search
            .activation_valid(window, ready.generation, &ready.path, &snapshot)
        {
            self.close_search(window);
            if let Some(ui) = self.window(window) {
                ui.search_revoked();
            }
            return;
        }
        let target = match ready.target {
            Ok(target) => target,
            Err(message) => {
                let text = format!("Search result unavailable: {message}");
                self.search.notice(window, &text);
                if let Some(ui) = self.window(window)
                    && let Some(view) = self.search.view(window)
                {
                    ui.search_status(&view);
                }
                return;
            }
        };
        let crate::search::NativeNavTarget { folder, select } = target;
        let same_folder = snapshot
            .tab(snapshot.focused_tab)
            .is_some_and(|tab| tab.folder == folder);
        let selected_name = select.clone();
        let text = path
            .to_str()
            .map(str::to_string)
            .unwrap_or_else(|| crate::search::NATIVE_NAV_PLACEHOLDER.to_string());
        self.set_pending_nav_evidence(text.clone(), folder.clone(), select, Caller::Ui);
        let outcome = self.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new("nav.go", vec![("path".to_string(), text)]),
        );
        // The handler consumes the evidence only after registry acceptance. A
        // refusal must leave no latent target for a later command.
        self.take_pending_nav();
        match outcome {
            Outcome::Done => {
                // The model's same-folder nav.go is a no-op; select the exact
                // file by name after the accepted navigation. A symlink that
                // resolves to a directory navigates elsewhere, so it is not
                // selected here.
                let landed = self.window_snapshot(window).and_then(|window| {
                    window.tab(window.focused_tab).map(|tab| tab.folder.clone())
                });
                if same_folder
                    && let Some(name) = selected_name
                    && landed.as_deref() == Some(folder.as_path())
                {
                    self.file_click_tab(
                        window,
                        Some(snapshot.focused_tab),
                        ClickRequest::Plain(name),
                    );
                }
                self.close_search(window);
            }
            Outcome::Failed(message) => {
                let text = format!("{message}: {}", path.display());
                self.search.notice(window, &text);
                if let Some(ui) = self.window(window)
                    && let Some(view) = self.search.view(window)
                {
                    ui.search_status(&view);
                }
            }
            Outcome::Error(error) => {
                let text = format!("Could not open {}: {error}", path.display());
                self.search.notice(window, &text);
                if let Some(ui) = self.window(window)
                    && let Some(view) = self.search.view(window)
                {
                    ui.search_status(&view);
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    pub(crate) fn search_selection_for_tests(
        &self,
        window: WindowId,
        generation: u64,
        index: usize,
    ) -> Option<Selection> {
        self.search_selection(window, generation, index)
    }

    #[cfg(test)]
    pub(crate) fn poll_search_for_tests(self: &Rc<Self>) {
        self.poll_search_all();
    }

    #[cfg(test)]
    pub(crate) fn search_view_for_tests(&self, window: WindowId) -> Option<SearchView> {
        self.search.view(window)
    }

    #[cfg(test)]
    pub(crate) fn search_root_event_for_tests(&self, window: WindowId) {
        self.search.root_changed(window);
    }

    #[cfg(test)]
    pub(crate) fn search_monitor_count_for_tests(&self) -> usize {
        self.search.monitor_count_for_tests()
    }

    #[cfg(test)]
    pub(crate) fn search_result_paths_for_tests(&self, window: WindowId) -> Vec<PathBuf> {
        let Some(view) = self.search.view(window) else {
            return Vec::new();
        };
        (0..view.rows)
            .filter_map(|index| self.search.result(window, view.generation, index))
            .map(|hit| hit.path)
            .collect()
    }

    pub fn quit_now(self: &Rc<Self>) {
        self.save_now(true);
        // Revoke monitors, pending activations and root checks before the app
        // leaves. A blocked native lookup is never joined.
        self.search.shutdown();
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
        let valid = match self.window_snapshot(id) {
            Some(window) => self.search.validate(id, &window),
            None => {
                self.search.close(id);
                false
            }
        };
        if let Some(ui) = self.window(id) {
            if !valid {
                ui.search_revoked();
            }
            if !self.suppress_header.get() {
                ui.refresh_header(fade);
            }
            ui.update_content();
        }
        if !valid {
            self.stop_search_poll_if_idle();
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

        // The read-only results page owns these actions. Nothing here may fall
        // through to the hidden folder selection or a preview buffer.
        let search_active = self.window(window).is_some_and(|ui| ui.search_active())
            || self.search.view(window).is_some();
        if search_active && let Some(message) = crate::search::refusal_for(command.name()) {
            self.search.notice(window, message);
            if let Some(ui) = self.window(window) {
                ui.search_notice(message);
            }
            return Outcome::Failed(message.to_string());
        }

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
                // Search activation and the agent's worker-resolved path
                // arrive as one-shot typed evidence. Search additionally binds
                // it to this caller and to the exact text the registry just
                // validated; a mismatched payload fails instead of falling
                // back to a different path or a GTK-thread lookup.
                let target = if let Some(evidence) = self.take_pending_nav() {
                    if !evidence.matches(caller, command) {
                        return Outcome::Failed(
                            "the navigation target changed before it was opened".to_string(),
                        );
                    }
                    #[cfg(test)]
                    self.native_nav_evidence_uses
                        .set(self.native_nav_evidence_uses.get() + 1);
                    Some((evidence.folder, evidence.select))
                } else {
                    let Some(path) = path else {
                        return Outcome::Failed("the navigation target does not exist".to_string());
                    };
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
                };
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
                let search_visible = self.window(window).is_some_and(|ui| ui.search_active());
                if search_visible || self.search.view(window).is_some() {
                    self.restart_search(window);
                    crate::log!("search", "nav.refresh restarted the visible query");
                } else if let Some(ui) = self.window(window) {
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
                let ui = self.window(window);
                if let Some(query) = text {
                    if let Some(ui) = &ui {
                        ui.open_search(Some(&query));
                    }
                    let Some((tab, root)) = self.search_target(window) else {
                        let message = "There is no folder to search.".to_string();
                        if let Some(ui) = &ui {
                            ui.show_summary(&message, None);
                        }
                        return Outcome::Failed(message);
                    };
                    match self.search.submit(window, tab, root, query) {
                        Ok(view) => {
                            crate::log!(
                                "search",
                                "submission generation {} window {} tab {:?} root {} query {} scalar(s)",
                                view.generation,
                                window,
                                tab,
                                view.root.display(),
                                view.query.chars().count()
                            );
                            if let Some(ui) = &ui {
                                ui.begin_search(&view);
                            }
                            self.ensure_search_poll();
                        }
                        Err(message) => {
                            if let Some(ui) = &ui {
                                ui.show_summary(&message, None);
                            }
                            crate::log!("search", "{message}");
                            return Outcome::Failed(message);
                        }
                    }
                } else if let Some(ui) = &ui {
                    ui.open_search(None);
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
                self.close_search(window);
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

fn log_outcome(invocation: &Invocation, outcome: &Outcome) {
    match outcome {
        Outcome::Error(error) => crate::log!("command", "{error}"),
        Outcome::Failed(message) => {
            crate::log!("command", "{}: {message}", invocation.name());
        }
        Outcome::DryRun(lines) => crate::log!(
            "command",
            "{} dry run: {} lines",
            invocation.name(),
            lines.len()
        ),
        Outcome::Done => {}
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::{CloseGuard, close_guard_decision};
    use fm_core::preview::{
        EditorActivity, EditorIdentity, FileRevision, LineEnding, Loaded, PreviewContent,
        PreviewDocument, PreviewSession, SaveCompletion, Selection, TextDocument, TextEncoding,
        WriteAccess,
    };
    use std::sync::Arc;

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

    // -----------------------------------------------------------------------
    // Search routing
    // -----------------------------------------------------------------------

    struct SearchScratch {
        root: PathBuf,
    }

    impl Drop for SearchScratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    struct NoClipboard;

    impl crate::fileops::clipboard::Reader for NoClipboard {
        fn read(
            &self,
            _display: Option<&gtk::gdk::Display>,
            done: Box<dyn FnOnce(Result<crate::fileops::clipboard::ClipboardPayload, String>)>,
        ) {
            done(Err("the routing test has no clipboard".to_string()));
        }
    }

    fn search_scratch(tag: &str) -> SearchScratch {
        let root = PathBuf::from(format!(
            "/tmp/gimbal-search-ui-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("home")).expect("scratch home");
        SearchScratch { root }
    }

    fn headless_search_process(scratch: &SearchScratch) -> Rc<Process> {
        // Each headless process owns its runtime. Production has one process
        // and therefore one runtime, but parallel tests must not compete for a
        // shared evidence result channel.
        headless_search_process_with(
            scratch,
            Rc::new(crate::search::SearchRuntime::with_service(Arc::new(
                crate::search::SearchService::new(),
            ))),
        )
    }

    /// A real GTK widget process for the entry-to-visible-results regression.
    /// It constructs the same window, header, content stack, list store and
    /// factory as production. The test owns the GLib context and never runs or
    /// registers the `GApplication`.
    fn gtk_search_process(
        scratch: &SearchScratch,
        application: &adw::Application,
        runtime: Rc<crate::search::SearchRuntime>,
    ) -> Rc<Process> {
        let path = scratch.root.join("config.toml");
        let (config_file, _problems) = ConfigFile::load(&path);
        let config = config_file.config().clone();
        let process = Rc::new(Process {
            application: Some(application.clone()),
            config: Rc::new(RefCell::new(config_file)),
            look: Some(Look::new(
                config.look.base * config.look.scale,
                config.look.opacity,
                Some(config.look.font.clone()).filter(|font| !font.is_empty()),
            )),
            workspace: RefCell::new(Workspace::with_home(scratch.root.join("home"))),
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
            agent: RefCell::new(crate::agent::AgentRuntime::new("gtk-test".to_string())),
            agent_poll: RefCell::new(None),
            command_job: Cell::new(None),
            reserved_job: RefCell::new(None),
            pending_authorization: RefCell::new(None),
            pending_runs: RefCell::new(HashMap::new()),
            next_work_id: Cell::new(0),
            pending_nav: RefCell::new(None),
            last_failure: RefCell::new(None),
            search: runtime,
            search_poll: RefCell::new(None),
            search_in_poll: Cell::new(false),
            clipboard_reader: Rc::new(NoClipboard),
            headless: false,
            trash_pages: RefCell::new(BTreeSet::new()),
            native_nav_evidence_uses: Cell::new(0),
        });
        *process.fileops.borrow_mut() = Some(FileOps::new_manual(&process));
        let weak = Rc::downgrade(&process);
        process.search.set_wake(move || {
            if let Some(process) = weak.upgrade() {
                process.ensure_search_poll();
            }
        });
        process
    }

    /// Build the real window without presenting it or dispatching through
    /// `GApplication`. This is the production widget tree with an owned
    /// `GMainContext`, not a headless stand-in.
    fn seed_gtk_window(process: &Rc<Process>, folder: PathBuf) -> WindowId {
        let events = process
            .workspace
            .borrow_mut()
            .window_new(Some(folder))
            .expect("gtk test window");
        let id = events
            .iter()
            .find_map(|event| match event {
                Event::WindowOpened(id) => Some(*id),
                _ => None,
            })
            .expect("window id");
        let application = process.application.as_ref().expect("test application");
        let ui = WindowUi::new(process, application, id);
        process.windows.borrow_mut().insert(id, ui.clone());
        ui.refresh_header(false);
        ui.update_content();
        id
    }

    fn headless_search_process_with(
        scratch: &SearchScratch,
        runtime: Rc<crate::search::SearchRuntime>,
    ) -> Rc<Process> {
        Process::headless_with_search(
            scratch.root.join("config.toml"),
            scratch.root.join("home"),
            Rc::new(NoClipboard),
            runtime,
        )
    }

    fn gated_search_process(
        scratch: &SearchScratch,
        activation_gate: Option<Arc<crate::search::test_support::Gate>>,
        root_gate: Option<Arc<crate::search::test_support::Gate>>,
    ) -> Rc<Process> {
        use crate::search::{ActivationRunner, EvidencePool, RootRunner, SearchService};

        let activation: ActivationRunner = Arc::new(move |path: &Path| {
            if let Some(gate) = &activation_gate {
                gate.block();
            }
            crate::search::resolve_native_navigation(path)
        });
        let root: RootRunner = Arc::new(move |path: &Path, expected: Option<&Path>| {
            if let Some(gate) = &root_gate {
                gate.block();
            }
            crate::search::check_root_lifetime(path, expected)
        });
        let runtime = Rc::new(crate::search::SearchRuntime::with_evidence(
            Arc::new(SearchService::new()),
            Arc::new(EvidencePool::with_runners(activation, root)),
        ));
        headless_search_process_with(scratch, runtime)
    }

    fn apply_workspace(
        process: &Rc<Process>,
        result: Result<Vec<Event>, fm_core::workspace::WorkspaceError>,
    ) {
        let events = result.expect("workspace operation");
        process.apply_events(&events);
    }

    fn write_fixture(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent");
        }
        std::fs::write(path, b"fixture").expect("fixture");
    }

    fn wait_for_search(process: &Rc<Process>, window: WindowId) -> SearchView {
        for _ in 0..300 {
            process.poll_search_for_tests();
            if let Some(view) = process.search_view_for_tests(window)
                && matches!(
                    view.phase,
                    crate::search::SearchPhase::Complete
                        | crate::search::SearchPhase::Partial(_)
                        | crate::search::SearchPhase::Unavailable(_)
                )
            {
                return view;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("search did not reach a terminal phase");
    }

    fn wait_for_activation(process: &Rc<Process>, window: WindowId) {
        for _ in 0..300 {
            process.poll_search_for_tests();
            match process.search_view_for_tests(window) {
                None => return,
                Some(view) if view.notice.is_some() => return,
                _ => {}
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("activation neither navigated nor explained its failure");
    }

    fn activate_and_wait(process: &Rc<Process>, window: WindowId, generation: u64, index: usize) {
        process.search_activate(window, generation, index);
        wait_for_activation(process, window);
    }

    fn wait_for_root_revocation(process: &Rc<Process>, window: WindowId) {
        for _ in 0..300 {
            process.poll_search_for_tests();
            if process.search_view_for_tests(window).is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the removed search root did not revoke its page");
    }

    fn wait_for_gate(gate: &Arc<crate::search::test_support::Gate>) {
        for _ in 0..500 {
            if gate.entered() > 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the blocking search worker never entered the source");
    }

    fn wait_for_gate_idle(gate: &Arc<crate::search::test_support::Gate>) {
        for _ in 0..500 {
            if gate.entered() == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the blocking search worker did not leave the source");
    }

    fn start_search(process: &Rc<Process>, window: WindowId, query: &str) {
        process.dispatch_for_window(
            window,
            Caller::Ui,
            Invocation::new(
                "search.open",
                vec![("query".to_string(), query.to_string())],
            ),
        );
    }

    #[test]
    fn split_tile_search_activates_the_exact_duplicate_in_the_focused_tile() {
        let scratch = search_scratch("split-exact");
        let first = scratch.root.join("first");
        let second = scratch.root.join("second");
        write_fixture(&first.join("report-one.txt"));
        write_fixture(&second.join("left/report.txt"));
        write_fixture(&second.join("right/report.txt"));
        let process = headless_search_process(&scratch);
        let a = process.seed_window(first.clone());
        let b = process.seed_window(first.clone());
        process.focus_window_for_tests(a);

        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_new(Some(second.clone()))
        };
        apply_workspace(&process, events);
        let second_tab = process.window_snapshot(a).expect("window A").focused_tab;
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            let mut events = workspace.pill_ctrl_click(0).expect("select first pill");
            events.extend(workspace.pill_ctrl_click(1).expect("select second pill"));
            events.extend(workspace.group_split().expect("split the two tiles"));
            events
        };
        process.apply_events(&events);
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.focus_tab(second_tab)
        };
        apply_workspace(&process, events);

        let before_b = process.window_snapshot(b).expect("window B");
        start_search(&process, a, "report");
        let view = wait_for_search(&process, a);
        assert_eq!(view.root, second);
        assert_eq!(view.rows, 2, "only the focused tile's folder is scanned");
        let paths = process.search_result_paths_for_tests(a);
        assert!(paths.contains(&second.join("left/report.txt")));
        assert!(paths.contains(&second.join("right/report.txt")));
        assert!(paths.iter().all(|path| !path.starts_with(&first)));
        let index = paths
            .iter()
            .position(|path| path == &second.join("right/report.txt"))
            .expect("right duplicate");

        activate_and_wait(&process, a, view.generation, index);

        let after_a = process.window_snapshot(a).expect("window A");
        let tab = after_a.tab(after_a.focused_tab).expect("focused tab");
        assert_eq!(tab.folder, second.join("right"));
        assert_eq!(
            tab.selection.names,
            std::collections::BTreeSet::from([OsString::from("report.txt")])
        );
        assert!(process.search_view_for_tests(a).is_none());

        let after_b = process.window_snapshot(b).expect("window B");
        assert_eq!(after_b.focused_tab, before_b.focused_tab);
        assert_eq!(after_b.pills.len(), before_b.pills.len());
        let before_tab = before_b.tab(before_b.focused_tab).expect("B tab");
        let after_tab = after_b.tab(after_b.focused_tab).expect("B tab");
        assert_eq!(after_tab.folder, before_tab.folder);
        assert_eq!(after_tab.selection, before_tab.selection);
    }

    #[test]
    fn opening_search_without_a_query_does_not_start_a_scan() {
        let scratch = search_scratch("open-only");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("report.txt"));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(folder);
        process.dispatch_for_window(
            window,
            Caller::Ui,
            Invocation::new("search.open", Vec::new()),
        );
        assert!(
            process.search_view_for_tests(window).is_none(),
            "opening the entry must not scan until a nonempty query is submitted"
        );
    }

    #[test]
    fn a_removed_root_closes_the_session_with_a_search_unavailable_notice() {
        use crate::search::test_support::TestSource;
        use crate::search::{SearchRuntime, SearchService};
        use fm_core::folder_size::SystemClock;

        let scratch = search_scratch("root-gone");
        let source = TestSource::new();
        // The model still names the folder, but the injected source reports it
        // as missing. No second search may reopen the page after this.
        let service = Arc::new(SearchService::with_parts(
            Arc::new(source),
            Arc::new(SystemClock),
        ));
        let process =
            headless_search_process_with(&scratch, Rc::new(SearchRuntime::with_service(service)));
        let window = process.seed_window(PathBuf::from("/missing"));
        start_search(&process, window, "anything");
        for _ in 0..300 {
            process.poll_search_for_tests();
            if process.search_view_for_tests(window).is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("a removed root left its search session open");
    }

    #[test]
    fn activating_a_file_already_in_the_focused_folder_selects_it() {
        let scratch = search_scratch("same-folder");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("report.txt"));
        write_fixture(&folder.join("other.txt"));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(folder.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.rows, 1);
        activate_and_wait(&process, window, view.generation, 0);

        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, folder);
        assert_eq!(
            tab.selection.names,
            std::collections::BTreeSet::from([OsString::from("report.txt")])
        );
    }

    #[test]
    fn a_result_preview_never_mutates_the_hidden_selection_and_obeys_the_dirty_guard() {
        let scratch = search_scratch("preview-guard");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("kept.txt"));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(folder.clone());
        start_search(&process, window, "kept");
        let view = wait_for_search(&process, window);
        let before = process.window_snapshot(window).expect("window");
        let before_tab = before
            .tab(before.focused_tab)
            .expect("tab")
            .selection
            .clone();
        let selection = process
            .search_selection_for_tests(window, view.generation, 0)
            .expect("exact preview selection");
        assert_eq!(selection, Selection::file(folder.join("kept.txt")));
        let after = process.window_snapshot(window).expect("window");
        assert_eq!(
            after.tab(after.focused_tab).expect("tab").selection,
            before_tab,
            "selecting a result never changes the hidden folder selection"
        );

        let mut dirty = dirty_preview("/tmp/gimbal-search-dirty.txt");
        let ticket = dirty.select_user(selection);
        assert!(ticket.is_none(), "the dirty guard refuses the navigation");
        assert!(dirty.is_dirty());
        assert_eq!(
            dirty.current_document().expect("dirty document").path,
            PathBuf::from("/tmp/gimbal-search-dirty.txt")
        );

        for command in ["file.undo", "preview.edit"] {
            let outcome = process.dispatch_with_outcome(
                window,
                Caller::Ui,
                &Invocation::new(command, Vec::new()),
            );
            assert!(
                matches!(outcome, Outcome::Failed(ref message)
                    if message.contains("unavailable while search results are shown")),
                "{command} was not refused with a readable explanation: {outcome:?}"
            );
        }

        let before_generation = process
            .search_view_for_tests(window)
            .expect("active search")
            .generation;
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new("nav.refresh", Vec::new()),
        );
        assert_eq!(outcome, Outcome::Done);
        let after_generation = process
            .search_view_for_tests(window)
            .expect("restarted search")
            .generation;
        assert_ne!(before_generation, after_generation);
    }

    #[test]
    fn tab_folder_and_window_changes_revoke_a_waiting_search() {
        use crate::search::test_support::{Gate, TestSource};
        use crate::search::{SearchRuntime, SearchService};
        use fm_core::folder_size::SystemClock;

        let scratch = search_scratch("revoke");
        let gate = Arc::new(Gate::default());
        let source = TestSource::new()
            .with_gate(gate.clone())
            .dir("/search/root", &["/search/root/x.txt"])
            .file("/search/root/x.txt", 1)
            .block("/search/root")
            .dir("/search/other", &[]);
        let service = Arc::new(SearchService::with_parts(
            Arc::new(source),
            Arc::new(SystemClock),
        ));
        let process =
            headless_search_process_with(&scratch, Rc::new(SearchRuntime::with_service(service)));
        let window = process.seed_window(PathBuf::from("/search/root"));

        start_search(&process, window, "x");
        wait_for_gate(&gate);
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_new(Some(PathBuf::from("/search/other")))
        };
        apply_workspace(&process, events);
        assert!(
            process.search_view_for_tests(window).is_none(),
            "a tab change revokes the waiting search"
        );
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        assert!(process.search_view_for_tests(window).is_none());

        gate.reset();
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.nav_go(PathBuf::from("/search/root"), None)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "x");
        wait_for_gate(&gate);
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.nav_go(PathBuf::from("/search/other"), None)
        };
        apply_workspace(&process, events);
        assert!(
            process.search_view_for_tests(window).is_none(),
            "a folder change revokes the waiting search"
        );
        gate.release();

        gate.reset();
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.nav_go(PathBuf::from("/search/root"), None)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "x");
        wait_for_gate(&gate);
        process.close_window_for_tests(window);
        assert!(
            process.search_view_for_tests(window).is_none(),
            "closing the window revokes the waiting search"
        );
        gate.release();
        wait_for_gate_idle(&gate);
    }

    #[test]
    fn a_blocked_activation_leaves_control_responsive_and_clear_makes_it_inert() {
        let scratch = search_scratch("activation-clear");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("report.txt"));
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, Some(gate.clone()), None);
        let window = process.seed_window(folder.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);

        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        // The GTK/control loop still processes a normal command while the
        // native lookup is inside the worker gate.
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new("nav.focus-path", Vec::new()),
        );
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(
            process.native_nav_evidence_uses_for_tests(),
            0,
            "no navigation starts before the worker returns"
        );
        assert!(process.search_view_for_tests(window).is_some());

        process.cancel_search(window);
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, folder);
        assert!(tab.selection.names.is_empty());
        assert!(process.search_view_for_tests(window).is_none());
    }

    #[test]
    fn replacement_tab_move_and_window_close_make_late_activation_inert() {
        // Replacement.
        let scratch = search_scratch("activation-replace");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("report.txt"));
        write_fixture(&folder.join("other.txt"));
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, Some(gate.clone()), None);
        let window = process.seed_window(folder.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        start_search(&process, window, "other");
        gate.release();
        wait_for_gate_idle(&gate);
        let replaced = wait_for_search(&process, window);
        assert_eq!(replaced.query, "other");
        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, folder, "the old activation never navigated");
        assert!(tab.selection.names.is_empty());

        // Tab move (the moved pill takes focus, a target change).
        gate.reset();
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_new(Some(folder.clone()))
        };
        apply_workspace(&process, events);
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_focus(0)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_move(1, 0)
        };
        apply_workspace(&process, events);
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        assert!(
            process.search_view_for_tests(window).is_none(),
            "a tab move revokes the activation page"
        );

        // Window close.
        gate.reset();
        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.tab_focus(0)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        process.close_window_for_tests(window);
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        assert!(process.search_view_for_tests(window).is_none());
    }

    #[test]
    fn one_pending_activation_is_deduplicated_and_a_different_result_is_busy() {
        let scratch = search_scratch("activation-busy");
        let folder = scratch.root.join("folder");
        write_fixture(&folder.join("report-one.txt"));
        write_fixture(&folder.join("report-two.txt"));
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, Some(gate.clone()), None);
        let window = process.seed_window(folder);
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.rows, 2);

        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        // The same result is absorbed rather than queued twice.
        process.search_activate(window, view.generation, 0);
        // A different result must not accumulate behind the blocked one.
        process.search_activate(window, view.generation, 1);
        let view = process.search_view_for_tests(window).expect("page");
        assert!(
            view.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("already being checked")),
            "{view:?}"
        );
        assert_eq!(gate.entered(), 1, "only one lookup entered the gate");
        process.cancel_search(window);
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        assert_eq!(process.native_nav_evidence_uses_for_tests(), 0);
    }

    #[test]
    fn a_vanished_result_explains_without_navigating_or_closing_the_page() {
        let scratch = search_scratch("activation-vanished");
        let folder = scratch.root.join("folder");
        let target = folder.join("report.txt");
        write_fixture(&target);
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, Some(gate.clone()), None);
        let window = process.seed_window(folder.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        std::fs::remove_file(&target).expect("remove owned fixture");
        gate.release();
        wait_for_gate_idle(&gate);
        wait_for_activation(&process, window);

        let view = process
            .search_view_for_tests(window)
            .expect("the results page stays open");
        assert!(
            view.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("no longer exists")),
            "{view:?}"
        );
        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, folder);
        assert!(tab.selection.names.is_empty());
        assert_eq!(process.native_nav_evidence_uses_for_tests(), 0);
    }

    #[test]
    fn directory_and_symlink_results_activate_the_exact_path() {
        let scratch = search_scratch("activation-links");
        let root = scratch.root.join("search-root");
        let target = root.join("target-dir");
        std::fs::create_dir_all(&target).expect("target directory");
        let link = root.join("link-dir");
        std::os::unix::fs::symlink(&target, &link).expect("owned symlink");
        let process = headless_search_process(&scratch);
        let window = process.seed_window(root.clone());

        start_search(&process, window, "target-dir");
        let view = wait_for_search(&process, window);
        activate_and_wait(&process, window, view.generation, 0);
        let after = process.window_snapshot(window).expect("window");
        assert_eq!(after.tab(after.focused_tab).expect("tab").folder, target);

        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.nav_go(root.clone(), None)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "link-dir");
        let view = wait_for_search(&process, window);
        activate_and_wait(&process, window, view.generation, 0);
        let after = process.window_snapshot(window).expect("window");
        assert_eq!(
            after.tab(after.focused_tab).expect("tab").folder,
            link,
            "a symlink result navigates through its original exact path"
        );
    }

    #[test]
    fn invalid_utf8_duplicate_paths_reach_nav_go_through_the_registry() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let scratch = search_scratch("native-registry");
        let parent_a = scratch.root.join(OsStr::from_bytes(b"left-\xfd"));
        let parent_b = scratch.root.join(OsStr::from_bytes(b"right-\xfe"));
        let name = OsStr::from_bytes(b"report-\xff.txt");
        write_fixture(&parent_a.join(name));
        write_fixture(&parent_b.join(name));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(scratch.root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.rows, 2, "both native duplicate names are retained");
        let paths = process.search_result_paths_for_tests(window);
        let index = paths
            .iter()
            .position(|path| path == &parent_b.join(name))
            .expect("the exact right duplicate");
        let before = process.native_nav_evidence_uses_for_tests();
        activate_and_wait(&process, window, view.generation, index);

        assert_eq!(
            process.native_nav_evidence_uses_for_tests(),
            before + 1,
            "the real nav.go handler consumed validated native evidence"
        );
        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, parent_b);
        assert_eq!(
            tab.selection.names,
            std::collections::BTreeSet::from([name.to_os_string()])
        );
    }

    #[test]
    fn native_navigation_evidence_is_bound_to_the_validated_text_and_caller() {
        let scratch = search_scratch("native-binding");
        let folder = scratch.root.join("folder");
        let target = folder.join("report.txt");
        write_fixture(&target);
        let process = headless_search_process(&scratch);
        let window = process.seed_window(scratch.root.clone());
        let select = Some(std::ffi::OsString::from("report.txt"));
        let before = process.native_nav_evidence_uses_for_tests();

        // The registry refuses a relative path before any handler runs.
        process.set_pending_nav_evidence_for_tests(
            "relative".to_string(),
            folder.clone(),
            select.clone(),
            Caller::Ui,
        );
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new("nav.go", vec![("path".to_string(), "relative".to_string())]),
        );
        assert!(matches!(outcome, Outcome::Error(_)));
        assert_eq!(process.native_nav_evidence_uses_for_tests(), before);
        process.set_pending_nav_target(None);

        // A different caller cannot consume UI-only evidence.
        process.set_pending_nav_evidence_for_tests(
            "/tmp/valid".to_string(),
            folder.clone(),
            select.clone(),
            Caller::Ui,
        );
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Agent,
            &Invocation::new(
                "nav.go",
                vec![("path".to_string(), "/tmp/valid".to_string())],
            ),
        );
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(process.native_nav_evidence_uses_for_tests(), before);

        // A changed text target fails instead of falling back to the old one.
        process.set_pending_nav_evidence_for_tests(
            "/tmp/valid".to_string(),
            folder.clone(),
            select.clone(),
            Caller::Ui,
        );
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new(
                "nav.go",
                vec![("path".to_string(), "/tmp/other".to_string())],
            ),
        );
        assert!(matches!(outcome, Outcome::Failed(_)));
        assert_eq!(process.native_nav_evidence_uses_for_tests(), before);
        let after = process.window_snapshot(window).expect("window");
        assert_eq!(
            after.tab(after.focused_tab).expect("tab").folder,
            scratch.root,
            "a refused payload never moves the hidden folder"
        );

        // The matching validated text reaches the handler with the exact
        // native selection and is counted once.
        process.set_pending_nav_evidence_for_tests(
            "/tmp/valid".to_string(),
            folder.clone(),
            select.clone(),
            Caller::Ui,
        );
        let outcome = process.dispatch_with_outcome(
            window,
            Caller::Ui,
            &Invocation::new(
                "nav.go",
                vec![("path".to_string(), "/tmp/valid".to_string())],
            ),
        );
        assert_eq!(outcome, Outcome::Done);
        assert_eq!(process.native_nav_evidence_uses_for_tests(), before + 1);
        let after = process.window_snapshot(window).expect("window");
        let tab = after.tab(after.focused_tab).expect("tab");
        assert_eq!(tab.folder, folder);
        assert_eq!(
            tab.selection.names,
            std::collections::BTreeSet::from([std::ffi::OsString::from("report.txt")])
        );
    }

    #[test]
    fn a_completed_search_revokes_when_its_root_is_removed() {
        let scratch = search_scratch("root-complete");
        let root = scratch.root.join("search-root");
        write_fixture(&root.join("report.txt"));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(
            view.phase,
            crate::search::SearchPhase::Complete,
            "the page starts completed"
        );
        assert_eq!(process.search_monitor_count_for_tests(), 1);
        std::fs::remove_dir_all(&root).expect("remove owned root");
        process.search_root_event_for_tests(window);
        wait_for_root_revocation(&process, window);
        assert_eq!(process.search_monitor_count_for_tests(), 0);
    }

    #[test]
    fn a_partial_search_revokes_when_its_root_is_removed() {
        use crate::search::{SearchRuntime, SearchService};
        use fm_core::search::SearchLimits;

        let scratch = search_scratch("root-partial");
        let root = scratch.root.join("search-root");
        write_fixture(&root.join("report-one.txt"));
        write_fixture(&root.join("report-two.txt"));
        let runtime = Rc::new(SearchRuntime::with_service(Arc::new(SearchService::new())));
        runtime.set_limits_for_tests(SearchLimits {
            max_visited: 1,
            ..SearchLimits::default()
        });
        let process = headless_search_process_with(&scratch, runtime);
        let window = process.seed_window(root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert!(
            matches!(view.phase, crate::search::SearchPhase::Partial(_)),
            "{view:?}"
        );
        std::fs::remove_dir_all(&root).expect("remove owned root");
        process.search_root_event_for_tests(window);
        wait_for_root_revocation(&process, window);
    }

    #[test]
    fn a_removed_root_alias_revokes_its_completed_search() {
        let scratch = search_scratch("root-alias");
        let real = scratch.root.join("real-root");
        write_fixture(&real.join("report.txt"));
        let alias = scratch.root.join("alias-root");
        std::os::unix::fs::symlink(&real, &alias).expect("owned alias");
        let process = headless_search_process(&scratch);
        let window = process.seed_window(alias.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.root, alias);
        assert!(view.walked_root.is_some());
        std::fs::remove_file(&alias).expect("remove owned alias");
        process.search_root_event_for_tests(window);
        wait_for_root_revocation(&process, window);
    }

    #[test]
    fn a_removed_root_revokes_a_pending_activation() {
        let scratch = search_scratch("root-activation");
        let root = scratch.root.join("search-root");
        write_fixture(&root.join("report.txt"));
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, Some(gate.clone()), None);
        let window = process.seed_window(root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        process.search_activate(window, view.generation, 0);
        wait_for_gate(&gate);
        std::fs::remove_dir_all(&root).expect("remove owned root");
        process.search_root_event_for_tests(window);
        wait_for_root_revocation(&process, window);
        gate.release();
        wait_for_gate_idle(&gate);
        process.poll_search_for_tests();
        assert!(process.search_view_for_tests(window).is_none());
        assert_eq!(process.native_nav_evidence_uses_for_tests(), 0);
    }

    #[test]
    fn stale_root_evidence_cannot_close_a_replacement_session() {
        let scratch = search_scratch("root-stale");
        let root_a = scratch.root.join("root-a");
        let root_b = scratch.root.join("root-b");
        write_fixture(&root_a.join("report.txt"));
        write_fixture(&root_b.join("other.txt"));
        let gate = Arc::new(crate::search::test_support::Gate::default());
        let process = gated_search_process(&scratch, None, Some(gate.clone()));
        let window = process.seed_window(root_a.clone());
        start_search(&process, window, "report");
        let first = wait_for_search(&process, window);
        assert_eq!(first.root, root_a);
        process.search_root_event_for_tests(window);
        process.poll_search_for_tests();
        wait_for_gate(&gate);
        std::fs::remove_dir_all(&root_a).expect("remove owned old root");

        let events = {
            let mut workspace = process.workspace.borrow_mut();
            workspace.nav_go(root_b.clone(), None)
        };
        apply_workspace(&process, events);
        start_search(&process, window, "other");
        let replacement = wait_for_search(&process, window);
        assert_eq!(replacement.root, root_b);
        assert_eq!(replacement.query, "other");

        gate.release();
        wait_for_gate_idle(&gate);
        for _ in 0..20 {
            process.poll_search_for_tests();
        }
        let still = process
            .search_view_for_tests(window)
            .expect("the replacement session survives the old root evidence");
        assert_eq!(still.root, root_b);
        assert_eq!(still.query, "other");
    }

    #[test]
    fn search_monitors_are_torn_down_on_replacement_clear_and_window_close() {
        let scratch = search_scratch("monitor-teardown");
        let root_a = scratch.root.join("root-a");
        let root_b = scratch.root.join("root-b");
        write_fixture(&root_a.join("a.txt"));
        write_fixture(&root_b.join("b.txt"));
        let process = headless_search_process(&scratch);
        let a = process.seed_window(root_a.clone());
        let b = process.seed_window(root_b.clone());
        start_search(&process, a, "a");
        let _ = wait_for_search(&process, a);
        start_search(&process, b, "b");
        let _ = wait_for_search(&process, b);
        assert_eq!(process.search_monitor_count_for_tests(), 2);

        start_search(&process, a, "a2");
        assert_eq!(
            process.search_monitor_count_for_tests(),
            2,
            "replacement drops the old monitor before installing one"
        );
        process.cancel_search(a);
        assert_eq!(process.search_monitor_count_for_tests(), 1);
        process.close_window_for_tests(b);
        assert_eq!(process.search_monitor_count_for_tests(), 0);
    }

    fn gtk_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The real widget/main-loop regression runs in its own process. GTK's
    /// `gtk::init` intentionally owns the process default main context for its
    /// lifetime, which would starve the other in-process GLib timer tests if
    /// this ran in the shared unit-test process.
    #[test]
    fn real_header_enter_shows_file_and_folder_rows_in_the_list_store() {
        let executable = std::env::current_exe().expect("the test executable");
        let output = std::process::Command::new(executable)
            .args([
                "--exact",
                "app::tests::real_header_enter_shows_file_and_folder_rows_in_the_list_store_inner",
                "--ignored",
                "--nocapture",
            ])
            .output()
            .expect("the owned widget regression process");
        assert!(
            output.status.success(),
            "the real GTK widget regression failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    #[ignore = "runs in its own process through the subprocess widget regression"]
    fn real_header_enter_shows_file_and_folder_rows_in_the_list_store_inner() {
        let _guard = gtk_test_lock();
        gtk::init().expect("the widget regression needs a GTK display");
        adw::init().expect("libadwaita init");

        let context = glib::MainContext::default();
        let scratch = search_scratch("gtk-entry");
        let root = scratch.root.join("tree");
        write_fixture(&root.join("report.txt"));
        write_fixture(&root.join("nested/report.txt"));
        std::fs::create_dir_all(root.join("report-folder")).expect("folder fixture");
        write_fixture(&root.join("unrelated.txt"));

        let application = adw::Application::builder()
            .application_id("org.gimbal.Files.SearchWidgetTest")
            .flags(gio::ApplicationFlags::NON_UNIQUE)
            .build();
        application
            .register(None::<&gio::Cancellable>)
            .expect("register the owned non-unique test application");
        let runtime = Rc::new(crate::search::SearchRuntime::with_service(Arc::new(
            crate::search::SearchService::new(),
        )));
        let process = gtk_search_process(&scratch, &application, runtime);
        let window = seed_gtk_window(&process, root.clone());
        let ui = process.window(window).expect("real window");
        ui.window().present();

        // The same header command Click Search uses, then the entry's real
        // activate signal that Enter emits. No desktop input injection.
        process.dispatch_for_window(
            window,
            Caller::Ui,
            Invocation::new("search.open", Vec::new()),
        );
        assert!(ui.header().search_open_for_tests());
        let entry = ui.header().search_entry_for_tests().clone();
        entry.set_text("report");
        assert!(
            ui.header().search_hint_visible_for_tests(),
            "typing shows the Press Enter hint before submission"
        );
        assert!(
            process.search_view_for_tests(window).is_none(),
            "typing alone must not start a scan"
        );
        entry.emit_by_name::<()>("activate", &[]);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut view = None;
        while std::time::Instant::now() < deadline {
            context.iteration(false);
            view = process.search_view_for_tests(window);
            let count = ui.content().search_page().store_count_for_tests();
            if view.as_ref().is_some_and(|view| {
                matches!(
                    view.phase,
                    crate::search::SearchPhase::Complete | crate::search::SearchPhase::Partial(_)
                ) && count >= 3
            }) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        let view = view.expect("the real entry started a search");
        assert_eq!(view.root, root);
        assert_eq!(view.query, "report");
        assert!(
            matches!(
                view.phase,
                crate::search::SearchPhase::Complete | crate::search::SearchPhase::Partial(_)
            ),
            "the real main-loop poll never finished the scan: {view:?}"
        );
        let page = ui.content().search_page();
        assert_eq!(page.store_count_for_tests(), 3, "{view:?}");
        let mut names = page.row_texts_for_tests();
        names.sort();
        assert_eq!(
            names,
            vec![
                ("report-folder".to_string(), ".".to_string()),
                ("report.txt".to_string(), ".".to_string()),
                ("report.txt".to_string(), "nested".to_string()),
            ]
        );
        assert!(
            page.list_mapped_for_tests(),
            "the result list was never mapped in the visible search page"
        );
        assert!(
            !ui.header().search_hint_visible_for_tests(),
            "the submitted page hides the pre-submission hint"
        );
        assert_eq!(
            ui.content().visible_page_for_tests().as_deref(),
            Some("search")
        );

        // The same real entry now submits exact file and folder names. Both
        // files with the exact basename survive as separate native paths.
        for (query, expected) in [
            (
                "report.txt",
                vec![root.join("nested/report.txt"), root.join("report.txt")],
            ),
            ("report-folder", vec![root.join("report-folder")]),
        ] {
            entry.set_text(query);
            entry.emit_by_name::<()>("activate", &[]);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                context.iteration(false);
                if process.search_view_for_tests(window).is_some_and(|view| {
                    matches!(
                        view.phase,
                        crate::search::SearchPhase::Complete
                            | crate::search::SearchPhase::Partial(_)
                    ) && view.query == query
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "the exact {query:?} search did not finish"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(page.row_paths_for_tests(), expected, "query {query:?}");
        }

        // Reordering rows after an exact search must keep the selected native
        // path, not the old row index. This uses the same real store/model.
        page.begin(&SearchView {
            generation: 99,
            root: root.clone(),
            walked_root: None,
            query: "report".to_string(),
            rows: 0,
            total_matches: 0,
            truncated: false,
            visited: 0,
            skipped: 0,
            mounts_skipped: 0,
            examples: Vec::new(),
            phase: crate::search::SearchPhase::Searching { queued: false },
            notice: None,
        });
        use fm_core::search::{MatchClass, MatchScore, SearchChange, SearchMatch};
        let row = |name: &str, score: MatchScore| SearchMatch {
            path: root.join(name),
            name: OsString::from(name),
            display_name: name.to_string(),
            kind: fm_core::folder_size::EntryKind::File,
            size: Some(1),
            modified_sec: Some(1_700_000_000),
            score,
        };
        let first = row(
            "first-report.txt",
            MatchScore {
                class: MatchClass::Substring,
                edits: 0,
                gaps: 0,
                start: 6,
            },
        );
        let second = row(
            "second-report.txt",
            MatchScore {
                class: MatchClass::Substring,
                edits: 0,
                gaps: 0,
                start: 7,
            },
        );
        let exact = row(
            "report",
            MatchScore {
                class: MatchClass::ExactName,
                edits: 0,
                gaps: 0,
                start: 0,
            },
        );
        page.apply_changes(&[
            SearchChange::Insert {
                index: 0,
                row: first,
            },
            SearchChange::Insert {
                index: 1,
                row: second.clone(),
            },
        ]);
        page.selection_for_tests().set_selected(1);
        assert_eq!(page.row_paths_for_tests()[1], second.path);
        page.apply_changes(&[SearchChange::Insert {
            index: 0,
            row: exact.clone(),
        }]);
        assert_eq!(
            page.selection_for_tests().selected(),
            2,
            "the selection moved with its native path"
        );
        assert_eq!(page.row_paths_for_tests()[2], second.path);
        page.apply_changes(&[SearchChange::Remove {
            index: 0,
            path: exact.path,
        }]);
        assert_eq!(
            page.selection_for_tests().selected(),
            1,
            "removing a row above the selection keeps the same path"
        );
        assert_eq!(page.row_paths_for_tests()[1], second.path);
        assert!(page.selection_for_tests().selected_item().is_some());
        assert_eq!(page.row_score_for_tests(1), Some(second.score));
    }

    #[test]
    fn visible_rows_rank_exact_then_prefix_then_substring_then_fuzzy() {
        let scratch = search_scratch("ranked");
        let root = scratch.root.join("tree");
        write_fixture(&root.join("report"));
        write_fixture(&root.join("report.txt"));
        write_fixture(&root.join("reporting.txt"));
        write_fixture(&root.join("my-report.txt"));
        write_fixture(&root.join("reprot.txt"));
        let process = headless_search_process(&scratch);
        let window = process.seed_window(root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.rows, 5);
        assert_eq!(view.total_matches, 5);
        assert!(!view.truncated);
        assert_eq!(
            process.search_result_paths_for_tests(window),
            vec![
                root.join("report"),
                root.join("report.txt"),
                root.join("reporting.txt"),
                root.join("my-report.txt"),
                root.join("reprot.txt"),
            ]
        );
    }

    #[test]
    fn a_late_exact_match_replaces_early_fuzzy_rows_and_reports_truncation() {
        let scratch = search_scratch("late-exact");
        let root = scratch.root.join("tree");
        for index in 0..20 {
            write_fixture(&root.join(format!("reprot-{index}.txt")));
        }
        write_fixture(&root.join("report"));
        let runtime = Rc::new(crate::search::SearchRuntime::with_service(Arc::new(
            crate::search::SearchService::new(),
        )));
        runtime.set_limits_for_tests(fm_core::search::SearchLimits {
            max_results: 2,
            ..fm_core::search::SearchLimits::default()
        });
        let process = headless_search_process_with(&scratch, runtime);
        let window = process.seed_window(root.clone());
        start_search(&process, window, "report");
        let view = wait_for_search(&process, window);
        assert_eq!(view.rows, 2, "only the best two rows are retained");
        assert_eq!(view.total_matches, 21);
        assert!(view.truncated);
        assert!(
            matches!(
                view.phase,
                crate::search::SearchPhase::Partial(ref reasons)
                    if reasons.contains(&fm_core::search::PartialReason::ResultLimit)
            ),
            "{view:?}"
        );
        let paths = process.search_result_paths_for_tests(window);
        assert_eq!(paths[0], root.join("report"), "the late exact name wins");
        assert!(
            paths[1]
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("reprot-")),
            "the best fuzzy row remains: {paths:?}"
        );
        let status = crate::search::status_text(&view);
        assert!(status.contains("2 best of 21 matches"), "{status}");
        assert!(status.contains("result limit"), "{status}");
    }
}
