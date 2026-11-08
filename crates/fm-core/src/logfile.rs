// SPDX-License-Identifier: GPL-3.0-or-later
//! The always-on log file, the crash marker and the bug-report file.
//!
//! Paths, rotation, the marker decision and crash-record retention are pure
//! functions over names, sizes and times, so the UI and the CLI share them
//! and the tests need no running app. The report builder is pure too: the CLI
//! reads the files, this module redacts and lays them out.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use toml_edit::{DocumentMut, Item, Value};

use crate::update::InstallKind;

pub const STATE_DIR: &str = "gimbal-files";
pub const LOG_DIR: &str = "logs";
pub const LOG_FILE: &str = "gimbal.log";
pub const MARKER_FILE: &str = "running";
pub const CRASH_DIR: &str = "crashes";

/// Rotate when the log reaches this size. Three files then hold at most
/// 15 MiB.
pub const LOG_LIMIT: u64 = 5 * 1024 * 1024;

/// How many lines a crash record keeps from each log file it copies.
pub const CRASH_LINES: usize = 2_000;

/// How many crash records `crashes/` keeps.
pub const KEEP_CRASHES: usize = 10;

/// The default report length, and the upper bound a caller may ask for.
pub const DEFAULT_REPORT_LINES: usize = 500;
pub const REPORT_LIMIT: usize = 20_000;

/// The command the Settings button copies and the README names.
pub const REPORT_COMMAND: &str = "gimbal-files-cli bug-report";

/// Where a bug is filed. `CARGO_PKG_REPOSITORY` is the workspace repository.
pub const ISSUE_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/issues/new/choose");

/// Every user-facing string this feature uses.
pub mod text {
    pub const REPORT_A_BUG: &str = "Report a bug";
    pub const OPEN_LOG_FOLDER: &str = "Open log folder";
    pub const COPY_REPORT_COMMAND: &str = "Copy report command";
    pub const CRASH_NOTICE: &str =
        "Gimbal closed unexpectedly last time. A crash record was saved; Settings → Report a bug.";
    pub const READ_BEFORE_ATTACHING: &str = "Read the file before attaching it to a new issue.";

    pub fn apport_attach(name: &str) -> String {
        format!("To attach it, run: ubuntu-bug /var/crash/{name}")
    }

    pub fn crash_saved(name: &str) -> String {
        format!("The crash record is crashes/{name}.")
    }
}

/// `$XDG_STATE_HOME/gimbal-files`, or `~/.local/state/gimbal-files`.
pub fn state_dir(xdg_state_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match xdg_state_home {
        Some(directory) if directory.is_absolute() => Some(directory.join(STATE_DIR)),
        _ => home.map(|home| home.join(".local").join("state").join(STATE_DIR)),
    }
}

pub fn default_state_dir() -> Option<PathBuf> {
    let xdg = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    state_dir(xdg.as_deref(), home.as_deref())
}

pub fn log_dir_in(state: &Path) -> PathBuf {
    state.join(LOG_DIR)
}

pub fn log_path_in(state: &Path) -> PathBuf {
    log_dir_in(state).join(LOG_FILE)
}

pub fn marker_path_in(state: &Path) -> PathBuf {
    state.join(MARKER_FILE)
}

pub fn crash_dir_in(state: &Path) -> PathBuf {
    state.join(CRASH_DIR)
}

pub fn default_log_dir() -> Option<PathBuf> {
    default_state_dir().map(|state| log_dir_in(&state))
}

pub fn default_log_path() -> Option<PathBuf> {
    default_state_dir().map(|state| log_path_in(&state))
}

pub fn default_marker_path() -> Option<PathBuf> {
    default_state_dir().map(|state| marker_path_in(&state))
}

pub fn default_crash_dir() -> Option<PathBuf> {
    default_state_dir().map(|state| crash_dir_in(&state))
}

/// `gimbal.log.1` and `gimbal.log.2`, built by appending rather than
/// replacing an extension.
pub fn archive_path(log: &Path, number: u8) -> PathBuf {
    let mut name = log.as_os_str().to_owned();
    name.push(format!(".{number}"));
    PathBuf::from(name)
}

/// The default report file: `~/gimbal-files-bug-report-<stamp>.txt`.
pub fn report_path(home: Option<&Path>, now: SystemTime) -> Option<PathBuf> {
    home.map(|home| home.join(format!("gimbal-files-bug-report-{}.txt", file_stamp(now))))
}

