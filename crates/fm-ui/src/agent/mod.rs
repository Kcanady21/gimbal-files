// SPDX-License-Identifier: GPL-3.0-or-later
//! The main-thread bridge between the local socket and the real UI.
//!
//! Transport workers never touch GTK. They send typed [`Inbound`] messages;
//! a GLib timeout on this thread reads them, rechecks the enable generation,
//! revalidates the target window and command, dispatches through the one
//! `Registry` as `Caller::Agent`, and sends the typed reply back. File jobs
//! keep their real state here, not a second filesystem implementation.

mod work;

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fm_agent::protect::{ProtectedRoots, ResolvedRoots};
use fm_agent::protocol::{
    ArgumentInfo, CommandInfo, Data, GroupInfo, JobInfo, NameInfo, Operation, Reply, Request,
    RunKind, SocketInfo, Status, TabInfo, WindowInfo,
};
use fm_agent::transport::{Inbound, Server, ServerConfig};
use fm_core::command::{
    ArgKind, Caller, Effect, Invocation, Outcome, Presence, Registry, Value, caller_allowed,
    must_supply,
};
use fm_core::fileops::Request as FileRequest;
use fm_core::workspace::{TabId, Window, WindowId};
use gtk::glib;

use crate::app::Process;

/// At most this many socket requests may wait for the UI at once.
pub const MAX_PENDING_REQUESTS: usize = 32;
/// At most this many file jobs may be waiting before a socket run is refused.
pub const MAX_PENDING_JOBS: usize = 32;
/// Terminal job history is bounded; older ids report expired or unknown.
pub const MAX_TERMINAL_JOBS: usize = 64;
/// How many jobs a state reply lists.
pub const STATE_JOBS: usize = 20;
/// How many selected names one tab reports before saying it truncated.
pub const SELECTION_NAMES: usize = 20;
/// A state reply lists at most this many groups per window...
pub const MAX_GROUPS_PER_WINDOW: usize = 64;
/// ...and this many tabs across the whole page. Remaining tabs are reported
/// as a count with `tabs_truncated`, never copied into one oversized frame.
pub const MAX_TABS_PER_STATE: usize = 128;

/// The exact model target a bridged command may execute against. It is
/// resolved from the request before any effect and revalidated after any
/// asynchronous evidence, so a later focus change cannot redirect the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionTarget {
    pub window: WindowId,
    pub tab: TabId,
}

/// What the Settings switch and `state` show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Off,
    On,
    Unavailable,
}

impl AgentState {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentState::Off => "off",
            AgentState::On => "on",
            AgentState::Unavailable => "unavailable",
        }
    }
}

/// A read-only snapshot for the menu and state replies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    pub state: AgentState,
    pub detail: Option<String>,
    pub path: Option<PathBuf>,
    pub generation: u64,
    pub instance: Option<String>,
}

/// One enable cycle's revocation state. Every accepted agent job clones this
/// ticket at submission, so Off invalidates queued, planning and callback work
/// immediately without consulting the client-visible job history.
#[derive(Debug)]
pub struct GenerationTicket {
    generation: u64,
    revoked: AtomicBool,
}

impl GenerationTicket {
    fn new(generation: u64) -> Arc<Self> {
        Arc::new(Self {
            generation,
            revoked: AtomicBool::new(false),
        })
    }

    fn revoke(&self) {
        self.revoked.store(true, Ordering::SeqCst);
    }

    pub fn is_active(&self) -> bool {
        !self.revoked.load(Ordering::SeqCst)
    }

    fn generation(&self) -> u64 {
        self.generation
    }
}

/// Immutable authorization captured when an agent operation is accepted. It
/// travels through queueing, planning, confirmation and execution; it is not
/// reconstructed from the job table.
#[derive(Clone)]
pub struct JobAuthorization {
    ticket: Arc<GenerationTicket>,
    pub instance: String,
    pub roots: ResolvedRoots,
}

impl JobAuthorization {
    pub fn is_active(&self) -> bool {
        self.ticket.is_active()
    }

    pub fn generation(&self) -> u64 {
        self.ticket.generation()
    }

    pub fn same_cycle(&self, ticket: &Arc<GenerationTicket>) -> bool {
        Arc::ptr_eq(&self.ticket, ticket)
    }

    #[cfg(test)]
    pub(crate) fn revoke_for_tests(&self) {
        self.ticket.revoke();
    }
}

#[cfg(test)]
pub(crate) fn test_authorization_with_roots(
    generation: u64,
    roots: ResolvedRoots,
) -> JobAuthorization {
    JobAuthorization {
        ticket: GenerationTicket::new(generation),
        instance: format!("test-g{generation}"),
        roots,
    }
}

#[cfg(test)]
pub(crate) fn test_authorization(generation: u64, revoked: bool) -> JobAuthorization {
    let ticket = GenerationTicket::new(generation);
    if revoked {
        ticket.revoke();
    }
    JobAuthorization {
        ticket,
        instance: format!("test-g{generation}"),
        roots: ProtectedRoots::new(
            "/tmp/gimbal-agent-test/config.toml",
            "/tmp/gimbal-agent-test/config",
            "/tmp/gimbal-agent-test/run",
        )
        .resolved()
        .expect("test roots"),
    }
}

impl std::fmt::Debug for JobAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobAuthorization")
            .field("generation", &self.generation())
            .field("instance", &self.instance)
            .field("active", &self.is_active())
            .finish_non_exhaustive()
    }
}

/// A bridge request parked on the bounded filesystem pool. The generation and
/// ticket are retained so a late result cannot dispatch into a new cycle.
pub struct PendingRun {
    pub inbound: Inbound,
    pub work_id: u64,
    pub target: ExecutionTarget,
    pub kind: PendingKind,
    pub authorization: JobAuthorization,
}

pub enum PendingKind {
    Nav { invocation: Invocation },
    DryRun,
}

