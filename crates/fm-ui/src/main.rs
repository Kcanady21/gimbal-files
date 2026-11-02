// SPDX-License-Identifier: GPL-3.0-or-later
//! Gimbal Files: the GTK4 + libadwaita front end.
//!
//! One process holds the config, workspace and session. Each model window has
//! one GTK window and header. A second launch hands its folders to the running
//! instance through GApplication's open signal.

mod app;
mod commands;
mod content;
mod events;
mod folder;
mod frame;
mod header;
mod layout;
mod log;
mod look;
mod open;
mod platform;
mod save;
mod sidebar;
mod split;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;

use crate::app::Process;

thread_local! {
    /// The process state, kept alive for as long as the app runs.
    static PROCESS: RefCell<Option<Rc<Process>>> = const { RefCell::new(None) };
}

fn main() -> gtk::glib::ExitCode {
    log::start();
    let app = adw::Application::builder()
        .application_id(fm_core::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_activate(activate);
    app.connect_open(open);
    app.run()
}

/// The first activation (or open) builds the process and restores the session.
/// `true` means this call created it.
fn ensure_process(application: &adw::Application) -> (Rc<Process>, bool) {
    PROCESS.with(|slot| {
        let existing = slot.borrow().clone();
        if let Some(process) = existing {
            return (process, false);
        }
        let process = Process::start(application);
        *slot.borrow_mut() = Some(process.clone());
        (process, true)
    })
}

fn activate(application: &adw::Application) {
    let (process, created) = ensure_process(application);
    if created {
        crate::log!("fm_ui", "activated; the session is restored");
    } else {
        crate::log!("fm_ui", "activated again; opening a window at home");
        process.open_request(&[]);
    }
}

fn open(application: &adw::Application, files: &[gio::File], hint: &str) {
    let (process, created) = ensure_process(application);
    let paths: Vec<PathBuf> = files.iter().filter_map(folder_for).collect();
    crate::log!(
        "fm_ui",
        "open ({hint}) with {} folder argument(s): {paths:?}",
        paths.len()
    );
    if created {
        crate::log!(
            "fm_ui",
            "first open; the session windows are restored first"
        );
    }
    if !paths.is_empty() {
        process.open_request(&paths);
    }
}

/// A folder argument opens itself; a file argument opens its parent folder.
fn folder_for(file: &gio::File) -> Option<PathBuf> {
    let path = file.path()?;
    if path.is_dir() {
        Some(path)
    } else {
        Some(
            path.parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or(path),
        )
    }
}
