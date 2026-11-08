// SPDX-License-Identifier: GPL-3.0-or-later
//! The command-line front end for Gimbal Files.
//!
//! `commands` and `check` are offline: they read the command registry and
//! never need a running app. `state`, `run` and `job` talk to the local socket
//! as `Caller::Agent`. There is deliberately no CLI enable switch: the user
//! turns automation on in Settings.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime};

use fm_agent::client::{self, ClientError, ReplyEnvelope};
use fm_agent::protocol::{Operation, Request};
use fm_core::command::{Caller, Invocation, Registry};
use fm_core::logfile::{self, ApportCrash, CrashRecord, ReportData};

const USAGE: &str = "\
Usage: gimbal-files-cli [--help | --version]
       gimbal-files-cli commands
       gimbal-files-cli check <command> [key=value ...]
       gimbal-files-cli state [--offset N] [--limit N]
       gimbal-files-cli run [--window ID] [--tab ID] [--wait] [--timeout SECS]
                            <command> [key=value ...]
       gimbal-files-cli job <job-id> [--wait] [--timeout SECS]
       gimbal-files-cli bug-report [--output FILE] [--lines N] [--no-paths]

`commands` lists every registry command. `check` validates one as the Cli
caller without a running app. `state`, `run` and `job` use the local socket;
every connection runs as the Agent caller. `bug-report` reads files only and
needs no running app or socket.

The socket is only there when Gimbal is running and Settings -> Allow local
automation is On. There is no CLI enable option and the client never starts
the app. Replies are printed as TOML.

`bug-report` writes one text file (500 log lines by default), prints its path
and sends nothing. Read it before attaching it. `--lines N` keeps more log
lines; `--no-paths` replaces every absolute path except your home folder.

Exit codes:
  0  success
  2  usage or invalid command/arguments
  3  Gimbal is not running, or local automation is off
  4  transport failure; a submitted request's outcome may be unknown
  5  Gimbal refused the request
  6  --wait timed out; the job may still be running

Post-implementation examples:
  # enable Settings -> Allow local automation first
  gimbal-files-cli state
  gimbal-files-cli run --window <window id from state> tab.new path=/tmp
  gimbal-files-cli run --window <id> file.copy paths=/tmp/a.txt paths=/tmp/b.txt \\
      to=/tmp/dest dry-run=true
  gimbal-files-cli run --window <id> file.copy paths=/tmp/a.txt to=/tmp/dest
  gimbal-files-cli job <job-id> --wait
  # then turn the Settings switch Off and verify clients fail
";

const EXIT_OK: u8 = 0;
const EXIT_USAGE: u8 = 2;
const EXIT_UNAVAILABLE: u8 = 3;
const EXIT_TRANSPORT: u8 = 4;
const EXIT_REFUSED: u8 = 5;
const EXIT_WAIT_TIMEOUT: u8 = 6;

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    Commands,
    Check(Vec<String>),
    State {
        offset: usize,
        limit: usize,
    },
    Run {
        window: Option<String>,
        tab: Option<String>,
        wait: bool,
        timeout: Duration,
        tokens: Vec<String>,
    },
    Job {
        id: String,
        wait: bool,
        timeout: Duration,
    },
    BugReport {
        output: Option<PathBuf>,
        lines: usize,
        no_paths: bool,
    },
    UsageError,
}

/// Decide what to do from the arguments after the program name.
///
/// A pure function so the decision can be tested without running a process.
fn action_for(args: &[String]) -> Action {
    match args {
        [] => Action::Help,
        [arg] if arg == "--help" => Action::Help,
        [arg] if arg == "--version" => Action::Version,
        [arg] if arg == "commands" => Action::Commands,
        [command, rest @ ..] if command == "check" && !rest.is_empty() => {
            Action::Check(rest.to_vec())
        }
        [command, rest @ ..] if command == "state" => parse_state(rest),
        [command, rest @ ..] if command == "run" => parse_run(rest),
        [command, rest @ ..] if command == "job" => parse_job(rest),
        [command, rest @ ..] if command == "bug-report" => parse_bug_report(rest),
        _ => Action::UsageError,
    }
}

