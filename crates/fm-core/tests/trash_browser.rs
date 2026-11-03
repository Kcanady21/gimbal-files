// SPDX-License-Identifier: GPL-3.0-or-later
//! Trash browser planning and execution against an injected scratch backend.
//!
//! The destructive paths in this file never touch GVfs's `trash:///`. A child
//! process owns a small filesystem trash under its own scratch folder and the
//! production `FileOperations` planner/executor. Only the undo's `g_file_trash`
//! call reaches GIO, and `XDG_DATA_HOME` points inside the same scratch folder,
//! as P2f requires.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fm_core::fileops::{
    self, Decision, Decisions, FileOperations, ItemKey, ItemOutcome, Request, TrashBackend,
    TrashError, TrashFingerprint, TrashIdentity, TrashItem, TrashUri, trashinfo,
};
use gio::prelude::*;
use glib::translate::IntoGlib;

const CHILD_ENV: &str = "GIMBAL_P2K2_CHILD";
static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Harness {
    base: PathBuf,
}

impl Harness {
    fn new(test: &str) -> Self {
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = cache_root().join(format!("p2k2-{}-{number}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("data")).unwrap();
        fs::create_dir_all(base.join("scratch")).unwrap();
        Self { base }
    }

    fn finish(&self) {
        let _ = fs::remove_dir_all(&self.base);
        let _ = fs::remove_dir(cache_root());
    }
}

fn cache_root() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".cache/gimbal-tests")
}

fn real_trash_entries() -> Vec<(String, String)> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").expect("HOME is set")).join(".local/share")
        });
    let trash = data.join("Trash");
    let mut entries = Vec::new();
    for subdirectory in ["info", "files"] {
        let mut names: Vec<String> = fs::read_dir(trash.join(subdirectory))
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        entries.extend(
            names
                .into_iter()
                .map(|name| (subdirectory.to_string(), name)),
        );
    }
    entries
}

fn run_child(test: &str, harness: &Harness) -> Output {
    Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--ignored", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("GIO_USE_VFS", "local")
        .env("XDG_DATA_HOME", harness.base.join("data"))
        .output()
        .unwrap()
}

fn parent(test: &str, child: &str) {
    let before = real_trash_entries();
    let harness = Harness::new(test);
    let output = run_child(child, &harness);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    harness.finish();
    assert!(
        output.status.success(),
        "child `{child}` failed\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    assert_eq!(
        before,
        real_trash_entries(),
        "Kyle's real Trash changed during `{child}`"
    );
}

#[test]
fn restore_and_its_undo_use_the_scratch_backend() {
    parent("restore-undo", "child_restore_and_undo");
}

#[test]
fn restore_conflicts_keep_both_replace_and_restore_the_victim_on_undo() {
    parent("restore-conflicts", "child_restore_conflicts");
}

#[test]
fn restore_same_original_keeps_each_answer_and_undoes_actual_destinations() {
    parent(
        "restore-same-original-keep-both",
        "child_restore_same_original_keep_both",
    );
}

#[test]
fn restore_same_original_replace_applies_only_to_its_item() {
    parent(
        "restore-same-original-replace",
        "child_restore_same_original_replace",
    );
}

#[test]
fn an_empty_keeps_later_entries_and_never_follows_a_symlink_out() {
    parent("empty-snapshot", "child_empty_snapshot_and_symlink");
}

#[test]
fn empty_partial_failure_and_cancel_leave_the_remaining_entry() {
    parent("empty-partial", "child_empty_partial_and_cancel");
}

#[test]
fn corrupt_metadata_is_not_listed_or_deleted() {
    parent("corrupt", "child_corrupt_metadata");
}

#[test]
fn an_empty_revalidates_identity_and_skips_already_gone_items() {
    parent("empty-revalidate", "child_empty_revalidate_and_missing");
}

