// SPDX-License-Identifier: GPL-3.0-or-later
//! The always-on log file.
//!
//! `crate::log!` writes every line to `$XDG_STATE_HOME/gimbal-files/logs/gimbal.log`
//! and, when `GIMBAL_LOG=1`, to stderr too. `crate::log_verbose!` is the
//! high-volume half: it writes only when `GIMBAL_LOG=1`.
//!
//! The primary instance opens the file once, in append mode, and writes each
//! line with one `write_all` so a crash loses at most the line in progress.
//! A failure to open or write falls back to stderr; logging never stops the
//! app. Panics, GLib messages and native signals are routed here as well.
#![allow(unsafe_code)]

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::raw::{c_int, c_void};
use std::panic::PanicHookInfo;
#[cfg(test)]
use std::sync::atomic::AtomicI8;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gtk::glib;

static START: OnceLock<Instant> = OnceLock::new();

/// The one file sink. Tests replace it with a memory sink.
static SINK: Mutex<Option<Box<dyn Sink>>> = Mutex::new(None);

/// The open file's descriptor for the fatal-signal handler. `-1` when no
/// primary log is open.
static LOG_FD: AtomicI32 = AtomicI32::new(-1);

/// Bytes in the current log, so rotation happens without a `metadata` call
/// per line.
static BYTES: AtomicU64 = AtomicU64::new(0);

static PANIC_HOOK: OnceLock<()> = OnceLock::new();
static GLIB_WRITER: OnceLock<()> = OnceLock::new();
static CRASH_NOTICE: Mutex<Option<String>> = Mutex::new(None);

#[cfg(test)]
static VERBOSE_OVERRIDE: AtomicI8 = AtomicI8::new(-1);

trait Sink: Send {
    fn write_line(&mut self, line: &str) -> std::io::Result<()>;
}

struct FileSink(File);

impl Sink for FileSink {
    fn write_line(&mut self, line: &str) -> std::io::Result<()> {
        self.0.write_all(line.as_bytes())
    }
}

pub fn start() {
    START.get_or_init(Instant::now);
}

pub fn elapsed_secs() -> f64 {
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn verbose_enabled() -> bool {
    #[cfg(test)]
    match VERBOSE_OVERRIDE.load(Ordering::Relaxed) {
        1 => return true,
        0 => return false,
        _ => {}
    }
    static VERBOSE: OnceLock<bool> = OnceLock::new();
    *VERBOSE.get_or_init(|| std::env::var("GIMBAL_LOG").as_deref() == Ok("1"))
}

#[cfg(test)]
fn set_verbose_for_test(value: Option<bool>) {
    let state = match value {
        Some(true) => 1,
        Some(false) => 0,
        None => -1,
    };
    VERBOSE_OVERRIDE.store(state, Ordering::Relaxed);
}

fn sink() -> std::sync::MutexGuard<'static, Option<Box<dyn Sink>>> {
    SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn make_line(module: &str, message: std::fmt::Arguments<'_>) -> String {
    format!(
        "{} [gimbal {:.3}] {}: {}\n",
        fm_core::logfile::wall_clock(SystemTime::now()),
        elapsed_secs(),
        module,
        message
    )
}

/// Write one line. `verbose` lines are file-and-stderr only with
/// `GIMBAL_LOG=1`; everyday lines always reach the file and reach stderr only
/// with the variable.
pub fn write_line(module: &str, message: std::fmt::Arguments<'_>, verbose: bool) {
    if verbose && !verbose_enabled() {
        return;
    }
    let line = make_line(module, message);
    let wrote_file = write_to_file(&line);
    if !wrote_file || verbose_enabled() {
        eprint!("{line}");
    }
}

fn write_to_file(line: &str) -> bool {
    let mut guard = sink();
    let Some(sink) = guard.as_mut() else {
        return false;
    };
    let bytes = BYTES.load(Ordering::Relaxed);
    if bytes.saturating_add(line.len() as u64) > fm_core::logfile::LOG_LIMIT {
        // Close, rotate and reopen while the same lock is held, so a line is
        // never written into a file that is being renamed.
        *guard = None;
        LOG_FD.store(-1, Ordering::Relaxed);
        if let Some(log) = fm_core::logfile::default_log_path() {
            let _ = fm_core::logfile::rotate(&log);
            if let Ok(file) = open_log(&log) {
                let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
                BYTES.store(size, Ordering::Relaxed);
                LOG_FD.store(file.as_raw_fd(), Ordering::Relaxed);
                *guard = Some(Box::new(FileSink(file)));
            }
        }
        let Some(sink) = guard.as_mut() else {
            return false;
        };
        return sink.write_line(line).is_ok();
    }
    match sink.write_line(line) {
        Ok(()) => {
            BYTES.fetch_add(line.len() as u64, Ordering::Relaxed);
            true
        }
        Err(_) => false,
    }
}

fn open_log(path: &std::path::Path) -> std::io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    OpenOptions::new().create(true).append(true).open(path)
}