fn parse_state(args: &[String]) -> Action {
    let mut offset = 0;
    let mut limit = 20;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--offset" => {
                let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) else {
                    return Action::UsageError;
                };
                offset = value;
                index += 2;
            }
            "--limit" => {
                let Some(value) = args.get(index + 1).and_then(|value| value.parse().ok()) else {
                    return Action::UsageError;
                };
                limit = value;
                index += 2;
            }
            _ => return Action::UsageError,
        }
    }
    Action::State { offset, limit }
}

fn parse_run(args: &[String]) -> Action {
    let mut window = None;
    let mut tab = None;
    let mut wait = false;
    let mut timeout = client::CLIENT_TIMEOUT;
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--window" => {
                let Some(value) = args.get(index + 1) else {
                    return Action::UsageError;
                };
                window = Some(value.clone());
                index += 2;
            }
            "--tab" => {
                let Some(value) = args.get(index + 1) else {
                    return Action::UsageError;
                };
                tab = Some(value.clone());
                index += 2;
            }
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--timeout" => {
                let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value > 0 && *value <= 600)
                else {
                    return Action::UsageError;
                };
                timeout = Duration::from_secs(value);
                index += 2;
            }
            token if token.starts_with("--") => return Action::UsageError,
            _ => {
                tokens.extend(args[index..].iter().cloned());
                break;
            }
        }
    }
    if tokens.is_empty() {
        return Action::UsageError;
    }
    Action::Run {
        window,
        tab,
        wait,
        timeout,
        tokens,
    }
}

fn parse_job(args: &[String]) -> Action {
    let mut id = None;
    let mut wait = false;
    let mut timeout = client::CLIENT_TIMEOUT;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--wait" => {
                wait = true;
                index += 1;
            }
            "--timeout" => {
                let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<u64>().ok())
                    .filter(|value| *value > 0 && *value <= 600)
                else {
                    return Action::UsageError;
                };
                timeout = Duration::from_secs(value);
                index += 2;
            }
            token if token.starts_with("--") => return Action::UsageError,
            token => {
                if id.is_some() {
                    return Action::UsageError;
                }
                id = Some(token.to_string());
                index += 1;
            }
        }
    }
    let Some(id) = id else {
        return Action::UsageError;
    };
    Action::Job { id, wait, timeout }
}

