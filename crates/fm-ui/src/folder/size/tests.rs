// SPDX-License-Identifier: GPL-3.0-or-later
//! Tests for the production scheduler and tooltip model, using a controlled
//! metadata source/barrier and a manual clock.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use fm_core::folder_size::{
    DirReader, EntryKind, EntryMeta, FolderTotals, ManualClock, PartialReason, Progress, RootProbe,
    RootState, ScanPolicy, ScanResult, ScanSource,
};
use fm_core::preview::FileRevision;

use super::*;

fn meta(kind: EntryKind, device: u64, inode: u64, size: u64, blocks: u64) -> EntryMeta {
    EntryMeta {
        device,
        inode,
        size,
        blocks,
        nlink: 1,
        kind,
        modified_sec: 1,
        modified_nsec: 2,
    }
}

fn dir(device: u64, inode: u64) -> EntryMeta {
    meta(EntryKind::Directory, device, inode, 4096, 8)
}

fn regular(device: u64, inode: u64, size: u64) -> EntryMeta {
    meta(EntryKind::File, device, inode, size, 1)
}

struct VecDir(std::vec::IntoIter<PathBuf>);

impl DirReader for VecDir {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        self.0.next().map(Ok)
    }
}

#[derive(Default)]
struct Gate {
    state: Mutex<(bool, usize)>,
    ready: Condvar,
}

impl Gate {
    fn wait(&self) {
        let mut state = self.state.lock().unwrap();
        state.1 += 1;
        self.ready.notify_all();
        while !state.0 {
            state = self.ready.wait(state).unwrap();
        }
    }

    fn wait_entered(&self, count: usize) {
        let mut state = self.state.lock().unwrap();
        while state.1 < count {
            let (next, timeout) = self
                .ready
                .wait_timeout(state, Duration::from_secs(5))
                .unwrap();
            state = next;
            if timeout.timed_out() && state.1 < count {
                panic!("only {} of {count} scans entered the barrier", state.1);
            }
        }
    }

    fn open(&self) {
        let mut state = self.state.lock().unwrap();
        state.0 = true;
        self.ready.notify_all();
    }
}

struct TestSource {
    nodes: HashMap<PathBuf, EntryMeta>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    state: Option<RootState>,
    gate: Option<Arc<Gate>>,
    reads: AtomicUsize,
}

impl Default for TestSource {
    fn default() -> Self {
        Self {
            nodes: HashMap::new(),
            children: HashMap::new(),
            state: None,
            gate: None,
            reads: AtomicUsize::new(0),
        }
    }
}

impl TestSource {
    fn add(&mut self, path: &str, entry: EntryMeta) {
        self.nodes.insert(PathBuf::from(path), entry);
    }

    fn child(&mut self, parent: &str, child: &str) {
        self.children
            .entry(PathBuf::from(parent))
            .or_default()
            .push(PathBuf::from(child));
    }
}

impl ScanSource for TestSource {
    fn probe(&self, path: &Path) -> RootProbe {
        if let Some(state) = self.state {
            return RootProbe::unavailable(state);
        }
        let Some(meta) = self.nodes.get(path).copied() else {
            return RootProbe::unavailable(RootState::Missing);
        };
        RootProbe {
            state: RootState::Ready,
            identity: Some(RootIdentity {
                path: path.to_path_buf(),
                device: meta.device,
                inode: meta.inode,
                modified_sec: meta.modified_sec,
                modified_nsec: meta.modified_nsec,
            }),
            meta: Some(meta),
        }
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        self.nodes
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(gate) = &self.gate {
            gate.wait();
        }
        Ok(Box::new(VecDir(
            self.children
                .get(path)
                .cloned()
                .unwrap_or_default()
                .into_iter(),
        )))
    }
}

fn identity(path: &str, device: u64, inode: u64) -> RootIdentity {
    RootIdentity {
        path: PathBuf::from(path),
        device,
        inode,
        modified_sec: 1,
        modified_nsec: 2,
    }
}

fn revision() -> FileRevision {
    FileRevision {
        device: 1,
        inode: 1,
        size: 0,
        modified_sec: 1,
        modified_nsec: 2,
        changed_sec: 0,
        changed_nsec: 0,
        etag: None,
    }
}

fn tooltip(path: &str, device: u64, inode: u64) -> TooltipModel {
    TooltipModel::new(
        TooltipIdentity {
            root: identity(path, device, inode),
            name: OsString::from("Folder"),
            revision: revision(),
        },
        "Folder".to_string(),
        "2026-11-03 12:00".to_string(),
    )
}

fn service_with(source: TestSource, clock: Arc<ManualClock>) -> Service {
    Service::with_parts(Arc::new(source), clock, ScanPolicy::default())
}