enum RunPrepared {
    Reply(Reply),
    Deferred {
        work_id: u64,
        kind: PendingKind,
        target: ExecutionTarget,
        authorization: JobAuthorization,
    },
}

struct JobRecord {
    token: String,
    /// The file-op worker's own id once the job reaches it.
    fileops_id: Option<u64>,
    state: String,
    summary: String,
    lines: Vec<String>,
    errors: Vec<String>,
    terminal: bool,
}

/// The enable generation, listener and bounded job history.
pub struct AgentRuntime {
    process_tag: String,
    generation: u64,
    instance: Option<String>,
    enabled: bool,
    state: AgentState,
    detail: Option<String>,
    path: Option<PathBuf>,
    roots: Option<ResolvedRoots>,
    ticket: Option<Arc<GenerationTicket>>,
    server: Option<Server>,
    receiver: Option<std::sync::mpsc::Receiver<Inbound>>,
    jobs: VecDeque<JobRecord>,
    next_job: u64,
    work: work::WorkPool,
}

impl AgentRuntime {
    pub fn new(process_tag: String) -> Self {
        Self {
            process_tag,
            generation: 0,
            instance: None,
            enabled: false,
            state: AgentState::Off,
            detail: None,
            path: None,
            roots: None,
            ticket: None,
            server: None,
            receiver: None,
            jobs: VecDeque::new(),
            next_job: 0,
            work: work::WorkPool::start(),
        }
    }

    fn snapshot(&self) -> AgentStatus {
        AgentStatus {
            state: self.state,
            detail: self.detail.clone(),
            path: self.path.clone(),
            generation: self.generation,
            instance: self.instance.clone(),
        }
    }

    fn current_instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// Capture the immutable authorization for newly accepted work.
    fn authorization(&self) -> Option<JobAuthorization> {
        Some(JobAuthorization {
            ticket: Arc::clone(self.ticket.as_ref()?),
            instance: self.instance.clone()?,
            roots: self.roots.clone()?,
        })
    }

    fn job_token(&self, id: u64) -> Option<String> {
        self.current_instance()
            .map(|instance| format!("{instance}/j{id}"))
    }

    /// Reserve a token before asynchronous work starts. The file-op id is
    /// attached later, so a clipboard read can be queryable immediately.
    fn reserve_job(&mut self, summary: String) -> Option<(u64, String)> {
        if self.jobs.len() >= MAX_TERMINAL_JOBS + MAX_PENDING_JOBS {
            return None;
        }
        self.next_job = self.next_job.wrapping_add(1).max(1);
        let id = self.next_job;
        let token = self.job_token(id)?;
        self.jobs.push_back(JobRecord {
            token: token.clone(),
            fileops_id: None,
            state: "queued".to_string(),
            summary,
            lines: Vec::new(),
            errors: Vec::new(),
            terminal: false,
        });
        self.trim_jobs();
        Some((id, token))
    }

    /// Attach the file-op id only to the exact reserved job token. A late
    /// callback from an earlier generation cannot rename or revive a new job
    /// whose local number happens to match.
    fn attach_fileops(&mut self, token: &str, fileops_id: u64, running: bool) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.token == token) {
            if job.terminal {
                return;
            }
            job.fileops_id = Some(fileops_id);
            if running {
                job.state = "running".to_string();
            }
        }
    }

    fn job_by_fileops(&mut self, fileops_id: u64, update: impl FnOnce(&mut JobRecord)) {
        if let Some(job) = self
            .jobs
            .iter_mut()
            .find(|job| job.fileops_id == Some(fileops_id))
        {
            if job.terminal {
                return;
            }
            update(job);
            if job.state == "complete" || job.state == "cancelled" || job.state == "failed" {
                job.terminal = true;
            }
        }
        self.trim_jobs();
    }

    /// Fail one exact reserved job token. A terminal job is never overwritten
    /// by an unrelated late callback, even in the same cycle.
    fn fail_reserved(&mut self, token: &str, message: String) {
        if let Some(job) = self.jobs.iter_mut().find(|job| job.token == token) {
            if job.terminal {
                return;
            }
            job.state = "failed".to_string();
            job.errors = vec![message];
            job.terminal = true;
        }
        self.trim_jobs();
    }

    fn trim_jobs(&mut self) {
        while self.jobs.iter().filter(|job| job.terminal).count() > MAX_TERMINAL_JOBS {
            let Some(index) = self.jobs.iter().position(|job| job.terminal) else {
                break;
            };
            self.jobs.remove(index);
        }
    }

    fn job_by_token(&mut self, token: &str) -> JobInfo {
        let Some(instance) = self.instance.clone() else {
            return unknown_job(token, "unknown");
        };
        let prefix = format!("{instance}/j");
        let Some(raw) = token.strip_prefix(&prefix) else {
            return unknown_job(token, "unknown");
        };
        let Ok(_id) = raw.parse::<u64>() else {
            return unknown_job(token, "unknown");
        };
        match self.jobs.iter().find(|job| job.token == token) {
            Some(job) => JobInfo {
                id: job.token.clone(),
                state: job.state.clone(),
                summary: job.summary.clone(),
                lines: job.lines.clone(),
                errors: job.errors.clone(),
            },
            None => unknown_job(token, "expired"),
        }
    }
}

fn unknown_job(token: &str, state: &str) -> JobInfo {
    JobInfo {
        id: token.to_string(),
        state: state.to_string(),
        summary: String::new(),
        lines: Vec::new(),
        errors: vec!["no job with this id exists in this session".to_string()],
    }
}

/// Start the listener when the saved preference is On. A failure is visible
/// and leaves runtime Off; it never enables ordinary app starts implicitly.
pub fn start_if_enabled(process: &Rc<Process>) {
    let saved = process.config.borrow().config().agent.enabled;
    if !saved {
        process.agent.borrow_mut().state = AgentState::Off;
        return;
    }
    if let Err(error) = enable(process) {
        crate::log!("agent", "could not start the saved listener: {error}");
        let mut runtime = process.agent.borrow_mut();
        runtime.state = AgentState::Unavailable;
        runtime.detail = Some(error);
    }
}

