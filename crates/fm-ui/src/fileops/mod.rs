// SPDX-License-Identifier: GPL-3.0-or-later
//! File operations in the window: one worker thread, one queue, progress and
//! the conflict menu.
//!
//! The worker owns `fm-core`'s `FileOperations` and therefore the undo stack.
//! The client never blocks the main thread: it submits a request, polls the
//! worker's events from a main-context timeout, and shows the plan's conflicts
//! one at a time. A second request waits in the client's queue and appears as
//! "1 waiting" in the progress row.

pub mod clipboard;
pub mod conflict;
pub mod dialogs;
pub mod keys;
pub mod menu;
pub mod progress;
mod worker;

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use crate::agent::{GenerationTicket, JobAuthorization};

use fm_core::fileops::{Decisions, Plan, Progress, Request};
use fm_core::workspace::WindowId;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::app::Process;
use crate::fileops::conflict::ConflictFlow;
use crate::fileops::progress::OpKind;

pub use menu::ItemInfo;
pub use progress::ProgressView;
pub use worker::PauseHook as WorkerPause;

/// How often the main context checks the worker's channel.
const POLL_MS: u64 = 30;
/// The file-op client queue is bounded even for ordinary UI work.
pub const MAX_QUEUED_JOBS: usize = 64;

struct Queued {
    id: u64,
    request: Request,
    window: WindowId,
    clear_clipboard: bool,
    dry_run: bool,
    authorization: Option<JobAuthorization>,
}

struct Current {
    id: u64,
    request: Request,
    window: WindowId,
    cancellable: gio::Cancellable,
    started: Instant,
    shown: bool,
    last: Option<Progress>,
    plan: Option<Plan>,
    clear_clipboard: bool,
    /// True while the conflict dialog owns the job; the progress row stays
    /// hidden until the decisions are in.
    answering: bool,
    authorization: Option<JobAuthorization>,
    /// Set by the worker immediately before it calls execute. Revocation may
    /// cancel planning/waiting work, but never pretends to roll back a file
    /// operation that has already begun.
    executing: Arc<AtomicBool>,
}

pub struct FileOps {
    jobs: Sender<worker::WorkerMsg>,
    control: Sender<worker::WorkerMsg>,
    events: Receiver<worker::WorkerEvent>,
    next_id: Cell<u64>,
    queue: RefCell<VecDeque<Queued>>,
    current: RefCell<Option<Current>>,
    poll: RefCell<Option<glib::SourceId>>,
    process: RefCell<Weak<Process>>,
    undo_summary: RefCell<Option<String>>,
    undo_lines: RefCell<Option<Vec<String>>>,
    undo_steps: RefCell<Vec<(std::path::PathBuf, Option<std::path::PathBuf>)>>,
    baobab: Cell<bool>,
    /// Headless tests drive `poll_once` themselves; adding a GLib source on a
    /// test thread would let another test's main-context iteration touch its
    /// thread-guarded callback.
    manual_poll: bool,
}

impl FileOps {
    pub fn new(process: &Rc<Process>) -> Rc<Self> {
        Self::build(process, None, false)
    }

    /// The production worker plus a deterministic test gate at named
    /// boundaries. No second executor or scheduler is introduced.
    #[cfg(test)]
    pub fn with_pause(process: &Rc<Process>, pause: WorkerPause) -> Rc<Self> {
        Self::build(process, Some(pause), true)
    }

    /// A headless test instance whose poll loop is driven by `poll_for_tests`.
    #[cfg(test)]
    pub fn new_manual(process: &Rc<Process>) -> Rc<Self> {
        Self::build(process, None, true)
    }

    fn build(process: &Rc<Process>, pause: Option<WorkerPause>, manual_poll: bool) -> Rc<Self> {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let senders = worker::spawn_with_pause(event_tx, pause);
        let baobab = menu::find_on_path("baobab", std::env::var_os("PATH").as_deref()).is_some();
        Rc::new(Self {
            jobs: senders.jobs,
            control: senders.control,
            events: event_rx,
            next_id: Cell::new(0),
            queue: RefCell::new(VecDeque::new()),
            current: RefCell::new(None),
            poll: RefCell::new(None),
            process: RefCell::new(Rc::downgrade(process)),
            undo_summary: RefCell::new(None),
            undo_lines: RefCell::new(None),
            undo_steps: RefCell::new(Vec::new()),
            baobab: Cell::new(baobab),
            manual_poll,
        })
    }

