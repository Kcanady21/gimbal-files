// SPDX-License-Identifier: GPL-3.0-or-later
//! Gimbal Files: the GTK4 + libadwaita front end.

mod commands;
mod frame;
mod layout;
mod log;
mod look;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;

thread_local! {
    /// Keeps the one window's state alive for as long as its widgets exist.
    /// GTK callbacks hold weak references, so the process needs one strong one.
    static APP: RefCell<Option<Rc<frame::App>>> = const { RefCell::new(None) };
}

fn main() -> gtk::glib::ExitCode {
    log::start();
    let app = adw::Application::builder()
        .application_id(fm_core::APP_ID)
        .build();
    app.connect_activate(activate);
    app.run()
}

fn activate(application: &adw::Application) {
    let path = fm_core::config::default_config_path().unwrap_or_else(|| {
        crate::log!(
            "config",
            "no config path (HOME is unset); using a relative file and defaults"
        );
        PathBuf::from("gimbal-files-config.toml")
    });
    if path.is_file() {
        crate::log!("config", "reading {}", path.display());
    } else {
        crate::log!(
            "config",
            "{} does not exist; defaults apply",
            path.display()
        );
    }
    let (config, problems) = fm_core::config::ConfigFile::load(&path);
    for problem in problems {
        crate::log!("config", "{problem}");
    }

    let app = frame::App::new(application, config);
    app.window().connect_destroy(|_| {
        APP.with(|slot| {
            slot.borrow_mut().take();
        });
    });
    APP.with(|slot| *slot.borrow_mut() = Some(app.clone()));
    app.present();
    crate::log!("fm_ui", "activated");
}