fn parse_bug_report(args: &[String]) -> Action {
    let mut output = None;
    let mut lines = logfile::DEFAULT_REPORT_LINES;
    let mut no_paths = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                let Some(value) = args.get(index + 1) else {
                    return Action::UsageError;
                };
                output = Some(PathBuf::from(value));
                index += 2;
            }
            "--lines" => {
                let Some(value) = args
                    .get(index + 1)
                    .and_then(|value| value.parse::<usize>().ok())
                    .filter(|value| *value > 0 && *value <= logfile::REPORT_LIMIT)
                else {
                    return Action::UsageError;
                };
                lines = value;
                index += 2;
            }
            "--no-paths" => {
                no_paths = true;
                index += 1;
            }
            _ => return Action::UsageError,
        }
    }
    Action::BugReport {
        output,
        lines,
        no_paths,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match action_for(&args) {
        Action::Help => {
            print!("{USAGE}");
            ExitCode::from(EXIT_OK)
        }
        Action::Version => {
            println!("gimbal-files-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::from(EXIT_OK)
        }
        Action::Commands => {
            print_commands();
            ExitCode::from(EXIT_OK)
        }
        Action::Check(tokens) => check_command(&tokens),
        Action::State { offset, limit } => {
            let request = Request {
                request_id: request_id(),
                instance: None,
                generation: None,
                operation: Operation::State {
                    offset,
                    limit: limit.clamp(1, 100),
                },
            };
            send(&request, client::CLIENT_TIMEOUT)
        }
        Action::Run {
            window,
            tab,
            wait,
            timeout,
            tokens,
        } => run_command(window, tab, wait, timeout, &tokens),
        Action::Job { id, wait, timeout } => job_command(&id, wait, timeout),
        Action::BugReport {
            output,
            lines,
            no_paths,
        } => bug_report(output, lines, no_paths),
        Action::UsageError => {
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

/// One command per line: name, effect, arguments, summary, in catalogue order.
fn print_commands() {
    for spec in Registry::new().commands() {
        let arguments = spec.arguments();
        let rendered = if arguments.is_empty() {
            "(none)".to_string()
        } else {
            arguments
                .iter()
                .map(|argument| argument.label())
                .collect::<Vec<String>>()
                .join(", ")
        };
        println!(
            "{} | {} | {} | {}",
            spec.name, spec.effect, rendered, spec.summary
        );
    }
}

fn bug_report(output: Option<PathBuf>, lines: usize, no_paths: bool) -> ExitCode {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data = report_data(lines.clamp(1, logfile::REPORT_LIMIT));
    let report = logfile::build_report(&data, home.as_deref(), no_paths);
    let path = match output.or_else(|| logfile::report_path(home.as_deref(), SystemTime::now())) {
        Some(path) => path,
        None => {
            eprintln!("HOME is not set; pass --output FILE");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = fs::create_dir_all(parent);
    }
    if let Err(error) = fs::write(&path, report) {
        eprintln!("could not write {}: {error}", path.display());
        return ExitCode::from(EXIT_TRANSPORT);
    }
    println!("{}", path.display());
    println!("{}", logfile::text::READ_BEFORE_ATTACHING);
    println!("Attach it to a new issue at {}", logfile::ISSUE_URL);
    ExitCode::from(EXIT_OK)
}

fn report_data(lines: usize) -> ReportData {
    let log_path = logfile::default_log_path();
    let log = log_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| logfile::last_lines(&text, lines));
    let config_path = fm_core::config::default_config_path();
    let config = config_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .map(|text| logfile::redact_pins(&text));

    ReportData {
        gimbal_version: env!("CARGO_PKG_VERSION").to_string(),
        install_kind: fm_core::update::current_install_kind(),
        package_version: installed_package_version(),
        cli_version: env!("CARGO_PKG_VERSION").to_string(),
        os: os_release(),
        kernel: kernel_release(),
        desktop: std::env::var("XDG_CURRENT_DESKTOP").ok(),
        display: display_kind(),
        crash: crash_record(),
        log,
        log_path,
        config,
        config_path,
        apport: apport_crash(),
    }
}

fn installed_package_version() -> Option<String> {
    let output = std::process::Command::new("dpkg-query")
        .args(["-W", "-f=${Version}", "gimbal-files"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!version.is_empty()).then_some(version)
}

fn os_release() -> Option<String> {
    let text = fs::read_to_string("/etc/os-release").ok()?;
    text.lines().find_map(|line| {
        let value = line.strip_prefix("PRETTY_NAME=")?;
        Some(value.trim().trim_matches('"').to_string())
    })
}

fn kernel_release() -> Option<String> {
    if let Ok(output) = std::process::Command::new("uname").arg("-r").output()
        && output.status.success()
    {
        let kernel = String::from_utf8(output.stdout).ok()?.trim().to_string();
        if !kernel.is_empty() {
            return Some(kernel);
        }
    }
    let text = fs::read_to_string("/proc/sys/kernel/osrelease").ok()?;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn display_kind() -> String {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "Wayland".to_string()
    } else if std::env::var_os("DISPLAY").is_some() {
        "X11".to_string()
    } else {
        "unknown".to_string()
    }
}

fn crash_record() -> Option<CrashRecord> {
    let directory = logfile::default_crash_dir()?;
    let mut newest: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(directory).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "log") || !path.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, path));
        }
    }
    let (_, path) = newest?;
    let timestamp = path.file_stem()?.to_string_lossy().to_string();
    let text = fs::read_to_string(path).ok()?;
    Some(CrashRecord { timestamp, text })
}

