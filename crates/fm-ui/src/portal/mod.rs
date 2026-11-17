// SPDX-License-Identifier: GPL-3.0-or-later
//! The file-chooser picker (`gimbal-files --portal`).
//!
//! The picker is a second, short-lived process with its own application id. It
//! never registers as, forwards to, or is forwarded to the main app: the main
//! app's `gimbal-files-cli` calls can never reach a dialog, and a dialog can
//! never touch the main session. It owns its own application hold per open
//! dialog and leaves 60 s after the last one closes
//! (`ApplicationBuilder::inactivity_timeout`), so the process does not linger
//! between dialogs.
//!
//! Only F1 exists: a basic single-file Open dialog. SaveFile and SaveFiles are
//! answered "not available" until F3.

pub mod picker;
pub mod service;
#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

use crate::app::Process;

/// The picker's own application id. It must differ from the main app's so the
/// two never share a GApplication instance or session.
pub const APP_ID: &str = "io.github.kcanady21.GimbalFiles.Portal";

/// How long the process stays after its last dialog closes. GApplication's
/// `inactivity-timeout` is in **milliseconds** (it feeds a `g_timeout_add`),
/// so 60 s is 60_000 here, not 60.
pub const INACTIVITY_MS: u32 = 60_000;
/// The same value for the log, where seconds read better.
pub const INACTIVITY_SECS: u32 = INACTIVITY_MS / 1000;

thread_local! {
    /// Everything the process must keep alive: the one `Process` and the
    /// exported service.
    static PORTAL: RefCell<Option<(Rc<Process>, Rc<service::Service>)>> =
        const { RefCell::new(None) };
}

/// Run the picker until its inactivity timeout. Called from `main` before the
/// normal application is built.
pub fn run() -> glib::ExitCode {
    crate::log::start();
    crate::log::install_panic_hook();
    crate::log::install_glib_writer();
    crate::log::init_portal();

    let application = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::IS_SERVICE)
        .inactivity_timeout(INACTIVITY_MS)
        .build();
    application.connect_startup(startup);
    application.connect_shutdown(|application| {
        // Never `log::shutdown()` here: that removes the main app's running
        // marker, which the picker did not write.
        crate::log!(
            "portal",
            "shutdown; inactivity timeout {} s",
            application.inactivity_timeout() / 1000
        );
    });
    application.connect_activate(|_| {
        // A plain `--portal` launch has no dialog to show. The process still
        // leaves on its inactivity timeout.
        crate::log!(
            "portal",
            "activated with no dialog; waiting {} s for a request",
            INACTIVITY_SECS
        );
    });
    application.run_with_args(&["gimbal-files"])
}

fn startup(application: &adw::Application) {
    crate::log::install_quit_signals(quit_from_signal);
    gtk::Window::set_default_icon_name(fm_core::APP_ID);
    crate::log!(
        "portal",
        "picker start: {}; pid {}; GTK {}.{}.{}; app id {}; inactivity {} s",
        env!("CARGO_PKG_VERSION"),
        std::process::id(),
        gtk::major_version(),
        gtk::minor_version(),
        gtk::micro_version(),
        APP_ID,
        application.inactivity_timeout() / 1000
    );

    let process = Process::portal(application);
    let service = service::Service::new();
    service.set_host(picker::PickerHost::new(&process, application));

    let Some(connection) = application.dbus_connection() else {
        // Without a session bus there is nothing to answer; a window opened by
        // hand is still inspectable.
        crate::log!(
            "portal",
            "no session-bus connection; the FileChooser backend is not exported"
        );
        PORTAL.with(|slot| *slot.borrow_mut() = Some((process, service)));
        return;
    };
    if service.install(&connection) {
        service.own_name(&connection);
    }
    PORTAL.with(|slot| *slot.borrow_mut() = Some((process, service)));
}

/// SIGTERM (logout, shutdown, `kill`), SIGINT and SIGHUP leave without asking:
/// a dialog has nothing to save.
fn quit_from_signal() {
    crate::log!("portal", "signal received; quitting");
    if let Some(application) = gio::Application::default() {
        application.quit();
    }
}
