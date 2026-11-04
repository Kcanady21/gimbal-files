// SPDX-License-Identifier: GPL-3.0-or-later
//! Delayed folder-size demand, bounded workers and the tooltip text model.
//!
//! Workers own only typed requests, cancellation flags and a bounded result
//! channel. The main thread validates the exact view/root/name/revision and
//! edits GTK; no filesystem call runs in a motion, tooltip or draw callback.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

use fm_core::folder_size::{
    CacheLookup, CachedFolderSize, Clock, FolderSize, FolderSizeCache, MAX_ACTIVE_SCANS,
    MAX_PENDING_DEMANDS, Progress, RootIdentity, RootState, ScanPolicy, ScanResult, ScanSource,
    SystemClock, folder_size_lines, scan_with_root,
};
use fm_core::preview::FileRevision;

/// GTK's own first-hover delay is 500 ms; the grid also starts its own
/// continuous-hover timer so browse mode's faster re-show cannot start a scan
/// without a fresh 500 ms dwell.
pub const HOVER_DELAY_MS: u64 = 500;
/// The tooltip poll interval; updates are still throttled to four per second.
pub const DEMAND_POLL_MS: u64 = 100;
/// Visible progress is shown at most this often.
pub const PROGRESS_UPDATE_MS: u64 = 250;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TooltipIdentity {
    pub root: RootIdentity,
    pub name: OsString,
    pub revision: FileRevision,
}

impl TooltipIdentity {
    pub fn matches(&self, root: &RootIdentity, name: &OsString, revision: &FileRevision) -> bool {
        &self.root == root && &self.name == name && &self.revision == revision
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TooltipPhase {
    Idle,
    Calculating,
    Stale,
    Sized,
    Remote,
    Symlink,
    Unavailable,
}

#[derive(Clone, Debug)]
pub struct TooltipModel {
    pub identity: TooltipIdentity,
    pub display_name: String,
    pub modified: String,
    pub phase: TooltipPhase,
    pub progress: Option<Progress>,
    pub size: Option<FolderSize>,
    pub cached: bool,
    pub age_ms: u64,
}

impl TooltipModel {
    pub fn new(identity: TooltipIdentity, display_name: String, modified: String) -> Self {
        Self {
            identity,
            display_name,
            modified,
            phase: TooltipPhase::Idle,
            progress: None,
            size: None,
            cached: false,
            age_ms: 0,
        }
    }

    pub fn start(&mut self, stale: Option<CachedFolderSize>, now_ms: u64) {
        self.progress = None;
        self.cached = false;
        match stale {
            Some(stale) => {
                self.phase = TooltipPhase::Stale;
                self.age_ms = now_ms.saturating_sub(stale.completed_at_ms);
                self.size = Some(stale.size);
            }
            None => {
                self.phase = TooltipPhase::Calculating;
                self.size = None;
            }
        }
    }

    pub fn set_progress(&mut self, progress: Progress) {
        if matches!(self.phase, TooltipPhase::Calculating | TooltipPhase::Stale) {
            self.progress = Some(progress);
        }
    }

    pub fn finish(&mut self, result: ScanResult, age_ms: u64, cached: bool) {
        self.progress = None;
        self.cached = cached;
        self.age_ms = age_ms;
        match result {
            ScanResult::Sized(size) => {
                self.phase = TooltipPhase::Sized;
                self.size = Some(size);
            }
            ScanResult::Cancelled => {
                self.phase = TooltipPhase::Idle;
                self.size = None;
            }
            ScanResult::Unavailable(RootState::Remote) => {
                self.phase = TooltipPhase::Remote;
                self.size = None;
            }
            ScanResult::Unavailable(RootState::Symlink) => {
                self.phase = TooltipPhase::Symlink;
                self.size = None;
            }
            ScanResult::Unavailable(_) => {
                self.phase = TooltipPhase::Unavailable;
                self.size = None;
            }
        }
    }

    pub fn matches(&self, root: &RootIdentity, name: &OsString, revision: &FileRevision) -> bool {
        self.identity.matches(root, name, revision)
    }

    pub fn text(&self) -> String {
        let status = match self.phase {
            TooltipPhase::Idle => "Size: Folder".to_string(),
            TooltipPhase::Calculating => "Size: Calculating…".to_string(),
            TooltipPhase::Stale => "Size: Last calculation (stale, updating…)".to_string(),
            TooltipPhase::Sized => "Size: Folder".to_string(),
            TooltipPhase::Remote => "Size: Not calculated for remote folders".to_string(),
            TooltipPhase::Symlink => {
                "Size: Not calculated (symlink target was not scanned)".to_string()
            }
            TooltipPhase::Unavailable => "Size unavailable".to_string(),
        };
        let mut lines = vec![
            self.display_name.clone(),
            "Type: Folder".to_string(),
            status,
            format!("Modified: {}", self.modified),
        ];
        match self.phase {
            TooltipPhase::Calculating => {
                if let Some(progress) = self.progress {
                    lines.push(progress_line(progress));
                }
            }
            TooltipPhase::Stale => {
                if let Some(size) = &self.size {
                    lines.extend(folder_size_lines(size, self.age_ms, false));
                }
                if let Some(progress) = self.progress {
                    lines.push(progress_line(progress));
                }
            }
            TooltipPhase::Sized => {
                if let Some(size) = &self.size {
                    lines.extend(folder_size_lines(size, self.age_ms, self.cached));
                }
            }
            TooltipPhase::Idle
            | TooltipPhase::Remote
            | TooltipPhase::Symlink
            | TooltipPhase::Unavailable => {}
        }
        lines.join("\n")
    }
}

fn progress_line(progress: Progress) -> String {
    let files = if progress.files == 1 {
        "1 file".to_string()
    } else {
        format!("{} files", progress.files)
    };
    let folders = if progress.folders == 1 {
        "1 folder".to_string()
    } else {
        format!("{} folders", progress.folders)
    };
    format!("Found {files}, {folders}")
}

/// Continuous-hover timing, independent of GTK browse-mode shortenings.
#[derive(Debug, Default)]
pub struct HoverDelay {
    started_ms: Option<u64>,
}

impl HoverDelay {
    pub fn enter(&mut self, now_ms: u64) {
        self.started_ms = Some(now_ms);
    }

    pub fn leave(&mut self) {
        self.started_ms = None;
    }

    pub fn due(&self, now_ms: u64) -> bool {
        self.started_ms
            .is_some_and(|started| now_ms.saturating_sub(started) >= HOVER_DELAY_MS)
    }
}

/// Keeps progress text to four updates per second even if the poll runs more
/// often. The first update is immediate.
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    last_ms: Option<u64>,
}

impl ProgressThrottle {
    pub fn should_show(&mut self, now_ms: u64) -> bool {
        if self
            .last_ms
            .is_some_and(|last| now_ms.saturating_sub(last) < PROGRESS_UPDATE_MS)
        {
            return false;
        }
        self.last_ms = Some(now_ms);
        true
    }
}

pub struct ProgressCell {
    files: AtomicU64,
    folders: AtomicU64,
    visited: AtomicU64,
    version: AtomicU64,
}

impl ProgressCell {
    fn new() -> Self {
        Self {
            files: AtomicU64::new(0),
            folders: AtomicU64::new(0),
            visited: AtomicU64::new(0),
            version: AtomicU64::new(0),
        }
    }

