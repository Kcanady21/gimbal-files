// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure progress, summary and failure-detail text.
//!
//! The progress row, the status line and the failure dialog all read from
//! these functions, so their wording and the half-second rule are unit-tested
//! without a display.

use std::path::Path;
use std::time::Duration;

use fm_core::fileops::{Action, ItemOutcome, Plan, Progress, Report, Request};

/// How long a finished operation's summary owns the status line.
pub const SUMMARY_MS: u64 = 4000;

/// The row only appears once an operation has run this long, so a one-item
/// trash does not flash it.
pub const APPEAR_AFTER_MS: u64 = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpKind {
    Trash,
    Copy,
    Move,
    Rename,
    NewFolder,
    Undo,
}

impl OpKind {
    pub fn from_request(request: &Request) -> Self {
        match request {
            Request::Trash { .. } => OpKind::Trash,
            Request::Copy { .. } => OpKind::Copy,
            Request::Move { .. } => OpKind::Move,
            Request::Rename { .. } => OpKind::Rename,
            Request::NewFolder { .. } => OpKind::NewFolder,
            Request::Undo => OpKind::Undo,
        }
    }

    pub fn verb(self) -> &'static str {
        match self {
            OpKind::Trash => "Trashing",
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Rename => "Renaming",
            OpKind::NewFolder => "Creating a folder",
            OpKind::Undo => "Undoing",
        }
    }
}

/// The progress row's text and bar.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressView {
    pub label: String,
    pub fraction: f64,
    pub waiting: usize,
}

impl ProgressView {
    pub fn new(kind: OpKind, progress: &Progress, waiting: usize) -> Self {
        Self {
            label: label(kind, progress.items_done, progress.items_total, waiting),
            fraction: fraction(progress),
            waiting,
        }
    }
}

/// "Copying 3 of 120", plus "— 1 waiting" when another request is queued.
pub fn label(kind: OpKind, done: usize, total: usize, waiting: usize) -> String {
    let mut text = format!("{} {done} of {total}", kind.verb());
    if waiting > 0 {
        text.push_str(&format!(" — {waiting} waiting"));
    }
    text
}

/// A byte-driven bar when GIO reported a total, an item-driven one otherwise.
pub fn fraction(progress: &Progress) -> f64 {
    let value = match progress.bytes_total {
        Some(total) if total > 0 => progress.bytes_done as f64 / total as f64,
        _ if progress.items_total > 0 => progress.items_done as f64 / progress.items_total as f64,
        _ => 0.0,
    };
    value.clamp(0.0, 1.0)
}

/// The row appears only after [`APPEAR_AFTER_MS`] has passed.
pub fn row_visible(elapsed: Duration) -> bool {
    elapsed >= Duration::from_millis(APPEAR_AFTER_MS)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub done: usize,
    pub skipped: usize,
    pub refused: usize,
    pub failed: usize,
    pub cancelled: usize,
}

impl Counts {
    pub fn total(self) -> usize {
        self.done + self.skipped + self.refused + self.failed + self.cancelled
    }

    fn not_done(self) -> usize {
        self.total() - self.done
    }
}

pub fn counts(report: &Report) -> Counts {
    let mut counts = Counts::default();
    for item in &report.items {
        match item.outcome {
            ItemOutcome::Done => counts.done += 1,
            ItemOutcome::Skipped => counts.skipped += 1,
            ItemOutcome::Refused(_) => counts.refused += 1,
            ItemOutcome::Failed(_) => counts.failed += 1,
            ItemOutcome::Cancelled => counts.cancelled += 1,
        }
    }
    counts
}

fn items(count: usize) -> String {
    if count == 1 {
        "1 item".to_string()
    } else {
        format!("{count} items")
    }
}

fn tail(counts: Counts) -> String {
    let mut parts = Vec::new();
    for (count, label) in [
        (counts.failed, "failed"),
        (counts.refused, "refused"),
        (counts.skipped, "skipped"),
        (counts.cancelled, "cancelled"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {label}"));
        }
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(": {}", parts.join(", "))
    }
}