#[test]
fn selected_deletion_deletes_only_its_snapshot_and_refuses_changes() {
    parent("selected-snapshot", "child_selected_delete_only_snapshot");
}

#[test]
fn selected_deletion_reports_mixed_results_and_final_cancel() {
    parent("selected-mixed", "child_selected_delete_mixed_and_cancel");
}

#[test]
fn selected_deletion_handles_folders_symlinks_external_uris_and_undo() {
    parent(
        "selected-tree",
        "child_selected_delete_tree_external_and_undo",
    );
}

// ---------------------------------------------------------------------------
// The scratch backend
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ScratchTrash {
    root: PathBuf,
    fail_delete: Arc<Mutex<HashSet<String>>>,
    generations: Arc<Mutex<HashMap<String, u64>>>,
    deleted: Arc<Mutex<Vec<String>>>,
    delete_calls: Arc<Mutex<Vec<String>>>,
    cancel_during_delete: Arc<Mutex<bool>>,
}

impl ScratchTrash {
    fn new(root: PathBuf) -> Self {
        fs::create_dir_all(root.join("files")).unwrap();
        fs::create_dir_all(root.join("info")).unwrap();
        Self {
            root,
            fail_delete: Arc::new(Mutex::new(HashSet::new())),
            generations: Arc::new(Mutex::new(HashMap::new())),
            deleted: Arc::new(Mutex::new(Vec::new())),
            delete_calls: Arc::new(Mutex::new(Vec::new())),
            cancel_during_delete: Arc::new(Mutex::new(false)),
        }
    }

    fn cancel_during_delete(&self) {
        *self.cancel_during_delete.lock().unwrap() = true;
    }

    fn fail_delete(&self, uri: &str) {
        self.fail_delete.lock().unwrap().insert(uri.to_string());
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }

    fn delete_calls(&self) -> Vec<String> {
        self.delete_calls.lock().unwrap().clone()
    }

    fn entry_path(&self, uri: &TrashUri) -> PathBuf {
        self.root.join("files").join(uri.name())
    }

    fn info_path(&self, uri: &TrashUri) -> PathBuf {
        let mut name = uri.name();
        name.push(".trashinfo");
        self.root.join("info").join(name)
    }

    fn add_file(&self, name: &str, original: &Path, date: &str, contents: &str) {
        fs::write(self.root.join("files").join(name), contents).unwrap();
        self.write_info(name, original, date);
    }

    /// Every `write_info` is a new generation. A replacement at the same
    /// trash URI therefore gets a new object token even when its original
    /// path and second-resolution date are identical.
    fn write_info(&self, name: &str, original: &Path, date: &str) {
        let mut generations = self.generations.lock().unwrap();
        let generation = generations.entry(name.to_string()).or_insert(0);
        *generation += 1;
        let encoded = percent_encode(original.as_os_str());
        fs::write(
            self.root.join("info").join(format!("{name}.trashinfo")),
            format!("[Trash Info]\nPath={encoded}\nDeletionDate={date}\n"),
        )
        .unwrap();
    }

    fn generation(&self, uri: &TrashUri) -> u64 {
        self.generations
            .lock()
            .unwrap()
            .get(&uri.name().to_string_lossy().into_owned())
            .copied()
            .unwrap_or(1)
    }
}