/// Create the endpoint for a new generation, using the user's real runtime
/// directory when production enables it.
fn enable(process: &Rc<Process>) -> Result<(), String> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "XDG_RUNTIME_DIR is not set; the local socket is unavailable".to_string())?;
    if !runtime_dir.is_absolute() {
        return Err(format!(
            "XDG_RUNTIME_DIR {} is not absolute; the local socket is unavailable",
            runtime_dir.display()
        ));
    }
    enable_at(process, &runtime_dir)
}

/// Enable at an explicit runtime directory. Tests use owned `/tmp` fixtures;
/// production passes the validated `$XDG_RUNTIME_DIR`.
pub(crate) fn enable_at(process: &Rc<Process>, runtime_dir: &Path) -> Result<(), String> {
    {
        let runtime = process.agent.borrow();
        if runtime.enabled {
            return Ok(());
        }
    }
    if !runtime_dir.is_absolute() {
        return Err(format!(
            "{} is not an absolute runtime directory",
            runtime_dir.display()
        ));
    }
    let uid = current_uid()?;

    let (generation, process_tag) = {
        let runtime = process.agent.borrow();
        (runtime.generation + 1, runtime.process_tag.clone())
    };
    let instance = format!("{process_tag}-g{generation}");
    let config_path = process.config_path();
    let config_dir = config_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| config_path.clone());
    let control_dir = runtime_dir.join("gimbal-files");
    let roots = ProtectedRoots::new(config_path, config_dir, control_dir)
        .resolved()
        .map_err(|error| error.to_string())?;

    let mut config = ServerConfig::new(runtime_dir, uid, instance.clone(), generation);
    config.protected = Some(roots.clone());
    let (server, receiver) = Server::start(config).map_err(|error| error.to_string())?;
    let path = server.path().to_path_buf();

    let mut runtime = process.agent.borrow_mut();
    runtime.generation = generation;
    runtime.instance = Some(instance);
    runtime.enabled = true;
    runtime.state = AgentState::On;
    runtime.detail = None;
    runtime.path = Some(path.clone());
    runtime.roots = Some(roots);
    runtime.ticket = Some(GenerationTicket::new(generation));
    runtime.server = Some(server);
    runtime.receiver = Some(receiver);
    runtime.jobs.clear();
    runtime.next_job = 0;
    crate::log!(
        "agent",
        "enabled generation {generation} at {}",
        path.display()
    );
    Ok(())
}

/// Stop immediately. Already-started file operations keep running through the
/// ordinary file-op worker; only further control is revoked.
fn disable(process: &Rc<Process>, detail: Option<String>) {
    // Publish Off and invalidate the generation under one short borrow. No
    // join, filesystem query or callback runs while this borrow is held.
    let (server, ticket) = {
        let mut runtime = process.agent.borrow_mut();
        let ticket = runtime.ticket.take();
        if let Some(ticket) = &ticket {
            ticket.revoke();
        }
        runtime.receiver = None;
        runtime.enabled = false;
        runtime.state = AgentState::Off;
        runtime.detail = detail;
        runtime.path = None;
        runtime.roots = None;
        runtime.instance = None;
        runtime.generation = runtime.generation.wrapping_add(1);
        runtime.jobs.clear();
        runtime.next_job = 0;
        (runtime.server.take(), ticket)
    };
    // Drop undelivered bridge requests; their reply channels close and the
    // transport stops waiting. Late preflight results are rejected by the
    // generation check before they can dispatch.
    process.clear_agent_pending();
    if let Some(ticket) = ticket
        && let Some(fileops) = process.fileops()
    {
        fileops.revoke_agent_work(&ticket);
    }
    if let Some(mut server) = server {
        // Nonblocking cleanup: an OS filesystem worker may still be blocked.
        server.stop();
    }
    crate::log!("agent", "disabled; pending requests were dropped");
}

/// Persist one key and undo the in-memory change when the write fails.
///
/// `ConfigFile::apply` changes memory before it writes, so an error must not be
/// treated as success or leave the runtime value claiming the disk changed.
fn persist(process: &Rc<Process>, enabled: bool) -> Result<(), String> {
    let result = {
        let mut config = process.config.borrow_mut();
        config.apply_agent_enabled(enabled)
    };
    result.map_err(|error| error.to_string())
}

/// The Settings switch's one path. `agent.set-enabled` is already restricted
/// to `Caller::Ui` by the registry; this function is never reached for Agent,
/// Cli or Hotkey.
pub fn set_enabled(process: &Rc<Process>, enabled: bool) -> Outcome {
    if enabled {
        if let Err(error) = persist(process, true) {
            let mut runtime = process.agent.borrow_mut();
            runtime.state = AgentState::Unavailable;
            runtime.detail = Some(format!("could not save the setting: {error}"));
            crate::log!("agent", "enable refused: saving failed: {error}");
            return Outcome::Failed(format!("could not save the setting: {error}"));
        }
        if let Err(error) = enable(process) {
            // The preference was saved but the listener is not active. Put the
            // preference back so a restart does not claim a working socket.
            let _ = persist(process, false);
            let mut runtime = process.agent.borrow_mut();
            runtime.state = AgentState::Unavailable;
            runtime.detail = Some(error.clone());
            crate::log!("agent", "enable refused: {error}");
            return Outcome::Failed(error);
        }
    } else {
        // Revoke first: a save failure must never keep the old socket open.
        disable(process, None);
        if let Err(error) = persist(process, false) {
            let detail =
                format!("off for this session; the preference could not be saved: {error}");
            let mut runtime = process.agent.borrow_mut();
            runtime.detail = Some(detail.clone());
            crate::log!("agent", "disabled but could not save: {error}");
            return Outcome::Failed(detail);
        }
    }
    Outcome::Done
}

