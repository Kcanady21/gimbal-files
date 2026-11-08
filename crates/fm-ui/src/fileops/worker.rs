// SPDX-License-Identifier: GPL-3.0-or-later
//! The one thread that owns `fm_core`'s `FileOperations`.
//!
//! It receives one job at a time, builds the plan, sends it to the main
//! context, waits for the decisions, then executes with progress. Waiting for
//! a decision never blocks the main thread: the UI keeps running and a second
//! job waits in the client's queue.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use fm_agent::protect;

use crate::agent::JobAuthorization;

use fm_core::fileops::{
    Decisions, EmptyConfirmation, ExecuteError, FileOperations, ItemOutcome, ItemResult, Plan,
    Progress, Report, Request, SelectedConfirmation,
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
        authorization: Option<JobAuthorization>,
        executing: Arc<AtomicBool>,
    },
    Answer {
        id: u64,
        decisions: Decisions,
    },
    /// The destructive Empty Trash dialog returned Empty Trash.
    ConfirmEmpty {
        id: u64,
    },
    /// The destructive selected-deletion dialog returned Delete Permanently.
    ConfirmSelected {
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
    /// The worker is about to call execute. Revocation may no longer claim to
    /// cancel this job; the ordinary Cancel button still can.
    Executing {
        id: u64,
    },
    /// Revoked authorization stopped the job before execution.
    Revoked {
        id: u64,
        request: Request,
        message: String,
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
    /// `dry-run=true` lines. `steps` lets the agent bridge refuse an undo that
    /// would reach a protected control path.
    UndoState {
        summary: Option<String>,
        lines: Vec<String>,
        steps: Vec<(std::path::PathBuf, Option<std::path::PathBuf>)>,
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

/// A deterministic test gate at a named worker boundary. Production leaves it
/// `None`; the real scheduling, protection and execution paths are unchanged.
pub type PauseHook = Arc<dyn Fn(&str, u64) + Send + Sync>;

fn pause(hook: &Option<PauseHook>, boundary: &str, id: u64) {
    if let Some(hook) = hook {
        hook(boundary, id);
    }
}

/// The two ends the client writes to. A `Start` goes on `jobs`; answers and
/// cancellation go on `control`, so a queued `Start` is never mistaken for an
/// answer while the worker waits for the conflict menu.
pub struct Senders {
    pub jobs: Sender<WorkerMsg>,
    pub control: Sender<WorkerMsg>,
}

pub fn spawn_with_pause(events: Sender<WorkerEvent>, pause: Option<PauseHook>) -> Senders {
    let (jobs_tx, jobs_rx) = std::sync::mpsc::channel();
    let (control_tx, control_rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("gimbal-fileops".to_string())
        .spawn(move || run(jobs_rx, control_rx, events, pause))
        .expect("the file-operations thread starts");
    Senders {
        jobs: jobs_tx,
        control: control_tx,
    }
}

fn run(
    jobs: Receiver<WorkerMsg>,
    control: Receiver<WorkerMsg>,
    events: Sender<WorkerEvent>,
    pause_hook: Option<PauseHook>,
) {
    let mut operations = FileOperations::new();

    while let Ok(WorkerMsg::Start {
        id,
        request,
        cancellable,
        dry_run,
        authorization,
        executing,
    }) = jobs.recv()
    {
        // The immutable ticket was captured when the command was accepted.
        // Off revokes it immediately; a queued or planning job never starts.
        if let Some(auth) = &authorization
            && !auth.is_active()
        {
            let _ = events.send(WorkerEvent::Revoked {
                id,
                request,
                message: "local automation was turned off before this job began".to_string(),
            });
            continue;
        }

        // Declared request paths are resolved here, on the worker, from the
        // same authorization. This repeats the transport check for callbacks
        // and any future path that did not pass through a typed run request.
        if let Some(auth) = &authorization
            && let Err(error) = protect::check_request(&request, &auth.roots)
        {
            let _ = events.send(WorkerEvent::Refused {
                id,
                request,
                message: error.to_string(),
            });
            continue;
        }

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

        if let Some(auth) = &authorization
            && let Err(error) = protect::check_steps(
                plan.steps().iter().filter_map(|step| {
                    let source = step.source.as_deref().or(step.target.as_deref())?;
                    Some((source, step.target.as_deref()))
                }),
                &auth.roots,
            )
        {
            let _ = events.send(WorkerEvent::Refused {
                id,
                request,
                message: error.to_string(),
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

        pause(&pause_hook, "planned", id);
        let _ = events.send(WorkerEvent::Planned {
            id,
            plan: plan.clone(),
        });

        if cancellable.is_cancelled() {
            finish_cancelled(id, request, plan, &events);
            continue;
        }

        match &request {
            Request::EmptyTrash => {
                if !wait_for_approval(&control, id, &cancellable, Approval::Empty) {
                    finish_cancelled(id, request, plan, &events);
                    continue;
                }
                let Some(confirmation) = EmptyConfirmation::for_plan(&plan) else {
                    finish_cancelled(id, request, plan, &events);
                    continue;
                };
                pause(&pause_hook, "revalidate", id);
                if !revalidate(&authorization, &executing, id, &request, &plan, &events) {
                    continue;
                }
                if !begin_execution(&authorization, &executing, id, &request, &events) {
                    continue;
                }
                let sender = events.clone();
                let mut last = Instant::now();
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
            Request::DeleteSelected { .. } => {
                if !wait_for_approval(&control, id, &cancellable, Approval::Selected) {
                    finish_cancelled(id, request, plan, &events);
                    continue;
                }
                let Some(confirmation) = SelectedConfirmation::for_plan(&plan) else {
                    finish_cancelled(id, request, plan, &events);
                    continue;
                };
                pause(&pause_hook, "revalidate", id);
                if !revalidate(&authorization, &executing, id, &request, &plan, &events) {
                    continue;
                }
                if !begin_execution(&authorization, &executing, id, &request, &events) {
                    continue;
                }
                let sender = events.clone();
                let mut last = Instant::now();
                let result = operations.execute_selected(
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
            _ => {
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
                pause(&pause_hook, "revalidate", id);
                if !revalidate(&authorization, &executing, id, &request, &plan, &events) {
                    continue;
                }
                if !begin_execution(&authorization, &executing, id, &request, &events) {
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
            }
        }
    }
}

/// Repeat the full protection check after the decision wait, immediately
/// before the execution boundary. The destination alias may have changed while
/// the user was answering, and Off may have revoked the ticket. The immutable
/// original roots are retained even after automation is off.
fn revalidate(
    authorization: &Option<JobAuthorization>,
    executing: &Arc<AtomicBool>,
    id: u64,
    request: &Request,
    plan: &Plan,
    events: &Sender<WorkerEvent>,
) -> bool {
    let Some(auth) = authorization else {
        return true;
    };
    if !auth.is_active() {
        let _ = events.send(WorkerEvent::Revoked {
            id,
            request: request.clone(),
            message: "local automation was turned off before this job began".to_string(),
        });
        return false;
    }
    if let Err(error) = protect::check_request(request, &auth.roots) {
        let _ = events.send(WorkerEvent::Refused {
            id,
            request: request.clone(),
            message: error.to_string(),
        });
        return false;
    }
    if let Err(error) = protect::check_steps(
        plan.steps().iter().filter_map(|step| {
            let source = step.source.as_deref().or(step.target.as_deref())?;
            Some((source, step.target.as_deref()))
        }),
        &auth.roots,
    ) {
        let _ = events.send(WorkerEvent::Refused {
            id,
            request: request.clone(),
            message: error.to_string(),
        });
        return false;
    }
    // Check the ticket once more after the blocking filesystem resolution and
    // before the effect.
    if !auth.is_active() {
        executing.store(false, Ordering::SeqCst);
        let _ = events.send(WorkerEvent::Revoked {
            id,
            request: request.clone(),
            message: "local automation was turned off before this job began".to_string(),
        });
        return false;
    }
    true
}

/// Publish the execution boundary before any write. A revoked ticket sends a
/// terminal `Revoked` event instead; once `executing` is published, Off no
/// longer cancels the job.
fn begin_execution(
    authorization: &Option<JobAuthorization>,
    executing: &Arc<AtomicBool>,
    id: u64,
    request: &Request,
    events: &Sender<WorkerEvent>,
) -> bool {
    if let Some(auth) = authorization
        && !auth.is_active()
    {
        let _ = events.send(WorkerEvent::Revoked {
            id,
            request: request.clone(),
            message: "local automation was turned off before this job began".to_string(),
        });
        return false;
    }
    executing.store(true, Ordering::SeqCst);
    if let Some(auth) = authorization
        && !auth.is_active()
    {
        executing.store(false, Ordering::SeqCst);
        let _ = events.send(WorkerEvent::Revoked {
            id,
            request: request.clone(),
            message: "local automation was turned off before this job began".to_string(),
        });
        return false;
    }
    let _ = events.send(WorkerEvent::Executing { id });
    true
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Approval {
    Empty,
    Selected,
}

/// Wait for the one destructive approval that belongs to this job and this
/// operation kind. The other kind is ignored: an Empty Trash dialog can never
/// authorise selected deletion, or the reverse.
fn wait_for_approval(
    control: &Receiver<WorkerMsg>,
    id: u64,
    cancellable: &gio::Cancellable,
    approval: Approval,
) -> bool {
    loop {
        match control.recv() {
            Ok(WorkerMsg::ConfirmEmpty { id: confirm_id })
                if approval == Approval::Empty && confirm_id == id =>
            {
                return true;
            }
            Ok(WorkerMsg::ConfirmSelected { id: confirm_id })
                if approval == Approval::Selected && confirm_id == id =>
            {
                return true;
            }
            Ok(WorkerMsg::Cancel { id: cancel_id }) if cancel_id == id => {
                cancellable.cancel();
                return false;
            }
            Ok(WorkerMsg::Shutdown) | Err(_) => return false,
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
            steps: Vec::new(),
        });
        return;
    }
    let summary = crate::fileops::progress::undo_summary(&plan);
    let lines = plan.lines();
    let steps = plan
        .steps()
        .iter()
        .map(|step| (step.source.clone().unwrap_or_default(), step.target.clone()))
        .collect();
    crate::log!(
        "fileops",
        "undo state: {}",
        summary.clone().unwrap_or_else(|| "nothing".to_string())
    );
    let _ = events.send(WorkerEvent::UndoState {
        summary,
        lines,
        steps,
    });
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

#[cfg(test)]
mod tests {
    use super::*;
    use fm_agent::ProtectedRoots;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::mpsc::channel;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "/tmp/gimbal-agent-worker-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            );
            fs::create_dir_all(&unique).expect("scratch");
            fs::set_permissions(&unique, fs::Permissions::from_mode(0o700)).expect("private");
            Self {
                root: fs::canonicalize(unique).expect("canonical"),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(scratch: &Scratch) -> (JobAuthorization, PathBuf, PathBuf) {
        let config_dir = scratch.root.join("config/gimbal-files");
        fs::create_dir_all(&config_dir).expect("config dir");
        let config_file = config_dir.join("config.toml");
        fs::write(&config_file, "[agent]\n").expect("config");
        let runtime = scratch.root.join("run/gimbal-files");
        fs::create_dir_all(&runtime).expect("runtime");
        let roots = ProtectedRoots::new(&config_file, &config_dir, &runtime)
            .resolved()
            .expect("roots");
        let source_dir = scratch.root.join("src");
        fs::create_dir_all(&source_dir).expect("source dir");
        let source = source_dir.join("a.txt");
        fs::write(&source, "payload").expect("source");
        let dest = scratch.root.join("dest");
        fs::create_dir_all(&dest).expect("dest");
        (
            crate::agent::test_authorization_with_roots(3, roots),
            source,
            dest,
        )
    }

    fn next_event(events: &Receiver<WorkerEvent>, label: &str) -> WorkerEvent {
        events
            .recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| panic!("{label}: {error}"))
    }

    #[test]
    fn a_destination_alias_changed_after_planned_is_refused_before_the_write() {
        let scratch = Scratch::new("final-guard");
        let (authorization, source, dest) = fixture(&scratch);
        let (events, received) = channel();
        let senders = spawn_with_pause(events, None);
        let executing = Arc::new(AtomicBool::new(false));
        senders
            .jobs
            .send(WorkerMsg::Start {
                id: 1,
                request: Request::Copy {
                    scope: fm_core::admin::Scope::User,
                    target_scope: fm_core::admin::Scope::User,
                    paths: vec![source],
                    to: dest.clone(),
                },
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: Some(authorization),
                executing,
            })
            .expect("start");

        let WorkerEvent::Planned { plan, .. } = next_event(&received, "planned") else {
            panic!("expected the plan");
        };
        // The user answers after the destination alias is swapped for a link
        // into the protected control directory.
        fs::remove_dir_all(&dest).expect("remove allowed dest");
        let protected = scratch.root.join("config/gimbal-files");
        std::os::unix::fs::symlink(&protected, &dest).expect("symlink dest");
        senders
            .control
            .send(WorkerMsg::Answer {
                id: 1,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer");

        let event = loop {
            match next_event(&received, "refusal") {
                WorkerEvent::Refused { message, .. } => break message,
                WorkerEvent::Executing { .. } => panic!("executed a changed destination"),
                _ => continue,
            }
        };
        assert!(event.contains("protected"), "{event}");
        assert!(
            !protected.join("a.txt").exists(),
            "the protected fixture was written"
        );
        assert_eq!(
            fs::read_to_string(protected.join("config.toml")).expect("config"),
            "[agent]\n"
        );
        let _ = senders.jobs.send(WorkerMsg::Shutdown);
    }

    fn refuse_after_plan(
        id: u64,
        request: Request,
        authorization: JobAuthorization,
        mutate: impl FnOnce(),
    ) {
        let (events, received) = channel();
        let senders = spawn_with_pause(events, None);
        let executing = Arc::new(AtomicBool::new(false));
        senders
            .jobs
            .send(WorkerMsg::Start {
                id,
                request,
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: Some(authorization),
                executing: Arc::clone(&executing),
            })
            .expect("start");
        let WorkerEvent::Planned { plan, .. } = next_event(&received, "planned") else {
            panic!("expected the plan");
        };
        mutate();
        senders
            .control
            .send(WorkerMsg::Answer {
                id,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer");
        loop {
            match next_event(&received, "refusal") {
                WorkerEvent::Refused { message, .. } => {
                    assert!(message.contains("protected"), "{message}");
                    break;
                }
                WorkerEvent::Executing { .. } => panic!("executed a changed destination"),
                _ => continue,
            }
        }
        assert!(!executing.load(Ordering::SeqCst));
        let _ = senders.jobs.send(WorkerMsg::Shutdown);
    }

    #[test]
    fn move_rename_and_new_folder_are_revalidated_after_planned() {
        // Build each request explicitly while keeping one shared refusal path.
        for kind in ["move", "rename", "new-folder"] {
            let scratch = Scratch::new(kind);
            let (authorization, source, dest) = fixture(&scratch);
            let mutation_target = if kind == "rename" {
                source.parent().expect("source parent").to_path_buf()
            } else {
                dest.clone()
            };
            let request = match kind {
                "move" => Request::Move {
                    scope: fm_core::admin::Scope::User,
                    target_scope: fm_core::admin::Scope::User,
                    paths: vec![source],
                    to: dest.clone(),
                },
                "rename" => Request::Rename {
                    scope: fm_core::admin::Scope::User,
                    path: source,
                    name: "renamed".to_string(),
                },
                _ => Request::NewFolder {
                    scope: fm_core::admin::Scope::User,
                    name: "new".to_string(),
                    in_folder: dest.clone(),
                },
            };
            let protected = scratch.root.join("config/gimbal-files");
            let id = match kind {
                "move" | "new-folder" => 10,
                _ => 11,
            };
            refuse_after_plan(id, request, authorization, || {
                let _ = fs::remove_dir_all(&mutation_target);
                std::os::unix::fs::symlink(&protected, &mutation_target).expect("symlink target");
            });
        }
    }

    #[test]
    fn a_new_file_that_appears_after_planning_is_refused_by_the_worker() {
        let scratch = Scratch::new("new-file-race");
        let folder = scratch.root.join("home");
        fs::create_dir_all(&folder).expect("folder");
        let target = folder.join("empty.txt");
        let (events, received) = channel();
        let senders = spawn_with_pause(events, None);
        let executing = Arc::new(AtomicBool::new(false));
        senders
            .jobs
            .send(WorkerMsg::Start {
                id: 31,
                request: Request::NewFile {
                    scope: fm_core::admin::Scope::User,
                    name: "empty.txt".to_string(),
                    in_folder: folder.clone(),
                },
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: None,
                executing,
            })
            .expect("start");
        let WorkerEvent::Planned { plan, .. } = next_event(&received, "planned") else {
            panic!("expected the plan");
        };
        fs::write(&target, "already here").expect("collision");
        senders
            .control
            .send(WorkerMsg::Answer {
                id: 31,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer");
        loop {
            match next_event(&received, "finished") {
                WorkerEvent::Finished { report, .. } => {
                    assert_eq!(
                        report.items[0].outcome,
                        ItemOutcome::Refused("the name is taken".to_string())
                    );
                    break;
                }
                WorkerEvent::Refused { message, .. } => panic!("refused: {message}"),
                _ => continue,
            }
        }
        assert_eq!(
            fs::read_to_string(&target).expect("existing contents"),
            "already here"
        );
        let _ = senders.jobs.send(WorkerMsg::Shutdown);
    }

    #[test]
    fn an_undo_plan_is_revalidated_after_the_decision_wait() {
        let scratch = Scratch::new("undo-revalidate");
        let (authorization, source, dest) = fixture(&scratch);
        let (events, received) = channel();
        let senders = spawn_with_pause(events, None);
        // First copy for real so the same worker owns an undo record.
        let executing = Arc::new(AtomicBool::new(false));
        senders
            .jobs
            .send(WorkerMsg::Start {
                id: 21,
                request: Request::Copy {
                    scope: fm_core::admin::Scope::User,
                    target_scope: fm_core::admin::Scope::User,
                    paths: vec![source.clone()],
                    to: dest.clone(),
                },
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: Some(authorization.clone()),
                executing: Arc::clone(&executing),
            })
            .expect("start copy");
        let WorkerEvent::Planned { plan, .. } = next_event(&received, "copy plan") else {
            panic!("expected the copy plan");
        };
        senders
            .control
            .send(WorkerMsg::Answer {
                id: 21,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer copy");
        loop {
            match next_event(&received, "copy finish") {
                WorkerEvent::Finished { .. } => break,
                WorkerEvent::Refused { message, .. } => panic!("copy refused: {message}"),
                _ => continue,
            }
        }
        assert!(dest.join("a.txt").exists());

        senders
            .jobs
            .send(WorkerMsg::Start {
                id: 22,
                request: Request::Undo,
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: Some(authorization),
                executing: Arc::clone(&executing),
            })
            .expect("start undo");
        let WorkerEvent::Planned { plan, .. } = next_event(&received, "undo plan") else {
            panic!("expected the undo plan");
        };
        assert!(!plan.steps().is_empty());
        // The undo step reads the copied file. Its folder becomes a protected
        // alias after the plan was published.
        let protected = scratch.root.join("config/gimbal-files");
        fs::remove_dir_all(&dest).expect("remove copied folder");
        std::os::unix::fs::symlink(&protected, &dest).expect("symlink copied folder");
        senders
            .control
            .send(WorkerMsg::Answer {
                id: 22,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer undo");
        loop {
            match next_event(&received, "undo refusal") {
                WorkerEvent::Refused { message, .. } => {
                    assert!(message.contains("protected"), "{message}");
                    break;
                }
                WorkerEvent::Executing { .. } => panic!("undo executed after the alias changed"),
                _ => continue,
            }
        }
        let _ = senders.jobs.send(WorkerMsg::Shutdown);
    }

    #[test]
    fn revocation_while_the_final_resolution_is_paused_never_executes() {
        let scratch = Scratch::new("final-revoke");
        let (authorization, source, dest) = fixture(&scratch);
        let entered = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let hook_entered = Arc::clone(&entered);
        let hook_release = Arc::clone(&release);
        let hook: PauseHook = Arc::new(move |boundary, _id| {
            if boundary != "revalidate" {
                return;
            }
            *hook_entered.0.lock().expect("entered") = true;
            hook_entered.1.notify_all();
            let mut open = hook_release.0.lock().expect("release");
            while !*open {
                open = hook_release.1.wait(open).expect("release");
            }
        });
        let (events, received) = channel();
        let senders = spawn_with_pause(events, Some(hook));
        let executing = Arc::new(AtomicBool::new(false));
        senders
            .jobs
            .send(WorkerMsg::Start {
                id: 2,
                request: Request::Copy {
                    scope: fm_core::admin::Scope::User,
                    target_scope: fm_core::admin::Scope::User,
                    paths: vec![source],
                    to: dest.clone(),
                },
                cancellable: gio::Cancellable::new(),
                dry_run: false,
                authorization: Some(authorization.clone()),
                executing: Arc::clone(&executing),
            })
            .expect("start");
        let WorkerEvent::Planned { plan, .. } = next_event(&received, "planned") else {
            panic!("expected the plan");
        };
        senders
            .control
            .send(WorkerMsg::Answer {
                id: 2,
                decisions: Decisions::from_plan(&plan),
            })
            .expect("answer");
        {
            let (lock, wake) = &*entered;
            let mut seen = lock.lock().expect("entered");
            while !*seen {
                seen = wake.wait(seen).expect("entered");
            }
        }
        authorization.revoke_for_tests();
        *release.0.lock().expect("release") = true;
        release.1.notify_all();

        let event = next_event(&received, "revoked");
        assert!(matches!(event, WorkerEvent::Revoked { .. }));
        assert!(!executing.load(Ordering::SeqCst));
        assert!(
            !dest.join("a.txt").exists(),
            "a revoked job wrote the destination"
        );
        let _ = senders.jobs.send(WorkerMsg::Shutdown);
    }

    fn request() -> Request {
        Request::Copy {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![std::path::PathBuf::from("/tmp/gimbal-agent-test/a")],
            to: std::path::PathBuf::from("/tmp/gimbal-agent-test/dest"),
        }
    }

    #[test]
    fn a_revoked_ticket_never_publishes_the_execution_boundary() {
        let authorization = Some(crate::agent::test_authorization(4, true));
        let executing = Arc::new(AtomicBool::new(false));
        let (events, received) = channel();
        assert!(!begin_execution(
            &authorization,
            &executing,
            4,
            &request(),
            &events,
        ));
        assert!(!executing.load(Ordering::SeqCst));
        assert!(matches!(
            received.recv().expect("event"),
            WorkerEvent::Revoked { .. }
        ));
    }

    #[test]
    fn an_active_ticket_publishes_execution_once_and_keeps_its_context() {
        let authorization = Some(crate::agent::test_authorization(7, false));
        let executing = Arc::new(AtomicBool::new(false));
        let (events, received) = channel();
        assert!(begin_execution(
            &authorization,
            &executing,
            11,
            &request(),
            &events,
        ));
        assert!(executing.load(Ordering::SeqCst));
        assert!(matches!(
            received.recv().expect("event"),
            WorkerEvent::Executing { id: 11 }
        ));
    }
}