impl TrashBackend for ScratchTrash {
    fn list(&self) -> Result<Vec<TrashItem>, TrashError> {
        let mut items = Vec::new();
        for entry in fs::read_dir(self.root.join("info"))
            .map_err(|error| TrashError::Unavailable(error.to_string()))?
            .flatten()
        {
            let path = entry.path();
            if path.extension().and_then(OsStr::to_str) != Some("trashinfo") {
                continue;
            }
            let Some(stem) = path.file_stem() else {
                continue;
            };
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            let Ok(parsed) = trashinfo::parse(&text) else {
                continue;
            };
            let Ok(uri) = TrashUri::from_name(stem) else {
                continue;
            };
            let files = self.root.join("files").join(stem);
            let Ok(metadata) = fs::symlink_metadata(&files) else {
                continue;
            };
            let is_dir = metadata.is_dir() && !metadata.file_type().is_symlink();
            let fingerprint =
                scratch_fingerprint(&uri, self.generation(&uri), is_dir, metadata.len());
            items.push(TrashItem::new(
                TrashIdentity::new(
                    uri,
                    Some(parsed.path),
                    parsed.deletion_date.to_unix(),
                    Some(fingerprint),
                ),
                stem.to_string_lossy().into_owned(),
                is_dir,
                None,
            ));
        }
        items.sort_by(|left, right| left.uri().as_str().cmp(right.uri().as_str()));
        Ok(items)
    }

    fn query_item(&self, uri: &TrashUri) -> Result<TrashItem, TrashError> {
        self.list()?
            .into_iter()
            .find(|candidate| candidate.uri() == uri)
            .ok_or(TrashError::Missing)
    }

    fn restore(
        &self,
        item: &TrashIdentity,
        target: &Path,
        _cancellable: &gio::Cancellable,
    ) -> Result<(), String> {
        if item.fingerprint().is_some() {
            self.revalidate(item).map_err(|error| error.to_string())?;
        }
        if fs::symlink_metadata(target).is_ok() {
            return Err("the original path is taken".to_string());
        }
        let files = self.entry_path(item.uri());
        fs::rename(&files, target).map_err(|error| error.to_string())?;
        let _ = fs::remove_file(self.info_path(item.uri()));
        Ok(())
    }

    fn delete_checked(
        &self,
        item: &TrashIdentity,
        _cancellable: &gio::Cancellable,
    ) -> Result<(), TrashError> {
        self.revalidate_object(item)?;
        if *self.cancel_during_delete.lock().unwrap() {
            _cancellable.cancel();
            return Err(TrashError::Cancelled);
        }
        if self
            .fail_delete
            .lock()
            .unwrap()
            .contains(item.uri().as_str())
        {
            return Err(TrashError::Io(
                "the scratch backend refused this deletion".to_string(),
            ));
        }
        let files = self.entry_path(item.uri());
        let metadata =
            fs::symlink_metadata(&files).map_err(|error| TrashError::Io(error.to_string()))?;
        self.delete_calls
            .lock()
            .unwrap()
            .push(item.uri().as_str().to_string());
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            fs::remove_file(&files).map_err(|error| TrashError::Io(error.to_string()))?;
        } else {
            // std's remove_dir_all unlinks symlinks; it never follows them.
            fs::remove_dir_all(&files).map_err(|error| TrashError::Io(error.to_string()))?;
        }
        let _ = fs::remove_file(self.info_path(item.uri()));
        self.deleted
            .lock()
            .unwrap()
            .push(item.uri().as_str().to_string());
        Ok(())
    }
}

fn scratch_fingerprint(
    uri: &TrashUri,
    generation: u64,
    is_dir: bool,
    size: u64,
) -> TrashFingerprint {
    let info = gio::FileInfo::new();
    info.set_attribute_string(
        gio::FILE_ATTRIBUTE_ID_FILE.as_str(),
        &format!("l1:{}:{generation}", uri.as_str()),
    );
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 1);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), generation);
    info.set_attribute_uint32(
        gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
        u32::try_from(
            if is_dir {
                gio::FileType::Directory
            } else {
                gio::FileType::Regular
            }
            .into_glib(),
        )
        .unwrap(),
    );
    info.set_attribute_uint64(
        gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(),
        1_700_000_000 + generation,
    );
    info.set_attribute_uint32("time::changed-nsec", 123_456_789);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(), 1_700_000_000);
    info.set_attribute_uint32("time::modified-nsec", 123_456_789);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(), size);
    info.set_attribute_string(
        gio::FILE_ATTRIBUTE_STANDARD_TARGET_URI.as_str(),
        uri.as_str(),
    );
    info.set_attribute_string(
        gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str(),
        &format!("generation-{generation}"),
    );
    TrashFingerprint::from_file_info(&info)
}

