// SPDX-License-Identifier: GPL-3.0-or-later
//! File operations: trash, copy, move, rename, new folder, and undo.
//!
//! Nothing here ever deletes a user's file. Removing anything goes through
//! GIO's trash, and undoing a copy trashes the copies. The module is pure
//! planning plus blocking GIO calls, so a UI can run [`FileOperations::execute`]
//! on a worker thread: it never needs the main loop.
//!
//! The flow is plan, decide, execute. [`FileOperations::plan`] inspects the
//! filesystem read-only and lists every item and conflict; the UI asks the
//! user about each conflict; [`FileOperations::execute`] then runs with every
//! decision already made and never calls back to ask.

pub mod trashinfo;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashSet, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gio::prelude::*;

use crate::command::{CheckedCommand, Outcome, Value};

/// The undo stack keeps at most this many records.
pub const MAX_UNDO: usize = 50;

/// Nautilus's `TRASH_TIME_EPSILON` (`nautilus-file-undo-operations.c:44`): a
/// `.trashinfo` deletion date within two seconds of the operation matches it.
const TRASH_TIME_EPSILON_SECS: i64 = 2;

/// The longest name the filesystem accepts, in bytes.
const MAX_NAME_BYTES: usize = 255;

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

/// One typed file operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Trash { paths: Vec<PathBuf> },
    Copy { paths: Vec<PathBuf>, to: PathBuf },
    Move { paths: Vec<PathBuf>, to: PathBuf },
    Rename { path: PathBuf, name: String },
    NewFolder { name: String, in_folder: PathBuf },
    Undo,
}

impl Request {
    /// Whether this request moves rather than copies.
    pub fn is_move(&self) -> bool {
        matches!(self, Request::Move { .. })
    }

    /// A one-line description for a dry-run plan.
    pub fn summary(&self) -> String {
        match self {
            Request::Trash { paths } => format!("trash {} item(s)", paths.len()),
            Request::Copy { paths, to } => {
                format!("copy {} item(s) to {}", paths.len(), to.display())
            }
            Request::Move { paths, to } => {
                format!("move {} item(s) to {}", paths.len(), to.display())
            }
            Request::Rename { path, name } => {
                format!("rename {} to {name}", path.display())
            }
            Request::NewFolder { name, in_folder } => {
                format!("create folder {name} in {}", in_folder.display())
            }
            Request::Undo => "undo the last file operation".to_string(),
        }
    }
}

/// A `Files` command whose context paths were not filled in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    MissingArgument(&'static str),
}

impl fmt::Display for RequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RequestError::MissingArgument(name) => {
                write!(f, "the handler did not fill the `{name}` argument")
            }
        }
    }
}

impl std::error::Error for RequestError {}

/// Map one checked `Files` command to its request.
///
/// `Ok(None)` means the command is not a file operation (for example
/// `preview.save`, which belongs to a later brief). A `Files` command whose
/// handler did not fill a context path is an error, never a guess.
pub fn request_for(command: &CheckedCommand) -> Result<Option<Request>, RequestError> {
    let paths = || match command.get("paths") {
        Some(Value::Paths(paths)) if !paths.is_empty() => Ok(paths.clone()),
        _ => Err(RequestError::MissingArgument("paths")),
    };
    let path = |name: &'static str| match command.get(name) {
        Some(Value::Path(path)) => Ok(path.clone()),
        _ => Err(RequestError::MissingArgument(name)),
    };
    let text = |name: &'static str| match command.get(name) {
        Some(Value::Text(text)) => Ok(text.clone()),
        _ => Err(RequestError::MissingArgument(name)),
    };

    let request = match command.name() {
        "file.trash" => Request::Trash { paths: paths()? },
        "file.copy" => Request::Copy {
            paths: paths()?,
            to: path("to")?,
        },
        "file.move" => Request::Move {
            paths: paths()?,
            to: path("to")?,
        },
        "file.rename" => Request::Rename {
            path: path("path")?,
            name: text("name")?,
        },
        "file.new-folder" => Request::NewFolder {
            name: text("name")?,
            in_folder: path("in")?,
        },
        "file.undo" => Request::Undo,
        _ => return Ok(None),
    };

    Ok(Some(request))
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

/// Whether `name` is one legal path component: not empty, no `/`, no NUL,
/// not `.` or `..`, and at most 255 bytes.
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
}

/// The `n`th `KeepBoth` name: `name (copy).ext`, then `name (copy 2).ext`.
fn copy_name(name: &OsStr, number: usize) -> OsString {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(name);
    let suffix = if number == 0 {
        " (copy)".to_string()
    } else {
        format!(" (copy {})", number + 1)
    };

    let mut result = stem.to_os_string();
    result.push(suffix);
    if let Some(extension) = path.extension() {
        result.push(".");
        result.push(extension);
    }
    result
}

/// The first free `KeepBoth` target for `target`, honouring names reserved by
/// earlier items in the same run.
fn free_copy_name(target: &Path, reserved: &HashSet<PathBuf>) -> Result<PathBuf, String> {
    let parent = target
        .parent()
        .ok_or_else(|| "the target has no folder".to_string())?;
    let name = target
        .file_name()
        .ok_or_else(|| "the target has no name".to_string())?;

    for number in 0..10_000 {
        let candidate = parent.join(copy_name(name, number));
        if fs::symlink_metadata(&candidate).is_err() && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
    }

    Err("no free name for the copy was found".to_string())
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

/// What a plan step does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Trash,
    Copy,
    Move,
    Rename,
    NewFolder,
    Restore,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Trash => "trash",
            Action::Copy => "copy",
            Action::Move => "move",
            Action::Rename => "rename",
            Action::NewFolder => "new-folder",
            Action::Restore => "restore",
        }
    }

    fn past(self) -> &'static str {
        match self {
            Action::Trash => "trashed",
            Action::Copy => "copied",
            Action::Move => "moved",
            Action::Rename => "renamed",
            Action::NewFolder => "created",
            Action::Restore => "restored",
        }
    }
}