/// Open the primary log, check the previous run's marker, save a crash record
/// if needed, write this run's marker and header line.
pub fn init_primary() {
    start();
    let Some(state) = fm_core::logfile::default_state_dir() else {
        return;
    };
    let log = fm_core::logfile::log_path_in(&state);
    let marker_path = fm_core::logfile::marker_path_in(&state);
    let crashes = fm_core::logfile::crash_dir_in(&state);

    let mut saved_crash = None;
    if let Some(marker) = fm_core::logfile::read_marker(&marker_path)
        && fm_core::logfile::marker_is_stale(
            &marker,
            fm_core::logfile::pid_alive(marker.pid),
            fm_core::logfile::pid_is_gimbal(marker.pid),
        )
        && let Ok(record) = fm_core::logfile::save_crash_record(&log, &crashes, &marker)
    {
        let name = record
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| record.display().to_string());
        saved_crash = Some((name, marker.started.clone()));
        *CRASH_NOTICE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(fm_core::logfile::text::CRASH_NOTICE.to_string());
    }

    let _ = fm_core::logfile::rotate(&log);
    let Ok(file) = open_log(&log) else {
        return;
    };
    let size = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    BYTES.store(size, Ordering::Relaxed);
    LOG_FD.store(file.as_raw_fd(), Ordering::Relaxed);
    *sink() = Some(Box::new(FileSink(file)));
    install_signal_handlers();

    let now = SystemTime::now();
    let started = fm_core::logfile::wall_clock(now);
    let started_unix = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let _ = fm_core::logfile::write_marker(
        &marker_path,
        &fm_core::logfile::RunMarker {
            pid: std::process::id(),
            started: started.clone(),
            started_unix,
            log_start: size,
        },
    );

    crate::log!(
        "run",
        "Gimbal Files {}; install kind {}; GTK {}; libadwaita {}; GLib {}; XDG_CURRENT_DESKTOP={}; display={}; started {}",
        env!("CARGO_PKG_VERSION"),
        fm_core::update::current_install_kind().as_str(),
        format!(
            "{}.{}.{}",
            gtk::major_version(),
            gtk::minor_version(),
            gtk::micro_version()
        ),
        format!(
            "{}.{}.{}",
            adw::major_version(),
            adw::minor_version(),
            adw::micro_version()
        ),
        glib_version(),
        std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_else(|_| "unset".to_string()),
        display_kind(),
        started
    );

    if let Some((name, started)) = saved_crash {
        crate::log!("crash", "saved {name} for the unclean run from {started}");
    }

    #[cfg(debug_assertions)]
    if std::env::var("GIMBAL_TEST_PANIC").as_deref() == Ok("1") {
        panic!("GIMBAL_TEST_PANIC=1");
    }
}

fn glib_version() -> String {
    // `glib_check_version` returns an error message, not the version, so the
    // header once read "GLib version too old (major mismatch)". Read GLib's
    // exported runtime version variables instead.
    // SAFETY: `glib_major_version` and friends are `const guint` data symbols
    // exported by libglib-2.0 (`nm -D`), initialised before `main` and never
    // written; reading them copies three integers.
    let (major, minor, micro) =
        unsafe { (glib_major_version, glib_minor_version, glib_micro_version) };
    format!("{major}.{minor}.{micro}")
}

