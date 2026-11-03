// SPDX-License-Identifier: GPL-3.0-or-later
//! The one thread that owns `fm_core`'s `FileOperations`.
//!
//! It receives one job at a time, builds the plan, sends it to the main
//! context, waits for the decisions, then executes with progress. Waiting for
//! a decision never blocks the main thread: the UI keeps running and a second
//! job waits in the client's queue.

use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use fm_core::fileops::{
    Decisions, EmptyConfirmation, ExecuteError, FileOperations, ItemOutcome, ItemResult, Plan,
    Progress, Report, Request,
};
use gtk::gio;
use gtk::gio::prelude::*;

/// One piece of work from the main thread.
pub enum WorkerMsg {
    Start {
        id: u64,
        request: Request,
        cancellable: gio::Cancellable,
        dry_run: bool,
    },
    Answer {
        id: u64,
        decisions: Decisions,
    },
    /// The destructive Empty Trash dialog returned Empty Trash.
    ConfirmEmpty {
        id: u64,
    },
    Cancel {
        id: u64,
    },
    Shutdown,
}

/// One update for the main thread.
pub enum WorkerEvent {
    Planned {
        id: u64,
        plan: Plan,
    },
    Progress {
        id: u64,
        progress: Progress,
    },
    Finished {
        id: u64,
        request: Request,
        plan: Plan,
        report: Report,
    },
    Refused {
        id: u64,
        request: Request,
        message: String,
    },
    /// The newest undo record, phrased for the background menu and its
    /// `dry-run=true` lines.
    UndoState {
        summary: Option<String>,
        lines: Vec<String>,
    },
    /// A worker-built dry run, used where planning itself lists the trash.
    DryRunReady {
        id: u64,
        request: Request,
        lines: Vec<String>,
    },
}

/// How often a running operation may send a progress update. GIO's byte
/// callback can fire thousands of times a second; the row needs 20.
const PROGRESS_MS: u64 = 50;

/// The two ends the client writes to. A `Start` goes on `jobs`; answers and
/// cancellation go on `control`, so a queued `Start` is never mistaken for an
/// answer while the worker waits for the conflict menu.
pub struct Senders {
    pub jobs: Sender<WorkerMsg>,
    pub control: Sender<WorkerMsg>,
}

pub fn spawn(events: Sender<WorkerEvent>) -> Senders {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("gimbal-fileops".to_string())
        .spawn(move || run(jobs_rx, control_rx, events))
        .expect("the file-operations thread starts");
    Senders {
        jobs: jobs_tx,
        control: control_tx,
    }
}

fn run(jobs: Receiver<WorkerMsg>, control: Receiver<WorkerMsg>, events: Sender<WorkerEvent>) {
    let mut operations = FileOperations::new();

    while let Ok(WorkerMsg::Start {
        id,
        request,
        cancellable,
        dry_run,
    }) = jobs.recv()
    {
        let plan = operations.plan(&request);
        log_plan(id, &plan);

        if let Some(problem) = plan.problem() {
            let _ = events.send(WorkerEvent::Refused {
                id,
                request,
                message: problem.to_string(),
            });
            continue;
        }

        if dry_run {
            let _ = events.send(WorkerEvent::DryRunReady {
                id,
                request,
                lines: plan.lines(),
            });
            continue;
        }

        let _ = events.send(WorkerEvent::Planned {
            id,
            plan: plan.clone(),
        });

        if cancellable.is_cancelled() {
            finish_cancelled(id, request, plan, &events);
            continue;
        }

        if request == Request::EmptyTrash {
            if wait_for_empty_confirmation(&control, id, &cancellable).is_none() {
                finish_cancelled(id, request, plan, &events);
                continue;
            }
        } else {
            let decisions = match wait_for_answer(&control, id, &cancellable) {
                Some(decisions) => decisions,
                None => {
                    finish_cancelled(id, request, plan, &events);
                    continue;
                }
            };

            if decisions.is_cancelled() || cancellable.is_cancelled() {
                finish_cancelled(id, request, plan, &events);
                continue;
            }

            let sender = events.clone();
            let mut last = Instant::now();
            let result = operations.execute(
                &plan,
                &decisions,
                &cancellable,
                move |progress: Progress| {
                    if last.elapsed() >= Duration::from_millis(PROGRESS_MS)
                        || progress.items_done == progress.items_total
                    {
                        last = Instant::now();
                        let _ = sender.send(WorkerEvent::Progress { id, progress });
                    }
                },
            );

            finish_operation(id, request, plan, result, &mut operations, &events);
            continue;
        }

        let sender = events.clone();
        let mut last = Instant::now();
        let Some(confirmation) = EmptyConfirmation::for_plan(&plan) else {
            finish_cancelled(id, request, plan, &events);
            continue;
        };
        let result = operations.execute_empty(
            &plan,
            confirmation,
            &cancellable,
            move |progress: Progress| {
                if last.elapsed() >= Duration::from_millis(PROGRESS_MS)
                    || progress.items_done == progress.items_total
                {
                    last = Instant::now();
                    let _ = sender.send(WorkerEvent::Progress { id, progress });
                }
            },
        );
        finish_operation(id, request, plan, result, &mut operations, &events);
    }
}

