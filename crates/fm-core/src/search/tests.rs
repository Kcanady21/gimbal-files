// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic tests for the filename matcher and the recursive walk.

use std::collections::{HashMap, VecDeque};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::folder_size::{DirReader, EntryKind, EntryMeta, ManualClock, SystemClock};

use super::*;

#[derive(Clone, Debug)]
enum FakeChild {
    Ok(PathBuf),
    Err(io::ErrorKind),
}

struct FakeReader {
    entries: VecDeque<FakeChild>,
}

impl DirReader for FakeReader {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        self.entries.pop_front().map(|child| match child {
            FakeChild::Ok(path) => Ok(path),
            FakeChild::Err(kind) => Err(io::Error::from(kind)),
        })
    }
}

#[derive(Default)]
struct FakeSource {
    dirs: HashMap<PathBuf, Result<Vec<FakeChild>, io::ErrorKind>>,
    metas: HashMap<PathBuf, EntryMeta>,
    resolved: HashMap<PathBuf, PathBuf>,
    clock: Option<Arc<ManualClock>>,
    lstat_step_ms: u64,
}

impl FakeSource {
    fn resolved(mut self, from: &str, to: &str) -> Self {
        self.resolved.insert(PathBuf::from(from), PathBuf::from(to));
        self
    }

    fn dir(mut self, path: &str, children: Vec<FakeChild>) -> Self {
        self.dirs.insert(PathBuf::from(path), Ok(children));
        self.meta(path, EntryKind::Directory, 1, 0)
    }

    fn unreadable_dir(mut self, path: &str, kind: io::ErrorKind) -> Self {
        self.dirs.insert(PathBuf::from(path), Err(kind));
        self.meta(path, EntryKind::Directory, 1, 0)
    }

    fn file(self, path: &str, device: u64) -> Self {
        self.meta(path, EntryKind::File, device, 12)
    }

    fn meta_bytes(mut self, path: &Path, kind: EntryKind) -> Self {
        let inode = self.metas.len() as u64 + 1;
        self.metas.insert(
            path.to_path_buf(),
            EntryMeta {
                device: 1,
                inode,
                size: 12,
                blocks: 1,
                nlink: 1,
                kind,
                modified_sec: 1_700_000_000,
                modified_nsec: 0,
            },
        );
        self
    }

    fn meta(mut self, path: &str, kind: EntryKind, device: u64, size: u64) -> Self {
        let inode = self.metas.len() as u64 + 1;
        self.metas.insert(
            PathBuf::from(path),
            EntryMeta {
                device,
                inode,
                size,
                blocks: 1,
                nlink: 1,
                kind,
                modified_sec: 1_700_000_000,
                modified_nsec: 0,
            },
        );
        self
    }

    fn manual_clock(mut self, clock: Arc<ManualClock>, step_ms: u64) -> Self {
        self.clock = Some(clock);
        self.lstat_step_ms = step_ms;
        self
    }
}

impl SearchSource for FakeSource {
    fn resolve_root(&self, root: &Path) -> io::Result<PathBuf> {
        if let Some(target) = self.resolved.get(root) {
            return Ok(target.clone());
        }
        if self.dirs.contains_key(root) || self.metas.contains_key(root) {
            return Ok(root.to_path_buf());
        }
        Err(io::Error::from(io::ErrorKind::NotFound))
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        if let Some(clock) = &self.clock {
            clock.advance(self.lstat_step_ms);
        }
        self.metas
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        match self.dirs.get(path) {
            Some(Ok(children)) => Ok(Box::new(FakeReader {
                entries: children.iter().cloned().collect(),
            })),
            Some(Err(kind)) => Err(io::Error::from(*kind)),
            None => Err(io::Error::from(io::ErrorKind::NotFound)),
        }
    }
}

struct VanishingSource {
    inner: FakeSource,
    root: PathBuf,
    calls: AtomicU64,
    vanish_at: u64,
}