fn percent_encode(name: &OsStr) -> String {
    let mut text = String::new();
    for byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            text.push(*byte as char);
        } else {
            text.push('%');
            text.push_str(&format!("{byte:02X}"));
        }
    }
    text
}

// ---------------------------------------------------------------------------
// Children
// ---------------------------------------------------------------------------

fn child_setup() -> PathBuf {
    assert_eq!(
        std::env::var(CHILD_ENV).ok().as_deref(),
        Some("1"),
        "this test only runs as a child of its parent"
    );
    let data = PathBuf::from(std::env::var_os("XDG_DATA_HOME").expect("XDG_DATA_HOME is set"));
    let base = data
        .parent()
        .expect("XDG_DATA_HOME points inside the harness")
        .to_path_buf();
    fs::create_dir_all(base.join("scratch")).unwrap();
    base
}

fn scratch() -> PathBuf {
    child_setup().join("scratch")
}

fn backend(root: &Path) -> ScratchTrash {
    ScratchTrash::new(root.join("fake-trash"))
}

fn run(
    operations: &mut FileOperations,
    request: &Request,
    decisions: &Decisions,
) -> fm_core::fileops::Report {
    let plan = operations.plan(request);
    operations
        .execute(&plan, decisions, &gio::Cancellable::new(), |_| {})
        .unwrap()
}

