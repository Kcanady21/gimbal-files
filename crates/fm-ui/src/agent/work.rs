// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded worker pool for filesystem evidence used by the socket bridge.
//!
//! The GTK thread never resolves a path or plans a file request. A bridge
//! request submits one typed job here; the result comes back to the same
//! polling loop and is checked against the enable generation before it is
//! applied. The queue and worker count are fixed so a blocked filesystem call
//! cannot create an unbounded set of threads.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};

use fm_core::fileops::{FileOperations, Request};

/// How many filesystem jobs may wait at once.
pub const QUEUE: usize = 32;
/// Two workers keep one blocked lookup from stopping the other evidence.
pub const WORKERS: usize = 2;

/// One piece of filesystem evidence.
#[derive(Debug, Clone)]
pub enum Work {
    /// Resolve a `nav.go` path without touching the model or focus.
    Nav { path: PathBuf },
    /// Build a read-only plan's real lines on a worker.
    DryRun {
        request: Request,
        /// The worker owns the undo stack, so an undo dry run is phrased from
        /// the snapshot the bridge already holds instead of a fresh planner.
        undo_lines: Option<Vec<String>>,
    },
}

/// A typed result, tagged with the generation that requested it.
#[derive(Debug, Clone)]
pub struct WorkResult {
    pub id: u64,
    pub generation: u64,
    pub outcome: Result<WorkOutcome, String>,
}

#[derive(Debug, Clone)]
pub enum WorkOutcome {
    Nav {
        folder: PathBuf,
        select: Option<OsString>,
    },
    DryRun(Vec<String>),
}

type Runner = Arc<dyn Fn(Work) -> Result<WorkOutcome, String> + Send + Sync>;

/// A fixed worker pool with a bounded inbound queue.
pub struct WorkPool {
    sender: Option<SyncSender<(u64, u64, Work)>>,
    results: Receiver<WorkResult>,
}

impl WorkPool {
    /// The production pool. `default_runner` is the only filesystem code.
    pub fn start() -> Self {
        Self::with_runner(Arc::new(default_runner))
    }

    /// A test seam that keeps the production scheduling and generation
    /// checks. The runner may block or inject an error.
    pub fn with_runner(runner: Runner) -> Self {
        let (sender, jobs) = std::sync::mpsc::sync_channel::<(u64, u64, Work)>(QUEUE);
        let (result_tx, results) = std::sync::mpsc::channel();
        let jobs = Arc::new(std::sync::Mutex::new(jobs));
        for _ in 0..WORKERS {
            let jobs = Arc::clone(&jobs);
            let result_tx = result_tx.clone();
            let runner = Arc::clone(&runner);
            // The worker set is fixed. Its handle is intentionally detached:
            // app shutdown must not join a filesystem call that may be blocked
            // in an uninterruptible lookup.
            std::thread::Builder::new()
                .name("gimbal-agent-work".to_string())
                .spawn(move || {
                    loop {
                        let job = match jobs.lock() {
                            Ok(receiver) => receiver.recv(),
                            Err(_) => return,
                        };
                        let Ok((id, generation, work)) = job else {
                            return;
                        };
                        let outcome = runner(work);
                        if result_tx
                            .send(WorkResult {
                                id,
                                generation,
                                outcome,
                            })
                            .is_err()
                        {
                            return;
                        }
                    }
                })
                .expect("the agent work thread starts");
        }
        Self {
            sender: Some(sender),
            results,
        }
    }

    /// Nonblocking admission. A full queue is reported, never waited on.
    pub fn try_submit(&self, id: u64, generation: u64, work: Work) -> Result<(), String> {
        let Some(sender) = &self.sender else {
            return Err("the filesystem worker pool is unavailable".to_string());
        };
        match sender.try_send((id, generation, work)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("the filesystem worker queue is full".to_string()),
            Err(TrySendError::Disconnected(_)) => {
                Err("the filesystem worker pool is unavailable".to_string())
            }
        }
    }

    pub fn try_result(&self) -> Option<WorkResult> {
        self.results.try_recv().ok()
    }
}

impl Drop for WorkPool {
    fn drop(&mut self) {
        // Closing the sender lets each fixed worker leave `recv` when it is
        // not blocked. Never join here: this may be the GTK/app-shutdown
        // thread and a filesystem lookup can be uninterruptible.
        drop(self.sender.take());
    }
}

fn default_runner(work: Work) -> Result<WorkOutcome, String> {
    match work {
        Work::Nav { path } => {
            if path.is_dir() {
                return Ok(WorkOutcome::Nav {
                    folder: path,
                    select: None,
                });
            }
            if path.is_file() {
                let parent = path
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/"));
                let name = path.file_name().map(OsString::from);
                return Ok(WorkOutcome::Nav {
                    folder: parent,
                    select: name,
                });
            }
            Err(format!("{} does not exist", path.display()))
        }
        Work::DryRun {
            request,
            undo_lines,
        } => {
            let lines = match undo_lines {
                Some(lines) if !lines.is_empty() => lines,
                _ => FileOperations::new().plan(&request).lines(),
            };
            Ok(WorkOutcome::DryRun(lines))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_full_queue_is_refused_without_blocking() {
        let gate = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        let worker_gate = Arc::clone(&gate);
        let pool = WorkPool::with_runner(Arc::new(move |_| {
            let (lock, wake) = &*worker_gate;
            let mut open = lock.lock().expect("gate");
            while !*open {
                open = wake.wait(open).expect("gate");
            }
            Ok(WorkOutcome::DryRun(vec!["done".to_string()]))
        }));
        // Fill the queue and both workers, then one more must be refused.
        let mut accepted = 0;
        for id in 0..(QUEUE + WORKERS + 4) as u64 {
            match pool.try_submit(
                id,
                1,
                Work::Nav {
                    path: PathBuf::from("/tmp"),
                },
            ) {
                Ok(()) => accepted += 1,
                Err(message) => {
                    assert!(message.contains("full"), "{message}");
                    break;
                }
            }
        }
        assert!(accepted >= WORKERS);
        let (lock, wake) = &*gate;
        *lock.lock().expect("gate") = true;
        wake.notify_all();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if pool.try_result().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn results_keep_the_request_generation_and_late_evidence_is_visible() {
        let pool = WorkPool::with_runner(Arc::new(|work| match work {
            Work::DryRun { request, .. } => Ok(WorkOutcome::DryRun(
                FileOperations::new().plan(&request).lines(),
            )),
            Work::Nav { path } => Err(format!("blocked {}", path.display())),
        }));
        pool.try_submit(
            7,
            41,
            Work::DryRun {
                request: Request::Trash { paths: Vec::new() },
                undo_lines: None,
            },
        )
        .expect("queued");
        let deadline = Instant::now() + Duration::from_secs(2);
        let result = loop {
            if let Some(result) = pool.try_result() {
                break result;
            }
            assert!(Instant::now() < deadline, "no result");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(result.id, 7);
        assert_eq!(result.generation, 41);
        assert!(matches!(result.outcome, Ok(WorkOutcome::DryRun(_))));
    }
}
