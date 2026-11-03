// SPDX-License-Identifier: GPL-3.0-or-later
//! The command registry: one catalogue of named actions, their arguments,
//! the rules every caller follows, and one dispatcher.
//!
//! Nothing here performs an action. Menus, hotkeys, the agent socket and the
//! CLI build an [`Invocation`] and call [`Registry::dispatch`] or
//! [`Registry::validate`]; handlers, which come with the features, do the
//! work. The catalogue in [`catalogue`] is the only place command names are
//! listed.

mod catalogue;
mod check;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;

use crate::fileops::TrashUri;
pub use catalogue::COMMANDS;
pub use check::{CheckedArgument, CheckedCommand, CommandError, CommandErrorKind, Invocation};

/// Something that can call a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// A menu, a button or the path bar.
    Ui,
    /// A hotkey from the config.
    Hotkey,
    /// `gimbal-files-cli`.
    Cli,
    /// The local agent socket.
    Agent,
}

impl Caller {
    pub fn as_str(self) -> &'static str {
        match self {
            Caller::Ui => "Ui",
            Caller::Hotkey => "Hotkey",
            Caller::Cli => "Cli",
            Caller::Agent => "Agent",
        }
    }
}

impl fmt::Display for Caller {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a command changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// UI state only.
    View,
    /// Writes the config.
    Settings,
    /// Changes files.
    Files,
    /// Starts another program.
    Launch,
    /// Asks the system, for example a mount that polkit may prompt for.
    System,
}

impl Effect {
    pub fn as_str(self) -> &'static str {
        match self {
            Effect::View => "view",
            Effect::Settings => "settings",
            Effect::Files => "files",
            Effect::Launch => "launch",
            Effect::System => "system",
        }
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where an omitted argument can be filled in from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSource {
    /// The current selection in the focused view.
    Selection,
    /// The folder the focused tab shows.
    CurrentFolder,
    /// The focused tab itself.
    FocusedTab,
}

impl ContextSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ContextSource::Selection => "selection",
            ContextSource::CurrentFolder => "current folder",
            ContextSource::FocusedTab => "focused tab",
        }
    }
}

/// Whether an argument is required, optional, or optional and filled from
/// context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Required,
    Optional,
    /// Optional for the UI and hotkeys; the handler fills it from the UI state
    /// when it is omitted.
    Context(ContextSource),
}

impl Presence {
    pub fn is_context(self) -> bool {
        matches!(self, Presence::Context(_))
    }
}

/// The type and bounds of an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    /// One absolute path. The registry never expands `~` or touches the
    /// filesystem.
    Path,
    /// One or more absolute paths; the invocation repeats the key.
    Paths,
    /// One or more validated GIO trash item URIs; the invocation repeats the
    /// key. The root, foreign schemes and traversal are refused.
    TrashItems,
    /// An integer in an inclusive range.
    Int {
        min: i64,
        max: i64,
    },
    /// A number in hundredths, so comparisons never use floats. `step` is
    /// also in hundredths, measured from `min`.
    Number {
        min: i32,
        max: i32,
        step: Option<i32>,
    },
    Bool,
    /// A non-empty string.
    Text,
    /// One of a fixed list, matched exactly.
    Choice(&'static [&'static str]),
}

impl ArgKind {
    /// A short human-readable description, used by `gimbal-files-cli
    /// commands`.
    pub fn label(&self) -> String {
        match self {
            ArgKind::Path => "path".to_string(),
            ArgKind::Paths => "paths".to_string(),
            ArgKind::TrashItems => "trash item URIs".to_string(),
            ArgKind::Int { min, max } => {
                if *max == i64::MAX {
                    format!("int, {min} or more")
                } else {
                    format!("int, {min}..={max}")
                }
            }
            ArgKind::Number { min, max, step } => {
                let mut label = format!(
                    "number, {}..{}",
                    format_hundredths(*min),
                    format_hundredths(*max)
                );
                if let Some(step) = step {
                    label.push_str(&format!(", step {}", format_hundredths(*step)));
                }
                label
            }
            ArgKind::Bool => "bool".to_string(),
            ArgKind::Text => "text".to_string(),
            ArgKind::Choice(values) => values.join("|"),
        }
    }
}

/// One argument a command takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArgSpec {
    pub name: &'static str,
    pub kind: ArgKind,
    pub presence: Presence,
    /// The value used when an optional argument is omitted. `None` means the
    /// handler decides.
    pub default: Option<&'static str>,
}