impl fmt::Display for Action {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A problem a plan found for one item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    /// The copy's or move's target already exists; the menu decides.
    TargetExists,
    /// A folder is being copied or moved into itself. Always refused.
    FolderIntoItself,
    /// A move names the folder the item is already in. Always refused.
    AlreadyThere,
    /// The source is missing. Always refused.
    MissingSource,
    /// A rename or new-folder name is not a legal path component.
    InvalidName,
    /// A rename or new-folder target already exists. Always refused.
    NameTaken,
    /// The destination folder is missing or is not a folder.
    MissingDestination,
    /// An undo folder is no longer empty.
    NotEmpty,
}

impl Conflict {
    /// Whether the conflict is the one the menu can decide (`Skip`,
    /// `KeepBoth`, `Replace`, `Cancel`). Every other conflict is refused for
    /// its item and cannot be overridden.
    pub fn needs_decision(self) -> bool {
        self == Conflict::TargetExists
    }

    /// How the conflict reads in a plan and in a refusal.
    pub fn text(self) -> &'static str {
        match self {
            Conflict::TargetExists => "the target exists",
            Conflict::FolderIntoItself => "the folder is inside itself",
            Conflict::AlreadyThere => "the item is already in that folder",
            Conflict::MissingSource => "the source is missing",
            Conflict::InvalidName => "the name is invalid",
            Conflict::NameTaken => "the name is taken",
            Conflict::MissingDestination => "the destination folder is missing",
            Conflict::NotEmpty => "the folder is not empty",
        }
    }
}

/// One item in a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub action: Action,
    /// The item being acted on: the source of a copy or move, the file being
    /// renamed, the containing folder of a new folder, or the trash entry
    /// being restored.
    pub source: Option<PathBuf>,
    /// Where the item goes, when it goes anywhere.
    pub target: Option<PathBuf>,
    pub conflict: Option<Conflict>,
    /// A copy into its own folder always gets a `(copy)` name and is not a
    /// conflict.
    pub keep_both: bool,
}

/// A read-only plan for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    request: Request,
    steps: Vec<Step>,
    undo: Option<UndoRecord>,
}

impl Plan {
    pub fn request(&self) -> &Request {
        &self.request
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// The steps whose conflict needs a menu answer, in plan order.
    pub fn decisions(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|step| step.conflict == Some(Conflict::TargetExists))
    }

    /// The dry-run text the handler returns: one header and one line per
    /// step. Nothing is written by producing it.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("dry run: {}", self.request.summary())];
        if self.steps.is_empty() {
            lines.push("nothing to do".to_string());
        }
        for step in &self.steps {
            let mut line = step.action.as_str().to_string();
            if let Some(source) = &step.source {
                line.push(' ');
                line.push_str(&source.display().to_string());
            }
            if let Some(target) = &step.target {
                line.push_str(" -> ");
                line.push_str(&target.display().to_string());
            }
            if step.keep_both {
                line.push_str(" [(copy) name]");
            }
            if let Some(conflict) = step.conflict {
                line.push_str(" [");
                line.push_str(conflict.text());
                line.push(']');
            }
            lines.push(line);
        }
        lines
    }

    /// The dry-run outcome a command handler can return directly.
    pub fn dry_run_outcome(&self) -> Outcome {
        Outcome::DryRun(self.lines())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Leave the item out.
    Skip,
    /// Name the new item `name (copy).ext`, then `(copy 2)`, and so on.
    KeepBoth,
    /// Trash the existing target first, then do the operation.
    Replace,
    /// Run nothing at all.
    Cancel,
}

/// The menu's answers for the conflicts in one plan.
///
/// This is a pure value: the UI answers on the main thread, then hands the
/// finished `Decisions` to a worker thread.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decisions {
    items: Vec<PathBuf>,
    answers: BTreeMap<PathBuf, Decision>,
    cancelled: bool,
}

impl Decisions {
    /// An empty set of answers for these conflicting items.
    pub fn new(items: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut unique = Vec::new();
        for item in items {
            if !unique.contains(&item) {
                unique.push(item);
            }
        }
        Self {
            items: unique,
            answers: BTreeMap::new(),
            cancelled: false,
        }
    }

    /// The answers needed by a plan.
    pub fn from_plan(plan: &Plan) -> Self {
        Self::new(plan.decisions().filter_map(|step| step.source.clone()))
    }

    /// Record one answer. With `apply_to_all`, the decision also covers every
    /// conflict that has no answer yet; an earlier, different answer stays.
    pub fn answer(&mut self, item: &Path, decision: Decision, apply_to_all: bool) {
        if !self.items.iter().any(|candidate| candidate == item) {
            return;
        }

        self.answers.insert(item.to_path_buf(), decision);
        if decision == Decision::Cancel {
            self.cancelled = true;
            return;
        }

        if apply_to_all {
            for conflict in &self.items {
                self.answers.entry(conflict.clone()).or_insert(decision);
            }
        }
    }

    /// The next conflict the menu should show, or `None` when every item has
    /// an answer or the user cancelled.
    pub fn next_unanswered(&self) -> Option<&Path> {
        if self.cancelled {
            return None;
        }
        self.items
            .iter()
            .find(|item| !self.answers.contains_key(*item))
            .map(PathBuf::as_path)
    }

