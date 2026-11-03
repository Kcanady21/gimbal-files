// SPDX-License-Identifier: GPL-3.0-or-later
//! Invocation parsing, argument checking, and the typed checked command.
//!
//! Errors name the command, the argument and the problem. A misspelled
//! command also gets the closest name when the edit distance is 2 or less.

use std::fmt;
use std::path::{Path, PathBuf};

use super::{
    ArgKind, ArgSpec, Caller, CommandSpec, Effect, Number, Registry, Value, caller_allowed,
    format_hundredths, must_supply,
};
use crate::fileops::TrashUri;

/// A command name and its `key=value` tokens, already split by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    name: String,
    arguments: Vec<(String, String)>,
}

impl Invocation {
    pub fn new(name: impl Into<String>, arguments: Vec<(String, String)>) -> Self {
        Self {
            name: name.into(),
            arguments,
        }
    }

    /// Build an invocation from tokens: the command name first, then
    /// `key=value` arguments. A value may contain `=`; the first `=` splits.
    /// Repeated keys are kept in order, so a `paths` argument can be a list.
    pub fn parse(tokens: &[String]) -> Result<Self, CommandError> {
        let Some(name) = tokens.first() else {
            return Err(CommandError::missing_command());
        };
        if name.is_empty() {
            return Err(CommandError::missing_command());
        }

        let mut arguments = Vec::new();
        for token in &tokens[1..] {
            let Some((key, value)) = token.split_once('=') else {
                return Err(CommandError::malformed_token(name, token));
            };
            if key.is_empty() {
                return Err(CommandError::empty_argument_name(name, token));
            }
            arguments.push((key.to_string(), value.to_string()));
        }

        Ok(Self {
            name: name.clone(),
            arguments,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn arguments(&self) -> &[(String, String)] {
        &self.arguments
    }
}

/// One checked argument, in catalogue order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedArgument {
    pub name: &'static str,
    pub value: Value,
}

/// A validated command, ready for a handler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedCommand {
    spec: &'static CommandSpec,
    arguments: Vec<CheckedArgument>,
}

impl CheckedCommand {
    pub fn spec(&self) -> &'static CommandSpec {
        self.spec
    }

    pub fn name(&self) -> &'static str {
        self.spec.name
    }

    pub fn effect(&self) -> Effect {
        self.spec.effect
    }

    pub fn undoable(&self) -> bool {
        self.spec.undo
    }

    pub fn arguments(&self) -> &[CheckedArgument] {
        &self.arguments
    }

    pub fn get(&self, name: &str) -> Option<&Value> {
        self.arguments
            .iter()
            .find(|argument| argument.name == name)
            .map(|argument| &argument.value)
    }

    /// Whether the caller asked for a plan instead of a change.
    pub fn dry_run(&self) -> bool {
        matches!(self.get("dry-run"), Some(Value::Bool(true)))
    }
}

impl fmt::Display for CheckedCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.spec.name)?;
        for argument in &self.arguments {
            match &argument.value {
                Value::Paths(paths) => {
                    for path in paths {
                        write!(f, " {}={}", argument.name, path.display())?;
                    }
                }
                Value::TrashItems(items) => {
                    for item in items {
                        write!(f, " {}={}", argument.name, item.as_str())?;
                    }
                }
                value => write!(f, " {}={}", argument.name, value)?,
            }
        }
        Ok(())
    }
}

/// The reason a command was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandErrorKind {
    MissingCommand,
    UnknownCommand {
        suggestion: Option<&'static str>,
    },
    UnknownArgument,
    MissingArgument,
    /// The argument may only come from context, and this caller may not use
    /// context for it.
    ForbiddenFromContext {
        caller: Caller,
    },
    /// A permanent Trash deletion is only an explicitly clicked UI action.
    ForbiddenCaller {
        caller: Caller,
    },
    DuplicateArgument,
    BadValue {
        value: String,
        reason: String,
    },
    OutOfRange {
        value: String,
        min: String,
        max: String,
    },
    OffStep {
        value: String,
        step: String,
    },
    MalformedToken {
        token: String,
    },
    EmptyArgumentName {
        token: String,
    },
}

/// A refusal from the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    command: Option<String>,
    argument: Option<String>,
    kind: CommandErrorKind,
}

impl CommandError {
    fn new(command: &str, argument: Option<&str>, kind: CommandErrorKind) -> Self {
        Self {
            command: Some(command.to_string()),
            argument: argument.map(str::to_string),
            kind,
        }
    }