fn current_uid() -> Result<u32, String> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(|error| format!("could not read the current uid: {error}"))
}

pub fn status(process: &Process) -> AgentStatus {
    process.agent.borrow().snapshot()
}

/// Read and answer at most one batch of queued requests. The caller installs
/// this as a 50 ms GLib timeout, so a slow client cannot block browsing.
pub fn poll(process: &Rc<Process>) -> glib::ControlFlow {
    // Late filesystem evidence is checked against the generation before it is
    // applied. A revoked cycle leaves it unclaimed and inert.
    loop {
        let result = {
            let runtime = process.agent.borrow();
            runtime.work.try_result()
        };
        let Some(result) = result else {
            break;
        };
        finish_pending(process, result);
    }

    let mut budget = MAX_PENDING_REQUESTS;
    loop {
        if budget == 0 {
            break;
        }
        budget -= 1;
        let inbound = {
            let runtime = process.agent.borrow();
            runtime
                .receiver
                .as_ref()
                .and_then(|receiver| receiver.try_recv().ok())
        };
        let Some(inbound) = inbound else {
            break;
        };
        handle(process, inbound);
    }
    glib::ControlFlow::Continue
}

fn handle(process: &Rc<Process>, inbound: Inbound) {
    if inbound.is_abandoned() {
        crate::log!(
            "agent",
            "dropped abandoned request {}",
            inbound.request.request_id
        );
        return;
    }
    let generation = inbound.generation;
    let instance = inbound.instance.clone();
    let request_id = inbound.request.request_id.clone();
    let operation = inbound.request.operation.name();

    {
        let runtime = process.agent.borrow();
        if !runtime.enabled {
            drop(runtime);
            inbound.respond(error_reply(
                request_id,
                operation,
                "disabled",
                "local automation is off",
            ));
            return;
        }
        if runtime.generation != generation {
            drop(runtime);
            inbound.respond(error_reply(
                request_id,
                operation,
                "stale",
                "this request belongs to an earlier enable cycle",
            ));
            return;
        }
    }
    if let Some(requested) = &inbound.request.instance
        && Some(requested.as_str()) != Some(instance.as_str())
    {
        inbound.respond(error_reply(
            request_id,
            operation,
            "stale",
            "this request names a different application instance",
        ));
        return;
    }

    match &inbound.request.operation {
        Operation::Commands { offset, limit } => {
            let reply = commands_reply(&inbound.request, *offset, *limit, &instance, generation);
            inbound.respond(reply);
        }
        Operation::State { offset, limit } => {
            let reply = state_reply(
                process,
                &inbound.request,
                *offset,
                *limit,
                &instance,
                generation,
            );
            inbound.respond(reply);
        }
        Operation::Run {
            window,
            tab,
            command,
            arguments,
            ..
        } => match run_prepare(
            process,
            &inbound.request,
            &instance,
            generation,
            window.as_deref(),
            tab.as_deref(),
            command,
            arguments,
        ) {
            RunPrepared::Reply(reply) => inbound.respond(reply),
            RunPrepared::Deferred {
                work_id,
                kind,
                target,
                authorization,
            } => {
                process.store_pending_run(PendingRun {
                    inbound,
                    work_id,
                    target,
                    kind,
                    authorization,
                });
            }
        },
        Operation::Job { id } => {
            let reply = job_reply(process, &inbound.request, &instance, generation, id);
            inbound.respond(reply);
        }
    }
}

fn finish_pending(process: &Rc<Process>, result: work::WorkResult) {
    let Some(pending) = process.take_pending_run(result.id) else {
        return;
    };
    if pending.inbound.is_abandoned() {
        crate::log!(
            "agent",
            "dropped abandoned late result {}",
            pending.inbound.request.request_id
        );
        return;
    }
    let request_id = pending.inbound.request.request_id.clone();
    let operation = pending.inbound.request.operation.name();

    {
        let runtime = process.agent.borrow();
        if !runtime.enabled || runtime.generation != result.generation {
            drop(runtime);
            pending.inbound.respond(error_reply(
                request_id,
                operation,
                "stale",
                "this result belongs to an earlier enable cycle",
            ));
            return;
        }
    }
    // The asynchronous lookup may have outlived the requested window or its
    // tab. Refuse instead of falling through to whichever window is focused.
    if !process.execution_target_valid(pending.target) {
        pending.inbound.respond(error_reply(
            request_id,
            operation,
            "invalid-target",
            "the requested window or tab no longer exists",
        ));
        return;
    }
    if !pending.authorization.is_active() {
        pending.inbound.respond(error_reply(
            request_id,
            operation,
            "disabled",
            "local automation was turned off before this request could run",
        ));
        return;
    }

    match result.outcome {
        Err(message) => {
            let code = if matches!(pending.kind, PendingKind::Nav { .. }) {
                "invalid-target"
            } else {
                "invalid"
            };
            pending
                .inbound
                .respond(error_reply(request_id, operation, code, message));
        }
        Ok(work::WorkOutcome::DryRun(lines)) => {
            // A dry run is read-only: it never binds focus, selection, folder,
            // the Trash page or configuration.
            let reply = Reply {
                request_id,
                operation: operation.to_string(),
                status: Status::Ok,
                instance: pending.inbound.instance.clone().into(),
                generation: Some(result.generation),
                data: Data::Run {
                    kind: RunKind::DryRun,
                    job: None,
                    lines,
                },
                error: None,
            };
            pending.inbound.respond(reply);
        }
        Ok(work::WorkOutcome::Nav { folder, select }) => {
            let PendingKind::Nav { ref invocation } = pending.kind else {
                pending.inbound.respond(error_reply(
                    request_id,
                    operation,
                    "invalid",
                    "unexpected navigation evidence",
                ));
                return;
            };
            // Bind the exact window and tab after the lookup, immediately
            // before the model effect.
            if !process.bind_execution_target(pending.target) {
                pending.inbound.respond(error_reply(
                    request_id,
                    operation,
                    "invalid-target",
                    "the requested window or tab no longer exists",
                ));
                return;
            }
            process.set_pending_nav_target(Some((folder, select)));
            process.set_pending_authorization(Some(pending.authorization.clone()));
            let mut handler = crate::commands::UiHandler {
                caller: Caller::Agent,
                window: pending.target.window,
                process: Rc::downgrade(process),
            };
            let outcome = Registry::new().dispatch(Caller::Agent, invocation, &mut handler);
            process.set_pending_authorization(None);
            process.set_pending_nav_target(None);
            let reply = outcome_reply(outcome, request_id, operation, &pending, result.generation);
            pending.inbound.respond(reply);
        }
    }
}

