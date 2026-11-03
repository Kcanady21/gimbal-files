// SPDX-License-Identifier: GPL-3.0-or-later
//! Production walker/cache tests. Synthetic sources cover remote, mounted,
//! permission and disappearing entries without a real mount; the real-fixture
//! tests use only owned `/tmp/gimbal-folder-size-*` trees.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::os::unix::fs::{MetadataExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::walk::{DirReader, ManualClock, NativeSource, ScanSource, probe_native, scan_with_root};
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

fn dir(device: u64, inode: u64, blocks: u64) -> EntryMeta {
    meta(EntryKind::Directory, device, inode, 4096, blocks)
}

fn regular(device: u64, inode: u64, size: u64, blocks: u64) -> EntryMeta {
    meta(EntryKind::File, device, inode, size, blocks)
}

#[derive(Default)]
struct FakeSource {
    nodes: HashMap<PathBuf, EntryMeta>,
    children: HashMap<PathBuf, Vec<PathBuf>>,
    read_errors: HashMap<PathBuf, io::ErrorKind>,
    lstat_errors: HashMap<PathBuf, io::ErrorKind>,
    root_state: Option<RootState>,
    reads: AtomicUsize,
}

impl FakeSource {
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

impl ScanSource for FakeSource {
    fn probe(&self, path: &Path) -> RootProbe {
        if let Some(state) = self.root_state {
            return RootProbe::unavailable(state);
        }
        let Some(meta) = self.nodes.get(path).copied() else {
            return RootProbe::unavailable(RootState::Missing);
        };
        RootProbe {
            state: if meta.kind == EntryKind::Directory {
                RootState::Ready
            } else {
                RootState::Unavailable
            },
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
        if let Some(kind) = self.lstat_errors.get(path) {
            return Err(io::Error::from(*kind));
        }
        self.nodes
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        if let Some(kind) = self.read_errors.get(path) {
            return Err(io::Error::from(*kind));
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

struct VecDir(std::vec::IntoIter<PathBuf>);

impl DirReader for VecDir {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        self.0.next().map(Ok)
    }
}

fn request(path: &str, device: u64, inode: u64) -> RootIdentity {
    RootIdentity {
        path: PathBuf::from(path),
        device,
        inode,
        modified_sec: 1,
        modified_nsec: 2,
    }
}

fn scan_fake(source: &FakeSource, root: &RootIdentity, policy: ScanPolicy) -> ScanResult {
    let clock = ManualClock::new(1_000);
    let probe = source.probe(&root.path);
    scan_with_root(root, &probe, policy, source, &clock, &|| false, &mut |_| {})
}

fn sized(result: ScanResult) -> FolderSize {
    match result {
        ScanResult::Sized(size) => size,
        other => panic!("expected a sized result, got {other:?}"),
    }
}

#[test]
fn empty_folders_have_zero_contents_and_no_contained_entries() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 0);
    assert_eq!(size.totals.disk_bytes, 8 * 512);
    assert_eq!(size.totals.files, 0);
    assert_eq!(size.totals.folders, 0);
    assert!(!size.is_partial());
    let lines = folder_size_lines(&size, 0, false);
    assert_eq!(lines[0], "Contents: 0 B");
    assert_eq!(lines[2], "Contains: 0 files, 0 folders");
}

#[test]
fn hard_links_are_counted_once_but_still_count_as_entries() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/a", regular(1, 10, 100, 8));
    let mut linked = regular(1, 10, 100, 8);
    linked.nlink = 2;
    source.add("/root/a", linked);
    source.add("/root/b", linked);
    source.child("/root", "/root/a");
    source.child("/root", "/root/b");
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 100);
    assert_eq!(size.totals.disk_bytes, 8 * 512 * 2);
    assert_eq!(size.totals.files, 2);
    assert_eq!(size.totals.hard_links_deduplicated, 1);
    let lines = folder_size_lines(&size, 0, false);
    assert!(lines.iter().any(|line| line.contains("duplicate entry")));
}