/// The short status-line summary for a completed request.
pub fn summary(request: &Request, report: &Report) -> String {
    if *request == Request::Undo {
        return "Undid the last file operation".to_string();
    }

    let counts = counts(report);
    let total = counts.total();
    if total == 0 {
        return "Nothing to do".to_string();
    }

    match request {
        Request::Trash { .. } => {
            if counts.not_done() == 0 {
                format!("Moved {} to the trash", items(counts.done))
            } else {
                format!(
                    "Moved {} of {} items to the trash{}",
                    counts.done,
                    total,
                    tail(counts)
                )
            }
        }
        Request::Copy { .. } => {
            if counts.not_done() == 0 {
                format!("Copied {}", items(counts.done))
            } else {
                format!("Copied {} of {}{}", counts.done, total, tail(counts))
            }
        }
        Request::Move { .. } => {
            if counts.not_done() == 0 {
                format!("Moved {}", items(counts.done))
            } else {
                format!("Moved {} of {}{}", counts.done, total, tail(counts))
            }
        }
        Request::Rename { .. } => {
            if counts.not_done() == 0 {
                "Renamed 1 item".to_string()
            } else {
                format!("Renamed 0 of 1{}", tail(counts))
            }
        }
        Request::NewFolder { .. } => {
            let name = report
                .items
                .first()
                .and_then(|item| item.target.as_deref())
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "folder".to_string());
            if counts.not_done() == 0 {
                format!("Created folder “{name}”")
            } else {
                format!("Created 0 of 1 folder{}", tail(counts))
            }
        }
        Request::Undo => "Undid the last file operation".to_string(),
    }
}

fn name_of(item: &fm_core::fileops::ItemResult) -> String {
    item.source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| item.source.display().to_string())
}

/// One line per item that did not succeed, for the details dialog.
pub fn failure_lines(report: &Report) -> Vec<String> {
    report
        .items
        .iter()
        .filter_map(|item| match &item.outcome {
            ItemOutcome::Failed(reason) | ItemOutcome::Refused(reason) => {
                Some(format!("{}: {reason}", item.source.display()))
            }
            _ => None,
        })
        .collect()
}

/// The undo record in words: what would be put back or taken away.
pub fn undo_summary(plan: &Plan) -> Option<String> {
    if plan.steps().is_empty() {
        return None;
    }

    let mut restore = 0;
    let mut trash = 0;
    let mut moves = 0;
    let mut renames = 0;
    for step in plan.steps() {
        match step.action {
            Action::Restore => restore += 1,
            Action::Trash => trash += 1,
            Action::Move => moves += 1,
            Action::Rename => renames += 1,
            Action::Copy | Action::NewFolder => {}
        }
    }

    let mut parts = Vec::new();
    if restore > 0 {
        parts.push(format!("restored {}", items(restore)));
    }
    if trash > 0 {
        parts.push(format!("trashed {}", items(trash)));
    }
    if moves > 0 {
        parts.push(format!("moved {} back", items(moves)));
    }
    if renames > 0 {
        parts.push("renamed 1 item back".to_string());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(", "))
    }
}