    pub fn decision_for(&self, item: &Path) -> Option<Decision> {
        self.answers.get(item).copied()
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Whether every conflict has an answer, or the user cancelled.
    pub fn is_complete(&self) -> bool {
        self.cancelled
            || self
                .items
                .iter()
                .all(|item| self.answers.contains_key(item))
    }
}

// ---------------------------------------------------------------------------
// Progress, reports and errors
// ---------------------------------------------------------------------------

/// One progress update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub items_done: usize,
    pub items_total: usize,
    /// Bytes copied so far, from GIO's own progress callback where it reports
    /// them, otherwise from the files' sizes.
    pub bytes_done: u64,
    pub bytes_total: Option<u64>,
}

/// The outcome of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemOutcome {
    Done,
    Skipped,
    Refused(String),
    Failed(String),
    Cancelled,
}

/// One line of a report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemResult {
    pub action: Action,
    pub source: PathBuf,
    pub target: Option<PathBuf>,
    pub outcome: ItemOutcome,
}

impl ItemResult {
    fn line(&self) -> String {
        let subject = match &self.target {
            Some(target) => format!("{} -> {}", self.source.display(), target.display()),
            None => self.source.display().to_string(),
        };
        match &self.outcome {
            ItemOutcome::Done => format!("{} {subject}", self.action.past()),
            ItemOutcome::Skipped => format!("skipped {subject}"),
            ItemOutcome::Refused(reason) => format!("refused {subject}: {reason}"),
            ItemOutcome::Failed(reason) => format!("failed {subject}: {reason}"),
            ItemOutcome::Cancelled => format!("cancelled {subject}"),
        }
    }
}

/// What one run actually did, item by item.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub cancelled: bool,
    pub items: Vec<ItemResult>,
}

impl Report {
    /// One line per item.
    pub fn lines(&self) -> Vec<String> {
        self.items.iter().map(ItemResult::line).collect()
    }

    /// Whether every item that was attempted succeeded.
    pub fn all_succeeded(&self) -> bool {
        !self.cancelled
            && self
                .items
                .iter()
                .all(|item| matches!(item.outcome, ItemOutcome::Done | ItemOutcome::Skipped))
    }
}

/// Why `execute` refused to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecuteError {
    /// Conflicts remain with no menu answer (the CLI and agents always get
    /// this when a conflict exists).
    UnansweredConflicts(Vec<PathBuf>),
    /// `file.undo` with an empty stack.
    NothingToUndo,
}

impl fmt::Display for ExecuteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecuteError::UnansweredConflicts(items) => {
                let names: Vec<String> = items
                    .iter()
                    .map(|item| item.display().to_string())
                    .collect();
                write!(
                    f,
                    "the operation has conflicts with no decision: {}",
                    names.join(", ")
                )
            }
            ExecuteError::NothingToUndo => f.write_str("there is nothing to undo"),
        }
    }
}

impl std::error::Error for ExecuteError {}

// ---------------------------------------------------------------------------
// Undo records
// ---------------------------------------------------------------------------