fn error_reply(
    request_id: String,
    operation: &str,
    code: &'static str,
    message: impl Into<String>,
) -> Reply {
    let mut reply = Reply::error(request_id, operation, code, message);
    reply.status = Status::Error;
    reply
}

fn outcome_reply(
    outcome: Outcome,
    request_id: String,
    operation: &str,
    pending: &PendingRun,
    generation: u64,
) -> Reply {
    match outcome {
        Outcome::Error(error) => error_reply(request_id, operation, "refused", error.to_string()),
        Outcome::Failed(message) => error_reply(request_id, operation, "failed", message),
        Outcome::DryRun(lines) => Reply {
            request_id,
            operation: operation.to_string(),
            status: Status::Ok,
            instance: Some(pending.authorization.instance.clone()),
            generation: Some(generation),
            data: Data::Run {
                kind: RunKind::DryRun,
                job: None,
                lines,
            },
            error: None,
        },
        Outcome::Done => Reply {
            request_id,
            operation: operation.to_string(),
            status: Status::Ok,
            instance: Some(pending.authorization.instance.clone()),
            generation: Some(generation),
            data: Data::Run {
                kind: RunKind::Done,
                job: None,
                lines: Vec::new(),
            },
            error: None,
        },
    }
}

fn commands_reply(
    request: &Request,
    offset: usize,
    limit: usize,
    instance: &str,
    generation: u64,
) -> Reply {
    let registry = Registry::new();
    let all = registry.commands();
    let total = all.len();
    let limit = limit.clamp(1, fm_agent::protocol::MAX_PAGE);
    let end = offset.saturating_add(limit).min(total);
    let mut commands = Vec::new();
    if offset < total {
        for spec in &all[offset..end] {
            let mut allowed = caller_allowed(spec.name, spec.effect, Caller::Agent);
            let mut reason = None;
            if spec.effect == Effect::System {
                allowed = false;
                reason = Some(
                    "system operations are not available through the local socket".to_string(),
                );
            } else if !allowed {
                reason = Some(match spec.effect {
                    Effect::Files => "this permanent deletion is a UI-only action".to_string(),
                    _ => "this command is reserved for the user interface".to_string(),
                });
            }
            let arguments = spec
                .arguments()
                .into_iter()
                .map(|argument| ArgumentInfo {
                    name: argument.name.to_string(),
                    kind: kind_label(&argument.kind),
                    presence: presence_label(argument.presence),
                    default: argument.default.map(str::to_string),
                    required: must_supply(Caller::Agent, spec.effect, argument),
                })
                .collect();
            commands.push(CommandInfo {
                name: spec.name.to_string(),
                summary: spec.summary.to_string(),
                effect: spec.effect.as_str().to_string(),
                undo: spec.undo,
                allowed,
                reason,
                arguments,
            });
        }
    }
    let next = (end < total).then_some(end);
    let mut reply = Reply {
        request_id: request.request_id.clone(),
        operation: request.operation.name().to_string(),
        status: Status::Ok,
        instance: Some(instance.to_string()),
        generation: Some(generation),
        data: Data::Commands {
            offset,
            total,
            next,
            commands,
        },
        error: None,
    };
    reply.status = Status::Ok;
    reply
}

fn kind_label(kind: &ArgKind) -> String {
    match kind {
        ArgKind::Path => "path".to_string(),
        ArgKind::Paths => "paths".to_string(),
        ArgKind::TrashItems => "trash-items".to_string(),
        ArgKind::Int { min, max } => format!("int:{min}..={max}"),
        ArgKind::Number { min, max, step } => match step {
            Some(step) => format!("number:{min}..={max}:step{step}"),
            None => format!("number:{min}..={max}"),
        },
        ArgKind::Bool => "bool".to_string(),
        ArgKind::Text => "text".to_string(),
        ArgKind::Choice(values) => format!("choice:{}", values.join("|")),
    }
}

fn presence_label(presence: Presence) -> String {
    match presence {
        Presence::Required => "required".to_string(),
        Presence::Optional => "optional".to_string(),
        Presence::Context(source) => format!("from {}", source.as_str()),
    }
}

fn state_reply(
    process: &Rc<Process>,
    request: &Request,
    offset: usize,
    limit: usize,
    instance: &str,
    generation: u64,
) -> Reply {
    let windows = process.windows_snapshot();
    let focused = process.focused_window_id();
    let total = windows.len();
    let limit = limit.clamp(1, fm_agent::protocol::MAX_PAGE);
    let end = offset.saturating_add(limit).min(total);
    let mut page = Vec::new();
    let mut tab_budget = MAX_TABS_PER_STATE;
    if offset < total {
        for window in &windows[offset..end] {
            page.push(window_info(
                window,
                focused == Some(window.id),
                instance,
                &mut tab_budget,
            ));
        }
    }
    let next = (end < total).then_some(end);
    let status = process.agent.borrow().snapshot();
    let socket = SocketInfo {
        status: status.state.as_str().to_string(),
        path: status.path.as_ref().map(|path| path.display().to_string()),
        detail: status.detail.clone(),
        generation,
    };
    let jobs = process
        .agent
        .borrow()
        .jobs
        .iter()
        .rev()
        .take(STATE_JOBS)
        .map(|job| JobInfo {
            id: job.token.clone(),
            state: job.state.clone(),
            summary: job.summary.clone(),
            lines: job.lines.clone(),
            errors: job.errors.clone(),
        })
        .collect();
    Reply {
        request_id: request.request_id.clone(),
        operation: request.operation.name().to_string(),
        status: Status::Ok,
        instance: Some(instance.to_string()),
        generation: Some(generation),
        data: Data::State {
            offset,
            total,
            next,
            socket,
            windows: page,
            jobs,
        },
        error: None,
    }
}