/// A filename-safe local timestamp: `YYYY-MM-DD-HHMMSS`.
pub fn file_stamp(now: SystemTime) -> String {
    let seconds = now
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0);
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|date| date.format("%Y-%m-%d-%H%M%S").ok())
        .map(|text| text.to_string())
        .unwrap_or_else(|| seconds.to_string())
}

/// The wall-clock prefix for a log line, with milliseconds:
/// `2026-11-08T09:16:17.123`.
pub fn wall_clock(now: SystemTime) -> String {
    let duration = now.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let millis = duration.subsec_millis();
    let stamp = glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|date| date.format("%Y-%m-%dT%H:%M:%S").ok())
        .map(|text| text.to_string())
        .unwrap_or_else(|| seconds.to_string());
    format!("{stamp}.{millis:03}")
}

/// What a rotation would do. The caller performs it so tests can assert the
/// plan without a filesystem.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rotation {
    pub remove_two: bool,
    pub one_to_two: bool,
    pub log_to_one: bool,
}

impl Rotation {
    pub fn needed(self) -> bool {
        self.remove_two || self.one_to_two || self.log_to_one
    }
}

/// Rotate at [`LOG_LIMIT`], keeping `gimbal.log`, `.1` and `.2` at most.
pub fn plan_rotation(log_size: u64, has_one: bool, has_two: bool) -> Rotation {
    if log_size < LOG_LIMIT {
        return Rotation::default();
    }
    Rotation {
        remove_two: has_two,
        one_to_two: has_one,
        log_to_one: true,
    }
}

/// Perform the plan above. A missing log is not an error: there is nothing to
/// rotate.
pub fn rotate(log: &Path) -> io::Result<()> {
    let size = fs::metadata(log).map(|meta| meta.len()).unwrap_or(0);
    let one = archive_path(log, 1);
    let two = archive_path(log, 2);
    let plan = plan_rotation(size, one.exists(), two.exists());
    if !plan.needed() {
        return Ok(());
    }
    if plan.remove_two {
        match fs::remove_file(&two) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    if plan.one_to_two {
        fs::rename(&one, &two)?;
    }
    if plan.log_to_one {
        fs::rename(log, &one)?;
    }
    Ok(())
}

/// The marker a primary instance writes at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMarker {
    pub pid: u32,
    pub started: String,
    pub started_unix: u64,
    pub log_start: u64,
}

pub fn format_marker(marker: &RunMarker) -> String {
    format!(
        "pid = {}\nstarted = \"{}\"\nstarted_unix = {}\nlog_start = {}\n",
        marker.pid, marker.started, marker.started_unix, marker.log_start
    )
}

pub fn parse_marker(text: &str) -> Option<RunMarker> {
    let mut pid = None;
    let mut started = None;
    let mut started_unix = None;
    let mut log_start = 0;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line.split_once('=')?;
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "pid" => pid = value.parse().ok(),
            "started" => started = Some(value.to_string()),
            "started_unix" => started_unix = value.parse().ok(),
            "log_start" => log_start = value.parse().unwrap_or(0),
            _ => return None,
        }
    }
    Some(RunMarker {
        pid: pid?,
        started: started?,
        started_unix: started_unix?,
        log_start,
    })
}

pub fn read_marker(path: &Path) -> Option<RunMarker> {
    parse_marker(&fs::read_to_string(path).ok()?)
}

pub fn write_marker(path: &Path, marker: &RunMarker) -> io::Result<()> {
    fs::write(path, format_marker(marker))
}

pub fn remove_marker(path: &Path) {
    let _ = fs::remove_file(path);
}

/// A marker belongs to a previous run only when that PID is not a live
/// gimbal-files. A reused PID running another program counts as a crash.
pub fn marker_is_stale(_marker: &RunMarker, pid_live: bool, pid_is_gimbal: bool) -> bool {
    !(pid_live && pid_is_gimbal)
}

/// Whether `/proc/<pid>` exists.
pub fn pid_alive(pid: u32) -> bool {
    Path::new("/proc").join(pid.to_string()).exists()
}

