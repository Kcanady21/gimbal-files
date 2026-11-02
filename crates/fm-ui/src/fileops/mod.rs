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
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use fm_core::fileops::{Decisions, FileOperations, Plan, Progress, Request};
use fm_core::workspace::WindowId;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use crate::app::Process;
use crate::fileops::conflict::ConflictFlow;
use crate::fileops::progress::OpKind;

pub use menu::ItemInfo;
pub use progress::ProgressView;

/// How often the main context checks the worker's channel.
const POLL_MS: u64 = 30;

struct Queued {
    id: u64,
    request: Request,
    window: WindowId,
    clear_clipboard: bool,
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
    baobab: Cell<bool>,
}

impl FileOps {
    pub fn new(process: &Rc<Process>) -> Rc<Self> {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        let senders = worker::spawn(event_tx);
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
            baobab: Cell::new(baobab),
        })
    }

    /// Queue one request and return its id. The first runs at once; later
    /// ones wait and are shown as "N waiting".
    pub fn submit(
        self: &Rc<Self>,
        window: WindowId,
        request: Request,
        clear_clipboard: bool,
    ) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        let job = Queued {
            id,
            request,
            window,
            clear_clipboard,
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

    fn start(self: &Rc<Self>, job: Queued) {
        let cancellable = gio::Cancellable::new();
        if self
            .jobs
            .send(worker::WorkerMsg::Start {
                id: job.id,
                request: job.request.clone(),
                cancellable: cancellable.clone(),
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
        });
        if let Some(process) = self.process.borrow().upgrade() {
            process.fileop_started(job.window);
        }
        crate::log!(
            "fileops",
            "started job {}: {}",
            job.id,
            job.request.summary()
        );
    }

    fn ensure_poll(self: &Rc<Self>) {
        if self.poll.borrow().is_some() {
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
            process.fileop_progress(window, Some(view));
        }
    }

    fn handle(self: &Rc<Self>, event: worker::WorkerEvent) {
        match event {
            worker::WorkerEvent::Planned { id, plan } => {
                let window = {
                    let mut current = self.current.borrow_mut();
                    let Some(current) = current.as_mut() else {
                        return;
                    };
                    if current.id != id {
                        return;
                    }
                    current.plan = Some(plan.clone());
                    current.window
                };
                let conflicts = plan.decisions().count();
                if conflicts == 0 {
                    self.answer(id, Decisions::from_plan(&plan));
                } else if let Some(process) = self.process.borrow().upgrade() {
                    crate::log!("fileops", "{conflicts} conflict(s) need a decision");
                    if let Some(current) = self.current.borrow_mut().as_mut() {
                        current.answering = true;
                    }
                    let fileops = self.clone();
                    process.fileop_conflicts(
                        window,
                        ConflictFlow::from_plan(&plan),
                        Box::new(move |decisions| fileops.answer(id, decisions)),
                    );
                } else {
                    self.answer(id, Decisions::new(Vec::new()));
                }
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
                        process.fileop_progress(window, Some(view));
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
                    process.fileop_finished(current.window, &request, &plan, &report);
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
                    process.fileop_refused(current.window, &request, &message);
                }
                self.start_next();
            }
            worker::WorkerEvent::UndoState { summary, lines } => {
                crate::log!(
                    "fileops",
                    "undo stack: {}",
                    summary.clone().unwrap_or_else(|| "empty".to_string())
                );
                *self.undo_summary.borrow_mut() = summary;
                *self.undo_lines.borrow_mut() = Some(lines);
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

    pub fn waiting(&self) -> usize {
        self.queue.borrow().len()
    }

    pub fn undo_summary(&self) -> Option<String> {
        self.undo_summary.borrow().clone()
    }

    pub fn baobab_available(&self) -> bool {
        self.baobab.get()
    }

    /// The `dry-run=true` lines. A normal plan is read-only, so it is safe to
    /// build here. Undo uses the worker's latest record, sent with every
    /// finished job, because only the worker owns the undo stack.
    pub fn plan_lines(&self, request: &Request) -> Vec<String> {
        if matches!(request, Request::Undo)
            && let Some(lines) = self.undo_lines.borrow().clone()
            && !lines.is_empty()
        {
            return lines;
        }
        FileOperations::new().plan(request).lines()
    }
}

impl Drop for FileOps {
    fn drop(&mut self) {
        let _ = self.jobs.send(worker::WorkerMsg::Shutdown);
        let _ = self.control.send(worker::WorkerMsg::Shutdown);
    }
}
