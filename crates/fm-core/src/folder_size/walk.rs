// SPDX-License-Identifier: GPL-3.0-or-later
//! The native metadata walk and its injectable source/clock boundaries.
//!
//! The walk is iterative and cooperative: cancellation and the budget are
//! checked between filesystem operations. A blocked native `read_dir` or
//! `lstat` cannot be interrupted by a Rust flag, so cancellation is honest
//! about waiting for that call to return; it never blocks the UI thread.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use gio as gio_crate;
use gio::prelude::*;

use super::{
    EntryKind, EntryMeta, FolderSize, FolderTotals, PROGRESS_INTERVAL_MS, PartialReason, Progress,
    RootIdentity, RootProbe, RootState, ScanPolicy, ScanResult,
};

/// The production filesystem/GIO boundary. Tests inject their own source so
/// remote and cross-device cases never require a real mount.
pub trait ScanSource: Send + Sync {
    fn probe(&self, path: &Path) -> RootProbe;
    fn lstat(&self, path: &Path) -> io::Result<EntryMeta>;
    /// Open a directory as a streaming reader. The caller consumes it between
    /// cancellation/budget checks, so a huge directory does not have to be
    /// materialised before the scan can stop.
    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>>;
}

pub trait DirReader {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>>;
}

struct NativeDir(fs::ReadDir);

impl DirReader for NativeDir {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        self.0.next().map(|entry| entry.map(|entry| entry.path()))
    }
}

pub struct NativeSource;

impl ScanSource for NativeSource {
    fn probe(&self, path: &Path) -> RootProbe {
        probe_native(path)
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        fs::symlink_metadata(path).map(entry_meta)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        Ok(Box::new(NativeDir(fs::read_dir(path)?)))
    }
}

pub fn entry_meta(metadata: fs::Metadata) -> EntryMeta {
    let file_type = metadata.file_type();
    let kind = if file_type.is_symlink() {
        EntryKind::Symlink
    } else if file_type.is_dir() {
        EntryKind::Directory
    } else if file_type.is_file() {
        EntryKind::File
    } else {
        EntryKind::Other
    };
    EntryMeta {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.size(),
        blocks: metadata.blocks(),
        nlink: metadata.nlink(),
        kind,
        modified_sec: metadata.mtime(),
        modified_nsec: metadata.mtime_nsec(),
    }
}

/// Probe a local path with GIO metadata before any recursive work. The GIO
/// call runs on the caller's worker thread, never on the UI thread.
pub fn probe_native(path: &Path) -> RootProbe {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) => {
            return RootProbe::unavailable(match error.kind() {
                io::ErrorKind::NotFound => RootState::Missing,
                io::ErrorKind::PermissionDenied => RootState::PermissionDenied,
                _ => RootState::Unavailable,
            });
        }
    };
    let meta = entry_meta(metadata);
    let identity = RootIdentity {
        path: path.to_path_buf(),
        device: meta.device,
        inode: meta.inode,
        modified_sec: meta.modified_sec,
        modified_nsec: meta.modified_nsec,
    };
    if meta.kind == EntryKind::Symlink {
        return RootProbe {
            state: RootState::Symlink,
            identity: Some(identity),
            meta: Some(meta),
        };
    }
    if meta.kind != EntryKind::Directory {
        return RootProbe {
            state: RootState::Unavailable,
            identity: Some(identity),
            meta: Some(meta),
        };
    }
    let file = gio_crate::File::for_path(path);
    if !file.is_native() {
        return RootProbe {
            state: RootState::Remote,
            identity: Some(identity),
            meta: Some(meta),
        };
    }
    if let Ok(info) =
        file.query_filesystem_info("filesystem::remote", None::<&gio_crate::Cancellable>)
        && info.boolean("filesystem::remote")
    {
        return RootProbe {
            state: RootState::Remote,
            identity: Some(identity),
            meta: Some(meta),
        };
    }
    RootProbe {
        state: RootState::Ready,
        identity: Some(identity),
        meta: Some(meta),
    }
}