impl ArgSpec {
    /// How the argument reads in `gimbal-files-cli commands` and in errors.
    pub fn label(&self) -> String {
        let mut label = self.name.to_string();
        if self.presence != Presence::Required {
            label.push('?');
        }
        label.push_str(" (");
        label.push_str(&self.kind.label());
        if let Presence::Context(source) = self.presence {
            label.push_str(", from ");
            label.push_str(source.as_str());
        }
        if let Some(default) = self.default {
            label.push_str("; default ");
            label.push_str(default);
        }
        label.push(')');
        label
    }
}

/// One command in the catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub summary: &'static str,
    pub effect: Effect,
    pub undo: bool,
    /// The declared arguments. `dry-run` is not here: the registry adds it to
    /// every [`Effect::Files`] command in [`CommandSpec::arguments`].
    arguments: &'static [ArgSpec],
}

impl CommandSpec {
    /// The arguments this command really takes, including the `dry-run` bool
    /// the registry adds to every `Files` command.
    pub fn arguments(&self) -> Vec<&'static ArgSpec> {
        let mut arguments: Vec<&'static ArgSpec> = self.arguments.iter().collect();
        if self.effect == Effect::Files {
            arguments.push(&DRY_RUN_ARG);
        }
        arguments
    }

    /// The `dry-run` argument every `Files` command gets.
    pub fn dry_run_argument(&self) -> Option<&'static ArgSpec> {
        if self.effect == Effect::Files {
            Some(&DRY_RUN_ARG)
        } else {
            None
        }
    }

    pub fn argument(&self, name: &str) -> Option<&'static ArgSpec> {
        self.arguments().into_iter().find(|arg| arg.name == name)
    }

    /// The command's name followed by its arguments, in catalogue order.
    pub fn usage(&self) -> String {
        let arguments = self.arguments();
        if arguments.is_empty() {
            self.name.to_string()
        } else {
            let labels: Vec<String> = arguments.iter().map(|arg| arg.label()).collect();
            format!("{} {}", self.name, labels.join(" "))
        }
    }
}

/// The bool the registry adds to every `Files` command. It defaults to false,
/// so an ordinary call still acts; `dry-run=true` asks the handler for a plan
/// instead.
pub static DRY_RUN_ARG: ArgSpec = ArgSpec {
    name: "dry-run",
    kind: ArgKind::Bool,
    presence: Presence::Optional,
    default: Some("false"),
};

/// Whether this caller may leave a context-filled argument for the handler to
/// fill from the UI state.
///
/// `Cli` and `Agent` may not do this for a `Files` command: an agent that
/// calls `file.trash` with no `paths` must get an error, never Kyle's
/// selection.
pub fn may_use_context(caller: Caller, effect: Effect, argument: &ArgSpec) -> bool {
    if !argument.presence.is_context() {
        return false;
    }

    !(matches!(caller, Caller::Cli | Caller::Agent) && effect == Effect::Files)
}

/// Whether this caller must supply the argument itself. Required arguments
/// always are; a context-filled argument is required for `Cli` and `Agent`
/// on a `Files` command.
pub fn must_supply(caller: Caller, effect: Effect, argument: &ArgSpec) -> bool {
    match argument.presence {
        Presence::Required => true,
        Presence::Optional => false,
        Presence::Context(_) => !may_use_context(caller, effect, argument),
    }
}

/// The permanent-delete commands Kyle approved. Both are explicitly clicked
/// UI operations: hotkeys, the CLI and the agent are refused before a handler,
/// including a dry run, can see them. This is the only caller exception.
pub const UI_ONLY_PERMANENT_COMMANDS: &[&str] = &["trash.empty", "trash.delete-permanently"];

/// The existing preview commands operate on the UI's active editor context.
/// Until a revision-aware editing API exists, only the UI and configured
/// hotkeys may call them; the CLI and agent are refused before any handler.
pub const PREVIEW_EDITING_COMMANDS: &[&str] = &[
    "preview.edit",
    "preview.discard",
    "preview.save",
    "preview.save-as",
];

/// The one user-only control that is not a file effect: enabling or disabling
/// the local automation socket. It never comes from a hotkey, the CLI or the
/// agent, even when the value is unchanged.
pub const UI_ONLY_SETTINGS_COMMANDS: &[&str] = &["agent.set-enabled"];

/// Whether this caller may run this command at all. The two approved permanent
/// deletions are explicitly clicked UI operations; the preview commands may
/// also come from a hotkey. `agent.set-enabled` is only the Settings switch.
pub fn caller_allowed(name: &str, effect: Effect, caller: Caller) -> bool {
    if UI_ONLY_SETTINGS_COMMANDS.contains(&name) {
        return caller == Caller::Ui;
    }
    if PREVIEW_EDITING_COMMANDS.contains(&name) {
        return matches!(caller, Caller::Ui | Caller::Hotkey);
    }
    !(effect == Effect::Files && UI_ONLY_PERMANENT_COMMANDS.contains(&name)) || caller == Caller::Ui
}