#[test]
fn hover_delay_needs_a_continuous_half_second() {
    let mut delay = HoverDelay::default();
    delay.enter(1_000);
    assert!(!delay.due(1_499));
    assert!(delay.due(1_500));
    delay.leave();
    assert!(!delay.due(2_000));
    delay.enter(2_000);
    assert!(!delay.due(2_100));
    assert!(delay.due(2_500));
}

#[test]
fn progress_text_is_throttled_to_four_updates_per_second() {
    let mut throttle = ProgressThrottle::default();
    assert!(throttle.should_show(0));
    assert!(!throttle.should_show(100));
    assert!(!throttle.should_show(249));
    assert!(throttle.should_show(250));
    assert!(!throttle.should_show(300));
    assert!(throttle.should_show(500));
}

#[test]
fn the_same_visible_tooltip_moves_from_calculating_to_progress_to_result() {
    let mut model = tooltip("/root", 1, 1);
    assert_eq!(model.phase, TooltipPhase::Idle);
    assert!(model.text().contains("Size: Folder"));

    model.start(None, 1_000);
    let calculating = model.text();
    assert!(calculating.contains("Folder\nType: Folder"));
    assert!(calculating.contains("Size: Calculating…"));
    assert!(calculating.contains("Modified: 2026-11-03 12:00"));

    model.set_progress(Progress {
        files: 240,
        folders: 18,
        visited: 259,
    });
    assert!(model.text().contains("Found 240 files, 18 folders"));

    let size = FolderSize {
        identity: identity("/root", 1, 1),
        totals: FolderTotals {
            contents_bytes: 13_000_000,
            disk_bytes: 13_200_000,
            files: 27,
            folders: 3,
            ..FolderTotals::default()
        },
    };
    model.finish(ScanResult::Sized(size), 0, false);
    let complete = model.text();
    assert!(complete.contains("Contents: 12.4 MiB"));
    assert!(complete.contains("On disk: 12.6 MiB"));
    assert!(complete.contains("Contains: 27 files, 3 folders"));
    assert!(complete.contains("Calculated: just now"));
    assert!(!complete.contains("Calculating"));
}

#[test]
fn a_cached_result_is_labelled_and_keeps_its_age() {
    let mut model = tooltip("/root", 1, 1);
    let size = FolderSize {
        identity: identity("/root", 1, 1),
        totals: FolderTotals {
            contents_bytes: 1,
            files: 1,
            ..FolderTotals::default()
        },
    };
    model.finish(ScanResult::Sized(size), 12_000, true);
    let text = model.text();
    assert!(text.contains("Calculated: 12 s ago (Cached)"));
}

#[test]
fn a_partial_result_is_never_presented_as_the_complete_size() {
    let mut model = tooltip("/root", 1, 1);
    let size = FolderSize {
        identity: identity("/root", 1, 1),
        totals: FolderTotals {
            contents_bytes: 1,
            files: 1,
            partial: vec![PartialReason::PermissionDenied],
            ..FolderTotals::default()
        },
    };
    model.finish(ScanResult::Sized(size), 0, false);
    let text = model.text();
    assert!(text.contains("Contents: At least 1 B (partial)"));
    assert!(text.contains("Partial: permission denied"));
    assert!(!text.contains("Contents: 1 B\n"));
}

#[test]
fn a_stale_total_is_explicit_while_updating() {
    let mut model = tooltip("/root", 1, 1);
    let old = CachedFolderSize {
        size: FolderSize {
            identity: identity("/root", 1, 1),
            totals: FolderTotals {
                contents_bytes: 10,
                files: 1,
                ..FolderTotals::default()
            },
        },
        completed_at_ms: 1_000,
    };
    model.start(Some(old), 20_000);
    let text = model.text();
    assert!(text.contains("Size: Last calculation (stale, updating…)"));
    assert!(text.contains("Contents: 10 B"));
    assert!(!text.contains("(Cached)"));
}

#[test]
fn remote_symlink_and_unavailable_roots_have_distinct_details() {
    let mut remote = tooltip("/root", 1, 1);
    remote.finish(ScanResult::Unavailable(RootState::Remote), 0, false);
    assert!(remote.text().contains("Not calculated for remote folders"));

    let mut link = tooltip("/root", 1, 1);
    link.finish(ScanResult::Unavailable(RootState::Symlink), 0, false);
    assert!(link.text().contains("symlink target was not scanned"));

    let mut missing = tooltip("/root", 1, 1);
    missing.finish(ScanResult::Unavailable(RootState::Missing), 0, false);
    assert!(missing.text().contains("Size unavailable"));
}

