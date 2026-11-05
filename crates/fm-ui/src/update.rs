// SPDX-License-Identifier: GPL-3.0-or-later
//! The UI side of the notice-only update check.
//!
//! One background thread per check performs the blocking ureq request. Its
//! result comes back over a channel and is drained on the GTK main loop, the
//! same worker pattern the file-operation and folder-size code uses. Nothing
//! here installs a release: the summary and the Settings row only tell the
//! user how their install updates.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use fm_core::command::{CheckedCommand, Invocation, Outcome, Value};
use fm_core::config;
use fm_core::update::{self, CHECK_INTERVAL_SECS, FailedReason, InstallKind, Version, text};
use fm_core::workspace::WindowId;
use gtk::glib;

use crate::app::Process;

const POLL: Duration = Duration::from_millis(100);

/// The process-wide update state. It is separate from the session file.
pub struct UpdateRuntime {
    state_path: Option<PathBuf>,
    state: update::UpdateState,
    kind: InstallKind,
    result: Option<UpdateResult>,
    checking: bool,
    receiver: Option<Receiver<UpdateResult>>,
    poll: Option<glib::SourceId>,
    timer: Option<glib::SourceId>,
}

/// A snapshot for the open Settings rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateStatus {
    pub kind: InstallKind,
    pub check_enabled: bool,
    pub checking: bool,
    pub result: Option<UpdateResult>,
}

pub use fm_core::update::UpdateResult;

impl UpdateRuntime {
    pub fn idle() -> Self {
        Self {
            state_path: None,
            state: update::UpdateState::default(),
            kind: InstallKind::Development,
            result: None,
            checking: false,
            receiver: None,
            poll: None,
            timer: None,
        }
    }
}

/// Start the schedule for a real process: load `update.toml`, check once at
/// startup when due, then every 24 hours while the app runs.
pub fn start(process: &Rc<Process>) {
    let state_path = update::default_state_path();
    let state = state_path
        .as_deref()
        .map(update::load_state)
        .unwrap_or_default();
    let kind = update::current_install_kind();
    {
        let mut runtime = process.update.borrow_mut();
        *runtime = Some(UpdateRuntime {
            state_path,
            state,
            kind,
            ..UpdateRuntime::idle()
        });
    }

    maybe_check(process);
    schedule(process);
}

/// Arm the next one-shot check. Re-arming from the last check rather than from
/// process start means a launch 23 hours after the last check waits one hour,
/// not a full day. While a check runs or checks are off, the next look is a
/// day away.
fn schedule(process: &Rc<Process>) {
    let delay = {
        let mut runtime = process.update.borrow_mut();
        let Some(runtime) = runtime.as_mut() else {
            return;
        };
        if let Some(timer) = runtime.timer.take() {
            timer.remove();
        }
        if runtime.checking {
            Duration::from_secs(CHECK_INTERVAL_SECS)
        } else {
            match update::seconds_until_check(update::now_seconds(), runtime.state.last_check) {
                0 => Duration::from_secs(CHECK_INTERVAL_SECS),
                seconds => Duration::from_secs(seconds),
            }
        }
    };

    let weak = Rc::downgrade(process);
    let timer = glib::timeout_add_local_once(delay, move || {
        let Some(process) = weak.upgrade() else {
            return;
        };
        // This one-shot has fired; do not try to remove it from its own
        // callback, then arm the next one.
        if let Some(runtime) = process.update.borrow_mut().as_mut() {
            runtime.timer = None;
        }
        maybe_check(&process);
        schedule(&process);
    });
    if let Some(runtime) = process.update.borrow_mut().as_mut() {
        runtime.timer = Some(timer);
    }
}

/// The state the Settings rows show.
pub fn status(process: &Process) -> UpdateStatus {
    let (kind, checking, result) = {
        let runtime = process.update.borrow();
        match runtime.as_ref() {
            Some(runtime) => (runtime.kind, runtime.checking, runtime.result),
            None => (InstallKind::Development, false, None),
        }
    };
    UpdateStatus {
        kind,
        check_enabled: process.config.borrow().config().updates.check,
        checking,
        result,
    }
}

/// Push the row state into every open Settings popover.
pub fn refresh_all(process: &Rc<Process>) {
    let status = status(process);
    let windows: Vec<Rc<crate::frame::WindowUi>> =
        process.windows.borrow().values().cloned().collect();
    for ui in windows {
        ui.header().refresh_update_status(&status);
    }
}