#[test]
#[ignore]
fn child_restore_and_undo() {
    let scratch = scratch();
    let one = scratch.join("one");
    let two = scratch.join("two");
    fs::create_dir_all(&one).unwrap();
    fs::create_dir_all(&two).unwrap();
    let one_file = one.join("one.txt");
    let two_file = two.join("two.txt");

    let backend = backend(&scratch);
    backend.add_file("one.txt", &one_file, "2026-11-02T10:00:00", "new one");
    backend.add_file("two.txt", &two_file, "2026-11-02T10:01:00", "new two");
    let items = backend.list().unwrap();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    let request = Request::Restore {
        items: items.iter().map(|item| item.identity().clone()).collect(),
    };
    let plan = operations.plan(&request);
    let decisions = Decisions::from_plan(&plan);
    let report = run(&mut operations, &request, &decisions);
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&one_file).unwrap(), "new one");
    assert_eq!(fs::read_to_string(&two_file).unwrap(), "new two");
    assert!(backend.list().unwrap().is_empty());

    let undo = operations.plan(&Request::Undo);
    assert_eq!(undo.steps().len(), 2);
    assert!(
        undo.steps()
            .iter()
            .all(|step| step.action == fm_core::fileops::Action::Trash)
    );
    let report = run(&mut operations, &Request::Undo, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert!(!one_file.exists());
    assert!(!two_file.exists());
    assert_eq!(
        fs::read_dir(PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash/files"))
            .unwrap()
            .count(),
        2,
        "the undo put both restored items in the scratch GIO trash"
    );
}

#[test]
#[ignore]
fn child_restore_conflicts() {
    let scratch = scratch();
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let one = folder.join("one.txt");
    let two = folder.join("two.txt");
    fs::write(&one, "old one").unwrap();
    fs::write(&two, "old two").unwrap();

    let backend = backend(&scratch);
    backend.add_file("one.txt", &one, "2026-11-02T10:00:00", "new one");
    backend.add_file("two.txt", &two, "2026-11-02T10:01:00", "new two");
    let items = backend.list().unwrap();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    let request = Request::Restore {
        items: items.iter().map(|item| item.identity().clone()).collect(),
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.decisions().count(), 2);
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(
        ItemKey::Trash(items[0].identity().uri().clone()),
        Decision::KeepBoth,
        false,
    );
    decisions.answer(
        ItemKey::Trash(items[1].identity().uri().clone()),
        Decision::Replace,
        false,
    );
    let report = run(&mut operations, &request, &decisions);
    assert!(report.all_succeeded(), "{:?}", report.lines());

    assert_eq!(fs::read_to_string(&one).unwrap(), "old one");
    assert_eq!(
        fs::read_to_string(folder.join("one (copy).txt")).unwrap(),
        "new one"
    );
    assert_eq!(fs::read_to_string(&two).unwrap(), "new two");

    let _undo = operations.plan(&Request::Undo);
    let report = run(&mut operations, &Request::Undo, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(
        fs::read_to_string(&two).unwrap(),
        "old two",
        "the Replace victim is restored by undo"
    );
    assert!(!folder.join("one (copy).txt").exists());
    assert_eq!(fs::read_to_string(&one).unwrap(), "old one");
}

#[test]
#[ignore]
fn child_restore_same_original_keep_both() {
    let scratch = scratch();
    let folder = scratch.join("same-original");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    fs::write(&target, "existing").unwrap();

    let backend = backend(&scratch);
    backend.add_file("first.txt", &target, "2026-11-02T10:00:00", "first");
    backend.add_file("second.txt", &target, "2026-11-02T10:01:00", "second");
    let items = backend.list().unwrap();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    let request = Request::Restore {
        items: items.iter().map(|item| item.identity().clone()).collect(),
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.decisions().count(), 2);
    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    assert_eq!(first_key, ItemKey::Trash(items[0].identity().uri().clone()));
    decisions.answer(&first_key, Decision::KeepBoth, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    assert_eq!(
        second_key,
        ItemKey::Trash(items[1].identity().uri().clone())
    );
    decisions.answer(&second_key, Decision::KeepBoth, false);

    let report = run(&mut operations, &request, &decisions);
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&target).unwrap(), "existing");
    let first_copy = folder.join("same (copy).txt");
    let second_copy = folder.join("same (copy 2).txt");
    assert_eq!(report.items[0].target, Some(first_copy.clone()));
    assert_eq!(report.items[1].target, Some(second_copy.clone()));
    assert_eq!(fs::read_to_string(&first_copy).unwrap(), "first");
    assert_eq!(fs::read_to_string(&second_copy).unwrap(), "second");
    assert!(backend.list().unwrap().is_empty());

    let undo = operations.plan(&Request::Undo);
    assert_eq!(undo.steps().len(), 2);
    assert!(
        undo.steps()
            .iter()
            .all(|step| step.action == fm_core::fileops::Action::Trash)
    );
    let report = run(&mut operations, &Request::Undo, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert!(!first_copy.exists());
    assert!(!second_copy.exists());
    assert_eq!(fs::read_to_string(&target).unwrap(), "existing");
    assert!(
        backend.list().unwrap().is_empty(),
        "the undo re-trashes the restored copies through GIO, not the scratch backend"
    );
}

#[test]
#[ignore]
fn child_restore_same_original_replace() {
    let scratch = scratch();
    let folder = scratch.join("same-original-replace");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    fs::write(&target, "existing").unwrap();

    let backend = backend(&scratch);
    backend.add_file("first.txt", &target, "2026-11-02T10:00:00", "first");
    backend.add_file("second.txt", &target, "2026-11-02T10:01:00", "second");
    let items = backend.list().unwrap();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::Restore {
        items: items.iter().map(|item| item.identity().clone()).collect(),
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.decisions().count(), 2);
    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(&first_key, Decision::Replace, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(&second_key, Decision::Skip, false);

    let report = run(&mut operations, &request, &decisions);
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&target).unwrap(), "first");
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert_eq!(report.items[1].outcome, ItemOutcome::Skipped);
    assert_eq!(
        backend
            .list()
            .unwrap()
            .iter()
            .map(|item| item.uri().as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["trash:///second.txt"]
    );
}

#[test]
#[ignore]
fn child_empty_snapshot_and_symlink() {
    let scratch = scratch();
    let backend = backend(&scratch);
    backend.add_file(
        "old.txt",
        &scratch.join("old.txt"),
        "2026-11-02T10:00:00",
        "old",
    );
    let tree = backend.root.join("files/tree");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("inside.txt"), "inside").unwrap();
    let outside = scratch.join("outside.txt");
    fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("escape")).unwrap();
    backend.write_info("tree", &scratch.join("tree"), "2026-11-02T10:01:00");

    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    assert_eq!(plan.steps().len(), 2, "{:?}", plan.lines());
    let confirmation = plan.empty_confirmation().unwrap();

    backend.add_file(
        "late.txt",
        &scratch.join("late.txt"),
        "2026-11-02T10:02:00",
        "late",
    );
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(
        backend.list().unwrap().len(),
        1,
        "the item added after the snapshot survives"
    );
    assert_eq!(
        backend.list().unwrap()[0].uri().as_str(),
        "trash:///late.txt"
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "outside");
    assert!(tree.join("escape").is_symlink() || !tree.exists());
    let mut calls = backend.delete_calls();
    calls.sort();
    assert_eq!(
        calls,
        vec!["trash:///old.txt", "trash:///tree"],
        "each confirmed top-level item, including the folder, reaches the backend once"
    );
}