    fn set(&self, progress: Progress) {
        self.files.store(progress.files, Ordering::Relaxed);
        self.folders.store(progress.folders, Ordering::Relaxed);
        self.visited.store(progress.visited, Ordering::Relaxed);
        self.version.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> (Progress, u64) {
        (
            Progress {
                files: self.files.load(Ordering::Relaxed),
                folders: self.folders.load(Ordering::Relaxed),
                visited: self.visited.load(Ordering::Relaxed),
            },
            self.version.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug)]
pub struct JobEvent {
    pub token: u64,
    pub result: ScanResult,
    pub completed_at_ms: u64,
    pub cached: bool,
}

pub struct Started {
    pub token: u64,
    cancel: Arc<AtomicBool>,
    progress: Arc<ProgressCell>,
    events: Receiver<JobEvent>,
    pub stale: Option<CachedFolderSize>,
}

impl Started {
    pub fn try_next(&self) -> Option<JobEvent> {
        self.events.try_recv().ok()
    }

    pub fn progress(&self) -> (Progress, u64) {
        self.progress.snapshot()
    }

    /// Wait for one typed result. Tests use this with a controlled barrier
    /// instead of a sleep race; the UI only calls `try_next`.
    #[cfg(test)]
    pub fn wait(&self, timeout: std::time::Duration) -> Option<JobEvent> {
        self.events.recv_timeout(timeout).ok()
    }
}

pub enum RequestOutcome {
    Cached(CachedFolderSize),
    Started(Started),
    Busy,
    Stopped,
}

struct Job {
    token: u64,
    request: RootIdentity,
    cancel: Arc<AtomicBool>,
    progress: Arc<ProgressCell>,
    events: SyncSender<JobEvent>,
    epoch: u64,
}

struct QueueState {
    pending: VecDeque<Job>,
    /// Active scans keep their root as well as their flag so one unmount can
    /// release exactly the handles under that mount.
    active: HashMap<u64, (PathBuf, Arc<AtomicBool>)>,
    stopped: bool,
}

struct Inner {
    queue: Mutex<QueueState>,
    ready: Condvar,
    cache: Mutex<FolderSizeCache>,
    policy: ScanPolicy,
    clock: Arc<dyn Clock>,
    source: Arc<dyn ScanSource>,
    workers: Mutex<Vec<JoinHandle<()>>>,
    progress: Mutex<HashMap<u64, Arc<ProgressCell>>>,
    next_token: AtomicU64,
    cache_epoch: AtomicU64,
    stopping: AtomicBool,
}

pub struct Service {
    inner: Arc<Inner>,
}

impl Service {
    pub fn new() -> Self {
        Self::with_parts(
            Arc::new(fm_core::folder_size::NativeSource),
            Arc::new(SystemClock),
            ScanPolicy::default(),
        )
    }

    pub fn with_parts(
        source: Arc<dyn ScanSource>,
        clock: Arc<dyn Clock>,
        policy: ScanPolicy,
    ) -> Self {
        let inner = Arc::new(Inner {
            queue: Mutex::new(QueueState {
                pending: VecDeque::new(),
                active: HashMap::new(),
                stopped: false,
            }),
            ready: Condvar::new(),
            cache: Mutex::new(FolderSizeCache::new(fm_core::folder_size::CACHE_ROOTS)),
            policy,
            clock,
            source,
            workers: Mutex::new(Vec::new()),
            progress: Mutex::new(HashMap::new()),
            next_token: AtomicU64::new(1),
            cache_epoch: AtomicU64::new(1),
            stopping: AtomicBool::new(false),
        });
        let workers: Vec<JoinHandle<()>> = (0..MAX_ACTIVE_SCANS)
            .map(|index| {
                let inner = inner.clone();
                thread::Builder::new()
                    .name(format!("folder-size-{index}"))
                    .spawn(move || worker_loop(inner))
                    .expect("folder-size worker")
            })
            .collect();
        *inner.workers.lock().unwrap() = workers;
        Self { inner }
    }

    pub fn now_ms(&self) -> u64 {
        self.inner.clock.now_ms()
    }

    pub fn request(&self, request: RootIdentity) -> RequestOutcome {
        let key = request.cache_key(self.inner.policy.version);
        let now = self.inner.clock.now_ms();
        let stale = {
            let cache = self.inner.cache.lock().unwrap();
            match cache.get(&key, now) {
                CacheLookup::Fresh(hit) => return RequestOutcome::Cached(hit),
                CacheLookup::Expired(hit) => Some(hit),
                CacheLookup::Miss => None,
            }
        };
        let token = self.inner.next_token.fetch_add(1, Ordering::SeqCst);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ProgressCell::new());
        let (sender, receiver) = sync_channel(2);
        let job = Job {
            token,
            request,
            cancel: cancel.clone(),
            progress: progress.clone(),
            events: sender,
            epoch: self.inner.cache_epoch.load(Ordering::SeqCst),
        };
        {
            let mut queue = self.inner.queue.lock().unwrap();
            if queue.stopped || self.inner.stopping.load(Ordering::SeqCst) {
                return RequestOutcome::Stopped;
            }
            if queue.pending.len() >= MAX_PENDING_DEMANDS {
                return RequestOutcome::Busy;
            }
            queue.pending.push_back(job);
            self.inner
                .progress
                .lock()
                .unwrap()
                .insert(token, progress.clone());
            self.inner.ready.notify_one();
        }
        RequestOutcome::Started(Started {
            token,
            cancel,
            progress,
            events: receiver,
            stale,
        })
    }

    pub fn cancel(&self, started: &Started) {
        started.cancel.store(true, Ordering::SeqCst);
        {
            let mut queue = self.inner.queue.lock().unwrap();
            queue.pending.retain(|job| job.token != started.token);
        }
        self.inner.progress.lock().unwrap().remove(&started.token);
    }

    pub fn invalidate_path(&self, path: &Path) {
        // Any invalidation is conservative for in-flight work: a result that
        // was already being calculated must not repopulate the cache.
        self.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
        self.inner.cache.lock().unwrap().invalidate_path(path);
    }

    pub fn invalidate_all(&self) {
        self.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
        self.inner.cache.lock().unwrap().invalidate_all();
    }

    /// Cancel queued and running folder-size walks below one mount root and
    /// forget their cached totals. Runs on the GTK thread and never waits for a
    /// worker: an already running native scan sees its flag at the next check.
    pub fn release_root(&self, root: &Path) -> usize {
        self.inner.cache_epoch.fetch_add(1, Ordering::SeqCst);
        let mut released = 0;
        {
            let mut queue = self.inner.queue.lock().unwrap();
            for (path, cancel) in queue.active.values() {
                if path.starts_with(root) {
                    cancel.store(true, Ordering::SeqCst);
                    released += 1;
                }
            }
            let before = queue.pending.len();
            queue.pending.retain(|job| {
                if job.request.path.starts_with(root) {
                    job.cancel.store(true, Ordering::SeqCst);
                    false
                } else {
                    true
                }
            });
            released += before - queue.pending.len();
        }
        // The cache has no root-range removal; dropping it is conservative and
        // cheap for a one-off unmount.
        self.inner.cache.lock().unwrap().invalidate_all();
        if released > 0 {
            crate::log!(
                "folder-size",
                "released {released} scan(s) under {}",
                root.display()
            );
        }
        released
    }

    /// Cancel queued and running work and wait for both workers to return.
    /// A worker blocked inside a local filesystem call can delay this join;
    /// the UI never waits for it during normal browsing.
    pub fn shutdown(&self) {
        if self.inner.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        {
            let mut queue = self.inner.queue.lock().unwrap();
            queue.stopped = true;
            for (_, cancel) in queue.active.values() {
                cancel.store(true, Ordering::SeqCst);
            }
            for job in queue.pending.drain(..) {
                job.cancel.store(true, Ordering::SeqCst);
            }
        }
        self.inner.ready.notify_all();
        let workers = std::mem::take(&mut *self.inner.workers.lock().unwrap());
        for worker in workers {
            let _ = worker.join();
        }
    }
}

impl Default for Service {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Service {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn worker_loop(inner: Arc<Inner>) {
    loop {
        let job = {
            let mut queue = inner.queue.lock().unwrap();
            loop {
                if queue.stopped {
                    return;
                }
                if let Some(job) = queue.pending.pop_front() {
                    queue
                        .active
                        .insert(job.token, (job.request.path.clone(), job.cancel.clone()));
                    break job;
                }
                queue = inner.ready.wait(queue).unwrap();
            }
        };
        let cancelled = job.cancel.load(Ordering::SeqCst);
        let result = if cancelled {
            ScanResult::Cancelled
        } else {
            let probe = inner.source.probe(&job.request.path);
            let progress = job.progress.clone();
            let cancel = job.cancel.clone();
            scan_with_root(
                &job.request,
                &probe,
                inner.policy,
                inner.source.as_ref(),
                inner.clock.as_ref(),
                &|| cancel.load(Ordering::SeqCst),
                &mut |snapshot| progress.set(snapshot),
            )
        };
        let completed_at_ms = inner.clock.now_ms();
        {
            let mut queue = inner.queue.lock().unwrap();
            queue.active.remove(&job.token);
        }
        inner.progress.lock().unwrap().remove(&job.token);
        let cancelled = job.cancel.load(Ordering::SeqCst);
        let epoch_current = job.epoch == inner.cache_epoch.load(Ordering::SeqCst);
        if !cancelled
            && epoch_current
            && let ScanResult::Sized(size) = &result
        {
            let key = job.request.cache_key(inner.policy.version);
            inner
                .cache
                .lock()
                .unwrap()
                .insert(key, size.clone(), completed_at_ms);
            crate::log!(
                "folder-size",
                "cache store {}: {} files, {} folders, partial {}",
                job.request.path.display(),
                size.totals.files,
                size.totals.folders,
                size.is_partial()
            );
        }
        let _ = job.events.send(JobEvent {
            token: job.token,
            result,
            completed_at_ms,
            cached: false,
        });
    }
}

thread_local! {
    static SERVICE: RefCell<Option<Rc<Service>>> = const { RefCell::new(None) };
}

pub fn service() -> Rc<Service> {
    SERVICE.with(|slot| {
        if let Some(service) = slot.borrow().as_ref() {
            return service.clone();
        }
        let service = Rc::new(Service::new());
        *slot.borrow_mut() = Some(service.clone());
        service
    })
}

pub fn invalidate_path(path: &Path) {
    SERVICE.with(|slot| {
        if let Some(service) = slot.borrow().as_ref() {
            service.invalidate_path(path);
        }
    });
}

pub fn invalidate_all() {
    SERVICE.with(|slot| {
        if let Some(service) = slot.borrow().as_ref() {
            service.invalidate_all();
        }
    });
}

/// Cancel every folder-size walk under one mount root, without waiting.
pub fn release_root(root: &Path) -> usize {
    SERVICE.with(|slot| {
        slot.borrow()
            .as_ref()
            .map(|service| service.release_root(root))
            .unwrap_or(0)
    })
}

pub fn shutdown() {
    SERVICE.with(|slot| {
        if let Some(service) = slot.borrow().as_ref() {
            service.shutdown();
        }
    });
}

#[cfg(test)]
mod tests;