impl SearchSource for VanishingSource {
    fn resolve_root(&self, root: &Path) -> io::Result<PathBuf> {
        self.inner.resolve_root(root)
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        if path == self.root && self.calls.fetch_add(1, Ordering::SeqCst) >= self.vanish_at {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        self.inner.lstat(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        self.inner.read_dir(path)
    }
}

fn ok(path: &str) -> FakeChild {
    FakeChild::Ok(PathBuf::from(path))
}

fn limits(
    max_visited: u64,
    max_results: usize,
    max_elapsed_ms: u64,
    max_depth: usize,
) -> SearchLimits {
    SearchLimits {
        max_visited,
        max_results,
        max_elapsed_ms,
        max_depth,
    }
}

fn apply_changes(rows: &mut Vec<SearchMatch>, changes: &[SearchChange]) {
    for change in changes {
        match change {
            SearchChange::Insert { index, row } => {
                let index = (*index).min(rows.len());
                rows.insert(index, row.clone());
            }
            SearchChange::Remove { index, path } => {
                let position = if rows.get(*index).is_some_and(|row| row.path == *path) {
                    *index
                } else {
                    match rows.iter().position(|row| row.path == *path) {
                        Some(position) => position,
                        None => continue,
                    }
                };
                rows.remove(position);
            }
        }
    }
}

fn run(
    request: &SearchRequest,
    source: &dyn SearchSource,
    clock: &dyn crate::folder_size::Clock,
) -> (SearchOutcome, Vec<SearchMatch>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut batches = Vec::new();
    let outcome = search(
        request,
        source,
        clock,
        &|| false,
        &mut |_| {},
        &mut |changes| {
            batches.push(changes.len());
            apply_changes(&mut rows, &changes);
            true
        },
    );
    (outcome, rows, batches)
}

fn finished(outcome: SearchOutcome) -> SearchReport {
    match outcome {
        SearchOutcome::Finished(report) => report,
        other => panic!("expected a finished report, got {other:?}"),
    }
}

#[test]
fn literal_substring_includes_extension_and_keeps_spaces_literal() {
    let source = FakeSource::default()
        .dir(
            "/root",
            vec![ok("/root/Report.TXT"), ok("/root/two  spaces")],
        )
        .file("/root/Report.TXT", 1)
        .file("/root/two  spaces", 1);
    let query = SearchQuery::new("port.tx");
    assert!(query.matches_name(std::ffi::OsStr::new("Report.TXT")));
    assert!(!query.matches_name(std::ffi::OsStr::new("Report.BIN")));

    let request = SearchRequest::new(PathBuf::from("/root"), "  ");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].display_name, "two  spaces");

    let all_space = SearchRequest::new(PathBuf::from("/root"), "   ");
    let (outcome, rows, _) = run(&all_space, &source, &SystemClock);
    assert_eq!(finished(outcome).matches, 0);
    assert!(rows.is_empty(), "an all-whitespace query is not match-all");
}

#[test]
fn exact_prefix_substring_and_fuzzy_classes_rank_in_that_order() {
    let query = SearchQuery::new("report");
    let score = |name: &str| query.score_name(std::ffi::OsStr::new(name), &|| false);
    let report = score("report").expect("exact name");
    let stem = score("report.txt").expect("exact stem");
    let prefix = score("reporting.txt").expect("prefix");
    let substring = score("my-report.txt").expect("substring");
    let fuzzy = score("reprot.txt").expect("transposed typo");
    assert_eq!(report.class, MatchClass::ExactName);
    assert_eq!(stem.class, MatchClass::ExactStem);
    assert_eq!(prefix.class, MatchClass::Prefix);
    assert_eq!(substring.class, MatchClass::Substring);
    assert_eq!(fuzzy.class, MatchClass::Fuzzy);
    assert!(report < stem);
    assert!(stem < prefix);
    assert!(prefix < substring);
    assert!(substring < fuzzy);
    assert_eq!(fuzzy.edits, 1, "adjacent transposition is one edit");
}

#[test]
fn fuzzy_subsequences_and_bounded_typos_have_length_rules() {
    let sub = SearchQuery::new("rpt");
    assert!(sub.matches_name(std::ffi::OsStr::new("report.txt")));
    assert!(!sub.matches_name(std::ffi::OsStr::new("notes.txt")));

    // 3-5 scalars: one edit only.
    assert!(SearchQuery::new("repot").matches_name(std::ffi::OsStr::new("report.txt")));
    assert!(!SearchQuery::new("rexxr").matches_name(std::ffi::OsStr::new("repor.txt")));
    // 6+ scalars: two edits.
    assert!(SearchQuery::new("rexxrt").matches_name(std::ffi::OsStr::new("report.txt")));
    // Two scalars never take the fuzzy path.
    assert!(!SearchQuery::new("rp").matches_name(std::ffi::OsStr::new("report.txt")));
    assert!(SearchQuery::new("po").matches_name(std::ffi::OsStr::new("report.txt")));
}