/// One item GIO put in the trash, and enough to find it again.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Trashed {
    original: PathBuf,
    at: SystemTime,
    /// The `files/<name>` path discovered just after trashing. The undo
    /// still verifies the `.trashinfo` when the path is gone.
    entry: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecordKind {
    Trash {
        items: Vec<Trashed>,
    },
    Copy {
        copies: Vec<PathBuf>,
        replaced: Vec<Trashed>,
    },
    Move {
        pairs: Vec<(PathBuf, PathBuf)>,
        replaced: Vec<Trashed>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    NewFolder {
        path: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct UndoRecord {
    kind: RecordKind,
}

// ---------------------------------------------------------------------------
// The operations object
// ---------------------------------------------------------------------------

/// Owns the in-memory undo stack (at most [`MAX_UNDO`] records).
#[derive(Debug, Default)]
pub struct FileOperations {
    undo: VecDeque<UndoRecord>,
}

impl FileOperations {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many records the undo stack holds.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Plan one request. Read-only: it never creates, trashes or moves
    /// anything. Undo plans look at the newest record.
    pub fn plan(&self, request: &Request) -> Plan {
        match request {
            Request::Trash { paths } => plan_trash(paths),
            Request::Copy { paths, to } => plan_copy_or_move(paths, to, false),
            Request::Move { paths, to } => plan_copy_or_move(paths, to, true),
            Request::Rename { path, name } => plan_rename(path, name),
            Request::NewFolder { name, in_folder } => plan_new_folder(name, in_folder),
            Request::Undo => plan_undo(self.undo.back()),
        }
    }

    /// The dry-run lines for a request.
    pub fn dry_run(&self, request: &Request) -> Vec<String> {
        self.plan(request).lines()
    }

    /// Run a plan on the calling thread.
    ///
    /// Every conflict must already have a decision. A cancelled run stops
    /// before the next item, so whatever finished stays and the undo record
    /// holds exactly that. A failed item does not stop the others.
    pub fn execute<P: FnMut(Progress)>(
        &mut self,
        plan: &Plan,
        decisions: &Decisions,
        cancellable: &gio::Cancellable,
        mut progress: P,
    ) -> Result<Report, ExecuteError> {
        if decisions.is_cancelled() {
            return Ok(Report {
                cancelled: true,
                items: plan.steps.iter().map(cancelled_result).collect(),
            });
        }

        let unanswered: Vec<PathBuf> = plan
            .decisions()
            .filter(|step| {
                step.source
                    .as_deref()
                    .is_none_or(|source| decisions.decision_for(source).is_none())
            })
            .filter_map(|step| step.source.clone())
            .collect();
        if !unanswered.is_empty() {
            return Err(ExecuteError::UnansweredConflicts(unanswered));
        }

        if plan.request == Request::Undo {
            if plan.undo.is_none() {
                return Err(ExecuteError::NothingToUndo);
            }
            return Ok(self.execute_undo(plan, cancellable, progress));
        }

        log_event(&format!("starting {}", plan.request.summary()));

        let mut reporter = Reporter {
            progress: &mut progress,
            items_total: plan.steps.len(),
            items_done: 0,
            bytes_done: 0,
            bytes_total: plan_bytes_total(plan),
        };
        let mut report = Report::default();
        let mut builder = RecordBuilder::default();

        if cancellable.is_cancelled() {
            report.cancelled = true;
            report.items.extend(plan.steps.iter().map(cancelled_result));
            return Ok(report);
        }

        // Names already claimed by items this run has reached. A planned
        // KeepBoth name is used when it is still free; otherwise the base
        // name is reused to find the next free one.
        let mut claimed: HashSet<PathBuf> = HashSet::new();

        for step in &plan.steps {
            if cancellable.is_cancelled() {
                report.cancelled = true;
                report.items.push(cancelled_result(step));
                continue;
            }

            let Some(source) = step.source.clone() else {
                report
                    .items
                    .push(failed_result(step, "the plan has no source"));
                reporter.item_finished();
                continue;
            };

            if step.action == Action::Trash {
                match trash_and_record(&source) {
                    Ok(trashed) => {
                        builder.trashed.push(trashed);
                        report.items.push(done_result(step, None));
                    }
                    Err(error) => report.items.push(failed_result(step, &error)),
                }
                reporter.item_finished();
                log_event(
                    &report
                        .items
                        .last()
                        .map(ItemResult::line)
                        .unwrap_or_default(),
                );
                continue;
            }

            let mut decision = None;
            if let Some(conflict) = step.conflict {
                if conflict.needs_decision() {
                    decision = decisions.decision_for(&source);
                    match decision {
                        Some(Decision::Skip) => {
                            report.items.push(result(step, None, ItemOutcome::Skipped));
                            reporter.item_finished();
                            continue;
                        }
                        Some(Decision::Cancel) | None => {
                            report.cancelled = true;
                            report.items.push(cancelled_result(step));
                            continue;
                        }
                        Some(Decision::KeepBoth | Decision::Replace) => {}
                    }
                } else {
                    report.items.push(result(
                        step,
                        None,
                        ItemOutcome::Refused(conflict.text().to_string()),
                    ));
                    reporter.item_finished();
                    continue;
                }
            }

            let Some(mut target) = step.target.clone() else {
                report
                    .items
                    .push(failed_result(step, "the plan has no target"));
                reporter.item_finished();
                continue;
            };

            if step.keep_both {
                let planned_is_free =
                    fs::symlink_metadata(&target).is_err() && !claimed.contains(&target);
                if !planned_is_free {
                    let parent = target.parent().unwrap_or_else(|| Path::new("/"));
                    let base = source
                        .file_name()
                        .map(|name| parent.join(name))
                        .unwrap_or_else(|| target.clone());
                    match free_copy_name(&base, &claimed) {
                        Ok(free) => target = free,
                        Err(error) => {
                            report.items.push(failed_result(step, &error));
                            reporter.item_finished();
                            continue;
                        }
                    }
                }
            } else if decision == Some(Decision::KeepBoth) {
                match free_copy_name(&target, &claimed) {
                    Ok(free) => target = free,
                    Err(error) => {
                        report.items.push(failed_result(step, &error));
                        reporter.item_finished();
                        continue;
                    }
                }
            }
            claimed.insert(target.clone());

            if decision == Some(Decision::Replace) && fs::symlink_metadata(&target).is_ok() {
                match trash_and_record(&target) {
                    Ok(trashed) => builder.replaced.push(trashed),
                    Err(error) => {
                        report.items.push(failed_result(
                            step,
                            &format!("could not trash the existing target: {error}"),
                        ));
                        reporter.item_finished();
                        continue;
                    }
                }
            }

            if fs::symlink_metadata(&source).is_err() {
                report.items.push(failed_result(step, "the source is gone"));
                reporter.item_finished();
                continue;
            }

            let outcome = match step.action {
                Action::Copy => copy_tree(&source, &target, &mut reporter).map(|()| {
                    builder.copies.push(target.clone());
                }),
                Action::Move => move_item(&source, &target, &mut reporter).map(|()| {
                    builder.move_pairs.push((source.clone(), target.clone()));
                }),
                Action::Rename => rename_item(&source, &target).map(|()| {
                    builder.rename = Some((source.clone(), target.clone()));
                }),
                Action::NewFolder => make_folder(&target).map(|()| {
                    builder.new_folder = Some(target.clone());
                }),
                Action::Trash | Action::Restore => {
                    Err("the plan has a step that cannot run here".to_string())
                }
            };

            match outcome {
                Ok(()) => report.items.push(done_result(step, Some(target))),
                Err(error) => {
                    report
                        .items
                        .push(result(step, Some(target), ItemOutcome::Failed(error)))
                }
            }

            reporter.item_finished();
            log_event(
                &report
                    .items
                    .last()
                    .map(ItemResult::line)
                    .unwrap_or_default(),
            );
        }

        if let Some(record) = builder.into_record(&plan.request) {
            self.push_undo(record);
        }

        log_event(&format!("finished {}", plan.request.summary()));
        Ok(report)
    }

    fn push_undo(&mut self, record: UndoRecord) {
        if self.undo.len() == MAX_UNDO {
            self.undo.pop_front();
        }
        self.undo.push_back(record);
    }

    fn execute_undo<P: FnMut(Progress)>(
        &mut self,
        plan: &Plan,
        cancellable: &gio::Cancellable,
        mut progress: P,
    ) -> Report {
        log_event("starting undo");
        let mut reporter = Reporter {
            progress: &mut progress,
            items_total: plan.steps.len(),
            items_done: 0,
            bytes_done: 0,
            bytes_total: plan_bytes_total(plan),
        };
        let mut report = Report::default();
        let new_folder_undo = matches!(
            plan.undo.as_ref().map(|record| &record.kind),
            Some(RecordKind::NewFolder { .. })
        );

        for step in &plan.steps {
            if cancellable.is_cancelled() {
                report.cancelled = true;
                report.items.push(cancelled_result(step));
                continue;
            }

            if let Some(conflict) = step.conflict {
                report.items.push(result(
                    step,
                    None,
                    ItemOutcome::Refused(conflict.text().to_string()),
                ));
                reporter.item_finished();
                continue;
            }

            let source = step.source.clone();
            let target = step.target.clone();

            let outcome: Result<Option<PathBuf>, String> = match step.action {
                Action::Restore => match (source, target) {
                    (Some(source), Some(target)) => {
                        restore_files(&source, &target).map(|()| Some(target))
                    }
                    _ => Err("the undo step is incomplete".to_string()),
                },
                Action::Trash => match source {
                    Some(source) => {
                        if new_folder_undo && !is_empty_dir(&source) {
                            Err("the folder is not empty".to_string())
                        } else {
                            trash_path(&source).map(|()| None)
                        }
                    }
                    None => Err("the undo step is incomplete".to_string()),
                },
                Action::Move => match (source, target) {
                    (Some(source), Some(target)) => {
                        move_item(&source, &target, &mut reporter).map(|()| Some(target))
                    }
                    _ => Err("the undo step is incomplete".to_string()),
                },
                Action::Rename => match (source, target) {
                    (Some(source), Some(target)) => {
                        rename_item(&source, &target).map(|()| Some(target))
                    }
                    _ => Err("the undo step is incomplete".to_string()),
                },
                _ => Err("the undo plan has an unsupported step".to_string()),
            };

            match outcome {
                Ok(target) => report.items.push(done_result(step, target)),
                Err(error) => report.items.push(result(
                    step,
                    step.target.clone(),
                    ItemOutcome::Failed(error),
                )),
            }
            reporter.item_finished();
            log_event(
                &report
                    .items
                    .last()
                    .map(ItemResult::line)
                    .unwrap_or_default(),
            );
        }

        // Undo cannot itself be undone, so the newest record is consumed even
        // when part of it was refused.
        self.undo.pop_back();
        log_event("finished undo");
        report
    }
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

fn unique_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(path) {
            unique.push(path.clone());
        }
    }
    unique
}

fn plan_trash(paths: &[PathBuf]) -> Plan {
    let steps = unique_paths(paths)
        .into_iter()
        .map(|path| Step {
            action: Action::Trash,
            conflict: fs::symlink_metadata(&path)
                .err()
                .map(|_| Conflict::MissingSource),
            source: Some(path),
            target: None,
            keep_both: false,
        })
        .collect();

    Plan {
        request: Request::Trash {
            paths: paths.to_vec(),
        },
        steps,
        undo: None,
    }
}

fn plan_copy_or_move(paths: &[PathBuf], to: &Path, is_move: bool) -> Plan {
    let destination_is_folder = to.is_dir();
    let mut reserved: HashSet<PathBuf> = HashSet::new();
    let mut steps = Vec::new();

    for source in unique_paths(paths) {
        let metadata = fs::symlink_metadata(&source).ok();
        let name = source.file_name().map(OsStr::to_os_string);
        let mut target = name
            .as_deref()
            .map(|name| to.join(name))
            .unwrap_or_else(|| to.to_path_buf());
        let mut conflict = None;
        let mut keep_both = false;

        if metadata.is_none() {
            conflict = Some(Conflict::MissingSource);
        } else if !destination_is_folder {
            conflict = Some(Conflict::MissingDestination);
        } else if let Some(metadata) = &metadata {
            let is_folder = metadata.is_dir() && !metadata.file_type().is_symlink();
            if is_folder && to.starts_with(&source) {
                conflict = Some(Conflict::FolderIntoItself);
            } else if source.parent() == Some(to) {
                if is_move {
                    conflict = Some(Conflict::AlreadyThere);
                } else {
                    keep_both = true;
                    if let Ok(free) = free_copy_name(&target, &reserved) {
                        target = free;
                    }
                }
            } else if fs::symlink_metadata(&target).is_ok() || reserved.contains(&target) {
                conflict = Some(Conflict::TargetExists);
            }
        }

        if conflict.is_none() {
            reserved.insert(target.clone());
        }

        steps.push(Step {
            action: if is_move { Action::Move } else { Action::Copy },
            source: Some(source),
            target: Some(target),
            conflict,
            keep_both,
        });
    }

    let request = if is_move {
        Request::Move {
            paths: paths.to_vec(),
            to: to.to_path_buf(),
        }
    } else {
        Request::Copy {
            paths: paths.to_vec(),
            to: to.to_path_buf(),
        }
    };

    Plan {
        request,
        steps,
        undo: None,
    }
}

fn plan_rename(path: &Path, name: &str) -> Plan {
    let parent = path.parent().unwrap_or_else(|| Path::new("/"));
    let target = parent.join(name);

    let conflict = if !is_valid_name(name) {
        Some(Conflict::InvalidName)
    } else if fs::symlink_metadata(path).is_err() {
        Some(Conflict::MissingSource)
    } else if target != path && fs::symlink_metadata(&target).is_ok() {
        Some(Conflict::NameTaken)
    } else {
        None
    };

    Plan {
        request: Request::Rename {
            path: path.to_path_buf(),
            name: name.to_string(),
        },
        steps: vec![Step {
            action: Action::Rename,
            source: Some(path.to_path_buf()),
            target: Some(target),
            conflict,
            keep_both: false,
        }],
        undo: None,
    }
}

fn plan_new_folder(name: &str, in_folder: &Path) -> Plan {
    let target = in_folder.join(name);

    let conflict = if !is_valid_name(name) {
        Some(Conflict::InvalidName)
    } else if !in_folder.is_dir() {
        Some(Conflict::MissingDestination)
    } else if fs::symlink_metadata(&target).is_ok() {
        Some(Conflict::NameTaken)
    } else {
        None
    };

    Plan {
        request: Request::NewFolder {
            name: name.to_string(),
            in_folder: in_folder.to_path_buf(),
        },
        steps: vec![Step {
            action: Action::NewFolder,
            source: Some(in_folder.to_path_buf()),
            target: Some(target),
            conflict,
            keep_both: false,
        }],
        undo: None,
    }
}

fn plan_undo(record: Option<&UndoRecord>) -> Plan {
    let mut steps = Vec::new();

    if let Some(record) = record {
        match &record.kind {
            RecordKind::Trash { items } => {
                let freed = HashSet::new();
                steps.extend(items.iter().map(|trashed| restore_step(trashed, &freed)));
            }
            RecordKind::Copy { copies, replaced } => {
                let mut freed = HashSet::new();
                for copy in copies {
                    let conflict = if fs::symlink_metadata(copy).is_ok() {
                        freed.insert(copy.clone());
                        None
                    } else {
                        Some(Conflict::MissingSource)
                    };
                    steps.push(Step {
                        action: Action::Trash,
                        source: Some(copy.clone()),
                        target: None,
                        conflict,
                        keep_both: false,
                    });
                }
                steps.extend(replaced.iter().map(|trashed| restore_step(trashed, &freed)));
            }
            RecordKind::Move { pairs, replaced } => {
                steps.extend(pairs.iter().map(|(source, target)| {
                    let conflict = if fs::symlink_metadata(target).is_err() {
                        Some(Conflict::MissingSource)
                    } else if fs::symlink_metadata(source).is_ok() {
                        Some(Conflict::TargetExists)
                    } else {
                        None
                    };
                    Step {
                        action: Action::Move,
                        source: Some(target.clone()),
                        target: Some(source.clone()),
                        conflict,
                        keep_both: false,
                    }
                }));
                steps.extend(
                    replaced
                        .iter()
                        .map(|trashed| restore_step(trashed, &HashSet::new())),
                );
            }
            RecordKind::Rename { from, to } => {
                let conflict = if fs::symlink_metadata(to).is_err() {
                    Some(Conflict::MissingSource)
                } else if fs::symlink_metadata(from).is_ok() {
                    Some(Conflict::TargetExists)
                } else {
                    None
                };
                steps.push(Step {
                    action: Action::Rename,
                    source: Some(to.clone()),
                    target: Some(from.clone()),
                    conflict,
                    keep_both: false,
                });
            }
            RecordKind::NewFolder { path } => {
                let conflict = if fs::symlink_metadata(path).is_err() {
                    Some(Conflict::MissingSource)
                } else if !is_empty_dir(path) {
                    Some(Conflict::NotEmpty)
                } else {
                    None
                };
                steps.push(Step {
                    action: Action::Trash,
                    source: Some(path.clone()),
                    target: None,
                    conflict,
                    keep_both: false,
                });
            }
        }
    }

    Plan {
        request: Request::Undo,
        steps,
        undo: record.cloned(),
    }
}

fn restore_step(trashed: &Trashed, freed: &HashSet<PathBuf>) -> Step {
    let entry = trashed
        .entry
        .clone()
        .filter(|files| fs::symlink_metadata(files).is_ok())
        .or_else(|| find_trash_entry(&trashed.original, trashed.at).map(|entry| entry.files));

    match entry {
        Some(files) => Step {
            action: Action::Restore,
            source: Some(files),
            target: Some(trashed.original.clone()),
            conflict: (fs::symlink_metadata(&trashed.original).is_ok()
                && !freed.contains(&trashed.original))
            .then_some(Conflict::TargetExists),
            keep_both: false,
        },
        None => Step {
            action: Action::Restore,
            source: None,
            target: Some(trashed.original.clone()),
            conflict: Some(Conflict::MissingSource),
            keep_both: false,
        },
    }
}

// ---------------------------------------------------------------------------
// Doing it through GIO
// ---------------------------------------------------------------------------

#[derive(Default)]
struct RecordBuilder {
    trashed: Vec<Trashed>,
    copies: Vec<PathBuf>,
    move_pairs: Vec<(PathBuf, PathBuf)>,
    replaced: Vec<Trashed>,
    rename: Option<(PathBuf, PathBuf)>,
    new_folder: Option<PathBuf>,
}

impl RecordBuilder {
    fn into_record(self, request: &Request) -> Option<UndoRecord> {
        let kind = match request {
            Request::Trash { .. } if !self.trashed.is_empty() => RecordKind::Trash {
                items: self.trashed,
            },
            Request::Copy { .. } if !self.copies.is_empty() || !self.replaced.is_empty() => {
                RecordKind::Copy {
                    copies: self.copies,
                    replaced: self.replaced,
                }
            }
            Request::Move { .. } if !self.move_pairs.is_empty() || !self.replaced.is_empty() => {
                RecordKind::Move {
                    pairs: self.move_pairs,
                    replaced: self.replaced,
                }
            }
            Request::Rename { .. } => {
                let (from, to) = self.rename?;
                RecordKind::Rename { from, to }
            }
            Request::NewFolder { .. } => RecordKind::NewFolder {
                path: self.new_folder?,
            },
            _ => return None,
        };

        Some(UndoRecord { kind })
    }
}

struct Reporter<'a> {
    progress: &'a mut dyn FnMut(Progress),
    items_total: usize,
    items_done: usize,
    bytes_done: u64,
    bytes_total: Option<u64>,
}

impl Reporter<'_> {
    fn report(&mut self) {
        (self.progress)(Progress {
            items_done: self.items_done,
            items_total: self.items_total,
            bytes_done: self.bytes_done,
            bytes_total: self.bytes_total,
        });
    }

    fn item_finished(&mut self) {
        self.items_done += 1;
        self.report();
    }
}

