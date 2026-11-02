// SPDX-License-Identifier: GPL-3.0-or-later
//! The one log line format from `CLAUDE.md`: when `GIMBAL_LOG=1`, print
//! `[gimbal <secs.millis since start>] <module>: <message>` to stderr.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();

pub fn start() {
    START.get_or_init(Instant::now);
}

pub fn enabled() -> bool {
    std::env::var("GIMBAL_LOG").as_deref() == Ok("1")
}

pub fn elapsed_secs() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[macro_export]
macro_rules! log {
    ($module:literal, $($arg:tt)*) => {{
        if $crate::log::enabled() {
            eprintln!(
                "[gimbal {:.3}] {}: {}",
                $crate::log::elapsed_secs(),
                $module,
                format_args!($($arg)*)
            );
        }
    }};
}