#[test]
fn symlink_entries_contribute_their_own_blocks_but_never_their_targets() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/link", meta(EntryKind::Symlink, 1, 20, 40, 1));
    source.add("/root/target", regular(1, 21, 999, 2));
    source.child("/root", "/root/link");
    source.child("/root", "/root/target");
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 999);
    assert_eq!(size.totals.symlinks, 1);
    assert_eq!(size.totals.files, 2);
    // Root 8 blocks + target 2 blocks + symlink 1 block.
    assert_eq!(size.totals.disk_bytes, (8 + 2 + 1) * 512);
    assert_eq!(source.reads.load(Ordering::SeqCst), 1);
}

#[test]
fn special_files_are_counted_but_never_opened_or_sized() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/fifo", meta(EntryKind::Other, 1, 30, 123_456, 99));
    source.child("/root", "/root/fifo");
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 0);
    assert_eq!(size.totals.disk_bytes, 8 * 512);
    assert_eq!(size.totals.files, 1);
}

#[test]
fn permission_failures_and_disappearing_entries_make_an_explicit_partial() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/ok", regular(1, 10, 5, 1));
    source.add("/root/gone", regular(1, 11, 7, 1));
    source.add("/root/locked", dir(1, 12, 8));
    source.child("/root", "/root/ok");
    source.child("/root", "/root/gone");
    source.child("/root", "/root/locked");
    source
        .lstat_errors
        .insert(PathBuf::from("/root/gone"), io::ErrorKind::NotFound);
    source.read_errors.insert(
        PathBuf::from("/root/locked"),
        io::ErrorKind::PermissionDenied,
    );
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 5);
    assert!(
        size.totals
            .partial
            .contains(&PartialReason::PermissionDenied)
    );
    assert!(size.totals.partial.contains(&PartialReason::Changed));
    let lines = folder_size_lines(&size, 0, false);
    assert_eq!(lines[0], "Contents: At least 5 B (partial)");
    let partial = lines
        .iter()
        .find(|line| line.starts_with("Partial: "))
        .expect("a partial reason line");
    assert!(partial.contains("permission denied"));
    assert!(partial.contains("items changed during scan"));
}

#[test]
fn a_mounted_child_is_skipped_and_disclosed() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/local", regular(1, 10, 5, 1));
    source.add("/root/mount", dir(2, 11, 8));
    source.add("/root/mount/remote", regular(2, 12, 1000, 2));
    source.child("/root", "/root/local");
    source.child("/root", "/root/mount");
    source.child("/root/mount", "/root/mount/remote");
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert_eq!(size.totals.contents_bytes, 5);
    assert_eq!(size.totals.folders, 0);
    assert_eq!(size.totals.mounts_skipped, 1);
    assert!(!size.is_partial());
    let lines = folder_size_lines(&size, 0, false);
    assert!(
        lines
            .iter()
            .any(|line| line == "Skipped: 1 mounted filesystem(s)")
    );
}

#[test]
fn the_entry_depth_and_time_limits_are_explicit_partials() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    for index in 1..6 {
        let path = format!("/root/f{index}");
        source.add(&path, regular(1, 10 + index, 1, 1));
        source.child("/root", &path);
    }
    let policy = ScanPolicy {
        max_entries: 3,
        ..ScanPolicy::default()
    };
    let entry_limited = sized(scan_fake(&source, &request("/root", 1, 1), policy));
    assert!(
        entry_limited
            .totals
            .partial
            .contains(&PartialReason::EntryLimit)
    );

    let mut depth_source = FakeSource::default();
    depth_source.add("/root", dir(1, 1, 8));
    depth_source.add("/root/a", dir(1, 2, 8));
    depth_source.add("/root/a/b", dir(1, 3, 8));
    depth_source.add("/root/a/b/file", regular(1, 4, 1, 1));
    depth_source.child("/root", "/root/a");
    depth_source.child("/root/a", "/root/a/b");
    depth_source.child("/root/a/b", "/root/a/b/file");
    let depth_policy = ScanPolicy {
        max_depth: 1,
        ..ScanPolicy::default()
    };
    let depth_limited = sized(scan_fake(
        &depth_source,
        &request("/root", 1, 1),
        depth_policy,
    ));
    assert!(
        depth_limited
            .totals
            .partial
            .contains(&PartialReason::DepthLimit)
    );
    assert_eq!(depth_limited.totals.contents_bytes, 0);

    let time_policy = ScanPolicy {
        budget_ms: 0,
        ..ScanPolicy::default()
    };
    let time_limited = sized(scan_fake(&source, &request("/root", 1, 1), time_policy));
    assert!(
        time_limited
            .totals
            .partial
            .contains(&PartialReason::TimeLimit)
    );
}