fn window_info(
    window: &Window,
    focused: bool,
    instance: &str,
    tab_budget: &mut usize,
) -> WindowInfo {
    let focused_group = window.focused_group().map(|group| group.id);
    let groups_total = window
        .pills
        .iter()
        .filter(|id| window.groups.contains_key(id))
        .count();
    let mut groups = Vec::new();
    for id in window.pills.iter().take(MAX_GROUPS_PER_WINDOW) {
        let Some(group) = window.groups.get(id) else {
            continue;
        };
        let tabs_total = group.tabs.len();
        let take = (*tab_budget).min(tabs_total);
        *tab_budget -= take;
        groups.push(GroupInfo {
            id: format!("{instance}/g{}", group.id.raw()),
            name: group.name.clone(),
            number: group.number,
            focused: focused_group == Some(group.id),
            split: group.is_split(),
            tabs_total,
            tabs_truncated: take < tabs_total,
            tabs: group
                .tabs
                .iter()
                .take(take)
                .map(|tab| tab_info(window, tab, instance))
                .collect(),
        });
    }
    WindowInfo {
        id: format!("{instance}/w{}", window.id.raw()),
        focused,
        focused_tab: format!("{instance}/t{}", window.focused_tab.raw()),
        groups_total,
        groups_truncated: groups_total > MAX_GROUPS_PER_WINDOW,
        groups,
    }
}

fn tab_info(window: &Window, tab: &fm_core::workspace::Tab, instance: &str) -> TabInfo {
    let (count, names, truncated) =
        selection_page(tab.selection.names.iter().map(|name| name.as_os_str()));
    TabInfo {
        id: format!("{instance}/t{}", tab.id.raw()),
        folder: tab.folder.display().to_string(),
        folder_hex: hex(tab.folder.as_os_str().as_bytes()),
        focused: window.focused_tab == tab.id,
        selection_count: count,
        selection: names,
        selection_truncated: truncated,
    }
}