/// Monotonic milliseconds, used for the scan budget, progress throttle and
/// cache TTL. Tests substitute a manual clock.
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        static START: OnceLock<Instant> = OnceLock::new();
        START
            .get_or_init(Instant::now)
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }
}

/// A simple atomic clock for tests; no sleeps are needed to cross the budget,
/// progress interval or cache TTL.
#[derive(Debug, Default)]
pub struct ManualClock {
    now_ms: AtomicU64,
}

impl ManualClock {
    pub fn new(now_ms: u64) -> Self {
        Self {
            now_ms: AtomicU64::new(now_ms),
        }
    }

    pub fn set(&self, now_ms: u64) {
        self.now_ms.store(now_ms, Ordering::SeqCst);
    }

    pub fn advance(&self, delta_ms: u64) {
        self.now_ms.fetch_add(delta_ms, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> u64 {
        self.now_ms.load(Ordering::SeqCst)
    }
}

/// Probe one folder and run the walk only when the probe allows it. A changed
/// device/inode, remote root or symlink is refused without recursive work.
pub fn scan_with_root(
    request: &RootIdentity,
    probe: &RootProbe,
    policy: ScanPolicy,
    source: &dyn ScanSource,
    clock: &dyn Clock,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> ScanResult {
    if probe.state != RootState::Ready {
        return match probe.state {
            RootState::Remote => ScanResult::Unavailable(RootState::Remote),
            RootState::Symlink => ScanResult::Unavailable(RootState::Symlink),
            state => ScanResult::Unavailable(state),
        };
    }
    let Some(metadata) = probe.meta else {
        return ScanResult::Unavailable(RootState::Unavailable);
    };
    // A listing that named a different inode/device must not receive this
    // path's result, even when the folder has the same basename.
    if request.device != 0
        && request.inode != 0
        && (request.device != metadata.device || request.inode != metadata.inode)
    {
        return ScanResult::Unavailable(RootState::Unavailable);
    }
    scan(request, metadata, policy, source, clock, cancel, progress)
}

/// Walk `root` on its own device. `root_meta` is the probe's lstat result; the
/// requested identity is retained in the result/cache key after its
/// device/inode have been checked by the caller.
pub fn scan(
    root: &RootIdentity,
    root_meta: EntryMeta,
    policy: ScanPolicy,
    source: &dyn ScanSource,
    clock: &dyn Clock,
    cancel: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Progress),
) -> ScanResult {
    if cancel() {
        return ScanResult::Cancelled;
    }
    if root_meta.kind != EntryKind::Directory {
        return ScanResult::Unavailable(RootState::Unavailable);
    }
    let root_device = root_meta.device;
    let started = clock.now_ms();
    let deadline = started.saturating_add(policy.budget_ms);
    let mut totals = FolderTotals::default();
    let mut overflowed = false;
    if !add_disk(&mut totals, root_meta.allocated_bytes()) {
        overflowed = true;
    }

    let mut visited_dirs: HashSet<(u64, u64)> = HashSet::new();
    visited_dirs.insert((root_meta.device, root_meta.inode));
    let mut seen_data: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.path.clone(), 0)];
    let mut visited: u64 = 1;
    let mut last_progress = started;