#[test]
fn sums_guard_against_overflow() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/huge", regular(1, 10, u64::MAX, 1));
    source.add("/root/more", regular(1, 11, 2, 1));
    source.child("/root", "/root/huge");
    source.child("/root", "/root/more");
    let size = sized(scan_fake(
        &source,
        &request("/root", 1, 1),
        ScanPolicy::default(),
    ));
    assert!(size.totals.partial.contains(&PartialReason::Overflow));
}

#[test]
fn a_cancelled_walk_never_returns_a_result_to_cache() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 1, 8));
    source.add("/root/a", regular(1, 10, 1, 1));
    source.child("/root", "/root/a");
    let clock = ManualClock::new(1_000);
    let probe = source.probe(Path::new("/root"));
    let result = scan_with_root(
        &root_path(),
        &probe,
        ScanPolicy::default(),
        &source,
        &clock,
        &|| true,
        &mut |_| {},
    );
    assert_eq!(result, ScanResult::Cancelled);
}

fn root_path() -> RootIdentity {
    request("/root", 1, 1)
}

#[test]
fn remote_and_symlink_roots_are_refused_without_a_walk() {
    let remote = FakeSource {
        root_state: Some(RootState::Remote),
        ..FakeSource::default()
    };
    let clock = ManualClock::new(0);
    let probe = remote.probe(Path::new("/root"));
    let result = scan_with_root(
        &root_path(),
        &probe,
        ScanPolicy::default(),
        &remote,
        &clock,
        &|| false,
        &mut |_| {},
    );
    assert_eq!(result, ScanResult::Unavailable(RootState::Remote));
    assert_eq!(remote.reads.load(Ordering::SeqCst), 0);

    let link = FakeSource {
        root_state: Some(RootState::Symlink),
        ..FakeSource::default()
    };
    let probe = link.probe(Path::new("/root"));
    let result = scan_with_root(
        &root_path(),
        &probe,
        ScanPolicy::default(),
        &link,
        &clock,
        &|| false,
        &mut |_| {},
    );
    assert_eq!(result, ScanResult::Unavailable(RootState::Symlink));
    assert_eq!(link.reads.load(Ordering::SeqCst), 0);
}

#[test]
fn a_changed_inode_or_device_rejects_the_requested_identity() {
    let mut source = FakeSource::default();
    source.add("/root", dir(1, 99, 8));
    let clock = ManualClock::new(0);
    let probe = source.probe(Path::new("/root"));
    let result = scan_with_root(
        &root_path(),
        &probe,
        ScanPolicy::default(),
        &source,
        &clock,
        &|| false,
        &mut |_| {},
    );
    assert_eq!(result, ScanResult::Unavailable(RootState::Unavailable));
}

#[test]
fn refreshing_a_view_invalidates_its_child_totals_and_cached_ancestors() {
    let mut cache = FolderSizeCache::new(8);
    let paths = [
        "/tree",
        "/tree/view",
        "/tree/view/child",
        "/tree/sibling",
        "/tree/view-other",
        "/elsewhere",
    ];
    for (index, path) in paths.iter().enumerate() {
        let identity = request(path, 1, index as u64 + 1);
        cache.insert(
            identity.cache_key(POLICY_VERSION),
            FolderSize {
                identity,
                totals: FolderTotals::default(),
            },
            1_000,
        );
    }
    // Refresh is invoked on the displayed parent, whereas its folder cells
    // cache child roots. Ancestors also include this subtree in their totals.
    cache.invalidate_path(Path::new("/tree/view"));
    for (index, path) in paths.iter().enumerate() {
        let key = request(path, 1, index as u64 + 1).cache_key(POLICY_VERSION);
        if index < 3 {
            assert!(
                matches!(cache.get(&key, 1_001), CacheLookup::Miss),
                "{path}"
            );
        } else {
            assert!(
                matches!(cache.get(&key, 1_001), CacheLookup::Fresh(_)),
                "an unrelated or prefix-lookalike path must stay cached: {path}"
            );
        }
    }
}