#[test]
fn fuzzy_tighter_and_earlier_spans_rank_before_looser_ones() {
    let query = SearchQuery::new("rpt");
    let tight = query
        .score_name(std::ffi::OsStr::new("r_p_t.txt"), &|| false)
        .expect("tight subsequence");
    let loose = query
        .score_name(std::ffi::OsStr::new("r__p_t.txt"), &|| false)
        .expect("loose subsequence");
    assert_eq!(tight.class, MatchClass::Fuzzy);
    assert_eq!(loose.class, MatchClass::Fuzzy);
    assert_eq!(tight.edits, 0);
    assert!(tight.gaps < loose.gaps);
    assert!(tight < loose);

    let early = query
        .score_name(std::ffi::OsStr::new("a_r_p_t.txt"), &|| false)
        .expect("early subsequence");
    let late = query
        .score_name(std::ffi::OsStr::new("zzzz_r_p_t.txt"), &|| false)
        .expect("late subsequence");
    assert_eq!(early.gaps, late.gaps);
    assert!(early.start < late.start);
    assert!(early < late);
}

#[test]
fn case_extension_and_unicode_lowercase_keep_literal_confidence() {
    let query = SearchQuery::new("REPORT");
    assert_eq!(
        query
            .score_name(std::ffi::OsStr::new("report"), &|| false)
            .expect("case-insensitive exact")
            .class,
        MatchClass::ExactName
    );
    assert_eq!(
        query
            .score_name(std::ffi::OsStr::new("REPORT.TXT"), &|| false)
            .expect("case-insensitive stem")
            .class,
        MatchClass::ExactStem
    );
    assert_eq!(
        SearchQuery::new("report.folder")
            .score_name(std::ffi::OsStr::new("report.folder"), &|| false)
            .expect("folder suffix is part of the name")
            .class,
        MatchClass::ExactName
    );
    assert_eq!(
        SearchQuery::new("report")
            .score_name(std::ffi::OsStr::new("report.folder"), &|| false)
            .expect("a folder extension does not hide its stem")
            .class,
        MatchClass::ExactStem
    );
    assert!(
        SearchQuery::new("folder").matches_name(std::ffi::OsStr::new("report.folder")),
        "a folder extension remains a literal substring"
    );
    assert!(
        SearchQuery::new("ångstrøm").matches_name(std::ffi::OsStr::new("ÅNGSTRÖM.txt")),
        "one substitution within the 6+ edit budget"
    );
}

#[test]
fn native_invalid_bytes_only_use_ascii_folded_literal_matching() {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::OsStr::from_bytes(b"REPORT-\xff.TXT");
    assert!(SearchQuery::new("report-").matches_name(name));
    assert_eq!(
        SearchQuery::new("report-")
            .score_name(name, &|| false)
            .expect("prefix")
            .class,
        MatchClass::Prefix
    );
    assert!(!SearchQuery::new("reprot-").matches_name(name));
    assert!(!SearchQuery::new("report").matches_name(std::ffi::OsStr::from_bytes(b"\xff")));
}

#[test]
fn long_queries_and_long_tokens_stay_literal_without_losing_exact_names() {
    let long = "a".repeat(70);
    let exact = SearchQuery::new(long.clone());
    assert!(exact.matches_name(std::ffi::OsStr::new(&long)));
    assert!(exact.matches_name(std::ffi::OsStr::new(&format!("x{long}y"))));
    let huge = format!("{}{}", "a".repeat(300), "b".repeat(300));
    let subsequence_only = SearchQuery::new("ab".repeat(32));
    assert!(
        !subsequence_only.matches_name(std::ffi::OsStr::new(&huge)),
        "a candidate token over 256 scalars is outside fuzzy work"
    );
}

#[test]
fn late_exact_matches_replace_early_fuzzy_rows_in_the_bounded_best_set() {
    let mut children = Vec::new();
    for index in 0..20 {
        children.push(ok(&format!("/root/reprot-{index}.txt")));
    }
    children.push(ok("/root/report.txt"));
    children.push(ok("/root/report"));
    let mut source = FakeSource::default().dir("/root", children);
    for index in 0..20 {
        source = source.file(&format!("/root/reprot-{index}.txt"), 1);
    }
    let source = source.file("/root/report.txt", 1).file("/root/report", 1);
    let mut request = SearchRequest::new(PathBuf::from("/root"), "report");
    request.limits = limits(1_000, 2, 30_000, 128);
    let (outcome, rows, batches) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(
        report.is_partial(),
        "result limit is reported, scan continues"
    );
    assert!(report.truncated);
    assert_eq!(report.matches, 22);
    assert_eq!(report.retained, 2);
    assert_eq!(
        rows.iter()
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["/root/report", "/root/report.txt"],
        "late exact matches outrank every early fuzzy row"
    );
    assert!(batches.iter().all(|len| *len <= BATCH_ROWS));
}