/// Whether the live process's command name or executable is gimbal-files.
pub fn pid_is_gimbal(pid: u32) -> bool {
    let base = Path::new("/proc").join(pid.to_string());
    if let Ok(comm) = fs::read_to_string(base.join("comm")) {
        let comm = comm.trim();
        if comm == "gimbal-files" {
            return true;
        }
    }
    fs::read_link(base.join("exe"))
        .ok()
        .and_then(|path| path.file_name().map(|name| name == "gimbal-files"))
        .unwrap_or(false)
}

/// Whether a found marker means the last run ended uncleanly.
pub fn previous_run_crashed(marker: Option<RunMarker>) -> Option<RunMarker> {
    let marker = marker?;
    if marker_is_stale(&marker, pid_alive(marker.pid), pid_is_gimbal(marker.pid)) {
        Some(marker)
    } else {
        None
    }
}

/// The crash record's file name, from the run's wall clock. Colons are not
/// portable in every tool, so they become dashes.
pub fn crash_record_name(started: &str) -> String {
    format!("{}.log", started.replace(':', "-"))
}

/// The newest `keep` names survive. The input need not be sorted.
pub fn crash_records_to_remove(names: &[String], keep: usize) -> Vec<String> {
    let mut sorted: Vec<&String> = names.iter().collect();
    sorted.sort();
    if sorted.len() <= keep {
        return Vec::new();
    }
    sorted[..sorted.len() - keep]
        .iter()
        .map(|name| (*name).clone())
        .collect()
}

/// Remove every crash record beyond the newest `keep` in `directory`.
pub fn retain_crash_records(directory: &Path, keep: usize) -> io::Result<()> {
    let mut names = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".log") && entry.file_type()?.is_file() {
            names.push(name);
        }
    }
    for name in crash_records_to_remove(&names, keep) {
        let _ = fs::remove_file(directory.join(name));
    }
    Ok(())
}

/// The tail of a text as lines, without a trailing newline. An empty text
/// gives an empty string.
pub fn last_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(count);
    lines[start..].join("\n")
}

fn modified_unix(path: &Path) -> Option<u64> {
    fs::metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

/// Copy the crashed run's tail from `gimbal.log`, and from `gimbal.log.1`
/// when the run began before the last rotation, into
/// `crashes/<started>.log`. Returns the record's path.
pub fn save_crash_record(
    log: &Path,
    crashes_dir: &Path,
    marker: &RunMarker,
) -> io::Result<PathBuf> {
    fs::create_dir_all(crashes_dir)?;
    let record = crashes_dir.join(crash_record_name(&marker.started));
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Gimbal crash record from {} (pid {})",
        marker.started, marker.pid
    );

    let previous = archive_path(log, 1);
    if modified_unix(&previous).is_some_and(|modified| modified >= marker.started_unix) {
        let _ = writeln!(text, "# Previous log file: {}", previous.display());
        if let Ok(previous_text) = fs::read_to_string(&previous) {
            text.push_str(&last_lines(&previous_text, CRASH_LINES));
            text.push('\n');
        }
    }

    let _ = writeln!(text, "# Log file: {}", log.display());
    if let Ok(log_text) = fs::read_to_string(log) {
        text.push_str(&last_lines(&log_text, CRASH_LINES));
        text.push('\n');
    }
    fs::write(&record, text)?;
    retain_crash_records(crashes_dir, KEEP_CRASHES)?;
    Ok(record)
}

/// One crash record for the report. `timestamp` is the file name without
/// `.log`; `text` is its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashRecord {
    pub timestamp: String,
    pub text: String,
}

/// One apport file. The report names it and its date, never its contents.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApportCrash {
    pub name: String,
    pub date: String,
}

/// Everything the report prints. The CLI fills this from the running system;
/// the builder only formats and redacts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportData {
    pub gimbal_version: String,
    pub install_kind: InstallKind,
    pub package_version: Option<String>,
    pub cli_version: String,
    pub os: Option<String>,
    pub kernel: Option<String>,
    pub desktop: Option<String>,
    pub display: String,
    pub crash: Option<CrashRecord>,
    pub log: Option<String>,
    pub log_path: Option<PathBuf>,
    pub config: Option<String>,
    pub config_path: Option<PathBuf>,
    pub apport: Option<ApportCrash>,
}