#[test]
fn the_cache_is_bounded_and_distinguishes_fresh_expired_and_invalidated() {
    let identity = request("/root", 1, 1);
    let key = identity.cache_key(POLICY_VERSION);
    let size = FolderSize {
        identity: identity.clone(),
        totals: FolderTotals::default(),
    };
    let mut cache = FolderSizeCache::new(2);
    cache.insert(key.clone(), size.clone(), 1_000);
    assert!(matches!(cache.get(&key, 30_999), CacheLookup::Fresh(_)));
    assert!(matches!(cache.get(&key, 31_001), CacheLookup::Expired(_)));
    cache.invalidate_path(Path::new("/root"));
    assert!(matches!(cache.get(&key, 1_000), CacheLookup::Miss));

    cache.insert(key.clone(), size.clone(), 0);
    cache.insert(
        request("/other", 1, 2).cache_key(POLICY_VERSION),
        size.clone(),
        0,
    );
    cache.insert(request("/third", 1, 3).cache_key(POLICY_VERSION), size, 0);
    assert_eq!(cache.len(), 2);
    assert!(matches!(cache.get(&key, 1), CacheLookup::Miss));

    cache.invalidate_all();
    assert!(cache.is_empty());
}

#[test]
fn the_policy_version_and_revision_are_part_of_the_key() {
    let a = request("/root", 1, 1).cache_key(1);
    let b = request("/root", 1, 1).cache_key(2);
    let mut other = request("/root", 1, 1);
    other.modified_sec = 9;
    assert_ne!(a, b);
    assert_ne!(a, other.cache_key(1));
}

#[test]
fn formatted_details_cover_empty_partial_cached_and_aged_results() {
    let identity = request("/root", 1, 1);
    let complete = FolderSize {
        identity: identity.clone(),
        totals: FolderTotals {
            contents_bytes: 13_000_000,
            disk_bytes: 13_200_000,
            files: 27,
            folders: 3,
            ..FolderTotals::default()
        },
    };
    let lines = folder_size_lines(&complete, 12_400, true);
    assert_eq!(lines[0], "Contents: 12.4 MiB");
    assert_eq!(lines[1], "On disk: 12.6 MiB");
    assert_eq!(lines[2], "Contains: 27 files, 3 folders");
    assert_eq!(lines[3], "Calculated: 12 s ago (Cached)");

    let partial = FolderSize {
        identity,
        totals: FolderTotals {
            contents_bytes: 1,
            files: 1,
            partial: vec![PartialReason::PermissionDenied],
            ..FolderTotals::default()
        },
    };
    let lines = folder_size_lines(&partial, 0, false);
    assert_eq!(lines[0], "Contents: At least 1 B (partial)");
    assert_eq!(lines[2], "Contains: 1 file, 0 folders (at least)");
    assert_eq!(lines[3], "Partial: permission denied");
    assert_eq!(lines[4], "Calculated: just now");
}

#[test]
fn byte_and_age_formats_are_stable() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1536), "1.5 KiB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
    assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    assert_eq!(age_text(0), "just now");
    assert_eq!(age_text(59_999), "59 s ago");
    assert_eq!(age_text(120_000), "2 min ago");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "gimbal-folder-size-{}-{name}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn real_root(path: &Path) -> RootIdentity {
    let metadata = fs::symlink_metadata(path).unwrap();
    RootIdentity {
        path: path.to_path_buf(),
        device: metadata.dev(),
        inode: metadata.ino(),
        modified_sec: metadata.mtime(),
        modified_nsec: metadata.mtime_nsec(),
    }
}