fn display_kind() -> String {
    use glib::prelude::ObjectExt;
    let Some(display) = gtk::gdk::Display::default() else {
        return if std::env::var_os("WAYLAND_DISPLAY").is_some() {
            "Wayland".to_string()
        } else if std::env::var_os("DISPLAY").is_some() {
            "X11".to_string()
        } else {
            "unknown".to_string()
        };
    };
    let name = display.type_().name();
    if name.contains("Wayland") {
        "Wayland".to_string()
    } else if name.contains("X11") {
        "X11".to_string()
    } else {
        name.to_string()
    }
}

/// The one-time crash line for the first window, if this start found an
/// unclean previous run.
pub fn take_crash_notice() -> Option<String> {
    CRASH_NOTICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// Remove the running marker on a clean exit. Safe to call twice.
pub fn shutdown() {
    if let Some(path) = fm_core::logfile::default_marker_path() {
        fm_core::logfile::remove_marker(&path);
    }
}

pub fn install_panic_hook() {
    if PANIC_HOOK.set(()).is_err() {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info: &PanicHookInfo<'_>| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|message| (*message).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_string());
        let location = info
            .location()
            .map(|location| {
                format!(
                    "{}:{}:{}",
                    location.file(),
                    location.line(),
                    location.column()
                )
            })
            .unwrap_or_else(|| "unknown location".to_string());
        write_line("panic", format_args!("{message} at {location}"), false);
        write_line(
            "panic",
            format_args!(
                "forced backtrace:\n{}",
                std::backtrace::Backtrace::force_capture()
            ),
            false,
        );
        previous(info);
    }));
}

/// GLib's structured log writer. Warnings and criticals always reach the
/// file; `Message`, `Info` and `Debug` only with `GIMBAL_LOG=1`. The default
/// writer still prints to stderr.
pub fn install_glib_writer() {
    if GLIB_WRITER.set(()).is_err() {
        return;
    }
    glib::log_set_writer_func(|level, fields| {
        let field = |name: &str| {
            fields
                .iter()
                .find(|field| field.key() == name)
                .and_then(|field| field.value_str())
                .unwrap_or("")
        };
        let always = matches!(
            level,
            glib::LogLevel::Error | glib::LogLevel::Critical | glib::LogLevel::Warning
        );
        if always || verbose_enabled() {
            let domain = field("GLIB_DOMAIN");
            let message = field("MESSAGE");
            let level = match level {
                glib::LogLevel::Error => "error",
                glib::LogLevel::Critical => "critical",
                glib::LogLevel::Warning => "warning",
                glib::LogLevel::Message => "message",
                glib::LogLevel::Info => "info",
                glib::LogLevel::Debug => "debug",
            };
            let domain = if domain.is_empty() { "GLib" } else { domain };
            let line = make_line("glib", format_args!("{domain} {level}: {message}"));
            let _ = write_to_file(&line);
        }
        glib::log_writer_default(level, fields)
    });
}

#[cfg(test)]
fn install_test_sink(sink: Box<dyn Sink>) {
    BYTES.store(0, Ordering::Relaxed);
    *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sink);
}