/// Lay out the six sections and redact the result.
pub fn build_report(data: &ReportData, home: Option<&Path>, no_paths: bool) -> String {
    let mut report = String::new();
    report.push_str("# Gimbal Files bug report\n\n");

    report.push_str("## Gimbal\n");
    let _ = writeln!(report, "Gimbal version: {}", data.gimbal_version);
    let _ = writeln!(report, "Install kind: {}", data.install_kind.as_str());
    match &data.package_version {
        Some(version) => {
            let _ = writeln!(
                report,
                "Installed package: {version} (dpkg-query -W gimbal-files)"
            );
        }
        None => report.push_str("Installed package: not installed through dpkg\n"),
    }
    let _ = writeln!(report, "CLI version: {}", data.cli_version);
    report.push('\n');

    report.push_str("## System\n");
    let _ = writeln!(report, "OS: {}", data.os.as_deref().unwrap_or("unknown"));
    let _ = writeln!(
        report,
        "Kernel: {}",
        data.kernel.as_deref().unwrap_or("unknown")
    );
    let _ = writeln!(
        report,
        "XDG_CURRENT_DESKTOP: {}",
        data.desktop.as_deref().unwrap_or("unset")
    );
    let _ = writeln!(report, "Display: {}", data.display);
    report.push('\n');

    report.push_str("## Last crash record\n");
    match &data.crash {
        Some(crash) => {
            let _ = writeln!(report, "Timestamp: {}", crash.timestamp);
            report.push_str(&crash.text);
            if !crash.text.ends_with('\n') {
                report.push('\n');
            }
        }
        None => report.push_str("No crash record was saved.\n"),
    }
    report.push('\n');

    report.push_str("## Log\n");
    match (&data.log, &data.log_path) {
        (Some(log), _) => {
            report.push_str(log);
            if !log.ends_with('\n') {
                report.push('\n');
            }
        }
        (None, Some(path)) => {
            let _ = writeln!(report, "No log file at {}.", path.display());
        }
        (None, None) => report.push_str("No log file path is available.\n"),
    }
    report.push('\n');

    report.push_str("## Config\n");
    match (&data.config, &data.config_path) {
        (Some(config), _) => {
            report.push_str(config);
            if !config.ends_with('\n') {
                report.push('\n');
            }
        }
        (None, Some(path)) => {
            let _ = writeln!(report, "No config file at {}.", path.display());
        }
        (None, None) => report.push_str("No config file path is available.\n"),
    }
    report.push('\n');

    report.push_str("## apport\n");
    match &data.apport {
        Some(crash) => {
            let _ = writeln!(report, "Crash file: {} ({})", crash.name, crash.date);
            let _ = writeln!(report, "{}", text::apport_attach(&crash.name));
            report.push_str("Or keep the file for the maintainer.\n");
        }
        None => report.push_str("No apport crash file found in /var/crash.\n"),
    }

    redact(&report, home, no_paths)
}

/// Replace the home folder with `~`, then, when asked, every other absolute
/// path with `<path>`.
pub fn redact(text: &str, home: Option<&Path>, no_paths: bool) -> String {
    let text = replace_home(text, home);
    if no_paths {
        replace_absolute_paths(&text)
    } else {
        text
    }
}

pub fn replace_home(text: &str, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return text.to_string();
    };
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return text.to_string();
    }
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find(home) {
        let after = &rest[position + home.len()..];
        let boundary = after.is_empty()
            || after.starts_with('/')
            || !after.starts_with(|character: char| {
                character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
            });
        output.push_str(&rest[..position]);
        if boundary {
            output.push('~');
            rest = after;
        } else {
            output.push_str(home);
            rest = after;
        }
    }
    output.push_str(rest);
    output
}

/// Replace absolute-path tokens, and `~/`-relative ones, with `<path>`. File
/// and folder names under the home folder are the private part, so
/// `--no-paths` hides those too; a bare `~` stays. URLs and the `//` in them
/// are left alone.
pub fn replace_absolute_paths(text: &str) -> String {
    let text = replace_home_relative_paths(text);
    let text = text.as_str();
    let mut output = String::with_capacity(text.len());
    let mut iterator = text.char_indices().peekable();
    while let Some((index, character)) = iterator.next() {
        if character != '/' {
            output.push(character);
            continue;
        }
        let previous = text[..index].chars().next_back();
        let next = iterator.peek().map(|(_, character)| *character);
        let starts_path = next.is_some_and(|next| !next.is_whitespace() && next != '/')
            && previous.is_none_or(|previous| {
                previous.is_whitespace()
                    || matches!(previous, '=' | ':' | '(' | '[' | '{' | '"' | '\'')
            });
        if !starts_path {
            output.push(character);
            continue;
        }
        while let Some((_, next)) = iterator.peek().copied() {
            if next.is_whitespace()
                || matches!(next, ',' | ')' | ']' | '}' | '"' | '\'' | ':' | ';')
            {
                break;
            }
            iterator.next();
        }
        output.push_str("<path>");
    }
    output
}