fn finish_operation(
    id: u64,
    request: Request,
    plan: Plan,
    result: Result<Report, ExecuteError>,
    operations: &mut FileOperations,
    events: &Sender<WorkerEvent>,
) {
    // The undo state goes before the final event, so the client's poll
    // sees it before it can decide the worker is idle.
    send_undo_state(operations, events);

    match result {
        Ok(report) => {
            crate::log!(
                "fileops",
                "report {}: {}",
                request.summary(),
                report.lines().join(" | ")
            );
            let _ = events.send(WorkerEvent::Finished {
                id,
                request,
                plan,
                report,
            });
        }
        Err(error) => {
            crate::log!("fileops", "refused {}: {error}", request.summary());
            let _ = events.send(WorkerEvent::Refused {
                id,
                request,
                message: error_message(&error),
            });
        }
    }
}

fn wait_for_empty_confirmation(
    control: &Receiver<WorkerMsg>,
    id: u64,
    cancellable: &gio::Cancellable,
) -> Option<()> {
    loop {
        match control.recv() {
            Ok(WorkerMsg::ConfirmEmpty { id: confirm_id }) if confirm_id == id => return Some(()),
            Ok(WorkerMsg::Cancel { id: cancel_id }) if cancel_id == id => {
                cancellable.cancel();
                return None;
            }
            Ok(WorkerMsg::Shutdown) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
}

fn wait_for_answer(
    control: &Receiver<WorkerMsg>,
    id: u64,
    cancellable: &gio::Cancellable,
) -> Option<Decisions> {
    loop {
        match control.recv() {
            Ok(WorkerMsg::Answer {
                id: answer_id,
                decisions,
            }) if answer_id == id => return Some(decisions),
            Ok(WorkerMsg::Cancel { id: cancel_id }) if cancel_id == id => {
                cancellable.cancel();
                return None;
            }
            Ok(WorkerMsg::Shutdown) | Err(_) => return None,
            // A stale answer for an earlier job is harmless.
            Ok(_) => continue,
        }
    }
}

fn finish_cancelled(id: u64, request: Request, plan: Plan, events: &Sender<WorkerEvent>) {
    let report = Report {
        cancelled: true,
        items: plan
            .steps()
            .iter()
            .map(|step| ItemResult {
                action: step.action,
                source: step
                    .source
                    .clone()
                    .or_else(|| step.target.clone())
                    .unwrap_or_default(),
                target: step.target.clone(),
                trash: step.trash.clone(),
                outcome: ItemOutcome::Cancelled,
            })
            .collect(),
    };
    crate::log!(
        "fileops",
        "cancelled before running {}: {} item(s)",
        request.summary(),
        report.items.len()
    );
    let _ = events.send(WorkerEvent::Finished {
        id,
        request,
        plan,
        report,
    });
}

fn send_undo_state(operations: &FileOperations, events: &Sender<WorkerEvent>) {
    let plan = operations.plan(&Request::Undo);
    if plan.steps().is_empty() {
        let _ = events.send(WorkerEvent::UndoState {
            summary: None,
            lines: Vec::new(),
        });
        return;
    }
    let summary = crate::fileops::progress::undo_summary(&plan);
    let lines = plan.lines();
    crate::log!(
        "fileops",
        "undo state: {}",
        summary.clone().unwrap_or_else(|| "nothing".to_string())
    );
    let _ = events.send(WorkerEvent::UndoState { summary, lines });
}

fn log_plan(id: u64, plan: &Plan) {
    let steps: Vec<String> = plan.lines().into_iter().skip(1).collect();
    crate::log!(
        "fileops",
        "plan {id} ({}): {} step(s): {}",
        plan.request().summary(),
        plan.steps().len(),
        steps.join(" | ")
    );
}

fn error_message(error: &ExecuteError) -> String {
    match error {
        ExecuteError::NothingToUndo => "there is nothing to undo".to_string(),
        other => other.to_string(),
    }
}
