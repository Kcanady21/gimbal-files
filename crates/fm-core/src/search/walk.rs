// SPDX-License-Identifier: GPL-3.0-or-later
//! The cooperative recursive walk behind [`super::search`].

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet, VecDeque};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::folder_size::{Clock, DirReader, EntryKind, EntryMeta, NativeSource, ScanSource};

use super::{
    BATCH_ROWS, MAX_EXAMPLES, PROGRESS_EVERY, PartialReason, RootFailure, SearchChange,
    SearchMatch, SearchOutcome, SearchProgress, SearchReport, SearchRequest, SearchSource,
    SkipExample,
};

/// The production native source. The root alias is resolved once here, on the
/// worker; child symlink directories are never followed.
pub struct NativeSearchSource;

impl SearchSource for NativeSearchSource {
    fn resolve_root(&self, root: &Path) -> io::Result<PathBuf> {
        fs::canonicalize(root)
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        fs::symlink_metadata(path).map(crate::folder_size::entry_meta)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        NativeSource.read_dir(path)
    }
}

/// A retained row ordered by its score tuple and then by native relative path.
/// The path is the identity; the position can change after every insert.
struct Ranked {
    row: SearchMatch,
}

impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Ranked {}

impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        self.row.score.cmp(&other.row.score).then_with(|| {
            self.row
                .path
                .as_os_str()
                .as_bytes()
                .cmp(other.row.path.as_os_str().as_bytes())
        })
    }
}

impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The bounded best-first retention set. When full, a worse match is dropped
/// and a better one replaces it; the scan still continues, so an exact match
/// found late can displace an early fuzzy one.
struct RankedSet {
    limit: usize,
    rows: BTreeSet<Ranked>,
    truncated: bool,
}