fn plan_bytes_total(plan: &Plan) -> Option<u64> {
    if !plan
        .steps
        .iter()
        .any(|step| matches!(step.action, Action::Copy | Action::Move))
    {
        return None;
    }

    let total: u64 = plan
        .steps
        .iter()
        .filter_map(|step| step.source.as_deref())
        .map(tree_size)
        .sum();
    (total > 0).then_some(total)
}

fn tree_size(path: &Path) -> u64 {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs::read_dir(path)
                .map(|entries| {
                    entries
                        .flatten()
                        .map(|entry| tree_size(&entry.path()))
                        .sum()
                })
                .unwrap_or(0)
        }
        Ok(metadata) => metadata.len(),
        Err(_) => 0,
    }
}

/// Copy a file, symlink or folder tree. A symlink stays a symlink.
fn copy_tree(src: &Path, dst: &Path, reporter: &mut Reporter<'_>) -> Result<(), String> {
    let metadata = fs::symlink_metadata(src).map_err(|error| error.to_string())?;

    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        let flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::ALL_METADATA;
        let mut last = 0u64;
        let mut report_progress = |current: i64, _total: i64| {
            let current = current.max(0) as u64;
            if current > last {
                reporter.bytes_done += current - last;
                last = current;
                reporter.report();
            }
        };
        let result = gio::File::for_path(src).copy(
            &gio::File::for_path(dst),
            flags,
            gio::Cancellable::NONE,
            Some(&mut report_progress),
        );

        result.map_err(|error| error.to_string())?;
        let size = metadata.len();
        if last < size {
            reporter.bytes_done += size - last;
        }
        return Ok(());
    }

    make_folder(dst)?;
    let entries = fs::read_dir(src).map_err(|error| error.to_string())?;
    for entry in entries {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry.file_name();
        copy_tree(&src.join(&name), &dst.join(&name), reporter)?;
    }

    // Nautilus makes the directory and copies its children; it does not run
    // g_file_copy_attributes() on the directory itself. GLib 2.8x rejects
    // ALL_METADATA there ("Setting attribute standard::size not supported").
    // Child files keep their metadata through G_FILE_COPY_ALL_METADATA.

    Ok(())
}