    fn missing_command() -> Self {
        Self {
            command: None,
            argument: None,
            kind: CommandErrorKind::MissingCommand,
        }
    }

    fn malformed_token(command: &str, token: &str) -> Self {
        Self::new(
            command,
            None,
            CommandErrorKind::MalformedToken {
                token: token.to_string(),
            },
        )
    }

    fn empty_argument_name(command: &str, token: &str) -> Self {
        Self::new(
            command,
            None,
            CommandErrorKind::EmptyArgumentName {
                token: token.to_string(),
            },
        )
    }

    fn unknown_argument(command: &str, argument: &str) -> Self {
        Self::new(command, Some(argument), CommandErrorKind::UnknownArgument)
    }

    fn missing_argument(command: &str, argument: &str) -> Self {
        Self::new(command, Some(argument), CommandErrorKind::MissingArgument)
    }

    fn forbidden_context(command: &str, argument: &str, caller: Caller) -> Self {
        Self::new(
            command,
            Some(argument),
            CommandErrorKind::ForbiddenFromContext { caller },
        )
    }

    fn forbidden_caller(command: &str, caller: Caller) -> Self {
        Self::new(command, None, CommandErrorKind::ForbiddenCaller { caller })
    }

    fn duplicate_argument(command: &str, argument: &str) -> Self {
        Self::new(command, Some(argument), CommandErrorKind::DuplicateArgument)
    }

    fn bad_value(command: &str, argument: &str, value: &str, reason: impl Into<String>) -> Self {
        Self::new(
            command,
            Some(argument),
            CommandErrorKind::BadValue {
                value: value.to_string(),
                reason: reason.into(),
            },
        )
    }

    fn out_of_range(command: &str, argument: &str, value: &str, min: String, max: String) -> Self {
        Self::new(
            command,
            Some(argument),
            CommandErrorKind::OutOfRange {
                value: value.to_string(),
                min,
                max,
            },
        )
    }

    fn off_step(command: &str, argument: &str, value: &str, step: i32) -> Self {
        Self::new(
            command,
            Some(argument),
            CommandErrorKind::OffStep {
                value: value.to_string(),
                step: format_hundredths(step),
            },
        )
    }

    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    pub fn argument(&self) -> Option<&str> {
        self.argument.as_deref()
    }

    pub fn kind(&self) -> &CommandErrorKind {
        &self.kind
    }

    pub fn suggestion(&self) -> Option<&'static str> {
        match &self.kind {
            CommandErrorKind::UnknownCommand { suggestion } => *suggestion,
            _ => None,
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            CommandErrorKind::MissingCommand => f.write_str("missing command name"),
            CommandErrorKind::UnknownCommand { suggestion } => {
                match &self.command {
                    Some(command) => write!(f, "unknown command `{command}`")?,
                    None => f.write_str("unknown command")?,
                }
                match suggestion {
                    Some(suggestion) => write!(f, "; did you mean `{suggestion}`?"),
                    None => Ok(()),
                }
            }
            CommandErrorKind::UnknownArgument => {
                match &self.command {
                    Some(command) => write!(f, "command `{command}`")?,
                    None => f.write_str("invocation")?,
                }
                match &self.argument {
                    Some(argument) => write!(f, ": unknown argument `{argument}`"),
                    None => f.write_str(": unknown argument"),
                }
            }
            _ => {
                match &self.command {
                    Some(command) => write!(f, "command `{command}`")?,
                    None => f.write_str("invocation")?,
                }
                if let Some(argument) = &self.argument {
                    write!(f, ": argument `{argument}`")?;
                }
                f.write_str(": ")?;
                match &self.kind {
                    CommandErrorKind::MissingArgument => f.write_str("missing"),
                    CommandErrorKind::ForbiddenFromContext { caller } => write!(
                        f,
                        "missing; the {caller} caller may not fill a Files path from context"
                    ),
                    CommandErrorKind::ForbiddenCaller { caller } => {
                        write!(f, "the {caller} caller may not run this command")
                    }
                    CommandErrorKind::DuplicateArgument => f.write_str("given more than once"),
                    CommandErrorKind::BadValue { value, reason } => {
                        write!(f, "bad value `{value}`: {reason}")
                    }
                    CommandErrorKind::OutOfRange { value, min, max } => {
                        write!(f, "value `{value}` is out of range {min}..{max}")
                    }
                    CommandErrorKind::OffStep { value, step } => {
                        write!(f, "value `{value}` is off the {step} step")
                    }
                    CommandErrorKind::MalformedToken { token } => {
                        write!(f, "malformed token `{token}`: expected key=value")
                    }
                    CommandErrorKind::EmptyArgumentName { token } => {
                        write!(f, "empty argument name in `{token}`")
                    }
                    CommandErrorKind::MissingCommand
                    | CommandErrorKind::UnknownCommand { .. }
                    | CommandErrorKind::UnknownArgument => unreachable!(),
                }
            }
        }
    }
}

