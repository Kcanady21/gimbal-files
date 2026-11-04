// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive filename search: the worker pool, the per-window session and the
//! virtualized results page.
//!
//! Workers own only typed search requests, cancellation flags and bounded
//! queues. The GTK thread validates the exact window/tab/root/generation and
//! applies at most a few batches per main-loop tick.

mod view;

#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use fm_core::folder_size::{Clock, SystemClock};
use fm_core::search::{
    NativeSearchSource, PartialReason, RootFailure, SearchChange, SearchLimits, SearchMatch,
    SearchOutcome, SearchProgress, SearchRequest, SearchSource, SkipExample,
};
use fm_core::workspace::{TabId, Window, WindowId};
use gtk::prelude::*;
use gtk::{gdk, gio};

pub use view::SearchPage;

/// How often the main loop drains worker events.
pub const SEARCH_POLL_MS: u64 = 30;
/// At most this many worker events are applied to the view per UI tick.
pub const MAX_EVENTS_PER_TICK: usize = 4;
/// At most this many activation/root evidence results are routed per tick.
pub const MAX_EVIDENCE_RESULTS_PER_TICK: usize = 8;
/// At most this many window sessions exist process-wide.
pub const MAX_SESSIONS: usize = 8;
/// At most this many jobs wait in the shared worker queue.
const MAX_PENDING_JOBS: usize = 8;
/// At most two filesystem search workers exist for the process lifetime.
pub const MAX_WORKERS: usize = 2;
/// The worker-to-UI queue holds this many events before delivery waits.
const QUEUE_EVENTS: usize = 8;
/// A full queue wakes this often to re-check cancellation.
const QUEUE_WAIT_MS: u64 = 20;

// ---------------------------------------------------------------------------
// Bounded worker-to-UI queue
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum WorkerEvent {
    Batch {
        token: u64,
        changes: Vec<SearchChange>,
    },
    Finished {
        token: u64,
        outcome: SearchOutcome,
    },
}

#[derive(Debug, Default)]
struct QueueState {
    events: VecDeque<WorkerEvent>,
    abandoned: bool,
}

struct BatchQueue {
    state: Mutex<QueueState>,
    space: Condvar,
}

impl BatchQueue {
    fn new() -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            space: Condvar::new(),
        }
    }

    /// Push one event, waiting while the queue is full. A cancellation or an
    /// abandoned consumer wakes the wait immediately; a blocked native
    /// filesystem call later in the walk remains cooperative.
    fn push(&self, event: WorkerEvent, cancel: &AtomicBool) -> bool {
        let mut guard = self.state.lock().unwrap();
        loop {
            if cancel.load(Ordering::SeqCst) || guard.abandoned {
                return false;
            }
            if guard.events.len() < QUEUE_EVENTS {
                guard.events.push_back(event);
                self.space.notify_all();
                return true;
            }
            let (next, _) = self
                .space
                .wait_timeout(guard, Duration::from_millis(QUEUE_WAIT_MS))
                .unwrap();
            guard = next;
        }
    }

    fn pop(&self) -> Option<WorkerEvent> {
        self.state.lock().unwrap().events.pop_front()
    }

    fn abandon(&self) {
        let mut guard = self.state.lock().unwrap();
        guard.abandoned = true;
        guard.events.clear();
        self.space.notify_all();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state.lock().unwrap().events.len()
    }
}

#[derive(Default)]
struct ProgressCell {
    visited: AtomicU64,
    matches: AtomicU64,
    retained: AtomicU64,
    skipped: AtomicU64,
}

impl ProgressCell {
    fn new() -> Self {
        Self::default()
    }

    fn set(&self, progress: SearchProgress) {
        self.visited.store(progress.visited, Ordering::Relaxed);
        self.matches.store(progress.matches, Ordering::Relaxed);
        self.retained.store(progress.retained, Ordering::Relaxed);
        self.skipped.store(progress.skipped, Ordering::Relaxed);
    }

    fn snapshot(&self) -> SearchProgress {
        SearchProgress {
            visited: self.visited.load(Ordering::Relaxed),
            matches: self.matches.load(Ordering::Relaxed),
            retained: self.retained.load(Ordering::Relaxed),
            skipped: self.skipped.load(Ordering::Relaxed),
        }
    }
}

// ---------------------------------------------------------------------------
// Deferred activation and root-lifetime evidence
// ---------------------------------------------------------------------------

/// The text a trusted native result sends as `nav.go path=...` when its exact
/// path is not valid UTF-8. The registry validates this as an ordinary absolute
/// path; the handler consumes the one-shot typed evidence below instead of
/// treating this placeholder as an identity.
pub(crate) const NATIVE_NAV_PLACEHOLDER: &str = "/.gimbal-native-search-result";

/// The exact native result path, resolved on a worker. The `nav.go` handler
/// receives this as trusted, one-shot UI evidence and never repeats a path
/// lookup on the GTK thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct NativeNavTarget {
    pub folder: PathBuf,
    pub select: Option<OsString>,
}