fn apport_crash() -> Option<ApportCrash> {
    let mut newest: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(Path::new("/var/crash")).ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("_usr_bin_gimbal-files.") || !name.ends_with(".crash") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        if newest.as_ref().is_none_or(|(time, _)| modified > *time) {
            newest = Some((modified, name));
        }
    }
    let (modified, name) = newest?;
    let stamp = logfile::wall_clock(modified);
    let date = stamp.get(..16).unwrap_or(&stamp).replace('T', " ");
    Some(ApportCrash { name, date })
}

fn check_command(tokens: &[String]) -> ExitCode {
    let result = Invocation::parse(tokens)
        .and_then(|invocation| Registry::new().validate(Caller::Cli, &invocation));

    match result {
        Ok(command) => {
            println!("{command}");
            ExitCode::from(EXIT_OK)
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Binding {
    instance: String,
    generation: u64,
}

/// A state reply is the documented bootstrap operation: it discovers the
/// current instance/generation that every write and job lookup must name.
fn bootstrap(timeout: Duration) -> Result<Binding, ClientError> {
    let request = Request {
        request_id: request_id(),
        instance: None,
        generation: None,
        operation: Operation::State {
            offset: 0,
            limit: 1,
        },
    };
    let (_text, envelope) = exchange(&request, timeout)?;
    if envelope.status == "error" {
        return Err(ClientError::Protocol(
            envelope
                .error_code
                .unwrap_or_else(|| "the state bootstrap was refused".to_string()),
        ));
    }
    match (envelope.instance, envelope.generation) {
        (Some(instance), Some(generation)) => Ok(Binding {
            instance,
            generation,
        }),
        _ => Err(ClientError::Protocol(
            "the state reply did not carry an instance and generation".to_string(),
        )),
    }
}

fn bound_request(binding: &Binding, operation: Operation) -> Request {
    Request {
        request_id: request_id(),
        instance: Some(binding.instance.clone()),
        generation: Some(binding.generation),
        operation,
    }
}

fn run_command(
    window: Option<String>,
    tab: Option<String>,
    wait: bool,
    timeout: Duration,
    tokens: &[String],
) -> ExitCode {
    // Validate offline first as the Agent caller, exactly as the server will.
    // This catches unknown commands, missing paths and forbidden callers
    // without opening a connection.
    let invocation = match Invocation::parse(tokens) {
        Ok(invocation) => invocation,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_USAGE);
        }
    };
    if let Err(error) = Registry::new().validate(Caller::Agent, &invocation) {
        eprintln!("{error}");
        return ExitCode::from(EXIT_USAGE);
    }

    let binding = match bootstrap(timeout) {
        Ok(binding) => binding,
        Err(error) => return exit_for(&error),
    };
    let request = bound_request(
        &binding,
        Operation::Run {
            window,
            tab,
            command: invocation.name().to_string(),
            arguments: invocation.arguments().to_vec(),
            wait,
        },
    );
    match exchange(&request, timeout) {
        Ok((text, envelope)) => {
            println!("{text}");
            if envelope.status == "error" {
                return ExitCode::from(EXIT_REFUSED);
            }
            if wait && let Some(job) = envelope.job {
                return follow_job(&job, &binding, timeout);
            }
            ExitCode::from(EXIT_OK)
        }
        Err(error) => exit_for(&error),
    }
}

fn job_command(id: &str, wait: bool, timeout: Duration) -> ExitCode {
    let binding = match bootstrap(timeout) {
        Ok(binding) => binding,
        Err(error) => return exit_for(&error),
    };
    if wait {
        follow_job(id, &binding, timeout)
    } else {
        let request = bound_request(&binding, Operation::Job { id: id.to_string() });
        match exchange(&request, timeout) {
            Ok((text, envelope)) => {
                println!("{text}");
                if envelope.status == "error" {
                    ExitCode::from(EXIT_REFUSED)
                } else {
                    ExitCode::from(EXIT_OK)
                }
            }
            Err(error) => exit_for(&error),
        }
    }
}

/// Ask about one job until it is terminal. A timeout says it may still be
/// running; it never replays the original `run`. Every lookup names the enable
/// cycle discovered before the run and is refused once that cycle changes.
fn follow_job(id: &str, binding: &Binding, timeout: Duration) -> ExitCode {
    let deadline = Instant::now() + timeout;
    loop {
        let request = bound_request(binding, Operation::Job { id: id.to_string() });
        let (text, envelope) = match exchange(&request, timeout) {
            Ok(reply) => reply,
            Err(error) => return exit_for(&error),
        };
        println!("{text}");
        if envelope.status == "error" {
            return ExitCode::from(EXIT_REFUSED);
        }
        match envelope.state.as_deref() {
            Some("complete" | "cancelled" | "failed") => return ExitCode::from(EXIT_OK),
            Some("expired" | "unknown") => return ExitCode::from(EXIT_REFUSED),
            _ => {}
        }
        if Instant::now() >= deadline {
            eprintln!("the job is still running; --wait timed out");
            return ExitCode::from(EXIT_WAIT_TIMEOUT);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn send(request: &Request, timeout: Duration) -> ExitCode {
    match exchange(request, timeout) {
        Ok((text, envelope)) => {
            println!("{text}");
            if envelope.status == "error" {
                ExitCode::from(EXIT_REFUSED)
            } else {
                ExitCode::from(EXIT_OK)
            }
        }
        Err(error) => exit_for(&error),
    }
}

fn exit_for(error: &ClientError) -> ExitCode {
    eprintln!("{error}");
    let code = match error {
        ClientError::Unavailable(_) => EXIT_UNAVAILABLE,
        ClientError::Timeout | ClientError::Transport(_) | ClientError::Protocol(_) => {
            EXIT_TRANSPORT
        }
    };
    ExitCode::from(code)
}

fn exchange(request: &Request, timeout: Duration) -> Result<(String, ReplyEnvelope), ClientError> {
    let runtime_dir = runtime_dir()?;
    let text = client::request(&runtime_dir, request, timeout)?;
    let envelope = client::parse_reply(&text)?;
    Ok((text, envelope))
}

fn runtime_dir() -> Result<PathBuf, ClientError> {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(value) if !value.is_empty() => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                Ok(path)
            } else {
                Err(ClientError::Unavailable(
                    "XDG_RUNTIME_DIR is not absolute; Gimbal's local socket is unavailable"
                        .to_string(),
                ))
            }
        }
        _ => Err(ClientError::Unavailable(
            "XDG_RUNTIME_DIR is not set; Gimbal's local socket is unavailable".to_string(),
        )),
    }
}

fn request_id() -> String {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_nanos())
        .unwrap_or(0);
    format!("cli-{}-{nanos}-{count}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::{Action, action_for};
    use std::time::Duration;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn no_arguments_shows_usage() {
        assert_eq!(action_for(&[]), Action::Help);
    }

    #[test]
    fn help_shows_usage() {
        assert_eq!(action_for(&args(&["--help"])), Action::Help);
    }

    #[test]
    fn version_shows_the_version() {
        assert_eq!(action_for(&args(&["--version"])), Action::Version);
    }

    #[test]
    fn commands_lists_the_registry() {
        assert_eq!(action_for(&args(&["commands"])), Action::Commands);
    }

    #[test]
    fn check_keeps_the_command_and_its_arguments() {
        assert_eq!(
            action_for(&args(&["check", "nav.go", "path=/tmp"])),
            Action::Check(args(&["nav.go", "path=/tmp"]))
        );
    }

    #[test]
    fn check_without_a_command_is_a_usage_error() {
        assert_eq!(action_for(&args(&["check"])), Action::UsageError);
    }

    #[test]
    fn state_accepts_paging() {
        assert_eq!(
            action_for(&args(&["state", "--offset", "10", "--limit", "5"])),
            Action::State {
                offset: 10,
                limit: 5
            }
        );
        assert_eq!(
            action_for(&args(&["state"])),
            Action::State {
                offset: 0,
                limit: 20
            }
        );
        assert_eq!(action_for(&args(&["state", "--limit"])), Action::UsageError);
    }

    #[test]
    fn run_keeps_window_targets_wait_and_key_values() {
        assert_eq!(
            action_for(&args(&[
                "run",
                "--window",
                "i1/w3",
                "--tab",
                "i1/t4",
                "--wait",
                "file.copy",
                "paths=/tmp/a b",
                "to=/tmp/dest",
            ])),
            Action::Run {
                window: Some("i1/w3".to_string()),
                tab: Some("i1/t4".to_string()),
                wait: true,
                timeout: Duration::from_secs(15),
                tokens: args(&["file.copy", "paths=/tmp/a b", "to=/tmp/dest"]),
            }
        );
    }

    #[test]
    fn run_without_a_command_is_a_usage_error() {
        assert_eq!(action_for(&args(&["run"])), Action::UsageError);
        assert_eq!(action_for(&args(&["run", "--wait"])), Action::UsageError);
        assert_eq!(
            action_for(&args(&["run", "--nope", "tab.new"])),
            Action::UsageError
        );
    }

    #[test]
    fn bug_report_takes_output_lines_and_no_paths() {
        assert_eq!(
            action_for(&args(&[
                "bug-report",
                "--output",
                "/tmp/report.txt",
                "--lines",
                "50",
                "--no-paths",
            ])),
            Action::BugReport {
                output: Some(std::path::PathBuf::from("/tmp/report.txt")),
                lines: 50,
                no_paths: true,
            }
        );
        assert_eq!(
            action_for(&args(&["bug-report"])),
            Action::BugReport {
                output: None,
                lines: super::logfile::DEFAULT_REPORT_LINES,
                no_paths: false,
            }
        );
        assert_eq!(
            action_for(&args(&["bug-report", "--lines", "0"])),
            Action::UsageError
        );
        assert_eq!(
            action_for(&args(&["bug-report", "extra"])),
            Action::UsageError
        );
    }

    #[test]
    fn job_takes_one_id_and_optional_wait() {
        assert_eq!(
            action_for(&args(&["job", "i1/j7", "--wait", "--timeout", "2"])),
            Action::Job {
                id: "i1/j7".to_string(),
                wait: true,
                timeout: Duration::from_secs(2),
            }
        );
        assert_eq!(action_for(&args(&["job"])), Action::UsageError);
        assert_eq!(action_for(&args(&["job", "a", "b"])), Action::UsageError);
    }

    #[test]
    fn write_requests_name_the_discovered_instance_and_generation() {
        let binding = super::Binding {
            instance: "p1-g3".to_string(),
            generation: 3,
        };
        let request = super::bound_request(
            &binding,
            super::Operation::Job {
                id: "p1-g3/j1".to_string(),
            },
        );
        assert_eq!(request.instance.as_deref(), Some("p1-g3"));
        assert_eq!(request.generation, Some(3));
        let text = request.to_toml();
        assert!(text.contains("instance = \"p1-g3\""), "{text}");
        assert!(text.contains("generation = 3"), "{text}");
    }

    #[test]
    fn anything_else_is_a_usage_error() {
        assert_eq!(action_for(&args(&["open"])), Action::UsageError);
        assert_eq!(
            action_for(&args(&["--version", "extra"])),
            Action::UsageError
        );
        assert_eq!(
            action_for(&args(&["commands", "extra"])),
            Action::UsageError
        );
    }
}