#[test]
fn duplicate_names_tie_break_by_native_relative_path() {
    let source = FakeSource::default()
        .dir(
            "/root",
            vec![ok("/root/b/report.txt"), ok("/root/a/report.txt")],
        )
        .dir("/root/a", vec![])
        .dir("/root/b", vec![])
        .file("/root/a/report.txt", 1)
        .file("/root/b/report.txt", 1);
    let request = SearchRequest::new(PathBuf::from("/root"), "report");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    assert!(finished(outcome).is_complete());
    assert_eq!(
        rows.iter()
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec!["/root/a/report.txt", "/root/b/report.txt"]
    );
}

/// Repeatable matcher-only benchmark. It is ignored by the correctness suite:
/// wall-clock results are recorded in `docs/notes/P2o-r2.md` and never asserted.
#[test]
#[ignore = "matcher-only benchmark; run explicitly for latency measurements"]
fn matcher_benchmark_over_generated_corpus() {
    use std::ffi::OsString;
    use std::time::Instant;

    let corpus_size = 100_000;
    let mut names = Vec::with_capacity(corpus_size);
    for index in 0..corpus_size {
        let name = match index % 4 {
            0 => format!("report-{index}.txt"),
            1 => format!("notes-{index}.md"),
            2 => format!("reprot-{index}.txt"),
            _ => {
                // Worst-length single token and a separator-heavy token mix.
                if index % 8 == 3 {
                    "z".repeat(256)
                } else {
                    "zz-".repeat(80)
                }
            }
        };
        names.push(OsString::from(name));
    }

    let queries = vec![
        "rpt".to_string(),
        "reprot".to_string(),
        "ab".repeat(16),
        "ab".repeat(32),
        "a".repeat(64),
    ];
    for query in queries {
        let query = SearchQuery::new(query);
        let started = Instant::now();
        let mut matches = 0usize;
        for name in &names {
            if query.score_name(name, &|| false).is_some() {
                matches += 1;
            }
        }
        let elapsed = started.elapsed();
        eprintln!(
            "benchmark: corpus={corpus_size} query={} scalars={} matches={matches} elapsed={:?} per_name={:.1}ns",
            query.text(),
            query.text().chars().count(),
            elapsed,
            elapsed.as_nanos() as f64 / corpus_size as f64,
        );
    }

    // Cancellation during bounded fuzzy work: the closure flips after the
    // initial check so the next in-loop check stops the candidate.
    let query = SearchQuery::new("ab".repeat(32));
    let candidate = OsString::from("a".repeat(256));
    let checks = std::sync::atomic::AtomicUsize::new(0);
    let started = Instant::now();
    for _ in 0..corpus_size {
        let cancelled = || checks.fetch_add(1, Ordering::Relaxed) >= 1;
        assert!(query.score_name(&candidate, &cancelled).is_none());
    }
    eprintln!(
        "benchmark: cancellation corpus={corpus_size} elapsed={:?}",
        started.elapsed()
    );
}

#[test]
fn unicode_lowercase_is_locale_independent() {
    let source = FakeSource::default()
        .dir(
            "/root",
            vec![ok("/root/ÅNGSTRÖM.txt"), ok("/root/ÄPFEL.txt")],
        )
        .file("/root/ÅNGSTRÖM.txt", 1)
        .file("/root/ÄPFEL.txt", 1);
    let request = SearchRequest::new(PathBuf::from("/root"), "ångström");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, OsString::from("ÅNGSTRÖM.txt"));
}