/// Resolve one result for the existing `nav.go` handler. This is the only
/// filesystem lookup the activation path performs, and it runs on a worker.
pub(crate) fn resolve_native_navigation(path: &Path) -> Result<NativeNavTarget, String> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(NativeNavTarget {
            folder: path.to_path_buf(),
            select: None,
        }),
        Ok(metadata) if metadata.is_file() => Ok(NativeNavTarget {
            folder: path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("/")),
            select: path.file_name().map(OsString::from),
        }),
        Ok(_) => Err(format!(
            "{} is no longer a regular file or folder",
            path.display()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("{} no longer exists", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Err(format!(
            "{} cannot be checked: permission denied",
            path.display()
        )),
        Err(error) => Err(format!("{} cannot be opened: {error}", path.display())),
    }
}

/// Validate the root's lifetime against the resolved folder the walk used.
/// A removed alias, a removed target and an alias retargeted elsewhere all
/// fail. The caller runs this on a worker, never on GTK.
pub(crate) fn check_root_lifetime(
    root: &Path,
    expected_walked: Option<&Path>,
) -> Result<(), RootFailure> {
    let metadata = fs::metadata(root).map_err(|error| root_failure(&error))?;
    if !metadata.is_dir() {
        return Err(RootFailure::NotDirectory);
    }
    if let Some(expected) = expected_walked {
        match fs::canonicalize(root) {
            Ok(resolved) if resolved == expected => Ok(()),
            Ok(_) => Err(RootFailure::Changed),
            Err(error) => Err(root_failure(&error)),
        }
    } else {
        Ok(())
    }
}

fn root_failure(error: &std::io::Error) -> RootFailure {
    match error.kind() {
        std::io::ErrorKind::NotFound => RootFailure::Missing,
        std::io::ErrorKind::PermissionDenied => RootFailure::PermissionDenied,
        _ => RootFailure::Unreadable,
    }
}

/// How many evidence jobs may wait at once. Two fixed workers keep one blocked
/// lookup from stopping all UI control; a full queue is reported, never waited.
pub const MAX_EVIDENCE_QUEUE: usize = 8;
/// The activation/root-evidence worker count for the process lifetime.
pub const MAX_EVIDENCE_WORKERS: usize = 2;

pub(crate) type ActivationRunner =
    Arc<dyn Fn(&Path) -> Result<NativeNavTarget, String> + Send + Sync>;
pub(crate) type RootRunner =
    Arc<dyn Fn(&Path, Option<&Path>) -> Result<(), RootFailure> + Send + Sync>;

#[derive(Debug)]
enum EvidenceJobKind {
    Activation {
        generation: u64,
        path: PathBuf,
    },
    RootCheck {
        root: PathBuf,
        expected_walked: Option<PathBuf>,
    },
}

#[derive(Debug)]
struct EvidenceJob {
    window: WindowId,
    kind: EvidenceJobKind,
}

#[derive(Debug)]
enum EvidenceOutcome {
    Activation {
        generation: u64,
        path: PathBuf,
        target: Result<NativeNavTarget, String>,
    },
    RootCheck {
        outcome: Result<(), RootFailure>,
    },
}

#[derive(Debug)]
struct EvidenceResult {
    id: u64,
    window: WindowId,
    outcome: EvidenceOutcome,
}

/// The process-lifetime, bounded worker pool for result activation and root
/// checks. These are short, read-only filesystem lookups, never scans; the
/// walker's own fixed pool remains separate.
pub(crate) struct EvidencePool {
    sender: Option<SyncSender<(u64, Arc<AtomicBool>, EvidenceJob)>>,
    results: Mutex<Receiver<EvidenceResult>>,
    next_id: AtomicU64,
}

impl EvidencePool {
    pub(crate) fn start() -> Self {
        Self::with_runners(
            Arc::new(resolve_native_navigation),
            Arc::new(check_root_lifetime),
        )
    }

    pub(crate) fn with_runners(activation: ActivationRunner, root: RootRunner) -> Self {
        let (sender, jobs) = std::sync::mpsc::sync_channel::<(u64, Arc<AtomicBool>, EvidenceJob)>(
            MAX_EVIDENCE_QUEUE,
        );
        let (result_tx, results) = std::sync::mpsc::channel();
        let jobs = Arc::new(Mutex::new(jobs));
        for index in 0..MAX_EVIDENCE_WORKERS {
            let jobs = Arc::clone(&jobs);
            let result_tx = result_tx.clone();
            let activation = Arc::clone(&activation);
            let root_runner = Arc::clone(&root);
            thread::Builder::new()
                .name(format!("search-evidence-{index}"))
                .spawn(move || {
                    loop {
                        let job = match jobs.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        let Ok((id, cancel, job)) = job else {
                            return;
                        };
                        if cancel.load(Ordering::SeqCst) {
                            continue;
                        }
                        let outcome = match job.kind {
                            EvidenceJobKind::Activation { generation, path } => {
                                let target = activation(&path);
                                EvidenceOutcome::Activation {
                                    generation,
                                    path,
                                    target,
                                }
                            }
                            EvidenceJobKind::RootCheck {
                                root: root_path,
                                expected_walked,
                            } => EvidenceOutcome::RootCheck {
                                outcome: root_runner(&root_path, expected_walked.as_deref()),
                            },
                        };
                        if cancel.load(Ordering::SeqCst) {
                            continue;
                        }
                        if result_tx
                            .send(EvidenceResult {
                                id,
                                window: job.window,
                                outcome,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .expect("the search evidence thread starts");
        }
        Self {
            sender: Some(sender),
            results: Mutex::new(results),
            next_id: AtomicU64::new(1),
        }
    }

    fn try_submit(
        &self,
        window: WindowId,
        job: EvidenceJobKind,
        cancel: Arc<AtomicBool>,
    ) -> Result<u64, String> {
        let Some(sender) = &self.sender else {
            return Err("the activation worker pool is unavailable".to_string());
        };
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        match sender.try_send((id, cancel, EvidenceJob { window, kind: job })) {
            Ok(()) => Ok(id),
            Err(TrySendError::Full(_)) => {
                Err("too many result checks are already waiting".to_string())
            }
            Err(TrySendError::Disconnected(_)) => {
                Err("the activation worker pool is unavailable".to_string())
            }
        }
    }

    fn try_result(&self) -> Option<EvidenceResult> {
        self.results.lock().ok()?.try_recv().ok()
    }
}

impl Drop for EvidencePool {
    fn drop(&mut self) {
        // Closing the sender lets idle workers leave `recv`. Never join: a
        // blocked native lookup must not hold up GTK or app shutdown.
        drop(self.sender.take());
    }
}

fn production_evidence() -> Arc<EvidencePool> {
    static POOL: OnceLock<Arc<EvidencePool>> = OnceLock::new();
    POOL.get_or_init(|| Arc::new(EvidencePool::start())).clone()
}

// ---------------------------------------------------------------------------
// The fixed worker pool
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartOutcome {
    Started(u64),
    Busy,
    Stopped,
}

struct Job {
    token: u64,
    request: SearchRequest,
    cancel: Arc<AtomicBool>,
    events: Arc<BatchQueue>,
    progress: Arc<ProgressCell>,
}

#[derive(Default)]
struct ServiceQueue {
    pending: VecDeque<Job>,
    active: HashMap<u64, Arc<AtomicBool>>,
    running: usize,
    stopped: bool,
}

struct ServiceInner {
    queue: Mutex<ServiceQueue>,
    ready: Condvar,
    stopped: AtomicBool,
    workers: Mutex<Vec<JoinHandle<()>>>,
    next_token: AtomicU64,
    source: Arc<dyn SearchSource>,
    clock: Arc<dyn Clock>,
}

pub struct SearchService {
    inner: Arc<ServiceInner>,
}

impl SearchService {
    pub fn new() -> Self {
        Self::with_parts(Arc::new(NativeSearchSource), Arc::new(SystemClock))
    }

    pub(crate) fn with_parts(source: Arc<dyn SearchSource>, clock: Arc<dyn Clock>) -> Self {
        let inner = Arc::new(ServiceInner {
            queue: Mutex::new(ServiceQueue::default()),
            ready: Condvar::new(),
            stopped: AtomicBool::new(false),
            workers: Mutex::new(Vec::new()),
            next_token: AtomicU64::new(1),
            source,
            clock,
        });
        let workers: Vec<JoinHandle<()>> = (0..MAX_WORKERS)
            .map(|index| {
                let inner = inner.clone();
                thread::Builder::new()
                    .name(format!("search-{index}"))
                    .spawn(move || worker_loop(inner))
                    .expect("search worker")
            })
            .collect();
        *inner.workers.lock().unwrap() = workers;
        Self { inner }
    }

    fn start(
        &self,
        request: SearchRequest,
        cancel: Arc<AtomicBool>,
        events: Arc<BatchQueue>,
        progress: Arc<ProgressCell>,
    ) -> StartOutcome {
        if self.inner.stopped.load(Ordering::SeqCst) {
            return StartOutcome::Stopped;
        }
        let token = self.inner.next_token.fetch_add(1, Ordering::SeqCst);
        let job = Job {
            token,
            request,
            cancel,
            events,
            progress,
        };
        let mut queue = self.inner.queue.lock().unwrap();
        if queue.stopped || self.inner.stopped.load(Ordering::SeqCst) {
            return StartOutcome::Stopped;
        }
        if queue.pending.len() >= MAX_PENDING_JOBS {
            return StartOutcome::Busy;
        }
        queue.pending.push_back(job);
        self.inner.ready.notify_one();
        StartOutcome::Started(token)
    }

    fn cancel(&self, token: u64) {
        let mut queue = self.inner.queue.lock().unwrap();
        queue.pending.retain(|job| job.token != token);
        self.inner.ready.notify_all();
    }

    /// Stop accepting work and cancel queued jobs. Never joins: a worker
    /// blocked in a native filesystem call must not hold up app shutdown.
    fn stop(&self) {
        if self.inner.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let mut queue = self.inner.queue.lock().unwrap();
        queue.stopped = true;
        for job in queue.pending.drain(..) {
            job.cancel.store(true, Ordering::SeqCst);
            job.events.abandon();
        }
        for cancel in queue.active.values() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.inner.ready.notify_all();
    }

    #[cfg(test)]
    fn worker_count(&self) -> usize {
        self.inner.workers.lock().unwrap().len()
    }

    #[cfg(test)]
    fn pending_len(&self) -> usize {
        self.inner.queue.lock().unwrap().pending.len()
    }

    #[cfg(test)]
    fn running_count(&self) -> usize {
        self.inner.queue.lock().unwrap().running
    }
}

impl Default for SearchService {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for SearchService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn worker_loop(inner: Arc<ServiceInner>) {
    loop {
        let job = {
            let mut queue = inner.queue.lock().unwrap();
            loop {
                if queue.stopped {
                    return;
                }
                if let Some(job) = queue.pending.pop_front() {
                    queue.running += 1;
                    queue.active.insert(job.token, job.cancel.clone());
                    break job;
                }
                queue = inner.ready.wait(queue).unwrap();
            }
        };
        let token = job.token;
        let outcome = if job.cancel.load(Ordering::SeqCst) {
            SearchOutcome::Cancelled
        } else {
            let events = job.events.clone();
            let progress = job.progress.clone();
            fm_core::search::search(
                &job.request,
                inner.source.as_ref(),
                inner.clock.as_ref(),
                &|| job.cancel.load(Ordering::SeqCst),
                &mut |snapshot| progress.set(snapshot),
                &mut |changes| events.push(WorkerEvent::Batch { token, changes }, &job.cancel),
            )
        };
        {
            let mut queue = inner.queue.lock().unwrap();
            queue.active.remove(&token);
            queue.running = queue.running.saturating_sub(1);
        }
        inner.ready.notify_all();
        job.events
            .push(WorkerEvent::Finished { token, outcome }, &job.cancel);
    }
}

fn production_service() -> Arc<SearchService> {
    static SERVICE: OnceLock<Arc<SearchService>> = OnceLock::new();
    SERVICE
        .get_or_init(|| Arc::new(SearchService::new()))
        .clone()
}

// ---------------------------------------------------------------------------
// Per-window sessions
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchPhase {
    Searching { queued: bool },
    Complete,
    Partial(Vec<PartialReason>),
    Cancelled,
    Unavailable(RootFailure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchView {
    pub generation: u64,
    pub root: PathBuf,
    pub walked_root: Option<PathBuf>,
    pub query: String,
    /// Retained ranked rows currently in the visible list.
    pub rows: usize,
    /// Every match encountered, including ones outside the retained best set.
    pub total_matches: u64,
    pub truncated: bool,
    pub visited: u64,
    pub skipped: u64,
    pub mounts_skipped: u64,
    pub examples: Vec<SkipExample>,
    pub phase: SearchPhase,
    pub notice: Option<String>,
}

/// A completed activation lookup for the app to validate against the current
/// model once more and then route through the registry.
#[derive(Clone, Debug)]
pub(crate) struct ActivationReady {
    pub generation: u64,
    pub path: PathBuf,
    pub target: Result<NativeNavTarget, String>,
}

#[derive(Default)]
pub struct SearchTick {
    /// Bounded ranked inserts/removals from the worker, in application order.
    pub changes: Vec<SearchChange>,
    pub changed: bool,
    pub alive: bool,
    pub terminal: bool,
    pub activation: Option<ActivationReady>,
    /// The active root (or its alias) disappeared while the results page was
    /// open. The session has already been removed and its work revoked.
    pub revoked: Option<RootFailure>,
}

#[derive(Clone)]
struct ActiveScan {
    token: u64,
    cancel: Arc<AtomicBool>,
    events: Arc<BatchQueue>,
    progress: Arc<ProgressCell>,
}

struct PendingScan {
    query: String,
    root: PathBuf,
    limits: SearchLimits,
}

struct PendingActivation {
    id: u64,
    generation: u64,
    path: PathBuf,
    cancel: Arc<AtomicBool>,
}

struct PendingRootCheck {
    id: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
}

/// A nonrecursive root/parent pair. Dropping this cancels both monitors.
struct RootMonitor {
    file: gio::FileMonitor,
    parent: Option<gio::FileMonitor>,
    dirty: Arc<AtomicBool>,
}

impl Drop for RootMonitor {
    fn drop(&mut self) {
        self.file.cancel();
        if let Some(parent) = &self.parent {
            parent.cancel();
        }
    }
}

struct WindowSession {
    generation: u64,
    tab: TabId,
    root: PathBuf,
    query: String,
    rows: Vec<SearchMatch>,
    total_matches: u64,
    truncated: bool,
    phase: SearchPhase,
    visited: u64,
    skipped: u64,
    mounts_skipped: u64,
    examples: Vec<SkipExample>,
    walked_root: Option<PathBuf>,
    notice: Option<String>,
    current: Option<ActiveScan>,
    pending: Option<PendingScan>,
    activation: Option<PendingActivation>,
    activation_ready: Option<ActivationReady>,
    root_monitor: Option<RootMonitor>,
    root_check: Option<PendingRootCheck>,
    root_dirty: bool,
    root_lost: Option<RootFailure>,
}

impl WindowSession {
    fn view(&self) -> SearchView {
        SearchView {
            generation: self.generation,
            root: self.root.clone(),
            walked_root: self.walked_root.clone(),
            query: self.query.clone(),
            rows: self.rows.len(),
            total_matches: self.total_matches,
            truncated: self.truncated,
            visited: self.visited,
            skipped: self.skipped,
            mounts_skipped: self.mounts_skipped,
            examples: self.examples.clone(),
            phase: self.phase.clone(),
            notice: self.notice.clone(),
        }
    }

    /// Apply ranked changes in the order the core emitted them. The index is
    /// only a position; the path is the identity used for removal checks and
    /// for activation, so a reorder can never make Enter hit a new occupant.
    fn apply_changes(&mut self, changes: Vec<SearchChange>) -> Vec<SearchChange> {
        let mut applied = Vec::with_capacity(changes.len());
        for change in changes {
            match &change {
                SearchChange::Insert { index, row } => {
                    let index = (*index).min(self.rows.len());
                    self.rows.insert(index, row.clone());
                }
                SearchChange::Remove { index, path } => {
                    let position = if self.rows.get(*index).is_some_and(|row| row.path == *path) {
                        *index
                    } else {
                        match self.rows.iter().position(|row| row.path == *path) {
                            Some(position) => position,
                            None => continue,
                        }
                    };
                    self.rows.remove(position);
                }
            }
            applied.push(change);
        }
        applied
    }

    /// Revoke every piece of asynchronous work this session owns. Late worker
    /// completions carry an id/generation that can no longer match.
    fn revoke_work(&mut self, service: &SearchService) {
        if let Some(active) = self.current.take() {
            active.cancel.store(true, Ordering::SeqCst);
            service.cancel(active.token);
            active.events.abandon();
        }
        self.pending = None;
        if let Some(activation) = self.activation.take() {
            activation.cancel.store(true, Ordering::SeqCst);
        }
        if let Some(check) = self.root_check.take() {
            check.cancel.store(true, Ordering::SeqCst);
        }
        self.root_monitor = None;
        self.root_dirty = false;
        self.root_lost = None;
        self.activation_ready = None;
    }

    fn needs_poll(&self) -> bool {
        self.current.is_some()
            || self.pending.is_some()
            || self.activation.is_some()
            || self.activation_ready.is_some()
            || self.root_check.is_some()
            || self.root_dirty
            || self.root_lost.is_some()
            || self
                .root_monitor
                .as_ref()
                .is_some_and(|monitor| monitor.dirty.load(Ordering::SeqCst))
    }

    fn take_monitor_event(&mut self) {
        if self
            .root_monitor
            .as_ref()
            .is_some_and(|monitor| monitor.dirty.swap(false, Ordering::SeqCst))
        {
            self.root_dirty = true;
        }
    }

    fn finish(&mut self, outcome: SearchOutcome) {
        match outcome {
            SearchOutcome::Unavailable(reason) => {
                self.phase = SearchPhase::Unavailable(reason);
                self.rows.clear();
                self.total_matches = 0;
                self.truncated = false;
            }
            SearchOutcome::Cancelled => {
                self.phase = SearchPhase::Cancelled;
            }
            SearchOutcome::Finished(report) => {
                self.visited = self.visited.max(report.visited);
                self.skipped = report.skipped;
                self.mounts_skipped = report.mounts_skipped;
                self.examples = report.examples;
                self.total_matches = report.matches;
                self.truncated = report.truncated;
                self.walked_root = Some(report.walked_root);
                self.phase = if report.cancelled {
                    SearchPhase::Cancelled
                } else if report.reasons.is_empty() {
                    SearchPhase::Complete
                } else {
                    SearchPhase::Partial(report.reasons)
                };
            }
        }
    }
}

pub struct SearchRuntime {
    service: Arc<SearchService>,
    evidence: Arc<EvidencePool>,
    state: RefCell<RuntimeState>,
    next_generation: Cell<u64>,
    wake: RefCell<Option<Rc<dyn Fn()>>>,
    #[cfg(test)]
    limits: Cell<SearchLimits>,
}

#[derive(Default)]
struct RuntimeState {
    sessions: BTreeMap<WindowId, WindowSession>,
}

impl Drop for SearchRuntime {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        for session in state.sessions.values_mut() {
            session.revoke_work(&self.service);
        }
    }
}

impl SearchRuntime {
    pub fn production() -> Self {
        Self::with_evidence(production_service(), production_evidence())
    }

    #[cfg(test)]
    pub(crate) fn with_service(service: Arc<SearchService>) -> Self {
        Self::with_evidence(service, Arc::new(EvidencePool::start()))
    }

    pub(crate) fn with_evidence(service: Arc<SearchService>, evidence: Arc<EvidencePool>) -> Self {
        Self {
            service,
            evidence,
            state: RefCell::new(RuntimeState::default()),
            next_generation: Cell::new(0),
            wake: RefCell::new(None),
            #[cfg(test)]
            limits: Cell::new(SearchLimits::default()),
        }
    }

    /// The app supplies a wake-up that restarts its search poll timer after a
    /// root monitor event. Without it, tests poll manually.
    pub fn set_wake(&self, wake: impl Fn() + 'static) {
        *self.wake.borrow_mut() = Some(Rc::new(wake));
    }

    /// Submit a query for the exact window/tab/root captured by the caller.
    /// A second submission replaces the current session in the same window.
    pub fn submit(
        &self,
        window: WindowId,
        tab: TabId,
        root: PathBuf,
        query: String,
    ) -> Result<SearchView, String> {
        if query.is_empty() {
            return Err("Search needs a nonempty query.".to_string());
        }
        let generation = self.next_generation.get().wrapping_add(1);
        self.next_generation.set(generation);
        #[cfg(test)]
        let limits = self.limits.get();
        #[cfg(not(test))]
        let limits = SearchLimits::default();
        let mut state = self.state.borrow_mut();
        if let Some(session) = state.sessions.get_mut(&window) {
            session.revoke_work(&self.service);
            session.generation = generation;
            session.tab = tab;
            session.root = root.clone();
            session.query = query.clone();
            session.rows.clear();
            session.total_matches = 0;
            session.truncated = false;
            session.phase = SearchPhase::Searching { queued: false };
            session.visited = 0;
            session.skipped = 0;
            session.mounts_skipped = 0;
            session.examples.clear();
            session.walked_root = None;
            session.notice = None;
            session.pending = Some(PendingScan {
                query,
                root: root.clone(),
                limits,
            });
            start_pending(session, &self.service);
            self.install_root_monitor(session);
            return Ok(session.view());
        }
        if state.sessions.len() >= MAX_SESSIONS {
            return Err(
                "Search is busy: eight windows already have a search open. Close one search and try again."
                    .to_string(),
            );
        }
        let mut session = WindowSession {
            generation,
            tab,
            root: root.clone(),
            query: query.clone(),
            rows: Vec::new(),
            total_matches: 0,
            truncated: false,
            phase: SearchPhase::Searching { queued: false },
            visited: 0,
            skipped: 0,
            mounts_skipped: 0,
            examples: Vec::new(),
            walked_root: None,
            notice: None,
            current: None,
            pending: Some(PendingScan {
                query,
                root,
                limits,
            }),
            activation: None,
            activation_ready: None,
            root_monitor: None,
            root_check: None,
            root_dirty: false,
            root_lost: None,
        };
        start_pending(&mut session, &self.service);
        self.install_root_monitor(&mut session);
        let view = session.view();
        state.sessions.insert(window, session);
        Ok(view)
    }

    /// A focused, nonrecursive pair of monitors: the root object itself and
    /// its parent directory. Monitor events only mark the session dirty and
    /// wake the app; a worker performs the actual metadata/canonicalize check.
    fn install_root_monitor(&self, session: &mut WindowSession) {
        let wake = self.wake.borrow().clone();
        let root = session.root.clone();
        let dirty = Arc::new(AtomicBool::new(false));
        let parent = root.parent().map(Path::to_path_buf);
        let Ok(file) = gio::File::for_path(&root)
            .monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
        else {
            crate::log!(
                "search",
                "cannot monitor search root {}; completed root changes will not be observed",
                root.display()
            );
            return;
        };
        let mark = {
            let dirty = dirty.clone();
            let wake = wake.clone();
            move |event: gio::FileMonitorEvent| {
                crate::log!(
                    "search",
                    "root monitor event {event:?} on {}",
                    root.display()
                );
                dirty.store(true, Ordering::SeqCst);
                if let Some(wake) = &wake {
                    wake();
                }
            }
        };
        let mark_file = mark.clone();
        file.connect_changed(move |_, _, _, event| mark_file(event));
        let parent_monitor = parent.and_then(|parent| {
            match gio::File::for_path(&parent).monitor_directory(
                gio::FileMonitorFlags::WATCH_MOVES,
                None::<&gio::Cancellable>,
            ) {
                Ok(monitor) => {
                    let watched = session.root.clone();
                    let mark_parent = mark.clone();
                    monitor.connect_changed(move |_, file, _, event| {
                        if file.path().is_some_and(|changed| changed == watched) {
                            mark_parent(event);
                        }
                    });
                    Some(monitor)
                }
                Err(error) => {
                    crate::log!(
                        "search",
                        "cannot monitor search root parent {}: {error}",
                        parent.display()
                    );
                    None
                }
            }
        });
        session.root_monitor = Some(RootMonitor {
            file,
            parent: parent_monitor,
            dirty,
        });
    }

    /// Restart the session's current query explicitly, used by Folder Refresh
    /// while results are visible.
    pub fn restart(&self, window: WindowId) -> Option<Result<SearchView, String>> {
        let (tab, root, query) = {
            let state = self.state.borrow();
            let session = state.sessions.get(&window)?;
            (session.tab, session.root.clone(), session.query.clone())
        };
        Some(self.submit(window, tab, root, query))
    }

    pub fn close(&self, window: WindowId) -> bool {
        let removed = {
            let mut state = self.state.borrow_mut();
            state.sessions.remove(&window)
        };
        match removed {
            Some(mut session) => {
                session.revoke_work(&self.service);
                true
            }
            None => false,
        }
    }

    /// Cancel and close every search session whose root is under one mount
    /// root. Returns the affected windows so the caller can leave the results
    /// page without touching a folder listing.
    pub fn cancel_under(&self, root: &Path) -> Vec<WindowId> {
        let windows: Vec<WindowId> = self
            .state
            .borrow()
            .sessions
            .iter()
            .filter(|(_, session)| {
                session.root.starts_with(root)
                    || session
                        .walked_root
                        .as_ref()
                        .is_some_and(|walked| walked.starts_with(root))
            })
            .map(|(window, _)| *window)
            .collect();
        for window in &windows {
            self.close(*window);
        }
        windows
    }

    /// Close every session and cancel its pending work. Called on app quit.
    pub fn shutdown(&self) {
        let windows: Vec<WindowId> = self.state.borrow().sessions.keys().copied().collect();
        for window in windows {
            self.close(window);
        }
    }

    /// A monitor event, or the test equivalent, asks for one worker check.
    #[cfg(test)]
    pub(crate) fn root_changed(&self, window: WindowId) {
        let marked = {
            let mut state = self.state.borrow_mut();
            match state.sessions.get_mut(&window) {
                Some(session) => {
                    session.root_dirty = true;
                    true
                }
                None => false,
            }
        };
        if marked && let Some(wake) = self.wake.borrow().as_ref() {
            wake();
        }
    }

    pub fn session_count(&self) -> usize {
        self.state.borrow().sessions.len()
    }

    pub fn sessions(&self) -> Vec<WindowId> {
        self.state.borrow().sessions.keys().copied().collect()
    }

    /// Whether any session still has worker work to drain or retry, a pending
    /// activation/root check, or a monitor event waiting for a root check.
    pub fn needs_poll(&self) -> bool {
        self.state
            .borrow()
            .sessions
            .values()
            .any(WindowSession::needs_poll)
    }

    #[cfg(test)]
    pub(crate) fn set_limits_for_tests(&self, limits: SearchLimits) {
        self.limits.set(limits);
    }

    #[cfg(test)]
    pub(crate) fn monitor_count_for_tests(&self) -> usize {
        self.state
            .borrow()
            .sessions
            .values()
            .filter(|session| session.root_monitor.is_some())
            .count()
    }

    pub fn view(&self, window: WindowId) -> Option<SearchView> {
        self.state
            .borrow()
            .sessions
            .get(&window)
            .map(WindowSession::view)
    }

    /// Validate a model window against the session's captured tab, folder and
    /// focus. Returns `false` when a session existed and was revoked.
    pub fn validate(&self, window: WindowId, model_window: &Window) -> bool {
        let invalid = self
            .state
            .borrow()
            .sessions
            .get(&window)
            .is_some_and(|session| {
                model_window.id != window
                    || model_window.focused_tab != session.tab
                    || model_window
                        .tab(session.tab)
                        .is_none_or(|tab| tab.folder != session.root)
            });
        if invalid {
            self.close(window);
        }
        !invalid
    }

    pub fn notice(&self, window: WindowId, message: &str) {
        if let Some(session) = self.state.borrow_mut().sessions.get_mut(&window) {
            session.notice = Some(message.to_string());
        }
    }

    pub fn clear_notice(&self, window: WindowId) {
        if let Some(session) = self.state.borrow_mut().sessions.get_mut(&window) {
            session.notice = None;
        }
    }

    pub fn result(&self, window: WindowId, generation: u64, index: usize) -> Option<SearchMatch> {
        let state = self.state.borrow();
        let session = state.sessions.get(&window)?;
        if session.generation != generation {
            return None;
        }
        session.rows.get(index).cloned()
    }

    /// Start one deferred, exact-path activation lookup. The GTK thread never
    /// resolves the path; a fixed worker does. At most one activation may be
    /// pending per window.
    pub(crate) fn begin_activation(
        &self,
        window: WindowId,
        generation: u64,
        path: PathBuf,
        model_window: &Window,
    ) -> ActivationStart {
        let invalid = {
            let state = self.state.borrow();
            match state.sessions.get(&window) {
                Some(session) => {
                    !activation_matches(session, window, generation, &path, model_window)
                }
                None => true,
            }
        };
        if invalid {
            self.close(window);
            return ActivationStart::Revoked;
        }

        let mut state = self.state.borrow_mut();
        let Some(session) = state.sessions.get_mut(&window) else {
            return ActivationStart::Revoked;
        };
        if let Some(pending) = &session.activation {
            if pending.generation == generation && pending.path == path {
                return ActivationStart::AlreadyPending;
            }
            return ActivationStart::Busy(
                "Another search result is already being checked. Wait for it or press Escape."
                    .to_string(),
            );
        }
        let cancel = Arc::new(AtomicBool::new(false));
        match self.evidence.try_submit(
            window,
            EvidenceJobKind::Activation {
                generation,
                path: path.clone(),
            },
            cancel.clone(),
        ) {
            Ok(id) => {
                session.activation = Some(PendingActivation {
                    id,
                    generation,
                    path,
                    cancel,
                });
                session.activation_ready = None;
                session.notice = None;
                ActivationStart::Started
            }
            Err(message) => {
                cancel.store(true, Ordering::SeqCst);
                ActivationStart::Busy(format!("Search is busy: {message}. Try again."))
            }
        }
    }

    /// Revalidate the exact target after the worker returns. The app calls this
    /// with a fresh model snapshot immediately before its registry dispatch.
    pub(crate) fn activation_valid(
        &self,
        window: WindowId,
        generation: u64,
        path: &Path,
        model_window: &Window,
    ) -> bool {
        let state = self.state.borrow();
        let Some(session) = state.sessions.get(&window) else {
            return false;
        };
        activation_matches(session, window, generation, path, model_window)
    }

    /// Drain a bounded number of events. The caller applies `changes` on the
    /// GTK thread and then asks for the current [`SearchView`] if `changed`.
    pub fn poll(&self, window: WindowId) -> SearchTick {
        let mut tick = SearchTick::default();
        let mut state = self.state.borrow_mut();
        self.drain_evidence(&mut state);
        let Some(session) = state.sessions.get_mut(&window) else {
            return tick;
        };
        tick.alive = true;
        session.take_monitor_event();
        if let Some(reason) = session.root_lost.take() {
            if let Some(mut removed) = state.sessions.remove(&window) {
                removed.revoke_work(&self.service);
            }
            tick.alive = false;
            tick.revoked = Some(reason);
            return tick;
        }
        if let Some(active) = session.current.clone() {
            for _ in 0..MAX_EVENTS_PER_TICK {
                let Some(event) = active.events.pop() else {
                    break;
                };
                match event {
                    WorkerEvent::Batch { token, changes } if token == active.token => {
                        if changes.is_empty() {
                            continue;
                        }
                        crate::log!(
                            "search",
                            "generation {} applied worker {} batch of {} change(s), {} retained before",
                            session.generation,
                            token,
                            changes.len(),
                            session.rows.len()
                        );
                        let applied = session.apply_changes(changes);
                        if !applied.is_empty() {
                            tick.changes.extend(applied);
                            tick.changed = true;
                        }
                    }
                    WorkerEvent::Finished { token, outcome } if token == active.token => {
                        session.finish(outcome);
                        crate::log!(
                            "search",
                            "generation {} worker {} finished; retained {} of {} match(es), visited {}",
                            session.generation,
                            token,
                            session.rows.len(),
                            session.total_matches,
                            session.visited
                        );
                        crate::log!(
                            "search",
                            "generation {} state {:?}",
                            session.generation,
                            session.phase
                        );
                        session.current = None;
                        tick.changed = true;
                        tick.terminal = true;
                        break;
                    }
                    WorkerEvent::Batch { token, changes } => {
                        crate::log!(
                            "search",
                            "generation {} discarded stale batch from worker {} ({} change(s))",
                            session.generation,
                            token,
                            changes.len()
                        );
                    }
                    WorkerEvent::Finished { token, .. } => {
                        crate::log!(
                            "search",
                            "generation {} discarded stale completion from worker {}",
                            session.generation,
                            token
                        );
                    }
                }
            }
            if let Some(current) = &session.current {
                let progress = current.progress.snapshot();
                let changed = progress.visited > session.visited
                    || progress.skipped > session.skipped
                    || progress.matches > session.total_matches;
                if changed {
                    session.visited = session.visited.max(progress.visited);
                    session.skipped = session.skipped.max(progress.skipped);
                    session.total_matches = session.total_matches.max(progress.matches);
                    tick.changed = true;
                }
            }
        }
        // Schedule one bounded root validation after the monitor event. After
        // Finished the walked root is available, so an alias can be compared
        // with the folder actually searched.
        if session.root_dirty && session.root_check.is_none() {
            let cancel = Arc::new(AtomicBool::new(false));
            let job = EvidenceJobKind::RootCheck {
                root: session.root.clone(),
                expected_walked: session.walked_root.clone(),
            };
            match self.evidence.try_submit(window, job, cancel.clone()) {
                Ok(id) => {
                    session.root_check = Some(PendingRootCheck {
                        id,
                        generation: session.generation,
                        cancel,
                    });
                    session.root_dirty = false;
                }
                Err(_) => {
                    cancel.store(true, Ordering::SeqCst);
                    // Leave root_dirty set; a later tick retries rather than
                    // dropping the only lifetime evidence.
                }
            }
        }
        if let Some(ready) = session.activation_ready.take() {
            tick.activation = Some(ready);
        }
        if session.current.is_none() && session.pending.is_some() {
            start_pending(session, &self.service);
            tick.changed = true;
        }
        tick
    }

    /// Route worker evidence back to the exact session that requested it. A
    /// stale id/generation cannot touch a replacement session.
    fn drain_evidence(&self, state: &mut RuntimeState) {
        for _ in 0..MAX_EVIDENCE_RESULTS_PER_TICK {
            let Some(result) = self.evidence.try_result() else {
                break;
            };
            let Some(session) = state.sessions.get_mut(&result.window) else {
                continue;
            };
            match result.outcome {
                EvidenceOutcome::Activation {
                    generation,
                    path,
                    target,
                } => {
                    let current = session.activation.as_ref().is_some_and(|pending| {
                        pending.id == result.id
                            && pending.generation == session.generation
                            && pending.generation == generation
                            && pending.path == path
                    });
                    if current {
                        session.activation = None;
                        session.activation_ready = Some(ActivationReady {
                            generation,
                            path,
                            target,
                        });
                    }
                }
                EvidenceOutcome::RootCheck { outcome } => {
                    let current = session.root_check.as_ref().is_some_and(|check| {
                        check.id == result.id && check.generation == session.generation
                    });
                    if current {
                        session.root_check = None;
                        if let Err(reason) = outcome
                            && session.root_lost.is_none()
                        {
                            session.root_lost = Some(reason);
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ActivationStart {
    Started,
    AlreadyPending,
    Busy(String),
    Revoked,
}

fn activation_matches(
    session: &WindowSession,
    window: WindowId,
    generation: u64,
    path: &Path,
    model_window: &Window,
) -> bool {
    model_window.id == window
        && session.generation == generation
        && session.tab == model_window.focused_tab
        && model_window
            .tab(session.tab)
            .is_some_and(|tab| tab.folder == session.root)
        && session.rows.iter().any(|row| row.path.as_path() == path)
}

fn start_pending(session: &mut WindowSession, service: &SearchService) {
    let Some(pending) = session.pending.take() else {
        return;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let events = Arc::new(BatchQueue::new());
    let progress = Arc::new(ProgressCell::new());
    let mut request = SearchRequest::new(pending.root.clone(), pending.query.clone());
    request.limits = pending.limits;
    match service.start(request, cancel.clone(), events.clone(), progress.clone()) {
        StartOutcome::Started(token) => {
            crate::log!(
                "search",
                "generation {} queued worker token {} for {} (query {} scalar(s))",
                session.generation,
                token,
                session.root.display(),
                session.query.chars().count()
            );
            session.current = Some(ActiveScan {
                token,
                cancel,
                events,
                progress,
            });
            session.phase = SearchPhase::Searching { queued: false };
        }
        StartOutcome::Busy => {
            session.pending = Some(pending);
            session.phase = SearchPhase::Searching { queued: true };
        }
        StartOutcome::Stopped => {
            session.phase = SearchPhase::Cancelled;
        }
    }
}

// ---------------------------------------------------------------------------
// Pure routing, status and keyboard ownership
// ---------------------------------------------------------------------------

/// Commands refused while the read-only results page is visible. `nav.go`,
/// `nav.refresh`, `search.open` and view settings are deliberately allowed.
pub fn refusal_for(command: &str) -> Option<&'static str> {
    match command {
        "preview.edit" => Some(
            "Editing is unavailable while search results are shown. Press Escape to close search first.",
        ),
        "selection.all" | "selection.none" => Some(
            "Selection commands are unavailable while search results are shown. Press Escape to close search first.",
        ),
        "file.open"
        | "file.trash"
        | "file.copy"
        | "file.move"
        | "file.rename"
        | "file.new-folder"
        | "file.undo"
        | "file.cut"
        | "file.copy-to-clipboard"
        | "file.paste" => Some(
            "File actions are unavailable while search results are shown. Press Escape to close search first.",
        ),
        _ => None,
    }
}

/// Keys the search entry must keep while it has focus, so a configured global
/// hotkey cannot move the results list or run a command instead.
pub fn search_entry_owns_key(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    if owns_modifier(modifiers) {
        return false;
    }
    matches!(
        key,
        gdk::Key::Return
            | gdk::Key::KP_Enter
            | gdk::Key::Tab
            | gdk::Key::ISO_Left_Tab
            | gdk::Key::Home
            | gdk::Key::End
            | gdk::Key::Up
            | gdk::Key::Down
            | gdk::Key::Left
            | gdk::Key::Right
            | gdk::Key::Page_Up
            | gdk::Key::Page_Down
            | gdk::Key::Escape
    )
}

/// Keys the results list owns: arrows/Home/End move rows, Enter activates and
/// Escape closes search. No second key controller is installed on the list.
pub fn search_results_own_key(key: gdk::Key, modifiers: gdk::ModifierType) -> bool {
    if owns_modifier(modifiers) {
        return false;
    }
    matches!(
        key,
        gdk::Key::Return
            | gdk::Key::KP_Enter
            | gdk::Key::Tab
            | gdk::Key::ISO_Left_Tab
            | gdk::Key::Home
            | gdk::Key::End
            | gdk::Key::Up
            | gdk::Key::Down
            | gdk::Key::Page_Up
            | gdk::Key::Page_Down
            | gdk::Key::Escape
    )
}

fn owns_modifier(modifiers: gdk::ModifierType) -> bool {
    modifiers.intersects(
        gdk::ModifierType::CONTROL_MASK
            | gdk::ModifierType::ALT_MASK
            | gdk::ModifierType::SUPER_MASK,
    )
}

/// The one-line result status. Only a completed scan may say "No matches".
pub fn status_text(view: &SearchView) -> String {
    let total = view.total_matches.max(view.rows as u64);
    let count = if view.truncated || view.rows as u64 != total {
        format!("{} best of {}", view.rows, match_count(total))
    } else {
        match_count(total)
    };
    let mut text = match &view.phase {
        SearchPhase::Searching { queued: true } => {
            format!("Waiting for a search worker… {count} so far")
        }
        SearchPhase::Searching { queued: false } => format!("Searching… {count}"),
        SearchPhase::Complete => {
            if total == 0 {
                "No matches".to_string()
            } else {
                count
            }
        }
        SearchPhase::Partial(reasons) => {
            let reasons = reasons
                .iter()
                .map(|reason| reason.label())
                .collect::<Vec<_>>()
                .join(", ");
            format!("Partial results — {reasons} — {count}")
        }
        SearchPhase::Cancelled => format!("Search cancelled — {count} so far"),
        SearchPhase::Unavailable(reason) => format!("Search unavailable — {}", reason.label()),
    };
    if matches!(
        view.phase,
        SearchPhase::Searching { .. } | SearchPhase::Partial(_)
    ) && view.visited > 0
    {
        text.push_str(&format!(" · {} entries checked", view.visited));
    }
    if view.skipped > 0 {
        text.push_str(&format!(" · {} skipped", view.skipped));
    }
    text
}

fn match_count(count: u64) -> String {
    if count == 1 {
        "1 match".to_string()
    } else {
        format!("{count} matches")
    }
}

/// The scope note beside the root. It never claims more than the walk does.
pub fn scope_text(view: &SearchView) -> String {
    let mut text =
        "Same filesystem only · hidden entries included · child symlink folders not followed · depth ≤ 128"
            .to_string();
    if view.mounts_skipped > 0 {
        text.push_str(&format!(
            " · {} mount point(s) listed, not entered",
            view.mounts_skipped
        ));
    }
    text
}

/// A bounded tooltip for the status: counts plus at most five examples.
pub fn status_tooltip(view: &SearchView) -> Option<String> {
    let mut lines = vec![status_text(view)];
    for example in &view.examples {
        lines.push(format!("{}: {}", example.path.display(), example.message));
    }
    if let Some(notice) = &view.notice {
        lines.push(notice.clone());
    }
    Some(lines.join("\n"))
}