/// Whether the name is `area.verb` in lowercase kebab case.
pub fn is_well_formed_name(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    if parts.len() != 2 {
        return false;
    }

    parts.iter().all(|part| {
        part.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && !part.ends_with('-')
            && !part.contains("--")
            && part
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    })
}

/// Whether a command would delete permanently. The catalogue test allows only
/// the two reviewed Trash commands and refuses every other entry for which this
/// is true. `pins.remove` is a `Settings` command, so it is not a file delete.
pub fn permanently_deletes(name: &str, effect: Effect) -> bool {
    if effect != Effect::Files {
        return false;
    }

    let mut parts = name.split('.');
    let area = parts.next().unwrap_or("");
    let verb = parts.next().unwrap_or("");

    (area == "trash" && matches!(verb, "empty" | "delete-permanently"))
        || matches!(
            verb,
            "delete" | "remove" | "purge" | "erase" | "wipe" | "destroy" | "unlink" | "shred"
        )
}

/// Format hundredths as a decimal with two places, without floating point.
pub fn format_hundredths(value: i32) -> String {
    let sign = if value < 0 { "-" } else { "" };
    let absolute = value.unsigned_abs();
    format!("{sign}{}.{:02}", absolute / 100, absolute % 100)
}

/// A number kept as integer hundredths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Number {
    hundredths: i32,
}

impl Number {
    pub const fn from_hundredths(hundredths: i32) -> Self {
        Self { hundredths }
    }

    pub const fn hundredths(self) -> i32 {
        self.hundredths
    }

    pub fn as_f64(self) -> f64 {
        f64::from(self.hundredths) / 100.0
    }
}

impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_hundredths(self.hundredths))
    }
}

/// The checked value of one argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Path(PathBuf),
    Paths(Vec<PathBuf>),
    TrashItems(Vec<TrashUri>),
    Int(i64),
    Number(Number),
    Bool(bool),
    Text(String),
    Choice(&'static str),
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Path(path) => write!(f, "{}", path.display()),
            Value::Paths(paths) => {
                let rendered: Vec<String> = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                f.write_str(&rendered.join(" "))
            }
            Value::TrashItems(items) => {
                let rendered: Vec<&str> = items.iter().map(TrashUri::as_str).collect();
                f.write_str(&rendered.join(" "))
            }
            Value::Int(value) => write!(f, "{value}"),
            Value::Number(value) => write!(f, "{value}"),
            Value::Bool(value) => write!(f, "{value}"),
            Value::Text(value) => f.write_str(value),
            Value::Choice(value) => f.write_str(value),
        }
    }
}

/// The outcome of a dispatch: the handler's result, or the registry's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The handler did the work.
    Done,
    /// A dry run: the lines of the plan, with nothing changed.
    DryRun(Vec<String>),
    /// The command was unknown, invalid, or forbidden for this caller.
    Error(CommandError),
    /// The command was accepted but the real handler or its effect failed.
    /// Unlike `Done`, this must never be reported as success.
    Failed(String),
}

/// What a handler does with a checked command.
///
/// Context filling happens here: the handler knows the UI state. The registry
/// only says which arguments may be filled.
pub trait Handler {
    fn handle(&mut self, command: &CheckedCommand) -> Outcome;
}

/// The one dispatcher. It validates, applies the caller rules, then calls the
/// handler with the checked command.
#[derive(Debug, Default, Clone, Copy)]
pub struct Registry;

impl Registry {
    pub const fn new() -> Self {
        Self
    }

    /// Every command, in catalogue order.
    pub fn commands(&self) -> &'static [CommandSpec] {
        COMMANDS
    }

    pub fn command(&self, name: &str) -> Option<&'static CommandSpec> {
        COMMANDS.iter().find(|spec| spec.name == name)
    }

    /// Validate an invocation for a caller without running it.
    pub fn validate(
        &self,
        caller: Caller,
        invocation: &Invocation,
    ) -> Result<CheckedCommand, CommandError> {
        check::validate(self, caller, invocation)
    }

    /// Validate, apply the caller rules, then hand the command to `handler`.
    /// A refused command never reaches the handler.
    pub fn dispatch<H: Handler>(
        &self,
        caller: Caller,
        invocation: &Invocation,
        handler: &mut H,
    ) -> Outcome {
        match self.validate(caller, invocation) {
            Ok(command) => handler.handle(&command),
            Err(error) => Outcome::Error(error),
        }
    }
}