#[test]
fn invalid_utf8_names_use_native_bytes_with_ascii_folding() {
    let name = OsString::from_vec(b"REPORT-\xff.TXT".to_vec());
    let path = PathBuf::from("/root").join(&name);
    let source = FakeSource::default()
        .dir("/root", vec![FakeChild::Ok(path.clone())])
        .meta_bytes(&path, EntryKind::File);
    let request = SearchRequest::new(PathBuf::from("/root"), "report-");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, path);
    assert_eq!(rows[0].name, name);

    let non_ascii = SearchRequest::new(PathBuf::from("/root"), "ÿ");
    let (outcome, rows, _) = run(&non_ascii, &source, &SystemClock);
    assert!(finished(outcome).is_complete());
    assert!(rows.is_empty(), "UTF-8 ÿ bytes never match native 0xff");
}

#[test]
fn hidden_files_duplicate_basenames_and_the_root_are_handled() {
    let source = FakeSource::default()
        .dir(
            "/needle-root",
            vec![
                ok("/needle-root/.hidden.txt"),
                ok("/needle-root/a"),
                ok("/needle-root/b"),
            ],
        )
        .dir("/needle-root/a", vec![ok("/needle-root/a/same.txt")])
        .dir("/needle-root/b", vec![ok("/needle-root/b/same.txt")])
        .file("/needle-root/.hidden.txt", 1)
        .file("/needle-root/a/same.txt", 1)
        .file("/needle-root/b/same.txt", 1);
    let hidden = SearchRequest::new(PathBuf::from("/needle-root"), "hidden");
    let (outcome, rows, _) = run(&hidden, &source, &SystemClock);
    assert_eq!(finished(outcome).matches, 1);
    assert_eq!(rows[0].name, OsString::from(".hidden.txt"));

    let duplicate = SearchRequest::new(PathBuf::from("/needle-root"), "same");
    let (outcome, rows, _) = run(&duplicate, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].path, rows[1].path);
    assert!(
        rows.iter().all(|row| row.path != Path::new("/needle-root")),
        "the root itself is never a result"
    );
}

#[test]
fn empty_folder_completes_with_no_matches() {
    let source = FakeSource::default().dir("/root", vec![]);
    let request = SearchRequest::new(PathBuf::from("/root"), "anything");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(report.matches, 0);
    assert!(rows.is_empty());
}

#[test]
fn a_nested_device_directory_matches_but_is_not_descended() {
    let source = FakeSource::default()
        .dir("/root", vec![ok("/root/mnt"), ok("/root/deep.txt")])
        .dir("/root/mnt", vec![ok("/root/mnt/deep.txt")])
        .file("/root/mnt/deep.txt", 2)
        .file("/root/deep.txt", 1);
    // The mount-point directory itself carries the nested device.
    let source = source.meta("/root/mnt", EntryKind::Directory, 2, 0);

    let inside = SearchRequest::new(PathBuf::from("/root"), "deep.txt");
    let (outcome, rows, _) = run(&inside, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, PathBuf::from("/root/deep.txt"));
    assert_eq!(report.mounts_skipped, 1);

    let mount = SearchRequest::new(PathBuf::from("/root"), "mnt");
    let (outcome, rows, _) = run(&mount, &source, &SystemClock);
    assert_eq!(finished(outcome).matches, 1);
    assert_eq!(rows[0].path, PathBuf::from("/root/mnt"));
}

#[test]
fn permission_and_disappearing_children_are_partial_and_bounded() {
    let mut children = Vec::new();
    for index in 0..10 {
        children.push(FakeChild::Err(io::ErrorKind::PermissionDenied));
        let _ = index;
    }
    children.push(ok("/root/keep.txt"));
    children.push(FakeChild::Err(io::ErrorKind::NotFound));
    children.push(FakeChild::Err(io::ErrorKind::Other));
    let source = FakeSource::default()
        .dir("/root", children)
        .file("/root/keep.txt", 1);
    let request = SearchRequest::new(PathBuf::from("/root"), "keep");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_partial());
    assert_eq!(rows.len(), 1);
    assert!(report.skipped >= 10);
    assert!(report.examples.len() <= MAX_EXAMPLES);
    assert!(report.reasons.contains(&PartialReason::PermissionDenied));
    assert!(report.reasons.contains(&PartialReason::Changed));
}

#[test]
fn entry_limit_counts_non_matches_and_stops_partial() {
    let children: Vec<_> = (0..10)
        .map(|index| ok(&format!("/root/file-{index}")))
        .collect();
    let source = FakeSource::default().dir("/root", children);
    let mut request = SearchRequest::new(PathBuf::from("/root"), "nothing-matches");
    request.limits = limits(3, 10, 30_000, 128);
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_partial());
    assert!(report.reasons.contains(&PartialReason::EntryLimit));
    assert_eq!(report.visited, 3);
    assert!(rows.is_empty());
}