/// Move one item. Never falls back to GIO's copy-and-delete: across a
/// filesystem boundary the item is copied and its source is trashed.
fn move_item(src: &Path, dst: &Path, reporter: &mut Reporter<'_>) -> Result<(), String> {
    let flags = gio::FileCopyFlags::NOFOLLOW_SYMLINKS
        | gio::FileCopyFlags::ALL_METADATA
        | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE;

    match gio::File::for_path(src).move_(
        &gio::File::for_path(dst),
        flags,
        gio::Cancellable::NONE,
        None,
    ) {
        Ok(()) => Ok(()),
        Err(error) if crosses_filesystems(&error) => {
            copy_tree(src, dst, reporter)?;
            // If the source can't be trashed, both copies stay: a duplicate
            // is recoverable, a deletion isn't. Nothing here deletes.
            if let Err(error) = trash_path(src) {
                return Err(format!(
                    "copied to {} but could not trash the source, so both copies remain: {error}",
                    dst.display()
                ));
            }
            Ok(())
        }
        Err(error) => Err(error.to_string()),
    }
}

fn crosses_filesystems(error: &glib::Error) -> bool {
    error.matches(gio::IOErrorEnum::WouldRecurse) || error.matches(gio::IOErrorEnum::NotSupported)
}

fn make_folder(path: &Path) -> Result<(), String> {
    gio::File::for_path(path)
        .make_directory(gio::Cancellable::NONE)
        .map_err(|error| error.to_string())
}

