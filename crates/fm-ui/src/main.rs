// SPDX-License-Identifier: GPL-3.0-or-later
use std::time::Instant;

use adw::prelude::*;

/// Print one event line to stderr when `GIMBAL_LOG=1` is set, in the format
/// required by `CLAUDE.md`: `[gimbal <secs.millis since start>] <module>: <message>`.
fn log(start: Instant, message: &str) {
    if std::env::var("GIMBAL_LOG").is_ok_and(|value| value == "1") {
        let elapsed = start.elapsed();
        eprintln!(
            "[gimbal {}.{:03}] fm_ui: {message}",
            elapsed.as_secs(),
            elapsed.subsec_millis()
        );
    }
}

fn main() -> gtk::glib::ExitCode {
    let start = Instant::now();
    let app = adw::Application::builder()
        .application_id(fm_core::APP_ID)
        .build();

    app.connect_activate(move |app| {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Gimbal Files")
            .build();

        window.present();
        log(start, "activated");
    });

    app.run()
}
