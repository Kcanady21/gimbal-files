// SPDX-License-Identifier: GPL-3.0-or-later
//! The same one-line stderr log as the app, for the transport worker.
//!
//! It never prints a raw frame, a preview buffer or file contents: only
//! listener, generation, client, request, outcome and stop events.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub(crate) fn event(module: &str, message: impl AsRef<str>) {
    if std::env::var_os("GIMBAL_LOG").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return;
    }
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed().as_millis();
    eprintln!(
        "[gimbal {}.{:03}] {module}: {}",
        elapsed / 1000,
        elapsed % 1000,
        message.as_ref()
    );
}