fn name_info(bytes: &[u8]) -> NameInfo {
    NameInfo {
        text: std::str::from_utf8(bytes).ok().map(str::to_string),
        hex: hex(bytes),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[allow(clippy::too_many_arguments)]
fn run_prepare(
    process: &Rc<Process>,
    request: &Request,
    instance: &str,
    generation: u64,
    window: Option<&str>,
    tab: Option<&str>,
    command: &str,
    arguments: &[(String, String)],
) -> RunPrepared {
    let request_id = request.request_id.clone();
    let operation = request.operation.name();

    // Pure registry and caller validation happens before any focus or model
    // change. A refused or malformed command leaves the workspace untouched.
    let Some(spec) = Registry::new().command(command) else {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "unknown-command",
            format!("unknown command `{command}`"),
        ));
    };
    if spec.effect == Effect::System {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "unsupported",
            "system operations are not available through the local socket",
        ));
    }
    if matches!(command, "app.quit" | "window.close") && process.user_prompt_pending() {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "busy",
            "Gimbal is waiting for a user to answer a dirty-close, saving or dialog prompt",
        ));
    }

    let window_id = match resolve_window(process, command, window, instance) {
        Ok(id) => id,
        Err(message) => {
            return RunPrepared::Reply(error_reply(
                request_id,
                operation,
                "invalid-target",
                message,
            ));
        }
    };
    let target = match resolve_target(process, instance, window_id, tab) {
        Ok(target) => target,
        Err(message) => {
            return RunPrepared::Reply(error_reply(
                request_id,
                operation,
                "invalid-target",
                message,
            ));
        }
    };

    let invocation = Invocation::new(command.to_string(), arguments.to_vec());
    let checked = match Registry::new().validate(Caller::Agent, &invocation) {
        Ok(checked) => checked,
        Err(error) => {
            let code = match error.kind() {
                fm_core::command::CommandErrorKind::ForbiddenCaller { .. }
                | fm_core::command::CommandErrorKind::ForbiddenFromContext { .. } => "forbidden",
                fm_core::command::CommandErrorKind::UnknownCommand { .. } => "unknown-command",
                _ => "invalid",
            };
            return RunPrepared::Reply(error_reply(request_id, operation, code, error.to_string()));
        }
    };

    let Some(authorization) = process.agent.borrow().authorization() else {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "disabled",
            "local automation is off",
        ));
    };

    let file_request = match fm_core::fileops::request_for(&checked) {
        Ok(request) => request,
        Err(error) => {
            return RunPrepared::Reply(error_reply(
                request_id,
                operation,
                "invalid",
                error.to_string(),
            ));
        }
    };

    // A dry run resolves its target/plan on the bounded worker. It never
    // focuses a tab or changes selection, folder or configuration.
    if checked.dry_run() {
        if command == "file.paste" {
            // Clipboard data arrives asynchronously; the accepted paste job
            // reports the real dry-run lines or failure.
        } else if let Some(file_request) = file_request {
            let work_id = process.next_work_id();
            let undo_lines = if matches!(file_request, FileRequest::Undo) {
                process.fileops().and_then(|fileops| {
                    let lines = fileops.undo_lines();
                    (!lines.is_empty()).then_some(lines)
                })
            } else {
                None
            };
            if let Err(message) = process.agent.borrow().work.try_submit(
                work_id,
                generation,
                work::Work::DryRun {
                    request: file_request,
                    undo_lines,
                },
            ) {
                return RunPrepared::Reply(error_reply(request_id, operation, "busy", message));
            }
            return RunPrepared::Deferred {
                work_id,
                kind: PendingKind::DryRun,
                target,
                authorization,
            };
        }
    }

    // `nav.go` resolves path type and existence on the worker; the GTK handler
    // consumes only the typed target.
    if command == "nav.go"
        && let Some(Value::Path(path)) = checked.get("path")
    {
        let work_id = process.next_work_id();
        if let Err(message) = process.agent.borrow().work.try_submit(
            work_id,
            generation,
            work::Work::Nav { path: path.clone() },
        ) {
            return RunPrepared::Reply(error_reply(request_id, operation, "busy", message));
        }
        return RunPrepared::Deferred {
            work_id,
            kind: PendingKind::Nav { invocation },
            target,
            authorization,
        };
    }

    // Queue pressure is checked before reserving any job.
    if let Some(file_request) = &file_request
        && matches!(
            file_request,
            FileRequest::Trash { .. }
                | FileRequest::Copy { .. }
                | FileRequest::Move { .. }
                | FileRequest::Rename { .. }
                | FileRequest::NewFolder { .. }
                | FileRequest::Restore { .. }
                | FileRequest::Undo
        )
        && process
            .fileops()
            .is_some_and(|fileops| fileops.waiting() >= MAX_PENDING_JOBS)
    {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "busy",
            "too many file jobs are already waiting",
        ));
    }

    // Reserve the queryable job before dispatch for file work. The file-op id
    // is attached after the real handler submits it.
    let needs_job = file_request.is_some() || command == "file.paste";
    let reserved = if needs_job {
        process
            .agent
            .borrow_mut()
            .reserve_job(spec.summary.to_string())
    } else {
        None
    };
    if needs_job && reserved.is_none() {
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "busy",
            "too many tracked jobs are already pending",
        ));
    }
    if let Some((_, token)) = &reserved
        && command == "file.paste"
    {
        process.set_reserved_job(Some(token.clone()));
    }

    // Every validation that can refuse the request has passed. Bind the exact
    // window and tab immediately before the effect. A dry run stays read-only
    // and never focuses or changes selection, folder or the Trash page.
    if !checked.dry_run() && !process.bind_execution_target(target) {
        if let Some((_, token)) = &reserved {
            fail_job(
                process,
                token,
                "the requested window or tab disappeared".to_string(),
            );
        }
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "invalid-target",
            "the requested window or tab no longer exists",
        ));
    }

    process.clear_command_job();
    process.set_pending_authorization(Some(authorization));
    let mut handler = crate::commands::UiHandler {
        caller: Caller::Agent,
        window: window_id,
        process: Rc::downgrade(process),
    };
    let outcome = Registry::new().dispatch(Caller::Agent, &invocation, &mut handler);
    process.set_pending_authorization(None);
    let fileops_job = process.take_command_job();
    if command == "file.paste"
        && let Some(token) = process.take_reserved_job()
    {
        fail_job(
            process,
            &token,
            "the clipboard paste did not start".to_string(),
        );
    }

    if let Outcome::Error(error) = &outcome {
        if let Some((_, token)) = &reserved {
            fail_job(process, token, error.to_string());
        }
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "refused",
            error.to_string(),
        ));
    }
    if let Outcome::Failed(message) = &outcome {
        if let Some((_, token)) = &reserved {
            fail_job(process, token, message.clone());
        }
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "failed",
            message.clone(),
        ));
    }

    if let Some((_id, token)) = reserved {
        if command == "file.paste" {
            // The clipboard callback owns this reservation. If the handler
            // could not start it, the reserved job is already failed.
            return RunPrepared::Reply(Reply {
                request_id,
                operation: operation.to_string(),
                status: Status::Accepted,
                instance: Some(instance.to_string()),
                generation: Some(generation),
                data: Data::Run {
                    kind: RunKind::Job,
                    job: Some(token),
                    lines: Vec::new(),
                },
                error: None,
            });
        }
        if let Some(fileops_id) = fileops_job {
            let running = process
                .fileops()
                .is_some_and(|fileops| fileops.current_id() == Some(fileops_id));
            attach_fileops(process, &token, fileops_id, running);
            return RunPrepared::Reply(Reply {
                request_id,
                operation: operation.to_string(),
                status: Status::Accepted,
                instance: Some(instance.to_string()),
                generation: Some(generation),
                data: Data::Run {
                    kind: RunKind::Job,
                    job: Some(token),
                    lines: Vec::new(),
                },
                error: None,
            });
        }
        fail_job(
            process,
            &token,
            "the file operation did not produce a job".to_string(),
        );
        return RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "failed",
            "the file operation did not produce a job",
        ));
    }

    match outcome {
        Outcome::Error(error) => RunPrepared::Reply(error_reply(
            request_id,
            operation,
            "refused",
            error.to_string(),
        )),
        Outcome::Failed(message) => {
            RunPrepared::Reply(error_reply(request_id, operation, "failed", message))
        }
        Outcome::DryRun(lines) => RunPrepared::Reply(Reply {
            request_id,
            operation: operation.to_string(),
            status: Status::Ok,
            instance: Some(instance.to_string()),
            generation: Some(generation),
            data: Data::Run {
                kind: RunKind::DryRun,
                job: None,
                lines,
            },
            error: None,
        }),
        Outcome::Done => RunPrepared::Reply(Reply {
            request_id,
            operation: operation.to_string(),
            status: Status::Ok,
            instance: Some(instance.to_string()),
            generation: Some(generation),
            data: Data::Run {
                kind: RunKind::Done,
                job: None,
                lines: Vec::new(),
            },
            error: None,
        }),
    }
}