    'walk: while let Some((directory, depth)) = stack.pop() {
        if cancel() {
            return ScanResult::Cancelled;
        }
        if clock.now_ms() >= deadline {
            push_reason(&mut totals.partial, PartialReason::TimeLimit);
            break;
        }
        if visited >= policy.max_entries {
            push_reason(&mut totals.partial, PartialReason::EntryLimit);
            break;
        }
        let mut entries = match source.read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                push_reason(&mut totals.partial, reason_for(&error));
                continue;
            }
        };
        loop {
            if cancel() {
                return ScanResult::Cancelled;
            }
            if clock.now_ms() >= deadline {
                push_reason(&mut totals.partial, PartialReason::TimeLimit);
                break 'walk;
            }
            if visited >= policy.max_entries {
                push_reason(&mut totals.partial, PartialReason::EntryLimit);
                break 'walk;
            }
            let Some(entry) = entries.next_entry() else {
                break;
            };
            let path = match entry {
                Ok(path) => path,
                Err(error) => {
                    push_reason(&mut totals.partial, reason_for(&error));
                    continue;
                }
            };
            visited = visited.saturating_add(1);
            let metadata = match source.lstat(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    push_reason(&mut totals.partial, reason_for(&error));
                    continue;
                }
            };
            let ok = match metadata.kind {
                EntryKind::Directory => {
                    if depth >= policy.max_depth {
                        push_reason(&mut totals.partial, PartialReason::DepthLimit);
                        continue;
                    }
                    if metadata.device != root_device {
                        totals.mounts_skipped = totals.mounts_skipped.saturating_add(1);
                        continue;
                    }
                    if !visited_dirs.insert((metadata.device, metadata.inode)) {
                        totals.aliases_skipped = totals.aliases_skipped.saturating_add(1);
                        continue;
                    }
                    let ok = add_disk(&mut totals, metadata.allocated_bytes())
                        && add_folder(&mut totals);
                    if ok {
                        stack.push((path, depth + 1));
                    }
                    ok
                }
                EntryKind::Symlink => {
                    add_file(&mut totals)
                        && add_symlink(&mut totals)
                        && if seen_data.insert((metadata.device, metadata.inode)) {
                            add_disk(&mut totals, metadata.allocated_bytes())
                        } else {
                            totals.hard_links_deduplicated =
                                totals.hard_links_deduplicated.saturating_add(1);
                            true
                        }
                }
                EntryKind::File => {
                    add_file(&mut totals)
                        && if seen_data.insert((metadata.device, metadata.inode)) {
                            add_contents(&mut totals, metadata.size)
                                && add_disk(&mut totals, metadata.allocated_bytes())
                        } else {
                            totals.hard_links_deduplicated =
                                totals.hard_links_deduplicated.saturating_add(1);
                            true
                        }
                }
                EntryKind::Other => {
                    // Never open a FIFO, device or socket. Its metadata may
                    // still be listed, but it contributes no data or blocks.
                    add_file(&mut totals)
                }
            };
            if !ok {
                overflowed = true;
                break 'walk;
            }
            let now = clock.now_ms();
            if now.saturating_sub(last_progress) >= PROGRESS_INTERVAL_MS {
                last_progress = now;
                progress(Progress {
                    files: totals.files,
                    folders: totals.folders,
                    visited,
                });
            }
        }
    }
    if overflowed {
        push_reason(&mut totals.partial, PartialReason::Overflow);
    }
    ScanResult::Sized(FolderSize {
        identity: root.clone(),
        totals,
    })
}

fn reason_for(error: &io::Error) -> PartialReason {
    match error.kind() {
        io::ErrorKind::PermissionDenied => PartialReason::PermissionDenied,
        io::ErrorKind::NotFound => PartialReason::Changed,
        _ => PartialReason::Unreadable,
    }
}

fn push_reason(reasons: &mut Vec<PartialReason>, reason: PartialReason) {
    if !reasons.contains(&reason) {
        reasons.push(reason);
    }
}

fn add_contents(totals: &mut FolderTotals, bytes: u64) -> bool {
    match totals.contents_bytes.checked_add(bytes) {
        Some(value) => {
            totals.contents_bytes = value;
            true
        }
        None => false,
    }
}

fn add_disk(totals: &mut FolderTotals, bytes: u64) -> bool {
    match totals.disk_bytes.checked_add(bytes) {
        Some(value) => {
            totals.disk_bytes = value;
            true
        }
        None => false,
    }
}

fn add_file(totals: &mut FolderTotals) -> bool {
    match totals.files.checked_add(1) {
        Some(value) => {
            totals.files = value;
            true
        }
        None => false,
    }
}

fn add_folder(totals: &mut FolderTotals) -> bool {
    match totals.folders.checked_add(1) {
        Some(value) => {
            totals.folders = value;
            true
        }
        None => false,
    }
}

fn add_symlink(totals: &mut FolderTotals) -> bool {
    match totals.symlinks.checked_add(1) {
        Some(value) => {
            totals.symlinks = value;
            true
        }
        None => false,
    }
}
