// SPDX-License-Identifier: GPL-3.0-or-later
//! Recursive folder totals for the delayed hover details.
//!
//! The totals are deliberately filesystem-reported numbers, not an exact
//! exclusive measure. Contents sums regular-file lengths; On disk sums the
//! allocated 512-byte blocks of visited files, directories (including the
//! root) and symlinks. Hard-linked data is counted once. Symlink targets are
//! never walked, special files are never opened, and the walk stays on the
//! root device.

mod walk;

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};

pub use walk::{
    Clock, DirReader, ManualClock, NativeSource, ScanSource, SystemClock, entry_meta, probe_native,
    scan, scan_with_root,
};

/// The policy version participates in every cache key. Change it when the
/// meaning of a stored total changes.
pub const POLICY_VERSION: u32 = 1;
/// At most two scans run at once, process-wide.
pub const MAX_ACTIVE_SCANS: usize = 2;
/// At most this many current demands wait behind the active scans.
pub const MAX_PENDING_DEMANDS: usize = 8;
/// The walk stops after this many visited entries.
pub const MAX_VISITED_ENTRIES: u64 = 100_000;
/// The walk stops descending at this directory depth (the root is depth 0).
pub const MAX_DEPTH: usize = 128;
/// One scan's wall-clock budget.
pub const DEFAULT_BUDGET_MS: u64 = 5_000;
/// A completed or partial result stays usable for this long.
pub const CACHE_TTL_MS: u64 = 30_000;
/// The process-memory cache is bounded to this many roots.
pub const CACHE_ROOTS: usize = 256;
/// Progress updates are emitted no more often than this.
pub const PROGRESS_INTERVAL_MS: u64 = 250;

/// The identity a cache key and a scan request use. The revision is the
/// listing's modified time; it is not a recursive change monitor.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RootIdentity {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub modified_sec: i64,
    pub modified_nsec: i64,
}

impl RootIdentity {
    pub fn cache_key(&self, policy_version: u32) -> CacheKey {
        CacheKey {
            path: self.path.clone(),
            device: self.device,
            inode: self.inode,
            modified_sec: self.modified_sec,
            modified_nsec: self.modified_nsec,
            policy_version,
        }
    }
}

/// What a root probe found before any recursive work starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootState {
    Ready,
    Missing,
    PermissionDenied,
    Symlink,
    Remote,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    Other,
}

/// One lstat result. `blocks` is the Unix 512-byte block count, not the
/// filesystem's preferred block-size field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryMeta {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub blocks: u64,
    pub nlink: u64,
    pub kind: EntryKind,
    pub modified_sec: i64,
    pub modified_nsec: i64,
}

impl EntryMeta {
    pub fn allocated_bytes(&self) -> u64 {
        self.blocks.saturating_mul(512)
    }
}

#[derive(Clone, Debug)]
pub struct RootProbe {
    pub state: RootState,
    pub identity: Option<RootIdentity>,
    pub meta: Option<EntryMeta>,
}

impl RootProbe {
    pub fn unavailable(state: RootState) -> Self {
        Self {
            state,
            identity: None,
            meta: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub files: u64,
    pub folders: u64,
    pub visited: u64,
}

/// Why a scan stopped before covering everything it could see. Partial totals
/// are always displayed as "At least", never as a complete folder size.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PartialReason {
    PermissionDenied,
    EntryLimit,
    DepthLimit,
    TimeLimit,
    Changed,
    Overflow,
    Unreadable,
}

impl PartialReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::PermissionDenied => "permission denied",
            Self::EntryLimit => "scan limit",
            Self::DepthLimit => "depth limit",
            Self::TimeLimit => "time limit",
            Self::Changed => "items changed during scan",
            Self::Overflow => "size overflow",
            Self::Unreadable => "unreadable entry",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderTotals {
    pub contents_bytes: u64,
    pub disk_bytes: u64,
    pub files: u64,
    pub folders: u64,
    pub symlinks: u64,
    pub hard_links_deduplicated: u64,
    pub mounts_skipped: u64,
    pub aliases_skipped: u64,
    pub partial: Vec<PartialReason>,
}

impl FolderTotals {
    pub fn is_partial(&self) -> bool {
        !self.partial.is_empty()
    }