#[test]
fn result_limit_keeps_scanning_and_reports_retained_best() {
    let source = FakeSource::default()
        .dir("/root", vec![ok("/root/a.txt"), ok("/root/b.txt")])
        .file("/root/a.txt", 1)
        .file("/root/b.txt", 1);
    let mut request = SearchRequest::new(PathBuf::from("/root"), ".txt");
    request.limits = limits(100, 1, 30_000, 128);
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_partial());
    assert!(report.reasons.contains(&PartialReason::ResultLimit));
    assert_eq!(
        report.matches, 2,
        "the scan did not stop at the retained cap"
    );
    assert_eq!(report.retained, 1);
    assert!(report.truncated);
    assert_eq!(rows.len(), 1);
}

#[test]
fn depth_limit_is_partial_only_when_a_deeper_directory_exists() {
    let source = FakeSource::default()
        .dir("/root", vec![ok("/root/a")])
        .dir("/root/a", vec![ok("/root/a/b")])
        .dir("/root/a/b", vec![ok("/root/a/b/file.txt")])
        .file("/root/a/b/file.txt", 1);
    let mut request = SearchRequest::new(PathBuf::from("/root"), "file");
    request.limits = limits(100, 10, 30_000, 1);
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_partial());
    assert!(report.reasons.contains(&PartialReason::DepthLimit));
    assert!(rows.is_empty());

    request.limits.max_depth = 3;
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
}

#[test]
fn time_limit_uses_the_cooperative_clock() {
    let clock = Arc::new(ManualClock::new(1_000));
    let source = FakeSource::default()
        .dir("/root", vec![ok("/root/a"), ok("/root/b"), ok("/root/c")])
        .file("/root/a", 1)
        .file("/root/b", 1)
        .file("/root/c", 1)
        .manual_clock(clock.clone(), 10_000);
    let mut request = SearchRequest::new(PathBuf::from("/root"), "none");
    request.limits = limits(100, 10, 15_000, 128);
    let (outcome, rows, _) = run(&request, &source, clock.as_ref());
    let report = finished(outcome);
    assert!(report.is_partial());
    assert!(report.reasons.contains(&PartialReason::TimeLimit));
    assert!(rows.is_empty());
    assert!(report.visited < 3);
}

#[test]
fn a_root_removed_during_the_walk_is_unavailable() {
    let inner = FakeSource::default()
        .dir("/root", vec![ok("/root/a.txt")])
        .file("/root/a.txt", 1);
    let source = VanishingSource {
        inner,
        root: PathBuf::from("/root"),
        calls: AtomicU64::new(0),
        vanish_at: 1,
    };
    let request = SearchRequest::new(PathBuf::from("/root"), "a.txt");
    let (outcome, _, _) = run(&request, &source, &SystemClock);
    assert_eq!(outcome, SearchOutcome::Unavailable(RootFailure::Missing));
}

#[test]
fn a_missing_root_is_unavailable_not_no_matches() {
    let source = FakeSource::default();
    let request = SearchRequest::new(PathBuf::from("/gone"), "anything");
    let (outcome, _, _) = run(&request, &source, &SystemClock);
    assert_eq!(outcome, SearchOutcome::Unavailable(RootFailure::Missing));

    let source = FakeSource::default().unreadable_dir("/denied", io::ErrorKind::PermissionDenied);
    let request = SearchRequest::new(PathBuf::from("/denied"), "anything");
    let (outcome, _, _) = run(&request, &source, &SystemClock);
    assert_eq!(
        outcome,
        SearchOutcome::Unavailable(RootFailure::PermissionDenied)
    );
}

#[test]
fn cancellation_between_entries_is_reported() {
    let children: Vec<_> = (0..100)
        .map(|index| ok(&format!("/root/match-{index}.txt")))
        .collect();
    let mut source = FakeSource::default().dir("/root", children);
    for index in 0..100 {
        source = source.file(&format!("/root/match-{index}.txt"), 1);
    }
    let request = SearchRequest::new(PathBuf::from("/root"), "match-");
    let cancelled = Arc::new(AtomicBool::new(false));
    let mut rows = Vec::new();
    let outcome = search(
        &request,
        &source,
        &SystemClock,
        &|| cancelled.load(Ordering::SeqCst),
        &mut |_| {},
        &mut |changes| {
            apply_changes(&mut rows, &changes);
            cancelled.store(true, Ordering::SeqCst);
            true
        },
    );
    let report = finished(outcome);
    assert!(report.cancelled);
    assert!(!report.is_complete());
    assert!(!rows.is_empty() && rows.len() <= BATCH_ROWS);
}

