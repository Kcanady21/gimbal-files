// SPDX-License-Identifier: GPL-3.0-or-later
//! Gimbal Files: the GTK4 + libadwaita front end.
//!
//! One process holds the config, workspace and session. Each model window has
//! one GTK window and header. A second launch hands its folders to the running
//! instance through GApplication's open signal.

mod agent;
mod app;
mod cli;
mod commands;
mod content;
mod dnd;
mod events;
mod filemanager1;
mod fileops;
mod folder;
mod frame;
mod header;
mod image;
mod layout;
mod log;
mod look;
mod open;
mod pdf;
mod platform;
mod popup;
mod preview;
mod save;
mod search;
mod sidebar;
mod split;
mod thumbs;
mod trash;
mod volumes;

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
    log::install_panic_hook();
    log::install_glib_writer();
    let service = filemanager1::is_service_launch(&std::env::args_os().collect::<Vec<_>>());
    let app = adw::Application::builder()
        .application_id(fm_core::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(move |application| startup(application, service));
    app.connect_activate(activate);
    app.connect_open(open);
    app.connect_shutdown(|_| log::shutdown());

    if service {
        // GApplication makes "cannot own the app id" fatal in service mode.
        // Register without IS_SERVICE first: a remote result means another
        // Gimbal is already running, and this process forwards to it instead
        // of printing "Failed to register".
        match app.register(None::<&gio::Cancellable>) {
            Ok(()) if app.is_remote() => return filemanager1::forward(app),
            Ok(()) => return app.run(),
            Err(error) => {
                crate::log!("fm_ui", "Failed to register: {error}");
                return gtk::glib::ExitCode::FAILURE;
            }
        }
    }

    app.run()
}

/// A `--gapplication-service` launch never activates, so the FileManager1
/// object and name are handled here, before the first call can arrive.
/// `service` comes from the raw arguments because a primary service registers
/// before `run` parses the flag.
fn startup(application: &adw::Application, service: bool) {
    log::install_quit_signals(quit_on_signal);
    // `startup` only runs on the primary instance. A service launch is the
    // bus's activation file starting us; an ordinary launch reaches activate.
    log::init_primary();
    filemanager1::start(application, service);
}

/// SIGTERM, SIGINT or SIGHUP: leave like a normal quit (session saved,
/// workers stopped, crash marker removed), without asking about unsaved
/// preview edits, since the session is ending.
fn quit_on_signal() {
    crate::log!("quit", "signal received; quitting cleanly");
    if let Some(process) = process() {
        process.quit_now();
    } else {
        log::shutdown();
        if let Some(application) = gio::Application::default() {
            application.quit();
        }
    }
}

/// The process, once startup has built it. The FileManager1 object uses this
/// from `startup`/idle callbacks that run before or after `activate`.
pub(crate) fn process() -> Option<Rc<Process>> {
    PROCESS.with(|slot| slot.borrow().clone())
}

/// The first activation (or open) builds the process and restores the session.
/// `true` means this call created it.
pub(crate) fn ensure_process(application: &adw::Application) -> (Rc<Process>, bool) {
    // GTK has initialized by the time activate/open runs. The installed
    // hicolor icon has the same name as the app id, which is also the Wayland
    // app id and the desktop entry's StartupWMClass.
    gtk::Window::set_default_icon_name(fm_core::APP_ID);
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
        process.clear_fresh_home();
    } else {
        crate::log!("fm_ui", "activated again; opening a window at home");
        process.open_request(&[]);
    }
}

fn open(application: &adw::Application, files: &[gio::File], hint: &str) {
    let (process, created) = ensure_process(application);
    let paths: Vec<PathBuf> = files.iter().filter_map(|file| file.path()).collect();
    crate::log!(
        "fm_ui",
        "open ({hint}) with {} argument(s): {paths:?}",
        paths.len()
    );
    if created {
        crate::log!(
            "fm_ui",
            "first open; the session windows are restored first"
        );
    }
    if !paths.is_empty() {
        // A forwarded FileManager1 call carries its xdg-activation token in
        // the open hint. GApplication's own launches pass "".
        if hint.is_empty() {
            process.open_arguments(&paths, created);
        } else {
            process.open_arguments_with_startup(&paths, created, Some(hint));
        }
    } else {
        process.clear_fresh_home();
    }
}