#[test]
fn a_recycled_cell_rejects_the_same_name_under_another_root() {
    let model = tooltip("/a/Folder", 1, 10);
    assert!(model.matches(
        &identity("/a/Folder", 1, 10),
        &OsString::from("Folder"),
        &revision()
    ));
    assert!(!model.matches(
        &identity("/b/Folder", 1, 11),
        &OsString::from("Folder"),
        &revision()
    ));
    assert!(!model.matches(
        &identity("/a/Folder", 1, 10),
        &OsString::from("Other"),
        &revision()
    ));
    let mut newer = revision();
    newer.modified_sec = 2;
    assert!(!model.matches(
        &identity("/a/Folder", 1, 10),
        &OsString::from("Folder"),
        &newer
    ));
}

#[test]
fn a_cancelled_model_returns_to_the_plain_folder_details() {
    let mut model = tooltip("/root", 1, 1);
    model.start(None, 0);
    model.finish(ScanResult::Cancelled, 0, false);
    assert_eq!(model.phase, TooltipPhase::Idle);
    assert!(model.text().contains("Size: Folder"));
    assert!(!model.text().contains("Calculating"));
}

#[test]
fn the_service_reports_a_real_scanned_result_then_a_cached_hit() {
    let mut source = TestSource::default();
    source.add("/root", dir(1, 1));
    source.add("/root/a", regular(1, 2, 100));
    source.child("/root", "/root/a");
    let clock = Arc::new(ManualClock::new(1_000));
    let service = service_with(source, clock.clone());

    let started = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("expected a started scan"),
    };
    let event = started.wait(Duration::from_secs(5)).expect("a result");
    let ScanResult::Sized(size) = event.result else {
        panic!("expected a sized result");
    };
    assert_eq!(size.totals.contents_bytes, 100);
    assert!(!event.cached);

    let cached = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Cached(cached) => cached,
        _ => panic!("expected a cached result"),
    };
    assert_eq!(cached.size.totals.contents_bytes, 100);
    assert_eq!(cached.completed_at_ms, 1_000);
}

#[test]
fn an_expired_cache_starts_a_new_scan_and_refresh_invalidates_it() {
    let mut source = TestSource::default();
    source.add("/root", dir(1, 1));
    source.add("/root/a", regular(1, 2, 100));
    source.child("/root", "/root/a");
    let clock = Arc::new(ManualClock::new(1_000));
    let service = service_with(source, clock.clone());

    let started = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("expected a started scan"),
    };
    started.wait(Duration::from_secs(5)).expect("a result");

    clock.advance(30_001);
    let expired = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("expired cache must recalculate"),
    };
    assert!(expired.stale.is_some());
    service.cancel(&expired);

    service.invalidate_path(Path::new("/root"));
    let refreshed = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("Refresh must request fresh work"),
    };
    assert!(refreshed.stale.is_none());
    refreshed.wait(Duration::from_secs(5)).expect("a result");
}

#[test]
fn remote_and_changed_roots_are_refused_without_reading_the_tree() {
    let remote = TestSource {
        state: Some(RootState::Remote),
        ..TestSource::default()
    };
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(remote, clock.clone());
    let started = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("expected a started probe"),
    };
    let event = started.wait(Duration::from_secs(5)).expect("a result");
    assert_eq!(event.result, ScanResult::Unavailable(RootState::Remote));
    // A remote refusal is not cached as a local total.
    assert!(matches!(
        service.request(identity("/root", 1, 1)),
        RequestOutcome::Started(_)
    ));
}

#[test]
fn a_changed_root_identity_is_not_applied_to_the_request() {
    let mut source = TestSource::default();
    source.add("/root", dir(2, 99));
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);
    let started = match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(started) => started,
        _ => panic!("expected a started probe"),
    };
    let event = started.wait(Duration::from_secs(5)).expect("a result");
    assert_eq!(
        event.result,
        ScanResult::Unavailable(RootState::Unavailable)
    );
}

#[test]
fn two_scans_run_and_eight_demands_wait_in_the_bounded_queue() {
    let gate = Arc::new(Gate::default());
    let mut source = TestSource {
        gate: Some(gate.clone()),
        ..TestSource::default()
    };
    for index in 0..14 {
        let path = format!("/root{index}");
        source.add(&path, dir(1, index + 1));
    }
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);

    let mut started = Vec::new();
    for index in 0..2 {
        let outcome = service.request(identity(&format!("/root{index}"), 1, index + 1));
        match outcome {
            RequestOutcome::Started(handle) => started.push(handle),
            _ => panic!("expected the active scans to start"),
        }
    }
    gate.wait_entered(2);
    for index in 2..10 {
        match service.request(identity(&format!("/root{index}"), 1, index + 1)) {
            RequestOutcome::Started(handle) => started.push(handle),
            _ => panic!("a pending slot should be available"),
        }
    }
    assert!(matches!(
        service.request(identity("/root10", 1, 11)),
        RequestOutcome::Busy
    ));
    gate.open();
    let mut results = 0;
    for handle in &started {
        if handle.wait(Duration::from_secs(5)).is_some() {
            results += 1;
        }
    }
    assert_eq!(results, 10);
}