#[test]
fn batches_keep_the_change_cap_and_deterministic_rank_order() {
    let mut root_children = vec![ok("/root/first.txt")];
    root_children.push(ok("/root/sub"));
    root_children.push(ok("/root/second.txt"));
    let source = FakeSource::default()
        .dir("/root", root_children)
        .dir(
            "/root/sub",
            vec![ok("/root/sub/third.txt"), ok("/root/sub/fourth.txt")],
        )
        .file("/root/first.txt", 1)
        .file("/root/second.txt", 1)
        .file("/root/sub/third.txt", 1)
        .file("/root/sub/fourth.txt", 1);
    let request = SearchRequest::new(PathBuf::from("/root"), ".txt");
    let (outcome, rows, batches) = run(&request, &source, &SystemClock);
    assert!(finished(outcome).is_complete());
    assert_eq!(
        rows.iter()
            .map(|row| row.path.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        vec![
            "/root/first.txt",
            "/root/sub/third.txt",
            "/root/second.txt",
            "/root/sub/fourth.txt",
        ],
        "substring start, then native path, decide the tie"
    );
    assert!(batches.iter().all(|len| *len <= BATCH_ROWS));
}

#[test]
fn a_root_alias_is_resolved_once() {
    let source = FakeSource::default()
        .resolved("/alias", "/real")
        .meta("/alias", EntryKind::Symlink, 1, 3)
        .dir("/real", vec![ok("/real/file.txt")])
        .file("/real/file.txt", 1);
    let request = SearchRequest::new(PathBuf::from("/alias"), "file");
    let (outcome, rows, _) = run(&request, &source, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(report.walked_root, PathBuf::from("/real"));
    assert_eq!(rows[0].path, PathBuf::from("/real/file.txt"));
}

// ---------------------------------------------------------------------------
// Native fixtures under /tmp/gimbal-search-*
// ---------------------------------------------------------------------------

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(1);

fn scratch(name: &str) -> PathBuf {
    let path = PathBuf::from(format!(
        "/tmp/gimbal-search-{}-{}-{name}",
        std::process::id(),
        NEXT_SCRATCH.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).expect("scratch folder");
    path
}

#[test]
fn native_invalid_utf8_names_keep_their_exact_bytes() {
    let root = scratch("invalid-utf8");
    let name = OsString::from_vec(b"REPORT-\xff.TXT".to_vec());
    let path = root.join(&name);
    fs::write(&path, b"fixture").expect("fixture file");
    let request = SearchRequest::new(root.clone(), "report-");
    let (outcome, rows, _) = run(&request, &NativeSearchSource, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name.as_encoded_bytes(), name.as_encoded_bytes());
    assert_eq!(rows[0].path, path);
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn native_symlink_cycle_is_not_followed() {
    let root = scratch("symlink-cycle");
    fs::create_dir_all(root.join("sub")).expect("sub");
    fs::write(root.join("sub/inside.txt"), b"fixture").expect("file");
    symlink(&root, root.join("loop")).expect("cycle symlink");
    let inside = SearchRequest::new(root.clone(), "inside");
    let (outcome, rows, _) = run(&inside, &NativeSearchSource, &SystemClock);
    let report = finished(outcome);
    assert!(report.is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].path, root.join("sub/inside.txt"));

    let loop_query = SearchRequest::new(root.clone(), "loop");
    let (outcome, rows, _) = run(&loop_query, &NativeSearchSource, &SystemClock);
    assert!(finished(outcome).is_complete());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, EntryKind::Symlink);
    let _ = fs::remove_dir_all(&root);
}
#[test]
fn fuzzy_scores_use_the_best_span_and_the_stems_actual_origin() {
    let query = SearchQuery::new("rpt");
    let score = query
        .score_name(OsStr::new("r___p___t__r_pt"), &|| false)
        .expect("subsequence");
    assert_eq!((score.gaps, score.start), (1, 11));

    let query = SearchQuery::new("report");
    let score = query
        .score_name(OsStr::new("reprot.txt"), &|| false)
        .expect("stem transposition");
    assert_eq!((score.edits, score.start), (1, 0));
}