    /// Queue one request and return its id. The first runs at once; later
    /// ones wait and are shown as "N waiting".
    pub fn submit(
        self: &Rc<Self>,
        window: WindowId,
        request: Request,
        clear_clipboard: bool,
        authorization: Option<JobAuthorization>,
    ) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let job = Queued {
            id,
            request,
            window,
            clear_clipboard,
            dry_run: false,
            authorization,
        };
        let idle = self.current.borrow().is_none();
        if idle {
            self.start(job);
        } else {
            crate::log!(
                "fileops",
                "queued job {id} ({} waiting)",
                self.queue.borrow().len() + 1
            );
            self.queue.borrow_mut().push_back(job);
        }
        self.ensure_poll();
        id
    }

    /// Queue a read-only dry run on the worker. `trash.empty` planning lists
    /// the trash, so it must not run on the GTK main thread.
    pub fn submit_dry_run(
        self: &Rc<Self>,
        window: WindowId,
        request: Request,
        authorization: Option<JobAuthorization>,
    ) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let job = Queued {
            id,
            request,
            window,
            clear_clipboard: false,
            dry_run: true,
            authorization,
        };
        let idle = self.current.borrow().is_none();
        if idle {
            self.start(job);
        } else {
            self.queue.borrow_mut().push_back(job);
        }
        self.ensure_poll();
        id
    }

    fn start(self: &Rc<Self>, job: Queued) {
        let cancellable = gio::Cancellable::new();
        let executing = Arc::new(AtomicBool::new(false));
        if self
            .jobs
            .send(worker::WorkerMsg::Start {
                id: job.id,
                request: job.request.clone(),
                cancellable: cancellable.clone(),
                dry_run: job.dry_run,
                authorization: job.authorization.clone(),
                executing: Arc::clone(&executing),
            })
            .is_err()
        {
            crate::log!("fileops", "the worker is gone; job {} dropped", job.id);
            return;
        }
        *self.current.borrow_mut() = Some(Current {
            id: job.id,
            request: job.request.clone(),
            window: job.window,
            cancellable,
            started: Instant::now(),
            shown: false,
            last: None,
            plan: None,
            clear_clipboard: job.clear_clipboard,
            answering: false,
            authorization: job.authorization,
            executing,
        });
        if let Some(process) = self.process.borrow().upgrade() {
            process.fileop_started(job.window, job.id);
        }
        crate::log!(
            "fileops",
            "started job {}: {}",
            job.id,
            job.request.summary()
        );
    }

    fn ensure_poll(self: &Rc<Self>) {
        if self.manual_poll || self.poll.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            let Some(fileops) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            fileops.poll_once()
        });
        *self.poll.borrow_mut() = Some(source);
    }

    #[cfg(test)]
    pub(crate) fn poll_for_tests(self: &Rc<Self>) -> glib::ControlFlow {
        self.poll_once()
    }

    #[cfg(test)]
    pub(crate) fn current_answering(&self) -> bool {
        self.current
            .borrow()
            .as_ref()
            .is_some_and(|current| current.answering)
    }

    fn poll_once(self: &Rc<Self>) -> glib::ControlFlow {
        loop {
            match self.events.try_recv() {
                Ok(event) => self.handle(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.maybe_reveal();
        let idle = self.current.borrow().is_none() && self.queue.borrow().is_empty();
        if idle {
            *self.poll.borrow_mut() = None;
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    }

    fn maybe_reveal(self: &Rc<Self>) {
        let reveal = {
            let current = self.current.borrow();
            let Some(current) = current.as_ref() else {
                return;
            };
            if current.shown
                || current.answering
                || !progress::row_visible(current.started.elapsed())
            {
                return;
            }
            (
                current.id,
                current.window,
                current.request.clone(),
                current.plan.as_ref().map_or(0, |plan| plan.steps().len()),
                current.last,
            )
        };
        let (id, window, request, total, last) = reveal;
        let progress = last.unwrap_or(Progress {
            items_done: 0,
            items_total: total,
            bytes_done: 0,
            bytes_total: None,
        });
        if let Some(current) = self.current.borrow_mut().as_mut()
            && current.id == id
        {
            current.shown = true;
        }
        let view = ProgressView::new(OpKind::from_request(&request), &progress, self.waiting());
        if let Some(process) = self.process.borrow().upgrade() {
            process.fileop_progress(window, id, Some(view));
        }
    }

    fn handle(self: &Rc<Self>, event: worker::WorkerEvent) {
        match event {
            worker::WorkerEvent::Planned { id, plan } => {
                let (window, revoked) = {
                    let mut current = self.current.borrow_mut();
                    let Some(current) = current.as_mut() else {
                        return;
                    };
                    if current.id != id {
                        return;
                    }
                    current.plan = Some(plan.clone());
                    let revoked = current
                        .authorization
                        .as_ref()
                        .is_some_and(|authorization| !authorization.is_active());
                    (current.window, revoked)
                };
                if revoked {
                    // Off can arrive while planning is paused. Never present a
                    // stale conflict/approval prompt for revoked agent work.
                    crate::log!("fileops", "dropping late plan for revoked job {id}");
                    self.cancel_job(id);
                    return;
                }
                let conflicts = plan.decisions().count();
                if plan.request().needs_permanent_confirmation() {
                    if plan.steps().is_empty() {
                        crate::log!(
                            "fileops",
                            "permanent deletion job {id} has nothing to confirm"
                        );
                        self.cancel_current();
                        return;
                    }
                    if let Some(current) = self.current.borrow_mut().as_mut() {
                        current.answering = true;
                    }
                    if let Some(process) = self.process.borrow().upgrade() {
                        crate::log!(
                            "fileops",
                            "permanent-delete confirmation for job {id}: {} item(s)",
                            plan.steps().len()
                        );
                        process.fileop_empty_confirmation(window, id, plan.clone());
                    }
                } else if conflicts == 0 {
                    self.answer(id, Decisions::from_plan(&plan));
                } else if let Some(process) = self.process.borrow().upgrade() {
                    crate::log!("fileops", "{conflicts} conflict(s) need a decision");
                    if let Some(current) = self.current.borrow_mut().as_mut() {
                        current.answering = true;
                    }
                    let fileops = self.clone();
                    process.fileop_conflicts(
                        window,
                        id,
                        ConflictFlow::from_plan(&plan),
                        Box::new(move |decisions| fileops.answer(id, decisions)),
                    );
                } else {
                    self.answer(id, Decisions::new(Vec::new()));
                }
            }
            worker::WorkerEvent::Executing { id } => {
                if let Some(current) = self.current.borrow_mut().as_mut()
                    && current.id == id
                {
                    current
                        .executing
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                }
            }
            worker::WorkerEvent::Revoked {
                id,
                request,
                message,
            } => {
                let current = {
                    let mut slot = self.current.borrow_mut();
                    match slot.as_ref() {
                        Some(current) if current.id == id => slot.take(),
                        _ => None,
                    }
                };
                let Some(current) = current else {
                    return;
                };
                crate::log!(
                    "fileops",
                    "revoked job {id} ({}): {message}",
                    request.summary()
                );
                if let Some(process) = self.process.borrow().upgrade() {
                    process.fileop_revoked(current.window, id, &request, &message);
                }
                self.start_next();
            }
            worker::WorkerEvent::Progress { id, progress } => {
                let (window, request, shown) = {
                    let mut current = self.current.borrow_mut();
                    let Some(current) = current.as_mut() else {
                        return;
                    };
                    if current.id != id {
                        return;
                    }
                    current.last = Some(progress);
                    (current.window, current.request.clone(), current.shown)
                };
                crate::log!(
                    "fileops",
                    "progress {}/{} items, {}/{} bytes",
                    progress.items_done,
                    progress.items_total,
                    progress.bytes_done,
                    progress
                        .bytes_total
                        .map(|total| total.to_string())
                        .unwrap_or_else(|| "?".to_string())
                );
                if shown {
                    let view = ProgressView::new(
                        OpKind::from_request(&request),
                        &progress,
                        self.waiting(),
                    );
                    if let Some(process) = self.process.borrow().upgrade() {
                        process.fileop_progress(window, id, Some(view));
                    }
                } else {
                    self.maybe_reveal();
                }
            }
            worker::WorkerEvent::Finished {
                id,
                request,
                plan,
                report,
            } => {
                let current = {
                    let mut slot = self.current.borrow_mut();
                    match slot.as_ref() {
                        Some(current) if current.id == id => slot.take(),
                        _ => None,
                    }
                };
                let Some(current) = current else {
                    return;
                };
                crate::log!(
                    "fileops",
                    "finished job {id} ({}): {}",
                    request.summary(),
                    report.lines().join(" | ")
                );
                if let Some(process) = self.process.borrow().upgrade() {
                    process.fileop_finished(current.window, id, &request, &plan, &report);
                    let moved = progress::counts(&report).done > 0;
                    if current.clear_clipboard && moved {
                        process.fileop_clear_clipboard(current.window);
                    }
                }
                self.start_next();
            }
            worker::WorkerEvent::Refused {
                id,
                request,
                message,
            } => {
                let current = {
                    let mut slot = self.current.borrow_mut();
                    match slot.as_ref() {
                        Some(current) if current.id == id => slot.take(),
                        _ => None,
                    }
                };
                let Some(current) = current else {
                    return;
                };
                crate::log!(
                    "fileops",
                    "refused job {id} ({}): {message}",
                    request.summary()
                );
                if let Some(process) = self.process.borrow().upgrade() {
                    process.fileop_refused(current.window, id, &request, &message);
                }
                self.start_next();
            }
            worker::WorkerEvent::DryRunReady { id, request, lines } => {
                let current = {
                    let mut slot = self.current.borrow_mut();
                    match slot.as_ref() {
                        Some(current) if current.id == id => slot.take(),
                        _ => None,
                    }
                };
                let Some(current) = current else {
                    return;
                };
                if let Some(process) = self.process.borrow().upgrade() {
                    crate::log!("fileops", "dry run job {id}: {}", lines.join(" | "));
                    process.fileop_dry_run(current.window, id, &request, &lines);
                }
                self.start_next();
            }
            worker::WorkerEvent::UndoState {
                summary,
                lines,
                steps,
            } => {
                crate::log!(
                    "fileops",
                    "undo stack: {}",
                    summary.clone().unwrap_or_else(|| "empty".to_string())
                );
                *self.undo_summary.borrow_mut() = summary;
                *self.undo_lines.borrow_mut() = Some(lines);
                *self.undo_steps.borrow_mut() = steps;
            }
        }
    }

    fn start_next(self: &Rc<Self>) {
        let next = self.queue.borrow_mut().pop_front();
        if let Some(job) = next {
            self.start(job);
        }
    }

    /// Answer the plan the dialog was showing.
    pub fn answer(self: &Rc<Self>, id: u64, decisions: Decisions) {
        {
            let mut current = self.current.borrow_mut();
            let Some(current) = current.as_mut() else {
                return;
            };
            if current.id != id {
                return;
            }
            current.answering = false;
        }
        crate::log!(
            "fileops",
            "answer job {id}: {}",
            if decisions.is_cancelled() {
                "cancel".to_string()
            } else {
                "decisions ready".to_string()
            }
        );
        let _ = self
            .control
            .send(worker::WorkerMsg::Answer { id, decisions });
    }

    /// The destructive dialog returned Empty Trash. The worker revalidates
    /// the immutable plan before deleting anything.
    pub fn confirm_empty(self: &Rc<Self>, id: u64) {
        {
            let mut slot = self.current.borrow_mut();
            if let Some(current) = slot.as_mut()
                && current.id == id
            {
                current.answering = false;
            }
        }
        crate::log!("fileops", "confirmed empty trash job {id}");
        let _ = self.control.send(worker::WorkerMsg::ConfirmEmpty { id });
    }

    /// The destructive dialog returned Delete Permanently for the selected
    /// Trash items. The worker revalidates the immutable selected plan before
    /// deleting anything.
    pub fn confirm_selected(self: &Rc<Self>, id: u64) {
        {
            let mut slot = self.current.borrow_mut();
            if let Some(current) = slot.as_mut()
                && current.id == id
            {
                current.answering = false;
            }
        }
        crate::log!("fileops", "confirmed selected deletion job {id}");
        let _ = self.control.send(worker::WorkerMsg::ConfirmSelected { id });
    }

    /// Cancel the running job. Whatever finished stays done; the rest is
    /// reported as cancelled.
    pub fn cancel_current(self: &Rc<Self>) {
        let id = {
            let current = self.current.borrow();
            let Some(current) = current.as_ref() else {
                return;
            };
            current.cancellable.cancel();
            current.id
        };
        crate::log!("fileops", "cancelled job {id}");
        let _ = self.control.send(worker::WorkerMsg::Cancel { id });
    }

    /// Cancel one exact job. A page that leaves while its confirmation is open
    /// uses this so it never cancels another window's current job. A job that
    /// is not current is left alone rather than sending a `Cancel` the worker
    /// would skip while waiting for that job's answer.
    pub fn cancel_job(self: &Rc<Self>, id: u64) {
        let cancel = {
            let current = self.current.borrow();
            match current.as_ref() {
                Some(current) if current.id == id => {
                    current.cancellable.cancel();
                    true
                }
                _ => false,
            }
        };
        if !cancel {
            crate::log!(
                "fileops",
                "job {id} is no longer current; not cancelling it"
            );
            return;
        }
        crate::log!("fileops", "cancelled job {id}");
        let _ = self.control.send(worker::WorkerMsg::Cancel { id });
    }

    /// Drop queued agent work for one revoked generation and cancel the
    /// planning/waiting current job. A job whose worker already published the
    /// execution boundary is left to finish under its original policy.
    pub fn revoke_agent_work(&self, ticket: &Arc<GenerationTicket>) {
        let mut dropped = 0usize;
        {
            let mut queue = self.queue.borrow_mut();
            let before = queue.len();
            queue.retain(|job| {
                let revoked = job
                    .authorization
                    .as_ref()
                    .is_some_and(|auth| auth.same_cycle(ticket));
                if !revoked {
                    return true;
                }
                dropped += 1;
                false
            });
            if dropped > 0 {
                crate::log!(
                    "fileops",
                    "dropped {dropped} revoked queued job(s); {} remain",
                    before - dropped
                );
            }
        }
        let cancel = {
            let current = self.current.borrow();
            current.as_ref().and_then(|current| {
                let revoked = current
                    .authorization
                    .as_ref()
                    .is_some_and(|auth| auth.same_cycle(ticket));
                (revoked && !current.executing.load(std::sync::atomic::Ordering::SeqCst))
                    .then_some(current.id)
            })
        };
        if let Some(id) = cancel {
            crate::log!("fileops", "cancelling revoked job {id} before execution");
            let _ = self.control.send(worker::WorkerMsg::Cancel { id });
        }
    }

    pub fn waiting(&self) -> usize {
        self.queue.borrow().len()
    }

    /// Whether another request may be queued without growing bookkeeping
    /// without bound. Agent callers turn a refusal into their own typed busy
    /// result rather than silently dropping work.
    pub fn can_accept(&self) -> bool {
        self.current.borrow().is_none() || self.waiting() < MAX_QUEUED_JOBS
    }

    /// The job the worker is running, if any.
    pub fn current_id(&self) -> Option<u64> {
        self.current.borrow().as_ref().map(|current| current.id)
    }

    pub fn undo_lines(&self) -> Vec<String> {
        self.undo_lines.borrow().clone().unwrap_or_default()
    }

    pub fn undo_summary(&self) -> Option<String> {
        self.undo_summary.borrow().clone()
    }

    pub fn baobab_available(&self) -> bool {
        self.baobab.get()
    }
}

impl Drop for FileOps {
    fn drop(&mut self) {
        let _ = self.jobs.send(worker::WorkerMsg::Shutdown);
        let _ = self.control.send(worker::WorkerMsg::Shutdown);
    }
}