#[test]
#[ignore]
fn child_empty_revalidate_and_missing() {
    let scratch = scratch();
    let backend = backend(&scratch);
    for name in ["a.txt", "b.txt", "c.txt"] {
        backend.add_file(name, &scratch.join(name), "2026-11-02T10:00:00", name);
    }
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    assert_eq!(plan.steps().len(), 3);
    let confirmation = plan.empty_confirmation().unwrap();

    // b is already gone before execution; c was replaced at the same URI with
    // the same original/date display metadata.
    let _ = fs::remove_file(backend.root.join("files/b.txt"));
    let _ = fs::remove_file(backend.root.join("info/b.txt.trashinfo"));
    backend.write_info("c.txt", &scratch.join("c.txt"), "2026-11-02T10:00:00");

    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert_eq!(report.items[1].outcome, ItemOutcome::Skipped);
    assert_eq!(
        report.items[2].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
    assert_eq!(backend.delete_calls(), vec!["trash:///a.txt"]);
    assert_eq!(
        backend
            .list()
            .unwrap()
            .iter()
            .map(|item| item.uri().as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["trash:///c.txt"]
    );
}

#[test]
#[ignore]
fn child_empty_partial_and_cancel() {
    let scratch = scratch();
    let backend = backend(&scratch);
    for name in ["a.txt", "b.txt", "c.txt"] {
        backend.add_file(name, &scratch.join(name), "2026-11-02T10:00:00", name);
    }
    backend.fail_delete("trash:///b.txt");
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = plan.empty_confirmation().unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(!report.all_succeeded());
    assert_eq!(report.items.len(), 3);
    assert_eq!(
        report.items[1].outcome,
        ItemOutcome::Failed(
            "the trash request failed: the scratch backend refused this deletion".to_string(),
        )
    );
    assert_eq!(
        backend
            .list()
            .unwrap()
            .iter()
            .map(|item| item.uri().as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["trash:///b.txt"]
    );

    let cancellable = gio::Cancellable::new();
    cancellable.cancel();
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = plan.empty_confirmation().unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &cancellable, |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(backend.list().unwrap().len(), 1);
}

#[test]
#[ignore]
fn child_selected_delete_only_snapshot() {
    let scratch = scratch();
    let backend = backend(&scratch);
    for name in ["a.txt", "b.txt", "c.txt"] {
        backend.add_file(name, &scratch.join(name), "2026-11-03T10:00:00", name);
    }
    let items = backend.list().unwrap();
    let selected = vec![items[0].identity().clone(), items[2].identity().clone()];
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected { items: selected };
    let plan = operations.plan(&request);
    assert_eq!(plan.steps().len(), 2);
    let confirmation = plan.selected_confirmation().unwrap();

    let dry = operations.dry_run(&request);
    assert!(dry.iter().any(|line| line.contains("trash:///a.txt")));
    assert!(dry.iter().any(|line| line.contains("trash:///c.txt")));
    assert!(backend.deleted().is_empty(), "a dry run writes nothing");

    // c was replaced at the same URI with the same original/date metadata.
    backend.write_info("c.txt", &scratch.join("c.txt"), "2026-11-03T10:00:00");
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert_eq!(
        report.items[1].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
    assert_eq!(
        backend
            .list()
            .unwrap()
            .iter()
            .map(|item| item.uri().as_str().to_string())
            .collect::<Vec<_>>(),
        vec!["trash:///b.txt", "trash:///c.txt"],
        "the unselected item and the refused replacement survive"
    );
}

#[test]
#[ignore]
fn child_selected_delete_mixed_and_cancel() {
    let scratch = scratch();
    let backend = backend(&scratch);
    for name in ["a.txt", "b.txt", "c.txt", "d.txt", "keep.txt"] {
        backend.add_file(name, &scratch.join(name), "2026-11-03T10:00:00", name);
    }
    let keep = backend
        .list()
        .unwrap()
        .into_iter()
        .find(|item| item.uri().as_str() == "trash:///keep.txt")
        .unwrap();
    backend.fail_delete("trash:///b.txt");
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let items = backend.list().unwrap();
    let request = Request::DeleteSelected {
        items: items
            .iter()
            .filter(|item| item.uri().as_str() != "trash:///keep.txt")
            .map(|item| item.identity().clone())
            .collect(),
    };
    let plan = operations.plan(&request);
    let confirmation = plan.selected_confirmation().unwrap();
    // c is already gone before execution.
    let _ = fs::remove_file(backend.root.join("files/c.txt"));
    let _ = fs::remove_file(backend.root.join("info/c.txt.trashinfo"));
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert!(matches!(report.items[1].outcome, ItemOutcome::Failed(_)));
    assert_eq!(report.items[2].outcome, ItemOutcome::Skipped);
    assert_eq!(report.items[3].outcome, ItemOutcome::Done);
    assert_eq!(backend.deleted(), vec!["trash:///a.txt", "trash:///d.txt"]);
    assert!(keep.uri().as_str() == "trash:///keep.txt");
    assert!(
        backend
            .list()
            .unwrap()
            .iter()
            .any(|item| item.uri().as_str() == "trash:///keep.txt"),
        "the unselected item survives"
    );
    assert!(
        operations.plan(&Request::Undo).steps().is_empty(),
        "selected permanent deletion creates no undo record"
    );

    // Cancellation during the only (final) item stays Cancelled, not Failed.
    let scratch2 = crate::scratch().join("cancel");
    let backend2 = crate::backend(&scratch2);
    backend2.add_file("f.txt", &scratch2.join("f.txt"), "2026-11-03T11:00:00", "f");
    let mut remaining = backend2.list().unwrap();
    let item = remaining.remove(0);
    backend2.cancel_during_delete();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend2.clone()));
    let request = Request::DeleteSelected {
        items: vec![item.identity().clone()],
    };
    let plan = operations.plan(&request);
    let confirmation = plan.selected_confirmation().unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items[0].outcome, ItemOutcome::Cancelled);
    assert!(backend2.deleted().is_empty());
    assert_eq!(backend2.list().unwrap().len(), 1);
}

#[test]
#[ignore]
fn child_selected_delete_tree_external_and_undo() {
    let scratch = scratch();
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let keep_path = folder.join("keep.txt");
    let backend = backend(&scratch);
    backend.add_file("keep.txt", &keep_path, "2026-11-03T10:00:00", "keep");

    let tree = backend.root.join("files/tree");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("inside.txt"), "inside").unwrap();
    let outside = scratch.join("outside.txt");
    fs::write(&outside, "outside").unwrap();
    std::os::unix::fs::symlink(&outside, tree.join("escape")).unwrap();
    backend.write_info("tree", &scratch.join("tree"), "2026-11-03T10:01:00");

    let external_name = r"\run\media\test\drive\.Trash-1000\files\external.txt";
    backend.add_file(
        external_name,
        &scratch.join("external.txt"),
        "2026-11-03T10:02:00",
        "external",
    );

    let items = backend.list().unwrap();
    let keep = items
        .iter()
        .find(|item| item.uri().as_str() == "trash:///keep.txt")
        .unwrap()
        .identity()
        .clone();
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    // Restore an unselected item first, so a later selected permanent deletion
    // must not discard that undo record.
    let restore_request = Request::Restore {
        items: vec![keep.clone()],
    };
    let restore_plan = operations.plan(&restore_request);
    let decisions = Decisions::from_plan(&restore_plan);
    let report = operations
        .execute(&restore_plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&keep_path).unwrap(), "keep");
    assert!(!operations.plan(&Request::Undo).steps().is_empty());

    let selected: Vec<_> = backend
        .list()
        .unwrap()
        .into_iter()
        .filter(|item| item.uri().as_str() != "trash:///keep.txt")
        .map(|item| item.identity().clone())
        .collect();
    assert!(
        selected
            .iter()
            .any(|identity| { identity.uri().as_str().contains("%5C") })
    );
    let request = Request::DeleteSelected { items: selected };
    let plan = operations.plan(&request);
    let confirmation = plan.selected_confirmation().unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert!(!tree.exists(), "the folder is handed to the backend once");
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "outside",
        "a symlink inside the deleted folder never follows its target"
    );
    assert!(
        backend
            .list()
            .unwrap()
            .iter()
            .all(|item| item.uri().as_str() != "trash:///keep.txt"),
        "restoring removed keep from the Trash"
    );

    // The earlier Restore undo still runs after the selected deletion.
    let undo = operations.plan(&Request::Undo);
    assert!(
        !undo.steps().is_empty(),
        "undo for the unselected item survives"
    );
    let report = operations
        .execute(
            &undo,
            &Decisions::new(Vec::new()),
            &gio::Cancellable::new(),
            |_| {},
        )
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert!(!keep_path.exists());
    assert_eq!(
        fs::read_dir(PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap()).join("Trash/files"))
            .unwrap()
            .count(),
        1,
        "the undo put the restored keep item in the scratch GIO trash"
    );
}

#[test]
#[ignore]
fn child_corrupt_metadata() {
    let scratch = scratch();
    let backend = backend(&scratch);
    backend.add_file(
        "good.txt",
        &scratch.join("good.txt"),
        "2026-11-02T10:00:00",
        "good",
    );
    let corrupt_files = backend.root.join("files/corrupt.txt");
    fs::write(&corrupt_files, "do not lose").unwrap();
    fs::write(
        backend.root.join("info/corrupt.txt.trashinfo"),
        "[Trash Info]\nPath=%zz\nDeletionDate=nope\n",
    )
    .unwrap();

    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    assert_eq!(plan.steps().len(), 1, "corrupt metadata is not listed");
    let confirmation = plan.empty_confirmation().unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&corrupt_files).unwrap(), "do not lose");
    assert_eq!(backend.list().unwrap().len(), 0);

    // The parser itself reports the corrupt metadata rather than guessing.
    assert!(trashinfo::parse("[Trash Info]\nPath=%zz\nDeletionDate=nope\n").is_err());
    let _ = fileops::list_trash; // proves the production entry point is still public
}