impl std::error::Error for CommandError {}

pub(super) fn validate(
    registry: &Registry,
    caller: Caller,
    invocation: &Invocation,
) -> Result<CheckedCommand, CommandError> {
    let name = invocation.name();
    let Some(spec) = registry.command(name) else {
        return Err(CommandError::new(
            name,
            None,
            CommandErrorKind::UnknownCommand {
                suggestion: closest_command(name),
            },
        ));
    };

    let defined = spec.arguments();

    if !caller_allowed(spec.name, spec.effect, caller) {
        return Err(CommandError::forbidden_caller(name, caller));
    }

    for (key, _) in invocation.arguments() {
        if !defined.iter().any(|argument| argument.name == key) {
            return Err(CommandError::unknown_argument(name, key));
        }
    }

    for argument in &defined {
        let count = invocation
            .arguments()
            .iter()
            .filter(|(key, _)| key == argument.name)
            .count();
        if count > 1 && !matches!(argument.kind, ArgKind::Paths | ArgKind::TrashItems) {
            return Err(CommandError::duplicate_argument(name, argument.name));
        }
    }

    let mut checked = Vec::with_capacity(defined.len());
    for argument in defined {
        let mut supplied = invocation
            .arguments()
            .iter()
            .filter(|(key, _)| key == argument.name)
            .map(|(_, value)| value.as_str());

        let value = if matches!(argument.kind, ArgKind::Paths | ArgKind::TrashItems) {
            let values: Vec<&str> = supplied.collect();
            if values.is_empty() {
                None
            } else if argument.kind == ArgKind::TrashItems {
                let mut items = Vec::with_capacity(values.len());
                for value in values {
                    items.push(TrashUri::parse(value).map_err(|error| {
                        CommandError::bad_value(
                            name,
                            argument.name,
                            value,
                            format!("expected a trash item URI: {error}"),
                        )
                    })?);
                }
                Some(Value::TrashItems(items))
            } else {
                let mut paths = Vec::with_capacity(values.len());
                for value in values {
                    paths.push(parse_path(name, argument, value)?);
                }
                Some(Value::Paths(paths))
            }
        } else {
            match supplied.next() {
                Some(value) => Some(parse_value(name, argument, value)?),
                None => None,
            }
        };

        if let Some(value) = value {
            checked.push(CheckedArgument {
                name: argument.name,
                value,
            });
            continue;
        }

        if must_supply(caller, spec.effect, argument) {
            return Err(if argument.presence.is_context() {
                CommandError::forbidden_context(name, argument.name, caller)
            } else {
                CommandError::missing_argument(name, argument.name)
            });
        }

        if let Some(default) = argument.default {
            checked.push(CheckedArgument {
                name: argument.name,
                value: parse_value(name, argument, default)?,
            });
        }
    }

    Ok(CheckedCommand {
        spec,
        arguments: checked,
    })
}

fn parse_path(command: &str, argument: &ArgSpec, text: &str) -> Result<PathBuf, CommandError> {
    if text.is_empty() || !Path::new(text).is_absolute() {
        Err(CommandError::bad_value(
            command,
            argument.name,
            text,
            "must be an absolute path",
        ))
    } else {
        Ok(PathBuf::from(text))
    }
}

