// SPDX-License-Identifier: GPL-3.0-or-later
//! The command-line front end for Gimbal Files.
//!
//! Commands are not available yet: they arrive through `fm-core`'s registry.

#![forbid(unsafe_code)]

use std::process::ExitCode;

const USAGE: &str = "\
Usage: gimbal-files-cli [--help | --version]

Commands come later, through the command registry; none are available yet.
";

#[derive(Debug, PartialEq, Eq)]
enum Action {
    Help,
    Version,
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
        Action::UsageError => {
            eprint!("{USAGE}");
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
    fn anything_else_is_a_usage_error() {
        assert_eq!(action_for(&args(&["open"])), Action::UsageError);
        assert_eq!(
            action_for(&args(&["--version", "extra"])),
            Action::UsageError
        );
    }
}
