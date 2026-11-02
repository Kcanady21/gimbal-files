// SPDX-License-Identifier: GPL-3.0-or-later
//! The command-line front end for Gimbal Files.
//!
//! The CLI reads the command registry: `commands` lists it, and `check`
//! validates an invocation as the `Cli` caller. Nothing runs a command yet.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use fm_core::command::{Caller, Invocation, Registry};

const USAGE: &str = "\
Usage: gimbal-files-cli [--help | --version]
       gimbal-files-cli commands
       gimbal-files-cli check <command> [key=value ...]

`commands` lists every command in the registry. `check` validates one as the
Cli caller and prints the checked command, or the error. Nothing runs a
command yet.
";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
    Commands,
    Check(Vec<String>),
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
        _ => Action::UsageError,
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match action_for(&args) {
        Action::Help => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Action::Version => {
            println!("gimbal-files-cli {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Action::Commands => {
            print_commands();
            ExitCode::SUCCESS
        }
        Action::Check(tokens) => check_command(&tokens),
        Action::UsageError => {
            eprint!("{USAGE}");
            ExitCode::from(2)
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

fn check_command(tokens: &[String]) -> ExitCode {
    let result = Invocation::parse(tokens)
        .and_then(|invocation| Registry::new().validate(Caller::Cli, &invocation));

    match result {
        Ok(command) => {
            println!("{command}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, action_for};

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