#[test]
fn cancelling_a_queued_demand_frees_its_slot_and_produces_no_result() {
    let gate = Arc::new(Gate::default());
    let mut source = TestSource {
        gate: Some(gate.clone()),
        ..TestSource::default()
    };
    for index in 0..14 {
        let path = format!("/root{index}");
        source.add(&path, dir(1, index + 1));
    }
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);

    let mut active = Vec::new();
    for index in 0..2 {
        let RequestOutcome::Started(handle) =
            service.request(identity(&format!("/root{index}"), 1, index + 1))
        else {
            panic!("active slot");
        };
        active.push(handle);
    }
    gate.wait_entered(2);
    let mut pending = Vec::new();
    for index in 2..10 {
        let RequestOutcome::Started(handle) =
            service.request(identity(&format!("/root{index}"), 1, index + 1))
        else {
            panic!("pending slot");
        };
        pending.push(handle);
    }
    let cancelled = pending.pop().expect("one pending");
    let cancelled_token = cancelled.token;
    service.cancel(&cancelled);
    let replacement = match service.request(identity("/root12", 1, 13)) {
        RequestOutcome::Started(handle) => handle,
        _ => panic!("cancelling a queued demand must free its slot"),
    };
    gate.open();

    for handle in &active {
        assert!(handle.wait(Duration::from_secs(5)).is_some());
    }
    for handle in &pending {
        assert!(handle.wait(Duration::from_secs(5)).is_some());
    }
    let result = replacement.wait(Duration::from_secs(5)).expect("a result");
    assert_ne!(result.token, cancelled_token);
    assert!(cancelled.wait(Duration::from_millis(100)).is_none());
}

#[test]
fn a_keyboard_focused_item_gets_the_same_states_without_a_pointer() {
    // The production tooltip model is fed by the focused cell's own binding,
    // not by pointer coordinates; the hover and keyboard paths share it.
    let mut model = tooltip("/root", 1, 1);
    model.start(None, 0);
    assert!(model.text().contains("Size: Calculating…"));
    model.set_progress(Progress {
        files: 1,
        folders: 0,
        visited: 1,
    });
    assert!(model.text().contains("Found 1 file, 0 folders"));
    let size = FolderSize {
        identity: identity("/root", 1, 1),
        totals: FolderTotals {
            contents_bytes: 0,
            disk_bytes: 4096,
            ..FolderTotals::default()
        },
    };
    model.finish(ScanResult::Sized(size), 0, false);
    assert!(model.text().contains("Contents: 0 B"));
    assert!(model.text().contains("Contains: 0 files, 0 folders"));
}

#[test]
fn shutdown_refuses_new_demands() {
    let mut source = TestSource::default();
    source.add("/root", dir(1, 1));
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);
    service.shutdown();
    assert!(matches!(
        service.request(identity("/root", 1, 1)),
        RequestOutcome::Stopped
    ));
}

#[test]
fn cancelling_an_active_scan_does_not_install_a_cached_complete_result() {
    let gate = Arc::new(Gate::default());
    let mut source = TestSource {
        gate: Some(gate.clone()),
        ..TestSource::default()
    };
    source.add("/root", dir(1, 1));
    source.add("/root/a", regular(1, 2, 100));
    source.child("/root", "/root/a");
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);

    let RequestOutcome::Started(started) = service.request(identity("/root", 1, 1)) else {
        panic!("expected a started scan");
    };
    gate.wait_entered(1);
    service.cancel(&started);
    gate.open();
    let event = started
        .wait(Duration::from_secs(5))
        .expect("a cancellation");
    assert_eq!(event.result, ScanResult::Cancelled);
    match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(next) => service.cancel(&next),
        _ => panic!("a cancelled scan must not be cached as a complete total"),
    }
}

#[test]
fn invalidation_during_an_active_scan_does_not_repopulate_the_cache() {
    let gate = Arc::new(Gate::default());
    let mut source = TestSource {
        gate: Some(gate.clone()),
        ..TestSource::default()
    };
    source.add("/root", dir(1, 1));
    source.add("/root/a", regular(1, 2, 100));
    source.child("/root", "/root/a");
    let clock = Arc::new(ManualClock::new(0));
    let service = service_with(source, clock);
    let RequestOutcome::Started(started) = service.request(identity("/root", 1, 1)) else {
        panic!("expected a started scan");
    };
    gate.wait_entered(1);
    service.invalidate_all();
    gate.open();
    let event = started.wait(Duration::from_secs(5)).expect("a result");
    assert!(matches!(event.result, ScanResult::Sized(_)));
    match service.request(identity("/root", 1, 1)) {
        RequestOutcome::Started(next) => service.cancel(&next),
        _ => panic!("an invalidated in-flight result must not be cached"),
    }
}