fn rename_item(src: &Path, dst: &Path) -> Result<(), String> {
    let Some(name) = dst.file_name() else {
        return Err("the new name has no file name".to_string());
    };

    // g_file_set_display_name() takes UTF-8. A new name from a command always
    // is; an undo of a rename back to a non-UTF-8 original name is not, so it
    // moves the file instead.
    match name.to_str() {
        Some(name) => gio::File::for_path(src)
            .set_display_name(name, gio::Cancellable::NONE)
            .map(|_| ())
            .map_err(|error| error.to_string()),
        None => gio::File::for_path(src)
            .move_(
                &gio::File::for_path(dst),
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
                None,
            )
            .map_err(|error| error.to_string()),
    }
}

fn trash_path(path: &Path) -> Result<(), String> {
    gio::File::for_path(path)
        .trash(gio::Cancellable::NONE)
        .map_err(|error| error.to_string())
}

fn is_empty_dir(path: &Path) -> bool {
    fs::read_dir(path)
        .map(|mut entries| entries.next().is_none())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// The trash, read directly (Trash Specification 1.0, section 5 and 6)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct TrashEntry {
    files: PathBuf,
}

fn trash_and_record(path: &Path) -> Result<Trashed, String> {
    let at = SystemTime::now();
    trash_path(path)?;
    let entry = find_trash_entry(path, at).map(|entry| entry.files);
    Ok(Trashed {
        original: path.to_path_buf(),
        at,
        entry,
    })
}