/// Start a check now, ignoring the 24-hour limit. Startup and the timer use
/// [`maybe_check`], which respects the schedule.
pub fn start_check(process: &Rc<Process>) -> bool {
    let (enabled, kind, checking) = {
        let runtime = process.update.borrow();
        match runtime.as_ref() {
            Some(runtime) => (
                process.config.borrow().config().updates.check,
                runtime.kind,
                runtime.checking,
            ),
            None => (false, InstallKind::Development, false),
        }
    };
    if !enabled || kind == InstallKind::Development || checking {
        return false;
    }

    let base = update::default_base_url();
    let key = update::release_key();
    let running = Version::running();
    crate::log!(
        "update",
        "check started (Gimbal Files {running}, {} install)",
        kind.as_str()
    );

    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let fetcher = update::HttpFetcher::new();
        let result = update::check(&fetcher, &base, key.as_ref(), running);
        let _ = sender.send(result);
    });

    {
        let mut runtime = process.update.borrow_mut();
        let Some(runtime) = runtime.as_mut() else {
            return false;
        };
        runtime.checking = true;
        runtime.receiver = Some(receiver);
    }
    ensure_poll(process);
    refresh_all(process);
    true
}

/// Check at startup and on the daily timer only when the user left checks on,
/// this is not a development build, and `update.toml` says the day is up.
fn maybe_check(process: &Rc<Process>) {
    let (enabled, kind, last_check) = {
        let runtime = process.update.borrow();
        let Some(runtime) = runtime.as_ref() else {
            return;
        };
        (
            process.config.borrow().config().updates.check,
            runtime.kind,
            runtime.state.last_check,
        )
    };
    if !enabled || kind == InstallKind::Development {
        return;
    }
    if update::should_check(update::now_seconds(), last_check) {
        start_check(process);
    }
}

fn ensure_poll(process: &Rc<Process>) {
    let mut runtime = process.update.borrow_mut();
    let Some(runtime) = runtime.as_mut() else {
        return;
    };
    if runtime.poll.is_some() {
        return;
    }
    let weak = Rc::downgrade(process);
    let source = glib::timeout_add_local(POLL, move || {
        let Some(process) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        poll(&process)
    });
    runtime.poll = Some(source);
}

fn poll(process: &Rc<Process>) -> glib::ControlFlow {
    let message = {
        let runtime = process.update.borrow();
        let Some(runtime) = runtime.as_ref() else {
            return glib::ControlFlow::Break;
        };
        let Some(receiver) = runtime.receiver.as_ref() else {
            return glib::ControlFlow::Break;
        };
        match receiver.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => UpdateResult::Failed(FailedReason::Offline),
        }
    };

    finish(process, message);
    if let Some(runtime) = process.update.borrow_mut().as_mut() {
        runtime.poll = None;
    }
    glib::ControlFlow::Break
}

fn finish(process: &Rc<Process>, result: UpdateResult) {
    let now = update::now_seconds();
    let (kind, enabled, notified) = {
        let runtime = process.update.borrow();
        match runtime.as_ref() {
            Some(runtime) => (
                runtime.kind,
                process.config.borrow().config().updates.check,
                runtime.state.notified,
            ),
            None => (InstallKind::Development, false, None),
        }
    };

    let notify = match result {
        UpdateResult::Available(version)
            if enabled && update::should_announce(notified, version) =>
        {
            Some(version)
        }
        _ => None,
    };
    let announced = match notify {
        Some(version) if show_summary(process, kind, version) => Some(version),
        _ => None,
    };

    let (state_path, state) = {
        let mut runtime = process.update.borrow_mut();
        let Some(runtime) = runtime.as_mut() else {
            return;
        };
        runtime.result = Some(result);
        runtime.checking = false;
        runtime.receiver = None;
        runtime.state.last_check = Some(now);
        if let Some(version) = announced {
            runtime.state.notified = Some(version);
        }
        (runtime.state_path.clone(), runtime.state.clone())
    };

    if let Some(path) = state_path
        && let Err(error) = update::save_state(&path, &state)
    {
        crate::log!(
            "update",
            "could not save update state to {}: {error}",
            path.display()
        );
    }
    crate::log!("update", "check finished: {result}");
    refresh_all(process);
    schedule(process);
}

fn show_summary(process: &Rc<Process>, kind: InstallKind, version: Version) -> bool {
    let Some(window) = process.focused_window_id() else {
        return false;
    };
    let Some(ui) = process.window(window) else {
        return false;
    };
    if ui.trash_active() || ui.search_active() {
        return false;
    }
    let text = text::summary(kind, version);
    ui.show_summary(&text, None);
    true
}