#[test]
fn a_real_nested_hidden_tree_preserves_contents_and_counts_hard_links_once() {
    let scratch = Scratch::new("real-tree");
    let root = scratch.path();
    fs::create_dir(root.join("empty")).unwrap();
    fs::create_dir_all(root.join("nested/deeper")).unwrap();
    fs::write(root.join("nested/a.txt"), b"hello").unwrap();
    fs::write(root.join("nested/deeper/b.bin"), b"0123456789").unwrap();
    fs::write(root.join(".hidden"), b"hid").unwrap();
    symlink("nested/a.txt", root.join("link")).unwrap();
    fs::hard_link(root.join("nested/a.txt"), root.join("hard")).unwrap();

    let identity = real_root(root);
    let clock = ManualClock::new(0);
    let result = scan_with_root(
        &identity,
        &probe_native(root),
        ScanPolicy::default(),
        &NativeSource,
        &clock,
        &|| false,
        &mut |_| {},
    );
    let size = sized(result);
    assert_eq!(size.totals.contents_bytes, 18);
    assert_eq!(size.totals.files, 5);
    assert_eq!(size.totals.folders, 3);
    assert_eq!(size.totals.symlinks, 1);
    assert_eq!(size.totals.hard_links_deduplicated, 1);
    assert!(!size.is_partial());
    assert_eq!(fs::read(root.join("nested/a.txt")).unwrap(), b"hello");
    assert_eq!(fs::read(root.join(".hidden")).unwrap(), b"hid");
}

#[test]
fn a_real_symlink_loop_and_external_target_are_never_walked() {
    let scratch = Scratch::new("symlink-loop");
    let root = scratch.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::create_dir(root.join("a")).unwrap();
    fs::write(root.join("a/file"), b"hello").unwrap();
    // A loop back to the root; following it would recurse forever.
    symlink("..", root.join("a/up")).unwrap();
    // A target outside the scanned root; its bytes must not be counted.
    let outside = scratch.path().join("outside");
    fs::write(&outside, vec![b'x'; 999]).unwrap();
    symlink(&outside, root.join("outside-link")).unwrap();

    let identity = real_root(&root);
    let clock = ManualClock::new(0);
    let size = sized(scan_with_root(
        &identity,
        &probe_native(&root),
        ScanPolicy::default(),
        &NativeSource,
        &clock,
        &|| false,
        &mut |_| {},
    ));
    assert_eq!(size.totals.contents_bytes, 5);
    assert_eq!(size.totals.files, 3); // file, loop symlink, outside symlink
    assert_eq!(size.totals.folders, 1);
    assert_eq!(size.totals.symlinks, 2);
    assert!(!size.is_partial());
}

#[test]
fn a_real_sparse_file_has_a_large_length_but_smaller_reported_blocks() {
    let scratch = Scratch::new("sparse");
    let root = scratch.path();
    let file = fs::File::create(root.join("sparse.bin")).unwrap();
    file.set_len(8 * 1024 * 1024).unwrap();
    drop(file);
    let identity = real_root(root);
    let clock = ManualClock::new(0);
    let size = sized(scan_with_root(
        &identity,
        &probe_native(root),
        ScanPolicy::default(),
        &NativeSource,
        &clock,
        &|| false,
        &mut |_| {},
    ));
    assert_eq!(size.totals.contents_bytes, 8 * 1024 * 1024);
    assert!(size.totals.disk_bytes < size.totals.contents_bytes);
}

#[test]
fn a_real_symlink_root_is_not_followed() {
    let scratch = Scratch::new("symlink-root");
    let target = scratch.path().join("target");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("a"), b"abc").unwrap();
    let link = scratch.path().join("link");
    symlink(&target, &link).unwrap();
    let probe = probe_native(&link);
    assert_eq!(probe.state, RootState::Symlink);
    let identity = RootIdentity {
        path: link.clone(),
        device: 1,
        inode: 1,
        modified_sec: 0,
        modified_nsec: 0,
    };
    let clock = ManualClock::new(0);
    let result = scan_with_root(
        &identity,
        &probe,
        ScanPolicy::default(),
        &NativeSource,
        &clock,
        &|| false,
        &mut |_| {},
    );
    assert_eq!(result, ScanResult::Unavailable(RootState::Symlink));
}