/// Replace each `~/...` token with `<path>`, at the same token boundaries as
/// absolute paths.
fn replace_home_relative_paths(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(position) = rest.find("~/") {
        let previous = rest[..position].chars().next_back();
        let at_boundary = previous.is_none_or(|previous| {
            previous.is_whitespace() || matches!(previous, '=' | ':' | '(' | '[' | '{' | '"' | '\'')
        });
        output.push_str(&rest[..position]);
        let after = &rest[position..];
        if !at_boundary {
            output.push_str("~/");
            rest = &after[2..];
            continue;
        }
        let end = after
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, ',' | ')' | ']' | '}' | '"' | '\'' | ':' | ';')
            })
            .unwrap_or(after.len());
        output.push_str("<path>");
        rest = &after[end..];
    }
    output.push_str(rest);
    output
}

/// Replace every `[pins] folders` entry with `<pin>`, keeping the rest of the
/// file and its comments.
pub fn redact_pins(text: &str) -> String {
    let Ok(mut document) = text.parse::<DocumentMut>() else {
        return text.to_string();
    };
    if let Some(pins) = document.get_mut("pins").and_then(Item::as_table_mut)
        && let Some(folders) = pins.get_mut("folders").and_then(Item::as_array_mut)
    {
        for value in folders.iter_mut() {
            *value = Value::from("<pin>");
        }
    }
    document.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "gimbal-p2ae-{name}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("scratch folder");
        path
    }

    #[test]
    fn paths_follow_the_xdg_rules() {
        let state =
            state_dir(Some(Path::new("/tmp/state")), Some(Path::new("/home/kyle"))).expect("state");
        assert_eq!(state, PathBuf::from("/tmp/state/gimbal-files"));
        assert_eq!(
            log_path_in(&state),
            PathBuf::from("/tmp/state/gimbal-files/logs/gimbal.log")
        );
        assert_eq!(
            marker_path_in(&state),
            PathBuf::from("/tmp/state/gimbal-files/running")
        );
        assert_eq!(
            crash_dir_in(&state),
            PathBuf::from("/tmp/state/gimbal-files/crashes")
        );

        let fallback = state_dir(None, Some(Path::new("/home/kyle"))).expect("fallback");
        assert_eq!(
            fallback,
            PathBuf::from("/home/kyle/.local/state/gimbal-files")
        );
        let relative = state_dir(Some(Path::new("relative")), Some(Path::new("/home/kyle")))
            .expect("relative falls back");
        assert_eq!(
            relative,
            PathBuf::from("/home/kyle/.local/state/gimbal-files")
        );
        assert_eq!(state_dir(None, None), None);
    }

    #[test]
    fn rotation_starts_at_five_mib_and_keeps_three_files() {
        assert_eq!(
            plan_rotation(LOG_LIMIT - 1, false, false),
            Rotation::default()
        );
        assert_eq!(
            plan_rotation(LOG_LIMIT, false, false),
            Rotation {
                remove_two: false,
                one_to_two: false,
                log_to_one: true,
            }
        );
        assert_eq!(
            plan_rotation(LOG_LIMIT + 1, true, false),
            Rotation {
                remove_two: false,
                one_to_two: true,
                log_to_one: true,
            }
        );
        assert_eq!(
            plan_rotation(LOG_LIMIT + 1, true, true),
            Rotation {
                remove_two: true,
                one_to_two: true,
                log_to_one: true,
            }
        );
        assert_eq!(
            archive_path(Path::new("/tmp/gimbal.log"), 1),
            PathBuf::from("/tmp/gimbal.log.1")
        );
    }

    #[test]
    fn rotating_three_times_never_leaves_more_than_three_files() {
        let scratch = scratch("rotate");
        let log = scratch.join("gimbal.log");
        for round in 0..4 {
            fs::write(&log, vec![b'x'; LOG_LIMIT as usize]).unwrap();
            rotate(&log).unwrap();
            fs::write(&log, format!("round {round}\n")).unwrap();
        }
        let names: Vec<String> = fs::read_dir(&scratch)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(names.len(), 3, "{names:?}");
        assert!(names.contains(&"gimbal.log".to_string()));
        assert!(names.contains(&"gimbal.log.1".to_string()));
        assert!(names.contains(&"gimbal.log.2".to_string()));
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn the_marker_round_trips_and_only_a_live_gimbal_is_clean() {
        let marker = RunMarker {
            pid: 4242,
            started: "2026-11-08T09:16:17.123".to_string(),
            started_unix: 1_762_593_377,
            log_start: 99,
        };
        assert_eq!(parse_marker(&format_marker(&marker)), Some(marker.clone()));
        assert_eq!(parse_marker("pid = 1\nbogus = 2\n"), None);
        assert!(!marker_is_stale(&marker, true, true));
        assert!(marker_is_stale(&marker, false, false));
        assert!(marker_is_stale(&marker, true, false));
        assert!(marker_is_stale(&marker, false, true));
        assert_eq!(
            crash_record_name(&marker.started),
            "2026-11-08T09-16-17.123.log"
        );
    }

    #[test]
    fn crash_records_keep_the_newest_ten_names() {
        let names: Vec<String> = (0..12)
            .map(|index| format!("2026-11-08T09-16-{index:02}.000.log"))
            .collect();
        let remove = crash_records_to_remove(&names, 10);
        assert_eq!(remove.len(), 2);
        assert!(remove.contains(&names[0]));
        assert!(remove.contains(&names[1]));

        let scratch = scratch("retain");
        for name in &names {
            fs::write(scratch.join(name), b"record").unwrap();
        }
        retain_crash_records(&scratch, 10).unwrap();
        assert_eq!(fs::read_dir(&scratch).unwrap().count(), 10);
        fs::remove_dir_all(&scratch).unwrap();
    }

    #[test]
    fn last_lines_takes_the_tail_and_tolerates_short_input() {
        assert_eq!(last_lines("a\nb\nc\n", 2), "b\nc");
        assert_eq!(last_lines("a\nb", 5), "a\nb");
        assert_eq!(last_lines("", 5), "");
    }

    #[test]
    fn a_crash_record_copies_the_tail_and_the_previous_file_when_needed() {
        let scratch = scratch("crash");
        let log_dir = scratch.join("logs");
        let crashes = scratch.join("crashes");
        fs::create_dir_all(&log_dir).unwrap();
        let log = log_dir.join("gimbal.log");
        fs::write(&log, "current one\ncurrent two\n").unwrap();
        fs::write(archive_path(&log, 1), "previous one\nprevious two\n").unwrap();

        let marker = RunMarker {
            pid: 7,
            started: "2026-11-08T09:16:17.123".to_string(),
            started_unix: 0,
            log_start: 0,
        };
        let record = save_crash_record(&log, &crashes, &marker).unwrap();
        let text = fs::read_to_string(&record).unwrap();
        assert!(text.contains("previous one"), "{text}");
        assert!(text.contains("current two"), "{text}");
        assert_eq!(record.file_name().unwrap(), "2026-11-08T09-16-17.123.log");

        // A previous file written before the run started is not copied.
        let later = RunMarker {
            started_unix: u64::MAX,
            ..marker
        };
        let record = save_crash_record(&log, &crashes, &later).unwrap();
        let text = fs::read_to_string(&record).unwrap();
        assert!(!text.contains("previous one"), "{text}");
        assert!(text.contains("current two"), "{text}");
        fs::remove_dir_all(&scratch).unwrap();
    }

    fn fixture_report(lines: usize) -> ReportData {
        let scratch = scratch("report");
        let log_path = scratch.join("logs/gimbal.log");
        fs::create_dir_all(log_path.parent().expect("log parent")).unwrap();
        let log = (0..10)
            .map(|index| format!("/home/kyle/Documents/file-{index}.txt opened"))
            .collect::<Vec<String>>()
            .join("\n");
        fs::write(&log_path, &log).unwrap();
        let config_path = scratch.join("config.toml");
        let config =
            "[look]\nscale = 1.0\n\n[pins]\nfolders = [\"/home/kyle\", \"/home/kyle/Downloads\"]\n";
        fs::write(&config_path, config).unwrap();
        let data = ReportData {
            gimbal_version: "0.1.2".to_string(),
            install_kind: InstallKind::Development,
            package_version: None,
            cli_version: "0.1.2".to_string(),
            os: Some("Ubuntu 26.04 LTS".to_string()),
            kernel: Some("6.14.0".to_string()),
            desktop: Some("niri".to_string()),
            display: "Wayland".to_string(),
            crash: None,
            log: Some(last_lines(
                &fs::read_to_string(&log_path).expect("log fixture"),
                lines,
            )),
            log_path: Some(log_path),
            config: Some(redact_pins(
                &fs::read_to_string(&config_path).expect("config fixture"),
            )),
            config_path: Some(config_path),
            apport: None,
        };
        fs::remove_dir_all(&scratch).unwrap();
        data
    }

    #[test]
    fn the_report_lays_out_six_sections_and_redacts_the_home_folder() {
        let data = fixture_report(3);
        let report = build_report(&data, Some(Path::new("/home/kyle")), false);
        for heading in [
            "## Gimbal",
            "## System",
            "## Last crash record",
            "## Log",
            "## Config",
            "## apport",
        ] {
            assert!(report.contains(heading), "missing {heading}: {report}");
        }
        assert!(!report.contains("/home/kyle"), "{report}");
        assert!(report.contains("~"), "{report}");
        assert!(report.contains("\"<pin>\""), "{report}");
        assert!(report.contains("file-9.txt"), "{report}");
        assert!(!report.contains("file-6.txt"), "{report}");
        assert!(report.contains("No crash record was saved."), "{report}");
    }

    #[test]
    fn no_paths_replaces_the_other_absolute_paths() {
        let data = fixture_report(10);
        let report = build_report(&data, Some(Path::new("/home/kyle")), true);
        assert!(!report.contains("~/Documents/file-0.txt"), "{report}");
        assert!(!report.contains("/var/crash"), "{report}");
        assert!(report.contains("<path>"), "{report}");

        let text =
            "keep https://github.com/x and /usr/lib/libgtk.so and ~/a here, ~ alone and a~/b";
        let redacted = replace_absolute_paths(text);
        assert_eq!(
            redacted,
            "keep https://github.com/x and <path> and <path> here, ~ alone and a~/b"
        );
    }

    #[test]
    fn a_missing_log_and_a_present_crash_record_are_reported() {
        let mut data = fixture_report(0);
        data.log = None;
        data.crash = Some(CrashRecord {
            timestamp: "2026-11-08T09-16-17.123".to_string(),
            text: "2026-11-08T09:16:17.123 [gimbal 2.000] glib: Gtk-WARNING: bad\n".to_string(),
        });
        data.apport = Some(ApportCrash {
            name: "_usr_bin_gimbal-files.1234.crash".to_string(),
            date: "2026-11-08 09:16".to_string(),
        });
        let report = build_report(&data, Some(Path::new("/home/kyle")), false);
        assert!(report.contains("No log file at"), "{report}");
        assert!(
            report.contains("Timestamp: 2026-11-08T09-16-17.123"),
            "{report}"
        );
        assert!(report.contains("Gtk-WARNING"), "{report}");
        assert!(
            report.contains("_usr_bin_gimbal-files.1234.crash (2026-11-08 09:16)"),
            "{report}"
        );
        assert!(
            report.contains("ubuntu-bug /var/crash/_usr_bin_gimbal-files.1234.crash"),
            "{report}"
        );
    }

    #[test]
    fn the_report_file_name_uses_the_local_wall_clock() {
        let now = UNIX_EPOCH + Duration::from_secs(1_762_593_377);
        let path = report_path(Some(Path::new("/home/kyle")), now).expect("path");
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("gimbal-files-bug-report-"), "{name}");
        assert!(name.ends_with(".txt"), "{name}");
        assert_eq!(
            name.len(),
            "gimbal-files-bug-report-2026-11-08-091617.txt".len()
        );
    }

    #[test]
    fn a_malformed_pins_table_is_left_as_text() {
        assert_eq!(redact_pins("not toml"), "not toml");
        let text = "[pins]\nfolders = [\"/home/kyle\"]\n";
        let redacted = redact_pins(text);
        assert!(redacted.contains("<pin>"), "{redacted}");
        assert!(!redacted.contains("/home/kyle"), "{redacted}");
    }
}