    pub fn contains_text(&self) -> String {
        let at_least = if self.is_partial() { " (at least)" } else { "" };
        format!(
            "{}, {}{at_least}",
            count_text(self.files, "file"),
            count_text(self.folders, "folder")
        )
    }
}

fn count_text(count: u64, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderSize {
    pub identity: RootIdentity,
    pub totals: FolderTotals,
}

impl FolderSize {
    pub fn is_partial(&self) -> bool {
        self.totals.is_partial()
    }

    pub fn partial_label(&self) -> Option<String> {
        (!self.totals.partial.is_empty()).then(|| {
            self.totals
                .partial
                .iter()
                .map(|reason| reason.label())
                .collect::<Vec<_>>()
                .join(", ")
        })
    }
}

/// The walker's outcome. Cancellation is never turned into a cached result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanResult {
    Sized(FolderSize),
    Cancelled,
    Unavailable(RootState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanPolicy {
    pub version: u32,
    pub max_entries: u64,
    pub max_depth: usize,
    pub budget_ms: u64,
}

impl Default for ScanPolicy {
    fn default() -> Self {
        Self {
            version: POLICY_VERSION,
            max_entries: MAX_VISITED_ENTRIES,
            max_depth: MAX_DEPTH,
            budget_ms: DEFAULT_BUDGET_MS,
        }
    }
}

/// The exact cache identity: absolute root, device/inode, listing revision and
/// scan-policy version.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub path: PathBuf,
    pub device: u64,
    pub inode: u64,
    pub modified_sec: i64,
    pub modified_nsec: i64,
    pub policy_version: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CachedFolderSize {
    pub size: FolderSize,
    pub completed_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CacheLookup {
    Miss,
    Fresh(CachedFolderSize),
    Expired(CachedFolderSize),
}

/// A small process-memory cache. It never pretends that a root mtime sees all
/// descendant edits; the TTL is the fallback for changes the view did not see.
#[derive(Debug)]
pub struct FolderSizeCache {
    entries: HashMap<CacheKey, CachedFolderSize>,
    order: VecDeque<CacheKey>,
    capacity: usize,
    ttl_ms: u64,
}

impl Default for FolderSizeCache {
    fn default() -> Self {
        Self::new(CACHE_ROOTS)
    }
}

impl FolderSizeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
            ttl_ms: CACHE_TTL_MS,
        }
    }

    pub fn with_ttl(capacity: usize, ttl_ms: u64) -> Self {
        Self {
            ttl_ms,
            ..Self::new(capacity)
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &CacheKey, now_ms: u64) -> CacheLookup {
        match self.entries.get(key) {
            None => CacheLookup::Miss,
            Some(entry) if now_ms.saturating_sub(entry.completed_at_ms) <= self.ttl_ms => {
                CacheLookup::Fresh(entry.clone())
            }
            Some(entry) => CacheLookup::Expired(entry.clone()),
        }
    }

    pub fn insert(&mut self, key: CacheKey, size: FolderSize, now_ms: u64) {
        if self.entries.contains_key(&key) {
            self.order.retain(|candidate| candidate != &key);
        } else if self.entries.len() >= self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.entries.remove(&oldest);
        }
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            CachedFolderSize {
                size,
                completed_at_ms: now_ms,
            },
        );
    }

    /// Refresh invalidates totals displayed for child folders and cached
    /// ancestors that include this subtree. Unrelated siblings stay cached.
    pub fn invalidate_path(&mut self, path: &Path) {
        let keys: Vec<CacheKey> = self
            .entries
            .keys()
            .filter(|key| key.path.starts_with(path) || path.starts_with(&key.path))
            .cloned()
            .collect();
        for key in keys {
            self.entries.remove(&key);
            self.order.retain(|candidate| candidate != &key);
        }
    }

    /// Known file-operation or listing changes invalidate every known root
    /// conservatively.
    pub fn invalidate_all(&mut self) {
        self.entries.clear();
        self.order.clear();
    }
}

pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const TIB: f64 = 1024.0 * 1024.0 * 1024.0 * 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let value = bytes as f64;
    if value < MIB {
        format!("{:.1} KiB", value / KIB)
    } else if value < GIB {
        format!("{:.1} MiB", value / MIB)
    } else if value < TIB {
        format!("{:.1} GiB", value / GIB)
    } else {
        format!("{:.1} TiB", value / TIB)
    }
}

pub fn age_text(age_ms: u64) -> String {
    if age_ms < 1_000 {
        "just now".to_string()
    } else if age_ms < 60_000 {
        format!("{} s ago", age_ms / 1_000)
    } else if age_ms < 3_600_000 {
        format!("{} min ago", age_ms / 60_000)
    } else {
        format!("{} h ago", age_ms / 3_600_000)
    }
}

/// The completed/partial detail lines. The caller keeps the name, type and
/// modified lines above these.
pub fn folder_size_lines(size: &FolderSize, age_ms: u64, cached: bool) -> Vec<String> {
    let partial = size.is_partial();
    let at_least = if partial { "At least " } else { "" };
    let partial_note = if partial { " (partial)" } else { "" };
    let mut lines = vec![
        format!(
            "Contents: {at_least}{}{partial_note}",
            format_bytes(size.totals.contents_bytes)
        ),
        format!(
            "On disk: {at_least}{}{partial_note}",
            format_bytes(size.totals.disk_bytes)
        ),
        format!("Contains: {}", size.totals.contains_text()),
    ];
    if let Some(label) = size.partial_label() {
        lines.push(format!("Partial: {label}"));
    }
    if size.totals.hard_links_deduplicated > 0 {
        let count = size.totals.hard_links_deduplicated;
        lines.push(format!(
            "Hard links: {count} duplicate entr{} counted once",
            if count == 1 { "y" } else { "ies" }
        ));
    }
    if size.totals.mounts_skipped > 0 {
        lines.push(format!(
            "Skipped: {} mounted filesystem(s)",
            size.totals.mounts_skipped
        ));
    }
    if size.totals.aliases_skipped > 0 {
        lines.push(format!(
            "Skipped: {} repeated director{}",
            size.totals.aliases_skipped,
            if size.totals.aliases_skipped == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }
    lines.push(format!(
        "Calculated: {}{}",
        age_text(age_ms),
        if cached { " (Cached)" } else { "" }
    ));
    lines
}

#[cfg(test)]
mod tests;