fn resolve_window(
    process: &Rc<Process>,
    command: &str,
    requested: Option<&str>,
    instance: &str,
) -> Result<WindowId, String> {
    match requested {
        Some(text) => parse_window_id(process, instance, text)
            .ok_or_else(|| "the window id is not part of this application instance".to_string()),
        None if context_free(command) => process
            .focused_window_id()
            .ok_or_else(|| "no window is open".to_string()),
        None => Err(
            "this command needs an explicit window id from `state`; it never falls back to the \
             focused window"
                .to_string(),
        ),
    }
}

/// Resolve the exact requested/default tab for a window before any effect.
fn resolve_target(
    process: &Rc<Process>,
    instance: &str,
    window: WindowId,
    tab: Option<&str>,
) -> Result<ExecutionTarget, String> {
    let snapshot = process
        .window_snapshot(window)
        .ok_or_else(|| "the requested window no longer exists".to_string())?;
    let tab = match tab {
        Some(text) => {
            let id = parse_tab_id(process, instance, text)
                .ok_or_else(|| "the tab id is not part of this application instance".to_string())?;
            if snapshot.group_of(id).is_none() {
                return Err("the tab does not belong to the requested window".to_string());
            }
            id
        }
        None => {
            if snapshot.tab(snapshot.focused_tab).is_none() {
                return Err("the requested window has no tab".to_string());
            }
            snapshot.focused_tab
        }
    };
    Ok(ExecutionTarget { window, tab })
}

fn parse_window_id(process: &Rc<Process>, instance: &str, text: &str) -> Option<WindowId> {
    let raw = text.strip_prefix(&format!("{instance}/w"))?;
    let id = raw.parse::<u64>().ok()?;
    let id = WindowId::from_raw(id);
    process.window_snapshot(id).map(|_| id)
}

fn parse_tab_id(process: &Rc<Process>, instance: &str, text: &str) -> Option<TabId> {
    let raw = text.strip_prefix(&format!("{instance}/t"))?;
    let id = TabId::from_raw(raw.parse::<u64>().ok()?);
    let exists = process
        .windows_snapshot()
        .iter()
        .any(|window| window.group_of(id).is_some());
    exists.then_some(id)
}

fn job_reply(
    process: &Rc<Process>,
    request: &Request,
    instance: &str,
    generation: u64,
    id: &str,
) -> Reply {
    let info = process.agent.borrow_mut().job_by_token(id);
    let status = if info.state == "expired" || info.state == "unknown" {
        Status::Error
    } else {
        Status::Ok
    };
    let error = (status == Status::Error).then(|| fm_agent::protocol::ErrorBody {
        code: "unknown-job",
        message: "the job id is unknown or expired in this session".to_string(),
    });
    Reply {
        request_id: request.request_id.clone(),
        operation: request.operation.name().to_string(),
        status,
        instance: Some(instance.to_string()),
        generation: Some(generation),
        data: Data::Job(info),
        error,
    }
}

// ---------------------------------------------------------------------------
// Job events from the file-op worker
// ---------------------------------------------------------------------------

pub fn job_started(process: &Process, id: u64) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.state = "running".to_string();
    });
}

pub fn job_waiting(process: &Process, id: u64) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.state = "waiting-for-user".to_string();
    });
}

pub fn job_finished(
    process: &Process,
    id: u64,
    summary: String,
    lines: Vec<String>,
    failures: Vec<String>,
    cancelled: bool,
) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.summary = summary;
        job.lines = lines;
        job.errors = failures;
        job.state = if cancelled {
            "cancelled".to_string()
        } else if job.errors.is_empty() {
            "complete".to_string()
        } else {
            "failed".to_string()
        };
    });
}

pub fn job_failed(process: &Process, id: u64, message: String) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.state = "failed".to_string();
        job.errors = vec![message];
    });
}

pub fn job_revoked(process: &Process, id: u64, message: String) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.state = "cancelled".to_string();
        job.errors = vec![message];
    });
}

pub fn job_dry_run(process: &Process, id: u64, lines: Vec<String>) {
    process.agent.borrow_mut().job_by_fileops(id, |job| {
        job.state = "complete".to_string();
        job.lines = lines;
    });
}

/// The clipboard callback's one gate. `None` is an ordinary UI paste; an agent
/// paste must still hold its original, unrevoked ticket.
pub fn clipboard_callback_allowed(auth: &Option<JobAuthorization>) -> bool {
    auth.as_ref().is_none_or(JobAuthorization::is_active)
}

pub fn attach_fileops(process: &Process, token: &str, fileops_id: u64, running: bool) {
    process
        .agent
        .borrow_mut()
        .attach_fileops(token, fileops_id, running);
}

pub fn fail_job(process: &Process, token: &str, message: String) {
    process.agent.borrow_mut().fail_reserved(token, message);
}

/// A `FileOps` dry run that needed the worker carries the id so its completion
/// updates the right job.
/// Poll interval for the bridge.
pub const POLL: Duration = Duration::from_millis(50);

/// A pure helper for tests: whether a name is a context-free process action.
pub fn context_free(name: &str) -> bool {
    matches!(name, "window.new" | "app.quit")
}

/// The selected-name count as a pure, bounded projection.
pub fn selection_page<'a>(
    names: impl IntoIterator<Item = &'a OsStr>,
) -> (usize, Vec<NameInfo>, bool) {
    let names: Vec<&OsStr> = names.into_iter().collect();
    let total = names.len();
    let page = names
        .into_iter()
        .take(SELECTION_NAMES)
        .map(|name| name_info(name.as_bytes()))
        .collect();
    (total, page, total > SELECTION_NAMES)
}

pub fn _unused(_: &Path) {}

#[cfg(test)]
mod tests;