/// The two UI-only commands.
pub fn command(process: &Rc<Process>, _window: WindowId, command: &CheckedCommand) -> Outcome {
    match command.name() {
        "updates.set-enabled" => {
            let Some(Value::Bool(enabled)) = command.get("enabled") else {
                return Outcome::Failed(text::SET_ENABLED_NEEDS_BOOL.to_string());
            };
            let current = process.home();
            let change = {
                let config = process.config.borrow();
                config::change_for(command, config.config(), &current)
            };
            let Some(change) = change else {
                return Outcome::Failed(text::NO_CONFIG_CHANGE.to_string());
            };
            match process.config.borrow_mut().apply(change) {
                Ok(()) => {
                    crate::log!(
                        "command",
                        "updates.set-enabled saved as {}",
                        if *enabled { "on" } else { "off" }
                    );
                    refresh_all(process);
                    if *enabled {
                        maybe_check(process);
                    }
                    schedule(process);
                    Outcome::Done
                }
                Err(error) => {
                    crate::log!("config", "{error}");
                    Outcome::Failed(error.to_string())
                }
            }
        }
        "updates.check" => {
            let status = status(process);
            if status.kind == InstallKind::Development {
                return Outcome::Failed(text::DEVELOPMENT.to_string());
            }
            if !status.check_enabled {
                return Outcome::Failed(text::CHECKS_OFF.to_string());
            }
            if status.checking {
                return Outcome::Failed(text::ALREADY_CHECKING.to_string());
            }
            if start_check(process) {
                Outcome::Done
            } else {
                Outcome::Failed(text::ALREADY_CHECKING.to_string())
            }
        }
        _ => Outcome::Failed(text::NO_CONFIG_CHANGE.to_string()),
    }
}

impl UpdateStatus {
    pub fn status_text(&self) -> String {
        if self.kind == InstallKind::Development {
            return text::DEVELOPMENT.to_string();
        }
        if !self.check_enabled {
            return text::CHECKS_OFF.to_string();
        }
        if self.checking {
            return text::CHECKING.to_string();
        }
        match self.result {
            Some(UpdateResult::Available(version)) => text::available(self.kind, version),
            Some(UpdateResult::Failed(reason)) => reason.message(),
            Some(UpdateResult::UpToDate) | None => text::UP_TO_DATE.to_string(),
        }
    }

    pub fn releases_visible(&self) -> bool {
        self.kind == InstallKind::Tarball
            && self.check_enabled
            && !self.checking
            && matches!(self.result, Some(UpdateResult::Available(_)))
    }
}

/// The Settings check-now button dispatch. Kept here so the button and the
/// command path use the same invocation.
pub fn check_now_invocation() -> Invocation {
    Invocation::new("updates.check", Vec::new())
}

/// The Settings switch dispatch.
pub fn set_enabled_invocation(enabled: bool) -> Invocation {
    Invocation::new(
        "updates.set-enabled",
        vec![("enabled".to_string(), enabled.to_string())],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(kind: InstallKind, result: Option<UpdateResult>) -> UpdateStatus {
        UpdateStatus {
            kind,
            check_enabled: true,
            checking: false,
            result,
        }
    }

    #[test]
    fn the_settings_line_uses_the_install_kind_and_the_one_string_table() {
        let version = Version::parse("0.1.1").unwrap();
        assert_eq!(
            status(InstallKind::System, Some(UpdateResult::Available(version))).status_text(),
            "0.1.1 is available. Update with your system updater (sudo apt upgrade)."
        );
        assert_eq!(
            status(InstallKind::Tarball, Some(UpdateResult::Available(version))).status_text(),
            "0.1.1 is available. Download it from the releases page."
        );
        assert_eq!(
            status(InstallKind::System, Some(UpdateResult::UpToDate)).status_text(),
            "Up to date"
        );
        assert_eq!(
            status(
                InstallKind::System,
                Some(UpdateResult::Failed(FailedReason::Http(404)))
            )
            .status_text(),
            "The update server returned HTTP 404."
        );
        assert_eq!(
            status(InstallKind::Development, None).status_text(),
            "Development build: update checks are off"
        );
    }

    #[test]
    fn the_releases_button_belongs_to_a_tarball_that_has_a_new_version() {
        let version = Version::parse("0.1.1").unwrap();
        assert!(
            status(InstallKind::Tarball, Some(UpdateResult::Available(version))).releases_visible()
        );
        assert!(
            !status(InstallKind::System, Some(UpdateResult::Available(version))).releases_visible()
        );
        assert!(!status(InstallKind::Tarball, Some(UpdateResult::UpToDate)).releases_visible());
    }

    #[test]
    fn a_summary_names_the_new_version_and_its_install_kind_how_to() {
        let version = Version::parse("0.1.1").unwrap();
        assert_eq!(
            text::summary(InstallKind::System, version),
            "Gimbal Files 0.1.1 is available. Update with your system updater (sudo apt upgrade)."
        );
        assert_eq!(
            text::summary(InstallKind::Tarball, version),
            "Gimbal Files 0.1.1 is available. Download it from the releases page."
        );
    }
}
