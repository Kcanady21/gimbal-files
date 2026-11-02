// SPDX-License-Identifier: GPL-3.0-or-later
//! The session save queue and its debounce.
//!
//! One model change restarts a one-second timeout. When it fires, the
//! workspace is cloned and handed to one background thread, so a slow disk
//! never blocks the window. A blocking variant is used at quit, where the
//! process is about to end and the last write must land.

use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use fm_core::session;
use fm_core::workspace::Workspace;

/// One second after the last model change.
pub const SAVE_DELAY_MS: u64 = 1_000;

static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub fn now_ms() -> u64 {
    START
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// A pure debounce over a caller-supplied clock. The runtime uses it to pick
/// the one timeout's remaining time; the tests drive it with a fake clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Debounce {
    delay_ms: u64,
    deadline: Option<u64>,
}

#[allow(dead_code)]
impl Debounce {
    pub const fn new(delay_ms: u64) -> Self {
        Self {
            delay_ms,
            deadline: None,
        }
    }

    /// Note a model change at `now_ms`: the next save is `delay_ms` later.
    pub fn change(&mut self, now_ms: u64) {
        self.deadline = Some(now_ms.saturating_add(self.delay_ms));
    }

    pub fn cancel(&mut self) {
        self.deadline = None;
    }

    pub fn pending(&self) -> bool {
        self.deadline.is_some()
    }

    /// How long until the pending save is due; zero when it is due or none is
    /// pending. Zero is the caller's "save now".
    pub fn remaining(&self, now_ms: u64) -> u64 {
        match self.deadline {
            Some(deadline) => deadline.saturating_sub(now_ms),
            None => 0,
        }
    }

    /// Whether the pending save is due at `now_ms`, cancelling it if so.
    pub fn take_due(&mut self, now_ms: u64) -> bool {
        if self.pending() && self.remaining(now_ms) == 0 {
            self.cancel();
            true
        } else {
            false
        }
    }
}

enum Task {
    Save {
        path: PathBuf,
        workspace: Workspace,
        preserve_bad: bool,
        done: Option<Sender<()>>,
    },
}

/// The one background writer. Tasks run in send order.
pub struct Saver {
    tx: Sender<Task>,
}

impl Saver {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<Task>();
        thread::Builder::new()
            .name("gimbal-session-save".to_string())
            .spawn(move || {
                for task in rx {
                    let Task::Save {
                        path,
                        workspace,
                        preserve_bad,
                        done,
                    } = task;
                    if preserve_bad {
                        match session::preserve_bad(&path) {
                            Ok(copy) => {
                                crate::log!("session", "kept the broken file as {}", copy.display())
                            }
                            Err(error) => {
                                crate::log!("session", "could not keep {}: {error}", path.display())
                            }
                        }
                    }
                    match session::save(&path, &workspace) {
                        Ok(()) => {
                            crate::log!("session", "saved {}", path.display());
                        }
                        Err(error) => {
                            crate::log!("session", "could not save {}: {error}", path.display());
                        }
                    }
                    if let Some(done) = done {
                        let _ = done.send(());
                    }
                }
            })
            .expect("the session save thread starts");
        Self { tx }
    }

    pub fn save(&self, path: PathBuf, workspace: Workspace, preserve_bad: bool) {
        let _ = self.tx.send(Task::Save {
            path,
            workspace,
            preserve_bad,
            done: None,
        });
    }

    /// Send the write and wait for it, used only when the app is quitting.
    pub fn save_now(&self, path: PathBuf, workspace: Workspace, preserve_bad: bool) {
        let (done, wait) = mpsc::channel();
        if self
            .tx
            .send(Task::Save {
                path,
                workspace,
                preserve_bad,
                done: Some(done),
            })
            .is_ok()
        {
            let _ = wait.recv_timeout(Duration::from_secs(10));
        }
    }
}

impl Default for Saver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_debounce_restarts_on_every_change_with_a_fake_clock() {
        let mut debounce = Debounce::new(SAVE_DELAY_MS);
        assert!(!debounce.pending());
        assert!(!debounce.take_due(0));

        debounce.change(100);
        assert!(debounce.pending());
        assert_eq!(debounce.remaining(100), 1_000);
        assert!(!debounce.take_due(1_099));

        // A second change restarts the single deadline.
        debounce.change(500);
        assert_eq!(debounce.remaining(900), 600);
        assert!(!debounce.take_due(1_400));
        assert!(debounce.take_due(1_500));
        assert!(!debounce.pending());
        assert!(!debounce.take_due(9_999));

        // A flush cancels it; the caller saves at once.
        debounce.change(2_000);
        debounce.cancel();
        assert!(!debounce.pending());
        assert_eq!(debounce.remaining(2_000), 0);
    }

    #[test]
    fn a_change_at_the_deadline_is_still_a_full_second_away() {
        let mut debounce = Debounce::new(SAVE_DELAY_MS);
        debounce.change(0);
        assert!(debounce.take_due(1_000));
        debounce.change(1_000);
        assert_eq!(debounce.remaining(1_000), 1_000);
        assert!(!debounce.take_due(1_999));
    }
}