#[cfg(test)]
fn clear_test_sink() {
    *SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

#[macro_export]
macro_rules! log {
    ($module:literal, $($arg:tt)*) => {{
        $crate::log::write_line($module, format_args!($($arg)*), false);
    }};
}

#[macro_export]
macro_rules! log_verbose {
    ($module:literal, $($arg:tt)*) => {{
        $crate::log::write_line($module, format_args!($($arg)*), true);
    }};
}

// Async-signal-safe libc entry points. They are declared here rather than
// pulling in a crate: std does not expose them.
unsafe extern "C" {
    static glib_major_version: std::ffi::c_uint;
    static glib_minor_version: std::ffi::c_uint;
    static glib_micro_version: std::ffi::c_uint;
    fn g_unix_signal_add(
        signum: c_int,
        handler: unsafe extern "C" fn(*mut c_void) -> c_int,
        user_data: *mut c_void,
    ) -> std::ffi::c_uint;
    fn write(fd: c_int, buffer: *const c_void, count: usize) -> isize;
    fn signal(signal: c_int, handler: usize) -> usize;
    fn raise(signal: c_int) -> c_int;
}

/// Write one `fatal signal <n>` line with `write(2)` only, then restore the
/// default handler and re-raise so apport still gets its core.
unsafe extern "C" fn fatal_signal(signal_number: c_int) {
    // SAFETY: the handler runs with only async-signal-safe calls. `LOG_FD` is
    // an atomic; `write` reads a stack buffer; `signal(SIG_DFL)` and `raise`
    // are async-signal-safe. No allocation, lock or Rust formatting happens.
    unsafe {
        let fd = LOG_FD.load(Ordering::Relaxed);
        if fd >= 0 {
            let mut buffer = *b"fatal signal 00\n";
            let number = signal_number.clamp(0, 99) as u8;
            buffer[13] = b'0' + number / 10;
            buffer[14] = b'0' + number % 10;
            write(fd, buffer.as_ptr().cast(), buffer.len());
        }
        signal(signal_number, 0);
        raise(signal_number);
    }
}

static QUIT_ON_SIGNAL: OnceLock<fn()> = OnceLock::new();

/// Runs on the main loop (GLib's unix signal source), not in signal context,
/// so it may call ordinary Rust and GTK code.
unsafe extern "C" fn quit_on_signal(_user_data: *mut c_void) -> c_int {
    if let Some(quit) = QUIT_ON_SIGNAL.get() {
        // A panic must not unwind into GLib's C frames.
        let _ = std::panic::catch_unwind(*quit);
    }
    0 // G_SOURCE_REMOVE
}

/// SIGTERM (logout, shutdown, `kill`), SIGINT and SIGHUP quit through `quit`
/// instead of killing the process outright. Without this every logout left the
/// `running` marker behind and the next start reported a crash.
pub fn install_quit_signals(quit: fn()) {
    if QUIT_ON_SIGNAL.set(quit).is_err() {
        return;
    }
    // SAFETY: `g_unix_signal_add` (GLib >= 2.30) registers a main-loop source
    // for these supported signals; `quit_on_signal` has the `GSourceFunc`
    // shape and static lifetime, and the user data is unused.
    unsafe {
        for signal_number in [15, 2, 1] {
            g_unix_signal_add(signal_number, quit_on_signal, std::ptr::null_mut());
        }
    }
}

fn install_signal_handlers() {
    // SAFETY: `signal(2)` only stores a handler pointer for the three fatal
    // signals. `fatal_signal` has the `extern "C" fn(c_int)` shape the C API
    // expects, and it is a plain function pointer with static lifetime.
    unsafe {
        signal(11, fatal_signal as *const () as usize);
        signal(6, fatal_signal as *const () as usize);
        signal(7, fatal_signal as *const () as usize);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    struct MemorySink(Arc<Mutex<String>>);

    impl Sink for MemorySink {
        fn write_line(&mut self, line: &str) -> std::io::Result<()> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push_str(line);
            Ok(())
        }
    }

    #[test]
    fn everyday_lines_reach_the_file_and_verbose_lines_need_the_variable() {
        let _guard = TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let buffer = Arc::new(Mutex::new(String::new()));
        install_test_sink(Box::new(MemorySink(buffer.clone())));
        set_verbose_for_test(None);

        write_line("test", format_args!("everyday line"), false);
        write_line("test", format_args!("verbose line"), true);
        let text = buffer.lock().unwrap().clone();
        assert!(text.contains("everyday line"), "{text}");
        assert!(!text.contains("verbose line"), "{text}");

        set_verbose_for_test(Some(true));
        write_line("test", format_args!("verbose line"), true);
        let text = buffer.lock().unwrap().clone();
        assert!(text.contains("verbose line"), "{text}");

        set_verbose_for_test(None);
        clear_test_sink();
    }

    #[test]
    fn the_line_format_has_the_wall_clock_and_run_time_prefixes() {
        let line = make_line("module", format_args!("message"));
        assert!(line.contains(" [gimbal "), "{line}");
        assert!(line.contains("] module: message\n"), "{line}");
        let wall = &line[..line.find(" [gimbal ").expect("prefix")];
        assert_eq!(wall.len(), 23, "{line}");
        assert_eq!(&wall[4..5], "-", "{line}");
        assert_eq!(&wall[10..11], "T", "{line}");
        assert_eq!(&wall[19..20], ".", "{line}");
    }
}