impl RankedSet {
    fn new(limit: usize) -> Self {
        Self {
            limit,
            rows: BTreeSet::new(),
            truncated: false,
        }
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    fn consider(&mut self, row: SearchMatch, changes: &mut Vec<SearchChange>) {
        if self.limit == 0 {
            self.truncated = true;
            return;
        }
        let ranked = Ranked { row: row.clone() };
        if self.rows.len() >= self.limit {
            let worse_or_equal = self.rows.last().is_none_or(|worst| ranked >= *worst);
            if worse_or_equal {
                self.truncated = true;
                return;
            }
            if let Some(worst) = self.rows.pop_last() {
                changes.push(SearchChange::Remove {
                    index: self.rows.len(),
                    path: worst.row.path,
                });
            }
            self.truncated = true;
        }
        let index = self.rows.range(..&ranked).count();
        self.rows.insert(ranked);
        changes.push(SearchChange::Insert { index, row });
    }
}

pub(super) fn walk(
    request: &SearchRequest,
    source: &dyn SearchSource,
    clock: &dyn Clock,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(SearchProgress),
    emit: &mut dyn FnMut(Vec<SearchChange>) -> bool,
) -> SearchOutcome {
    if cancel() {
        return SearchOutcome::Cancelled;
    }
    let walked_root = match source.resolve_root(&request.root) {
        Ok(path) => path,
        Err(error) => return SearchOutcome::Unavailable(root_failure(&error)),
    };
    let root_meta = match source.lstat(&walked_root) {
        Ok(meta) => meta,
        Err(error) => return SearchOutcome::Unavailable(root_failure(&error)),
    };
    if root_meta.kind != EntryKind::Directory {
        return SearchOutcome::Unavailable(RootFailure::NotDirectory);
    }

    let started = clock.now_ms();
    let deadline = started.saturating_add(request.limits.max_elapsed_ms);
    let root_device = root_meta.device;
    let mut report = SearchReport {
        root: request.root.clone(),
        walked_root: walked_root.clone(),
        visited: 0,
        matches: 0,
        retained: 0,
        truncated: false,
        skipped: 0,
        examples: Vec::new(),
        reasons: Vec::new(),
        mounts_skipped: 0,
        cancelled: false,
    };
    let mut ranked = RankedSet::new(request.limits.max_results);
    let mut seen_dirs: HashSet<(u64, u64)> = HashSet::new();
    seen_dirs.insert((root_meta.device, root_meta.inode));
    let mut pending: VecDeque<(PathBuf, usize)> = VecDeque::new();
    pending.push_back((walked_root.clone(), 0));
    let mut changes: Vec<SearchChange> = Vec::with_capacity(BATCH_ROWS);
    let mut last_progress = 0;

    'walk: while let Some((directory, depth)) = pending.pop_front() {
        if cancel() {
            report.cancelled = true;
            break;
        }
        if clock.now_ms() >= deadline {
            push_reason(&mut report.reasons, PartialReason::TimeLimit);
            break;
        }
        let mut entries = match source.read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                if directory == walked_root && report.visited == 0 {
                    return SearchOutcome::Unavailable(root_failure(&error));
                }
                record_skip(&mut report, &directory, &error);
                continue;
            }
        };
        loop {
            if cancel() {
                report.cancelled = true;
                break 'walk;
            }
            if clock.now_ms() >= deadline {
                push_reason(&mut report.reasons, PartialReason::TimeLimit);
                break 'walk;
            }
            if report.visited >= request.limits.max_visited {
                push_reason(&mut report.reasons, PartialReason::EntryLimit);
                break 'walk;
            }
            let Some(entry) = entries.next_entry() else {
                break;
            };
            let path = match entry {
                Ok(path) => path,
                Err(error) => {
                    record_skip(&mut report, &directory, &error);
                    continue;
                }
            };
            report.visited = report.visited.saturating_add(1);
            let metadata = match source.lstat(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    record_skip(&mut report, &path, &error);
                    continue;
                }
            };
            let Some(name) = path.file_name().map(std::ffi::OsStr::to_os_string) else {
                continue;
            };
            if let Some(score) = request.query.score_name(&name, cancel) {
                report.matches = report.matches.saturating_add(1);
                let was_truncated = ranked.truncated;
                // Reserve room for the at most two changes one consideration
                // can produce, so no batch exceeds BATCH_ROWS row changes.
                if changes.len() >= BATCH_ROWS - 1
                    && !flush(std::mem::take(&mut changes), emit, &mut report)
                {
                    break 'walk;
                }
                ranked.consider(
                    SearchMatch {
                        path: path.clone(),
                        display_name: name.to_string_lossy().into_owned(),
                        name,
                        kind: metadata.kind,
                        size: (metadata.kind == EntryKind::File).then_some(metadata.size),
                        modified_sec: Some(metadata.modified_sec),
                        score,
                    },
                    &mut changes,
                );
                if ranked.truncated && !was_truncated {
                    push_reason(&mut report.reasons, PartialReason::ResultLimit);
                }
                if changes.len() >= BATCH_ROWS
                    && !flush(std::mem::take(&mut changes), emit, &mut report)
                {
                    break 'walk;
                }
            }
            if metadata.kind == EntryKind::Directory {
                if depth >= request.limits.max_depth {
                    push_reason(&mut report.reasons, PartialReason::DepthLimit);
                } else if metadata.device != root_device {
                    // A mount point's directory may match, but the walk never
                    // enters another device.
                    report.mounts_skipped = report.mounts_skipped.saturating_add(1);
                } else if seen_dirs.insert((metadata.device, metadata.inode)) {
                    pending.push_back((path.clone(), depth + 1));
                }
            }
            if report.visited.saturating_sub(last_progress) >= PROGRESS_EVERY {
                last_progress = report.visited;
                progress(SearchProgress {
                    visited: report.visited,
                    matches: report.matches,
                    retained: ranked.len() as u64,
                    skipped: report.skipped,
                });
                std::thread::yield_now();
            }
        }
    }

    if !report.cancelled && !changes.is_empty() {
        let _ = flush(std::mem::take(&mut changes), emit, &mut report);
    }
    // A root (or the alias the user chose) removed while the walk was running
    // must not leave a completed page that claims to describe it. These are
    // final lstats, not another walk.
    if !report.cancelled
        && (missing(source, &walked_root)
            || (request.root != walked_root && missing(source, &request.root)))
    {
        return SearchOutcome::Unavailable(RootFailure::Missing);
    }
    report.retained = ranked.len();
    report.truncated = ranked.truncated;
    if report.truncated {
        push_reason(&mut report.reasons, PartialReason::ResultLimit);
    }
    progress(SearchProgress {
        visited: report.visited,
        matches: report.matches,
        retained: report.retained as u64,
        skipped: report.skipped,
    });
    SearchOutcome::Finished(report)
}

fn missing(source: &dyn SearchSource, path: &Path) -> bool {
    source
        .lstat(path)
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
}

fn flush(
    rows: Vec<SearchChange>,
    emit: &mut dyn FnMut(Vec<SearchChange>) -> bool,
    report: &mut SearchReport,
) -> bool {
    if emit(rows) {
        true
    } else {
        report.cancelled = true;
        false
    }
}

fn record_skip(report: &mut SearchReport, path: &Path, error: &io::Error) {
    report.skipped = report.skipped.saturating_add(1);
    let reason = match error.kind() {
        io::ErrorKind::PermissionDenied => PartialReason::PermissionDenied,
        io::ErrorKind::NotFound => PartialReason::Changed,
        _ => PartialReason::Unreadable,
    };
    push_reason(&mut report.reasons, reason);
    if report.examples.len() < MAX_EXAMPLES {
        report.examples.push(SkipExample {
            path: path.to_path_buf(),
            message: error.to_string(),
        });
    }
}

fn push_reason(reasons: &mut Vec<PartialReason>, reason: PartialReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

fn root_failure(error: &io::Error) -> RootFailure {
    match error.kind() {
        io::ErrorKind::NotFound => RootFailure::Missing,
        io::ErrorKind::PermissionDenied => RootFailure::PermissionDenied,
        _ => RootFailure::Unreadable,
    }
}