/// Find the `.trashinfo` whose `Path` is `original` and whose
/// `DeletionDate` is within Nautilus's two-second epsilon of `at`.
fn find_trash_entry(original: &Path, at: SystemTime) -> Option<TrashEntry> {
    let wanted = unix_seconds(at);
    let mut best: Option<(SystemTime, TrashEntry)> = None;

    for trash in trash_dirs(original) {
        let info_dir = trash.join("info");
        let files_dir = trash.join("files");
        let Ok(entries) = fs::read_dir(&info_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let info_path = entry.path();
            if info_path.extension().and_then(OsStr::to_str) != Some("trashinfo") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&info_path) else {
                continue;
            };
            let Ok(parsed) = trashinfo::parse(&text) else {
                continue;
            };
            if parsed.path != original {
                continue;
            }
            let Some(deletion) = parsed.deletion_date.to_unix() else {
                continue;
            };
            if (deletion - wanted).abs() > TRASH_TIME_EPSILON_SECS {
                continue;
            }

            let Some(name) = info_path.file_stem() else {
                continue;
            };
            let files_path = files_dir.join(name);
            if fs::symlink_metadata(&files_path).is_err() {
                continue;
            }
            let modified = fs::metadata(&info_path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(UNIX_EPOCH);

            if best
                .as_ref()
                .is_none_or(|(best_time, _)| modified > *best_time)
            {
                best = Some((modified, TrashEntry { files: files_path }));
            }
        }
    }

    best.map(|(_, entry)| entry)
}

/// Move a trashed item back, without overwriting, then drop its `.trashinfo`.
fn restore_files(files: &Path, original: &Path) -> Result<(), String> {
    if fs::symlink_metadata(original).is_ok() {
        return Err("the original path is taken".to_string());
    }

    let info = trash_info_for(files)
        .ok_or_else(|| "the trash entry has no information file".to_string())?;

    fs::rename(files, original).map_err(|error| format!("could not move it back: {error}"))?;
    let _ = fs::remove_file(info);
    Ok(())
}

fn trash_info_for(files: &Path) -> Option<PathBuf> {
    let name = files.file_name()?;
    let trash = files.parent()?.parent()?;
    let mut info_name = name.to_os_string();
    info_name.push(".trashinfo");
    Some(trash.join("info").join(info_name))
}

/// The trash directories a path could have been trashed into: the home trash
/// and, for another mount, `$topdir/.Trash/$uid` or `$topdir/.Trash-$uid`.
fn trash_dirs(original: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![glib::user_data_dir().join("Trash")];
    let uid = current_uid();

    if let Some(top) = mount_top(original) {
        let shared = top.join(".Trash");
        if shared.is_dir()
            && fs::symlink_metadata(&shared).is_ok_and(|m| !m.file_type().is_symlink())
        {
            let user = shared.join(uid.to_string());
            if user.is_dir() {
                dirs.push(user);
            }
        }

        let per_user = top.join(format!(".Trash-{uid}"));
        if per_user.is_dir() {
            dirs.push(per_user);
        }
    }

    dirs
}

/// The top directory of the filesystem holding `path`.
fn mount_top(path: &Path) -> Option<PathBuf> {
    let probe = if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent()?.to_path_buf()
    };

    if let Ok(mount) = gio::File::for_path(&probe).find_enclosing_mount(gio::Cancellable::NONE)
        && let Some(root) = mount.root().path()
    {
        return Some(root);
    }

    // The local VFS is not always available; fall back to comparing device
    // numbers up the tree.
    let mut current = probe;
    let device = fs::metadata(&current).ok()?.dev();

    while let Some(parent) = current.parent() {
        match fs::metadata(parent) {
            Ok(metadata) if metadata.dev() == device => current = parent.to_path_buf(),
            _ => break,
        }
    }

    Some(current)
}

fn current_uid() -> u32 {
    fs::metadata("/proc/self")
        .ok()
        .map(|metadata| metadata.uid())
        .or_else(|| {
            fs::metadata(glib::home_dir())
                .ok()
                .map(|metadata| metadata.uid())
        })
        .unwrap_or(0)
}

fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Report helpers
// ---------------------------------------------------------------------------

fn result(step: &Step, target: Option<PathBuf>, outcome: ItemOutcome) -> ItemResult {
    ItemResult {
        action: step.action,
        source: step
            .source
            .clone()
            .or_else(|| step.target.clone())
            .unwrap_or_default(),
        target,
        outcome,
    }
}

fn done_result(step: &Step, target: Option<PathBuf>) -> ItemResult {
    result(step, target, ItemOutcome::Done)
}

fn failed_result(step: &Step, reason: &str) -> ItemResult {
    result(
        step,
        step.target.clone(),
        ItemOutcome::Failed(reason.to_string()),
    )
}

fn cancelled_result(step: &Step) -> ItemResult {
    result(step, step.target.clone(), ItemOutcome::Cancelled)
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

fn log_event(message: &str) {
    if !std::env::var("GIMBAL_LOG").is_ok_and(|value| value == "1") {
        return;
    }

    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    let elapsed = start.elapsed();
    eprintln!(
        "[gimbal {}.{:03}] fileops: {message}",
        elapsed.as_secs(),
        elapsed.subsec_millis()
    );
}
