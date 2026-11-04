// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive filename search: a GTK-free matcher and a cooperative walker.
//!
//! The walker matches basenames through exact/prefix/substring classes and,
//! for queries of at least three scalars, ordered-character subsequences and
//! bounded edit distance. It stays on the root filesystem device, never follows
//! child symlink directories, retains a bounded ranked set and reports partial
//! scans honestly. It performs no indexing, content search or cache write.

mod matcher;
mod walk;

#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use crate::folder_size::{DirReader, EntryKind, EntryMeta};

pub use matcher::{MatchClass, MatchScore, SearchQuery};
pub use walk::NativeSearchSource;

/// The most entries one scan visits, matching or not.
pub const MAX_VISITED_ENTRIES: u64 = 250_000;
/// The most best-ranked matches one scan retains for display.
pub const MAX_RESULTS: usize = 10_000;
/// One scan's cooperative elapsed-work budget.
pub const MAX_ELAPSED_MS: u64 = 30_000;
/// The deepest directory the walk descends to; the root is depth 0.
pub const MAX_DEPTH: usize = 128;
/// A result batch carries at most this many row changes.
pub const BATCH_ROWS: usize = 64;
/// At most this many error examples are retained per scan.
pub const MAX_EXAMPLES: usize = 5;
/// How often the walker publishes a progress snapshot.
const PROGRESS_EVERY: u64 = 256;

/// The initial internal scan limits. Tests may lower them; settings never do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SearchLimits {
    pub max_visited: u64,
    pub max_results: usize,
    pub max_elapsed_ms: u64,
    pub max_depth: usize,
}

impl Default for SearchLimits {
    fn default() -> Self {
        Self {
            max_visited: MAX_VISITED_ENTRIES,
            max_results: MAX_RESULTS,
            max_elapsed_ms: MAX_ELAPSED_MS,
            max_depth: MAX_DEPTH,
        }
    }
}

/// One submitted search. `root` is the user's folder; the walker resolves an
/// alias once on its worker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchRequest {
    pub root: PathBuf,
    pub query: SearchQuery,
    pub limits: SearchLimits,
}

impl SearchRequest {
    pub fn new(root: PathBuf, query: impl Into<String>) -> Self {
        Self {
            root,
            query: SearchQuery::new(query),
            limits: SearchLimits::default(),
        }
    }
}

/// One retained match. `path` is the exact filesystem path; display text is
/// derived later and never used for identity or activation. `score` is the
/// deterministic `(class, edits, gaps, start)` tuple used to rank rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub name: OsString,
    pub display_name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub modified_sec: Option<i64>,
    pub score: MatchScore,
}

/// One bounded change to the ranked result list. Indices refer to the retained
/// ranked vector at the moment the change is emitted; the UI applies batches in
/// order. `path` is the stable identity, never the row index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchChange {
    Insert { index: usize, row: SearchMatch },
    Remove { index: usize, path: PathBuf },
}

/// Why a scan stopped before covering everything it could see.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartialReason {
    EntryLimit,
    ResultLimit,
    TimeLimit,
    DepthLimit,
    PermissionDenied,
    Changed,
    Unreadable,
}

impl PartialReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::EntryLimit => "entry limit",
            Self::ResultLimit => "result limit",
            Self::TimeLimit => "time limit",
            Self::DepthLimit => "depth limit",
            Self::PermissionDenied => "permission denied",
            Self::Changed => "items changed during search",
            Self::Unreadable => "unreadable entry",
        }
    }
}

/// Why the root could not be searched at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RootFailure {
    Missing,
    PermissionDenied,
    NotDirectory,
    Unreadable,
    /// The user-selected root alias now resolves somewhere other than the
    /// folder the walk actually searched.
    Changed,
}

impl RootFailure {
    pub fn label(self) -> &'static str {
        match self {
            Self::Missing => "the folder no longer exists",
            Self::PermissionDenied => "permission denied",
            Self::NotDirectory => "the folder is no longer a directory",
            Self::Unreadable => "the folder could not be read",
            Self::Changed => "the folder changed after the search",
        }
    }
}

/// One bounded error example.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkipExample {
    pub path: PathBuf,
    pub message: String,
}

/// A progress snapshot. It contains counts only, never the retained paths.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SearchProgress {
    pub visited: u64,
    /// Every match encountered, including ones outside the retained best set.
    pub matches: u64,
    /// How many of those matches are currently retained for display.
    pub retained: u64,
    pub skipped: u64,
}

/// The report a finished (or cancelled) walk returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchReport {
    pub root: PathBuf,
    /// The path actually walked after resolving a root alias.
    pub walked_root: PathBuf,
    pub visited: u64,
    /// Every match encountered, not just the retained rows.
    pub matches: u64,
    /// The number of ranked rows that fit under `max_results`.
    pub retained: usize,
    /// True once at least one match was dropped because the ranked set was
    /// full. The scan still continues to the visited/time limits.
    pub truncated: bool,
    pub skipped: u64,
    pub examples: Vec<SkipExample>,
    pub reasons: Vec<PartialReason>,
    pub mounts_skipped: u64,
    pub cancelled: bool,
}

impl SearchReport {
    /// Only a completed scan with no skipped reason may say "No matches".
    pub fn is_complete(&self) -> bool {
        !self.cancelled && self.reasons.is_empty()
    }

    pub fn is_partial(&self) -> bool {
        !self.cancelled && !self.reasons.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchOutcome {
    Unavailable(RootFailure),
    Finished(SearchReport),
    Cancelled,
}

/// The filesystem boundary. Tests inject a tree, errors and device ids so no
/// mount or permission setup is needed.
pub trait SearchSource: Send + Sync {
    fn resolve_root(&self, root: &Path) -> io::Result<PathBuf>;
    fn lstat(&self, path: &Path) -> io::Result<EntryMeta>;
    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>>;
}

/// Walk `request` on the caller's worker. `emit` receives at most
/// [`BATCH_ROWS`] ranked inserts/removals and returns `false` when the consumer
/// can no longer accept work. `cancel` is checked between entries and inside
/// bounded fuzzy work; a blocked native filesystem call remains cooperative,
/// never interruptible.
pub fn search(
    request: &SearchRequest,
    source: &dyn SearchSource,
    clock: &dyn crate::folder_size::Clock,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(SearchProgress),
    emit: &mut dyn FnMut(Vec<SearchChange>) -> bool,
) -> SearchOutcome {
    walk::walk(request, source, clock, cancel, progress, emit)
}