/// "Undid: restored 1 item; 2 not undone: a.txt (the target exists), ..."
pub fn undo_status(plan: &Plan, report: &Report) -> String {
    let base = format!(
        "Undid: {}",
        undo_summary(plan).unwrap_or_else(|| "nothing".to_string())
    );
    let problems: Vec<String> = report
        .items
        .iter()
        .filter_map(|item| match &item.outcome {
            ItemOutcome::Failed(reason) | ItemOutcome::Refused(reason) => {
                Some(format!("{} ({reason})", name_of(item)))
            }
            _ => None,
        })
        .collect();
    if problems.is_empty() {
        base
    } else {
        format!(
            "{base}; {} not undone: {}",
            problems.len(),
            problems.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::fileops::{ItemResult, Progress};

    fn report(outcomes: &[ItemOutcome]) -> Report {
        Report {
            cancelled: outcomes.contains(&ItemOutcome::Cancelled),
            items: outcomes
                .iter()
                .enumerate()
                .map(|(index, outcome)| ItemResult {
                    action: Action::Copy,
                    source: std::path::PathBuf::from(format!("/tmp/file{index}.txt")),
                    target: Some(std::path::PathBuf::from(format!(
                        "/tmp/dst/file{index}.txt"
                    ))),
                    outcome: outcome.clone(),
                })
                .collect(),
        }
    }

    #[test]
    fn the_row_waits_half_a_second_and_then_shows_the_operation() {
        assert!(!row_visible(Duration::from_millis(0)));
        assert!(!row_visible(Duration::from_millis(499)));
        assert!(row_visible(Duration::from_millis(500)));
        assert!(row_visible(Duration::from_secs(5)));

        let progress = Progress {
            items_done: 3,
            items_total: 120,
            bytes_done: 0,
            bytes_total: None,
        };
        assert_eq!(
            ProgressView::new(OpKind::Copy, &progress, 0).label,
            "Copying 3 of 120"
        );
        assert_eq!(
            ProgressView::new(OpKind::Copy, &progress, 1).label,
            "Copying 3 of 120 — 1 waiting"
        );
        assert_eq!(
            ProgressView::new(OpKind::Trash, &progress, 2).label,
            "Trashing 3 of 120 — 2 waiting"
        );
    }

    #[test]
    fn the_bar_prefers_bytes_and_falls_back_to_items() {
        let bytes = Progress {
            items_done: 1,
            items_total: 4,
            bytes_done: 25,
            bytes_total: Some(100),
        };
        assert_eq!(fraction(&bytes), 0.25);
        let items = Progress {
            items_done: 1,
            items_total: 4,
            bytes_done: 99,
            bytes_total: None,
        };
        assert_eq!(fraction(&items), 0.25);
        let zero = Progress {
            items_done: 0,
            items_total: 0,
            bytes_done: 0,
            bytes_total: None,
        };
        assert_eq!(fraction(&zero), 0.0);
        let over = Progress {
            items_done: 9,
            items_total: 4,
            bytes_done: 0,
            bytes_total: None,
        };
        assert_eq!(fraction(&over), 1.0);
    }

    #[test]
    fn summaries_cover_every_outcome_mix() {
        let trash = Request::Trash { paths: Vec::new() };
        assert_eq!(
            summary(
                &trash,
                &report(&[ItemOutcome::Done, ItemOutcome::Done, ItemOutcome::Done])
            ),
            "Moved 3 items to the trash"
        );
        assert_eq!(
            summary(&trash, &report(&[ItemOutcome::Done])),
            "Moved 1 item to the trash"
        );

        let copy = Request::Copy {
            paths: Vec::new(),
            to: std::path::PathBuf::from("/tmp/dst"),
        };
        let outcomes = [
            ItemOutcome::Done,
            ItemOutcome::Done,
            ItemOutcome::Failed("no space".into()),
            ItemOutcome::Skipped,
        ];
        assert_eq!(
            summary(&copy, &report(&outcomes)),
            "Copied 2 of 4: 1 failed, 1 skipped"
        );

        let mixed = [
            ItemOutcome::Done,
            ItemOutcome::Refused("the target exists".into()),
            ItemOutcome::Cancelled,
        ];
        assert_eq!(
            summary(&copy, &report(&mixed)),
            "Copied 1 of 3: 1 refused, 1 cancelled"
        );
        assert_eq!(
            summary(&copy, &report(&[ItemOutcome::Done, ItemOutcome::Done])),
            "Copied 2 items"
        );

        let rename = Request::Rename {
            path: std::path::PathBuf::from("/tmp/a"),
            name: "b".into(),
        };
        assert_eq!(
            summary(&rename, &report(&[ItemOutcome::Done])),
            "Renamed 1 item"
        );
        assert_eq!(
            summary(
                &rename,
                &report(&[ItemOutcome::Failed("the name is taken".into())])
            ),
            "Renamed 0 of 1: 1 failed"
        );

        let folder = Request::NewFolder {
            name: "new".into(),
            in_folder: std::path::PathBuf::from("/tmp"),
        };
        let made = Report {
            cancelled: false,
            items: vec![ItemResult {
                action: Action::NewFolder,
                source: std::path::PathBuf::from("/tmp"),
                target: Some(std::path::PathBuf::from("/tmp/new")),
                outcome: ItemOutcome::Done,
            }],
        };
        assert_eq!(summary(&folder, &made), "Created folder “new”");
    }

    #[test]
    fn failures_list_the_path_and_the_message() {
        let report = report(&[
            ItemOutcome::Done,
            ItemOutcome::Failed("no space".into()),
            ItemOutcome::Refused("the target exists".into()),
            ItemOutcome::Skipped,
        ]);
        let lines = failure_lines(&report);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("file1.txt: no space"));
        assert!(lines[1].contains("file2.txt: the target exists"));
    }
}