fn parse_value(command: &str, argument: &ArgSpec, text: &str) -> Result<Value, CommandError> {
    match argument.kind {
        ArgKind::Path => parse_path(command, argument, text).map(Value::Path),
        ArgKind::Paths => parse_path(command, argument, text).map(|path| Value::Paths(vec![path])),
        ArgKind::TrashItems => TrashUri::parse(text)
            .map(|uri| Value::TrashItems(vec![uri]))
            .map_err(|error| {
                CommandError::bad_value(
                    command,
                    argument.name,
                    text,
                    format!("expected a trash item URI: {error}"),
                )
            }),
        ArgKind::Int { min, max } => {
            let value: i64 = text.parse().map_err(|_| {
                CommandError::bad_value(command, argument.name, text, "expected an integer")
            })?;
            if (min..=max).contains(&value) {
                Ok(Value::Int(value))
            } else {
                Err(CommandError::out_of_range(
                    command,
                    argument.name,
                    text,
                    min.to_string(),
                    max.to_string(),
                ))
            }
        }
        ArgKind::Number { min, max, step } => {
            let hundredths = parse_hundredths(text)
                .map_err(|reason| CommandError::bad_value(command, argument.name, text, reason))?;
            if !(min..=max).contains(&hundredths) {
                return Err(CommandError::out_of_range(
                    command,
                    argument.name,
                    text,
                    format_hundredths(min),
                    format_hundredths(max),
                ));
            }
            if let Some(step) = step
                && (step <= 0 || (hundredths - min).rem_euclid(step) != 0)
            {
                return Err(CommandError::off_step(command, argument.name, text, step));
            }
            Ok(Value::Number(Number::from_hundredths(hundredths)))
        }
        ArgKind::Bool => match text {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(CommandError::bad_value(
                command,
                argument.name,
                text,
                "expected `true` or `false`",
            )),
        },
        ArgKind::Text => {
            if text.is_empty() {
                Err(CommandError::bad_value(
                    command,
                    argument.name,
                    text,
                    "must not be empty",
                ))
            } else {
                Ok(Value::Text(text.to_string()))
            }
        }
        ArgKind::Choice(values) => match values.iter().copied().find(|value| *value == text) {
            Some(value) => Ok(Value::Choice(value)),
            None => Err(CommandError::bad_value(
                command,
                argument.name,
                text,
                format!("expected one of: {}", values.join(", ")),
            )),
        },
    }
}

/// Parse a decimal into hundredths without touching floats. At most two
/// decimal places are accepted.
fn parse_hundredths(text: &str) -> Result<i32, String> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    if rest.is_empty() {
        return Err("expected a number".to_string());
    }

    let (whole, fraction) = match rest.split_once('.') {
        Some((whole, fraction)) => (whole, Some(fraction)),
        None => (rest, None),
    };
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected a number".to_string());
    }

    let fraction = fraction.unwrap_or("");
    if fraction.is_empty() && rest.contains('.') {
        return Err("expected a number".to_string());
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("expected a number".to_string());
    }
    if fraction.len() > 2 {
        return Err("expected at most two decimal places".to_string());
    }

    let whole: i32 = whole
        .parse()
        .map_err(|_| "the number is too large".to_string())?;
    let fraction: i32 = match fraction.len() {
        0 => 0,
        1 => {
            fraction
                .parse::<i32>()
                .map_err(|_| "expected a number".to_string())?
                * 10
        }
        2 => fraction
            .parse()
            .map_err(|_| "expected a number".to_string())?,
        _ => return Err("expected at most two decimal places".to_string()),
    };

    let hundredths = whole
        .checked_mul(100)
        .and_then(|value| value.checked_add(fraction))
        .ok_or_else(|| "the number is too large".to_string())?;

    Ok(if negative { -hundredths } else { hundredths })
}

/// The closest catalogue name within edit distance 2, or `None`.
fn closest_command(name: &str) -> Option<&'static str> {
    let mut best: Option<(usize, &'static str)> = None;
    for spec in super::COMMANDS {
        let distance = edit_distance(name, spec.name);
        if distance > 2 {
            continue;
        }
        match best {
            Some((best_distance, _)) if best_distance <= distance => {}
            _ => best = Some((distance, spec.name)),
        }
    }
    best.map(|(_, name)| name)
}

fn edit_distance(left: &str, right: &str) -> usize {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();

    let mut previous: Vec<usize> = (0..=right.len()).collect();
    let mut current = vec![0; right.len() + 1];

    for (row, left_char) in left.iter().enumerate() {
        current[0] = row + 1;
        for (column, right_char) in right.iter().enumerate() {
            let substitution = if left_char == right_char { 0 } else { 1 };
            current[column + 1] = (previous[column + 1] + 1)
                .min(current[column] + 1)
                .min(previous[column] + substitution);
        }
        std::mem::swap(&mut previous, &mut current);
    }

    previous[right.len()]
}
