// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests for planning, decisions, names, reports and the trash-info
//! parser. Anything that trashes a file lives in `tests/trash.rs`, where a
//! child process points `XDG_DATA_HOME` at a scratch folder.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use glib::translate::IntoGlib;

use super::*;
use crate::command::{Caller, Invocation, Registry};

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(name: &str) -> Self {
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/tmp")
            .join(format!("gimbal-p2f-{}-{number}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn key(path: impl Into<PathBuf>) -> ItemKey {
    ItemKey::path(path)
}

fn symlink(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

fn check(name: &str, arguments: &[(&str, &str)]) -> CheckedCommand {
    let invocation = Invocation::new(
        name,
        arguments
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect(),
    );
    Registry::new().validate(Caller::Ui, &invocation).unwrap()
}

fn run(
    operations: &mut FileOperations,
    request: &Request,
    decisions: &Decisions,
    cancellable: &gio::Cancellable,
) -> Report {
    let plan = operations.plan(request);
    operations
        .execute(&plan, decisions, cancellable, |_| {})
        .unwrap()
}

#[derive(Default)]
struct TestTrashState {
    items: Vec<TrashItem>,
    deleted: Vec<String>,
    fail_restore: Vec<String>,
    fail_delete: Vec<String>,
    cancel_during_delete: bool,
    list_calls: usize,
}

#[derive(Clone, Default)]
struct TestTrash {
    state: Arc<Mutex<TestTrashState>>,
}

impl TestTrash {
    fn with(items: Vec<TrashItem>) -> Self {
        Self {
            state: Arc::new(Mutex::new(TestTrashState {
                items,
                ..TestTrashState::default()
            })),
        }
    }

    fn add(&self, item: TrashItem) {
        self.state.lock().unwrap().items.push(item);
    }

    fn deleted(&self) -> Vec<String> {
        self.state.lock().unwrap().deleted.clone()
    }

    fn remaining(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .items
            .iter()
            .map(|item| item.uri().as_str().to_string())
            .collect()
    }

    fn list_calls(&self) -> usize {
        self.state.lock().unwrap().list_calls
    }
}

impl TrashBackend for TestTrash {
    fn list(&self) -> Result<Vec<TrashItem>, TrashError> {
        let mut state = self.state.lock().unwrap();
        state.list_calls += 1;
        Ok(state.items.clone())
    }

    fn query_item(&self, uri: &TrashUri) -> Result<TrashItem, TrashError> {
        self.state
            .lock()
            .unwrap()
            .items
            .iter()
            .find(|candidate| candidate.uri() == uri)
            .cloned()
            .ok_or(TrashError::Missing)
    }

    fn restore(
        &self,
        item: &TrashIdentity,
        target: &Path,
        _cancellable: &gio::Cancellable,
    ) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        if state
            .fail_restore
            .iter()
            .any(|uri| uri == item.uri().as_str())
        {
            return Err("the test backend refused this restore".to_string());
        }
        let Some(index) = state
            .items
            .iter()
            .position(|candidate| candidate.uri() == item.uri())
        else {
            return Err("missing".to_string());
        };
        let fresh = state.items[index].clone();
        if !item.agrees_with(fresh.identity()) {
            return Err("the item changed in the trash".to_string());
        }
        fs::write(target, format!("restored {}", fresh.display_name()))
            .map_err(|error| error.to_string())?;
        state.items.remove(index);
        Ok(())
    }

    fn delete_checked(
        &self,
        item: &TrashIdentity,
        cancellable: &gio::Cancellable,
    ) -> Result<(), TrashError> {
        self.revalidate_object(item)?;
        let mut state = self.state.lock().unwrap();
        if state.cancel_during_delete {
            cancellable.cancel();
            return Err(TrashError::Cancelled);
        }
        if state
            .fail_delete
            .iter()
            .any(|uri| uri == item.uri().as_str())
        {
            return Err(TrashError::Io(
                "the test backend refused this deletion".to_string(),
            ));
        }
        let Some(index) = state
            .items
            .iter()
            .position(|candidate| candidate.uri() == item.uri())
        else {
            return Err(TrashError::Missing);
        };
        state.items.remove(index);
        state.deleted.push(item.uri().as_str().to_string());
        Ok(())
    }
}

fn fingerprint(generation: &str) -> TrashFingerprint {
    let info = gio::FileInfo::new();
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), generation);
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 1);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), 42);
    info.set_attribute_uint32(
        gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
        u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
    );
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
    info.set_attribute_uint32(gio::FILE_ATTRIBUTE_TIME_CHANGED_USEC.as_str(), 123_456);
    info.set_attribute_uint32("time::changed-nsec", 123_456_789);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(), 1_700_000_000);
    info.set_attribute_uint32("time::modified-nsec", 123_456_789);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(), 7);
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str(), generation);
    TrashFingerprint::from_file_info(&info)
}

fn core_fingerprint(generation: &str, changed_nsec: u32) -> TrashFingerprint {
    let info = gio::FileInfo::new();
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), generation);
    info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 1);
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), 42);
    info.set_attribute_uint32(
        gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
        u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
    );
    info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
    info.set_attribute_uint32("time::changed-nsec", changed_nsec);
    TrashFingerprint::from_file_info(&info)
}

fn trash_item(uri: &str, original: Option<&Path>, deleted_at: Option<i64>) -> TrashItem {
    trash_item_with_fingerprint(uri, original, deleted_at, Some("generation-1"))
}

fn trash_item_with_fingerprint(
    uri: &str,
    original: Option<&Path>,
    deleted_at: Option<i64>,
    generation: Option<&str>,
) -> TrashItem {
    let uri = TrashUri::parse(uri).unwrap();
    let display = uri.name().to_string_lossy().into_owned();
    TrashItem::new(
        TrashIdentity::new(
            uri,
            original.map(Path::to_path_buf),
            deleted_at,
            generation.map(fingerprint),
        ),
        display,
        false,
        None,
    )
}

fn snapshot(root: &Path) -> Vec<(PathBuf, String)> {
    fn walk(root: &Path, current: &Path, entries: &mut Vec<(PathBuf, String)>) {
        let mut children: Vec<_> = fs::read_dir(current).unwrap().flatten().collect();
        children.sort_by_key(|entry| entry.file_name());
        for child in children {
            let path = child.path();
            let metadata = fs::symlink_metadata(&path).unwrap();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if metadata.file_type().is_symlink() {
                entries.push((
                    relative,
                    format!("link:{}", fs::read_link(&path).unwrap().display()),
                ));
            } else if metadata.is_dir() {
                entries.push((relative, "dir".to_string()));
                walk(root, &path, entries);
            } else {
                entries.push((relative, fs::read_to_string(&path).unwrap_or_default()));
            }
        }
    }

    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    entries
}

// ---------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Restore and empty trash
// ---------------------------------------------------------------------------

fn restored_operations(_scratch: &Scratch, items: Vec<TrashItem>) -> (FileOperations, TestTrash) {
    for item in &items {
        if let Some(original) = item.original()
            && let Some(parent) = original.parent()
        {
            fs::create_dir_all(parent).unwrap();
        }
    }
    let backend = TestTrash::with(items);
    (
        FileOperations::with_trash_backend(Box::new(backend.clone())),
        backend,
    )
}

#[test]
fn missing_dates_restore_but_missing_locations_do_not() {
    let scratch = Scratch::new("restore-metadata");
    let original = scratch.join("a.txt");
    let with_date = trash_item("trash:///a.txt", Some(&original), Some(1_700_000_000));
    let without_date = trash_item("trash:///b.txt", Some(&scratch.join("b.txt")), None);
    let no_original = trash_item("trash:///c.txt", None, None);
    let (operations, _) =
        restored_operations(&scratch, vec![with_date, without_date.clone(), no_original]);

    let plan = operations.plan(&Request::Restore {
        items: vec![
            without_date.identity().clone(),
            trash_item("trash:///c.txt", None, None).identity().clone(),
        ],
    });
    assert_eq!(plan.steps().len(), 2);
    assert_eq!(plan.steps()[0].conflict, None);
    assert_eq!(plan.steps()[0].target, Some(scratch.join("b.txt")));
    assert_eq!(plan.steps()[1].conflict, Some(Conflict::MissingOriginal));
    assert_eq!(
        plan.steps()[1]
            .trash
            .as_ref()
            .map(|item| item.uri().as_str()),
        Some("trash:///c.txt")
    );
}

#[test]
fn a_missing_original_parent_is_refused_with_its_own_message() {
    let scratch = Scratch::new("restore-parent");
    let missing = scratch.join("gone").join("a.txt");
    let item = trash_item("trash:///a.txt", Some(&missing), None);
    let mut operations =
        FileOperations::with_trash_backend(Box::new(TestTrash::with(vec![item.clone()])));
    let plan = operations.plan(&Request::Restore {
        items: vec![item.identity().clone()],
    });
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::MissingParent));
    assert_eq!(
        Conflict::MissingParent.text(),
        "Original folder no longer exists"
    );

    let report = operations
        .execute(
            &plan,
            &Decisions::new(Vec::new()),
            &gio::Cancellable::new(),
            |_| {},
        )
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("Original folder no longer exists".to_string())
    );
}

#[test]
fn restore_conflicts_take_apply_to_all_and_a_cancel_runs_nothing() {
    let scratch = Scratch::new("restore-conflicts");
    let folder_a = scratch.join("a");
    let folder_b = scratch.join("b");
    fs::create_dir_all(&folder_a).unwrap();
    fs::create_dir_all(&folder_b).unwrap();
    let target_a = folder_a.join("one.txt");
    let target_b = folder_b.join("two.txt");
    write(&target_a, "old");
    let a = trash_item("trash:///one.txt", Some(&target_a), None);
    let b = trash_item("trash:///two.txt", Some(&target_b), None);
    let (mut operations, backend) = restored_operations(&scratch, vec![a.clone(), b.clone()]);

    let plan = operations.plan(&Request::Restore {
        items: vec![a.identity().clone(), b.identity().clone()],
    });
    assert_eq!(plan.decisions().count(), 1);
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(
        ItemKey::Trash(a.identity().uri().clone()),
        Decision::KeepBoth,
        true,
    );
    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(read(&target_a), "old");
    assert_eq!(read(&folder_a.join("one (copy).txt")), "restored one.txt");
    assert_eq!(read(&target_b), "restored two.txt");
    assert!(backend.remaining().is_empty());

    let a2 = trash_item(
        "trash:///three.txt",
        Some(&folder_a.join("three.txt")),
        None,
    );
    write(&folder_a.join("three.txt"), "existing");
    let (mut operations, backend) = restored_operations(&scratch, vec![a2.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![a2.identity().clone()],
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(
        ItemKey::Trash(a2.identity().uri().clone()),
        Decision::Cancel,
        false,
    );
    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(read(&folder_a.join("three.txt")), "existing");
    assert_eq!(backend.remaining(), vec!["trash:///three.txt"]);
}

#[test]
fn a_confirmed_empty_deletes_only_its_snapshot_and_refuses_changes() {
    let scratch = Scratch::new("empty-snapshot");
    let original = scratch.join("a.txt");
    let first = trash_item("trash:///a.txt", Some(&original), Some(1_700_000_000));
    let second = trash_item(
        "trash:///b.txt",
        Some(&scratch.join("b.txt")),
        Some(1_700_000_100),
    );
    let backend = TestTrash::with(vec![first.clone(), second.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    let plan = operations.plan(&Request::EmptyTrash);
    assert_eq!(plan.steps().len(), 2);
    let confirmation = EmptyConfirmation::for_plan(&plan).expect("an empty plan confirms");

    let late = trash_item("trash:///late.txt", Some(&scratch.join("late.txt")), None);
    backend.add(late);
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(
        backend.deleted(),
        vec!["trash:///a.txt", "trash:///b.txt"],
        "the later item is not in the confirmed snapshot"
    );
    assert_eq!(backend.remaining(), vec!["trash:///late.txt"]);

    // A different object at the same URI with identical display metadata is
    // refused. Original path and date alone are not object identity.
    let changing = trash_item(
        "trash:///change.txt",
        Some(&scratch.join("first.txt")),
        Some(1_700_000_200),
    );
    let backend = TestTrash::with(vec![changing.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    {
        let mut state = backend.state.lock().unwrap();
        state.items[0] = trash_item_with_fingerprint(
            "trash:///change.txt",
            Some(&scratch.join("first.txt")),
            Some(1_700_000_200),
            Some("generation-2"),
        );
    }
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert!(backend.deleted().is_empty());
}

#[test]
fn no_confirmation_means_no_deletion() {
    let scratch = Scratch::new("empty-no-confirmation");
    let item = trash_item("trash:///a.txt", Some(&scratch.join("a.txt")), None);
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let error = operations
        .execute(
            &plan,
            &Decisions::new(Vec::new()),
            &gio::Cancellable::new(),
            |_| {},
        )
        .unwrap_err();
    assert_eq!(error, ExecuteError::ConfirmationRequired);
    assert!(backend.deleted().is_empty());

    let wrong = operations.plan(&Request::EmptyTrash);
    let other = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![scratch.join("a.txt")],
        to: scratch.join("dst"),
    });
    let token = EmptyConfirmation::for_plan(&wrong).unwrap();
    assert_eq!(
        operations
            .execute_empty(&other, token, &gio::Cancellable::new(), |_| {})
            .unwrap_err(),
        ExecuteError::NotAnEmptyPlan
    );
}

#[test]
fn a_partial_empty_and_a_cancel_report_what_was_deleted() {
    let scratch = Scratch::new("empty-partial");
    let a = trash_item("trash:///a.txt", Some(&scratch.join("a.txt")), None);
    let b = trash_item("trash:///b.txt", Some(&scratch.join("b.txt")), None);
    let c = trash_item("trash:///c.txt", Some(&scratch.join("c.txt")), None);
    let backend = TestTrash::with(vec![a, b.clone(), c]);
    backend
        .state
        .lock()
        .unwrap()
        .fail_delete
        .push("trash:///b.txt".to_string());
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(!report.all_succeeded());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt", "trash:///c.txt"]);
    assert_eq!(backend.remaining(), vec!["trash:///b.txt"]);

    let cancellable = gio::Cancellable::new();
    cancellable.cancel();
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &cancellable, |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(
        backend.deleted(),
        vec!["trash:///a.txt", "trash:///c.txt"],
        "cancel adds no deletion"
    );
    assert_eq!(backend.remaining(), vec!["trash:///b.txt"]);
}

#[test]
fn an_already_gone_item_is_skipped_and_the_rest_continue() {
    let scratch = Scratch::new("empty-already-gone");
    let a = trash_item("trash:///a.txt", Some(&scratch.join("a.txt")), None);
    let b = trash_item("trash:///b.txt", Some(&scratch.join("b.txt")), None);
    let backend = TestTrash::with(vec![a, b]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    backend.state.lock().unwrap().items.remove(0);

    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(report.items[0].outcome, ItemOutcome::Skipped);
    assert_eq!(report.items[1].outcome, ItemOutcome::Done);
    assert_eq!(backend.deleted(), vec!["trash:///b.txt"]);
}

#[test]
fn supported_identity_without_optional_etag_or_metadata_deletes() {
    let _scratch = Scratch::new("empty-no-optional");
    let uri = TrashUri::parse("trash:///a.txt").unwrap();
    let item = TrashItem::new(
        TrashIdentity::new(
            uri,
            None,
            None,
            Some(core_fingerprint("generation-1", 123_456_789)),
        ),
        "a.txt".to_string(),
        false,
        None,
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
}

#[test]
fn a_precise_ctime_change_refuses_the_confirmed_item() {
    let scratch = Scratch::new("empty-ctime-change");
    let uri = TrashUri::parse("trash:///a.txt").unwrap();
    let item = TrashItem::new(
        TrashIdentity::new(
            uri.clone(),
            Some(scratch.join("a.txt")),
            Some(1_700_000_000),
            Some(core_fingerprint("generation-1", 123_456_789)),
        ),
        "a.txt".to_string(),
        false,
        None,
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    backend.state.lock().unwrap().items[0] = TrashItem::new(
        TrashIdentity::new(
            uri,
            Some(scratch.join("a.txt")),
            Some(1_700_000_000),
            Some(core_fingerprint("generation-1", 123_456_790)),
        ),
        "a.txt".to_string(),
        false,
        None,
    );

    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert!(backend.deleted().is_empty());
}

#[test]
fn trash_dry_runs_list_without_writing() {
    let scratch = Scratch::new("trash-dry-run");
    let item = trash_item("trash:///a.txt", Some(&scratch.join("a.txt")), None);
    let backend = TestTrash::with(vec![item.clone()]);
    let operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let restore = operations.dry_run(&Request::Restore {
        items: vec![item.identity().clone()],
    });
    assert!(restore.iter().any(|line| line.contains("trash:///a.txt")));
    assert_eq!(backend.remaining(), vec!["trash:///a.txt"]);

    let empty = operations.dry_run(&Request::EmptyTrash);
    assert!(
        empty
            .iter()
            .any(|line| line.contains("delete trash:///a.txt"))
    );
    assert_eq!(backend.deleted(), Vec::<String>::new());
    assert_eq!(backend.remaining(), vec!["trash:///a.txt"]);
}

#[test]
fn selected_delete_plans_only_the_typed_items_and_never_lists_trash() {
    let a = trash_item("trash:///a.txt", None, None);
    let b = trash_item("trash:///b.txt", None, None);
    let backend = TestTrash::with(vec![a.clone(), b.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));

    let request = Request::DeleteSelected {
        items: vec![a.identity().clone()],
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.steps().len(), 1);
    assert_eq!(
        plan.steps()[0]
            .trash
            .as_ref()
            .map(|item| item.uri().as_str()),
        Some("trash:///a.txt")
    );
    assert_eq!(
        backend.list_calls(),
        0,
        "the selected plan must not scan the whole Trash"
    );

    let dry_run = operations.dry_run(&request);
    assert!(dry_run.iter().any(|line| line.contains("trash:///a.txt")));
    assert!(backend.deleted().is_empty());

    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
    assert_eq!(backend.remaining(), vec!["trash:///b.txt"]);
}

#[test]
fn selected_delete_skips_missing_and_refuses_changed_items() {
    let a = trash_item("trash:///a.txt", None, None);
    let b = trash_item("trash:///b.txt", None, None);
    let unselected = trash_item("trash:///keep.txt", None, None);
    let backend = TestTrash::with(vec![a.clone(), b.clone(), unselected.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected {
        items: vec![a.identity().clone(), b.identity().clone()],
    };
    let plan = operations.plan(&request);
    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();

    backend.state.lock().unwrap().items.remove(1);
    backend.state.lock().unwrap().items[0] =
        trash_item_with_fingerprint("trash:///a.txt", None, None, Some("generation-2"));

    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert_eq!(report.items[1].outcome, ItemOutcome::Skipped);
    assert!(backend.deleted().is_empty());
    assert_eq!(
        backend.remaining(),
        vec!["trash:///a.txt", "trash:///keep.txt"]
    );
}

#[test]
fn selected_delete_missing_optional_metadata_still_deletes() {
    let item = trash_item_with_fingerprint("trash:///a.txt", None, None, Some("generation-1"));
    let backend = TestTrash::with(vec![item.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected {
        items: vec![item.identity().clone()],
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.steps()[0].conflict, None);
    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
}

#[test]
fn selected_delete_refuses_an_unavailable_identity() {
    let item = trash_item_with_fingerprint("trash:///a.txt", None, None, None);
    let backend = TestTrash::with(vec![item.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected {
        items: vec![item.identity().clone()],
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::Unverified));
    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("Cannot verify this Trash item is unchanged".to_string())
    );
    assert!(backend.deleted().is_empty());
    assert_eq!(backend.remaining(), vec!["trash:///a.txt"]);
}

#[test]
fn selected_delete_reports_a_mixed_result_and_a_final_cancel() {
    let a = trash_item("trash:///a.txt", None, None);
    let b = trash_item("trash:///b.txt", None, None);
    let c = trash_item("trash:///c.txt", None, None);
    let backend = TestTrash::with(vec![a.clone(), b.clone(), c.clone()]);
    backend
        .state
        .lock()
        .unwrap()
        .fail_delete
        .push("trash:///b.txt".to_string());
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected {
        items: vec![
            a.identity().clone(),
            b.identity().clone(),
            c.identity().clone(),
        ],
    };
    let plan = operations.plan(&request);
    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(!report.all_succeeded());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt", "trash:///c.txt"]);
    assert_eq!(backend.remaining(), vec!["trash:///b.txt"]);

    let d = trash_item("trash:///d.txt", None, None);
    let backend = TestTrash::with(vec![d.clone()]);
    backend.state.lock().unwrap().cancel_during_delete = true;
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let request = Request::DeleteSelected {
        items: vec![d.identity().clone()],
    };
    let plan = operations.plan(&request);
    let confirmation = SelectedConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_selected(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items[0].outcome, ItemOutcome::Cancelled);
    assert!(backend.deleted().is_empty());
}

#[test]
fn selected_delete_needs_its_own_confirmation_and_exact_plan() {
    let a = trash_item("trash:///a.txt", None, None);
    let b = trash_item("trash:///b.txt", None, None);
    let backend = TestTrash::with(vec![a.clone(), b.clone()]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::DeleteSelected {
        items: vec![a.identity().clone()],
    });
    assert!(EmptyConfirmation::for_plan(&plan).is_none());
    assert!(plan.selected_confirmation().is_some());
    assert_eq!(
        operations
            .execute(
                &plan,
                &Decisions::new(Vec::new()),
                &gio::Cancellable::new(),
                |_| {}
            )
            .unwrap_err(),
        ExecuteError::ConfirmationRequired
    );
    assert!(backend.deleted().is_empty());

    let token = SelectedConfirmation::for_plan(&plan).unwrap();
    let different = operations.plan(&Request::DeleteSelected {
        items: vec![a.identity().clone(), b.identity().clone()],
    });
    assert_eq!(
        operations
            .execute_selected(&different, token, &gio::Cancellable::new(), |_| {})
            .unwrap_err(),
        ExecuteError::ConfirmationMismatch
    );
    assert!(backend.deleted().is_empty());

    let empty = operations.plan(&Request::EmptyTrash);
    assert!(SelectedConfirmation::for_plan(&empty).is_none());
    let empty_token = EmptyConfirmation::for_plan(&empty).unwrap();
    assert_eq!(
        operations
            .execute_empty(&plan, empty_token, &gio::Cancellable::new(), |_| {})
            .unwrap_err(),
        ExecuteError::NotAnEmptyPlan
    );
}

#[test]
fn selected_delete_rejects_an_empty_or_inconsistent_snapshot() {
    let a = trash_item("trash:///a.txt", None, None);
    let backend = TestTrash::with(vec![a.clone()]);
    let operations = FileOperations::with_trash_backend(Box::new(backend));
    let empty = operations.plan(&Request::DeleteSelected { items: Vec::new() });
    assert!(empty.problem().is_some());
    assert_eq!(empty.steps().len(), 0);

    let duplicate = operations.plan(&Request::DeleteSelected {
        items: vec![a.identity().clone(), a.identity().clone()],
    });
    assert_eq!(duplicate.steps().len(), 1);
    assert!(duplicate.problem().is_none());

    let replacement =
        trash_item_with_fingerprint("trash:///a.txt", None, None, Some("generation-2"));
    let inconsistent = operations.plan(&Request::DeleteSelected {
        items: vec![a.identity().clone(), replacement.identity().clone()],
    });
    assert!(inconsistent.problem().is_some());
    assert_eq!(inconsistent.steps().len(), 0);
}

#[test]
fn a_restore_records_the_actual_path_for_undo() {
    // The executor itself is exercised in tests/trash_browser.rs with a
    // filesystem backend. Here the plan's destination is the actual one,
    // including a Keep Both name.
    let scratch = Scratch::new("restore-undo-plan");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("a.txt");
    write(&target, "old");
    let item = trash_item("trash:///a.txt", Some(&target), None);
    let (mut operations, _) = restored_operations(&scratch, vec![item.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![item.identity().clone()],
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(
        ItemKey::Trash(item.identity().uri().clone()),
        Decision::KeepBoth,
        false,
    );
    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded());
    let actual = folder.join("a (copy).txt");
    assert_eq!(report.items[0].target.as_deref(), Some(actual.as_path()));

    let undo = operations.plan(&Request::Undo);
    assert_eq!(undo.steps().len(), 1);
    assert_eq!(undo.steps()[0].action, Action::Trash);
    assert_eq!(undo.steps()[0].source.as_deref(), Some(actual.as_path()));
}

#[test]
fn topdir_relative_original_paths_resolve_against_the_mount() {
    use crate::fileops::trashinfo::resolve_original;
    let home = PathBuf::from("/home/kyle");
    let home_trash = home.join(".local/share/Trash");
    assert_eq!(
        resolve_original(&home_trash, &home_trash, Path::new("/home/kyle/a")),
        PathBuf::from("/home/kyle/a")
    );
    assert_eq!(
        resolve_original(
            Path::new("/media/usb/.Trash-1000"),
            &home_trash,
            Path::new("docs/a.txt")
        ),
        PathBuf::from("/media/usb/docs/a.txt")
    );
    assert_eq!(
        resolve_original(
            Path::new("/media/usb/.Trash/1000"),
            &home_trash,
            Path::new("docs/a.txt")
        ),
        PathBuf::from("/media/usb/docs/a.txt")
    );
}

#[test]
fn names_are_checked_against_the_rules() {
    assert!(is_valid_name("file.txt"));
    assert!(is_valid_name("folder"));
    assert!(is_valid_name(&"a".repeat(255)));

    assert!(!is_valid_name(""));
    assert!(!is_valid_name("a/b"));
    assert!(!is_valid_name("a\0b"));
    assert!(!is_valid_name("."));
    assert!(!is_valid_name(".."));
    assert!(!is_valid_name(&"a".repeat(256)));
}

#[test]
fn copy_names_add_copy_then_numbers() {
    assert_eq!(copy_name(OsStr::new("file.txt"), 0), "file (copy).txt");
    assert_eq!(copy_name(OsStr::new("file.txt"), 1), "file (copy 2).txt");
    assert_eq!(copy_name(OsStr::new("folder"), 0), "folder (copy)");
    assert_eq!(copy_name(OsStr::new("folder"), 2), "folder (copy 3)");
    assert_eq!(copy_name(OsStr::new(".bashrc"), 0), ".bashrc (copy)");
    assert_eq!(copy_name(OsStr::new("a.tar.gz"), 0), "a.tar (copy).gz");
}

// ---------------------------------------------------------------------------
// Request mapping
// ---------------------------------------------------------------------------

#[test]
fn every_files_command_maps_to_its_request() {
    assert_eq!(
        request_for(&check("file.trash", &[("paths", "/tmp/a")]))
            .unwrap()
            .unwrap(),
        Request::Trash {
            scope: Scope::User,
            paths: vec![PathBuf::from("/tmp/a")]
        }
    );
    assert_eq!(
        request_for(&check(
            "file.copy",
            &[("paths", "/tmp/a"), ("to", "/tmp/b")]
        ))
        .unwrap()
        .unwrap(),
        Request::Copy {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![PathBuf::from("/tmp/a")],
            to: PathBuf::from("/tmp/b"),
        }
    );
    assert_eq!(
        request_for(&check(
            "file.move",
            &[("paths", "/tmp/a"), ("paths", "/tmp/c"), ("to", "/tmp/b")]
        ))
        .unwrap()
        .unwrap(),
        Request::Move {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/c")],
            to: PathBuf::from("/tmp/b"),
        }
    );
    assert_eq!(
        request_for(&check("file.rename", &[("path", "/tmp/a"), ("name", "b")]))
            .unwrap()
            .unwrap(),
        Request::Rename {
            scope: Scope::User,
            path: PathBuf::from("/tmp/a"),
            name: "b".to_string(),
        }
    );
    assert_eq!(
        request_for(&check(
            "file.new-folder",
            &[("name", "new"), ("in", "/tmp")]
        ))
        .unwrap()
        .unwrap(),
        Request::NewFolder {
            scope: Scope::User,
            name: "new".to_string(),
            in_folder: PathBuf::from("/tmp"),
        }
    );
    assert_eq!(
        request_for(&check(
            "file.new-file",
            &[("name", "empty.txt"), ("in", "/tmp")]
        ))
        .unwrap()
        .unwrap(),
        Request::NewFile {
            scope: Scope::User,
            name: "empty.txt".to_string(),
            in_folder: PathBuf::from("/tmp"),
        }
    );
    assert_eq!(
        request_for(&check("file.undo", &[])).unwrap().unwrap(),
        Request::Undo
    );
    assert_eq!(
        request_for(&check(
            "trash.delete-permanently",
            &[("items", "trash:///a.txt"), ("items", "trash:///b.txt")]
        ))
        .unwrap()
        .unwrap(),
        Request::DeleteSelected {
            items: vec![
                TrashIdentity::new(TrashUri::parse("trash:///a.txt").unwrap(), None, None, None),
                TrashIdentity::new(TrashUri::parse("trash:///b.txt").unwrap(), None, None, None),
            ],
        }
    );
}

#[test]
fn a_command_outside_the_file_operations_is_not_a_request() {
    assert_eq!(
        request_for(&check("nav.go", &[("path", "/tmp")])).unwrap(),
        None
    );
    assert_eq!(request_for(&check("preview.save", &[])).unwrap(), None);
}

#[test]
fn request_mapping_refuses_unfilled_context_paths() {
    // Ui validation allows the context paths to be omitted; the handler must
    // fill them. request_for refuses rather than guessing.
    let error = request_for(&check("file.trash", &[])).unwrap_err();
    assert_eq!(error, RequestError::MissingArgument("paths"));

    let error = request_for(&check("file.rename", &[("name", "b")])).unwrap_err();
    assert_eq!(error, RequestError::MissingArgument("path"));

    let error = request_for(&check("file.new-folder", &[("name", "new")])).unwrap_err();
    assert_eq!(error, RequestError::MissingArgument("in"));

    let error = request_for(&check("file.new-file", &[("name", "empty.txt")])).unwrap_err();
    assert_eq!(error, RequestError::MissingArgument("in"));
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

fn conflicts() -> Vec<ItemKey> {
    vec![
        ItemKey::path("/tmp/a"),
        ItemKey::path("/tmp/b"),
        ItemKey::path("/tmp/c"),
    ]
}

#[test]
fn three_conflicts_are_answered_one_by_one() {
    let mut decisions = Decisions::new(conflicts());
    assert_eq!(decisions.next_unanswered(), Some(&ItemKey::path("/tmp/a")));

    decisions.answer(ItemKey::path("/tmp/a"), Decision::Skip, false);
    assert_eq!(decisions.next_unanswered(), Some(&ItemKey::path("/tmp/b")));

    decisions.answer(ItemKey::path("/tmp/b"), Decision::KeepBoth, false);
    assert_eq!(decisions.next_unanswered(), Some(&ItemKey::path("/tmp/c")));

    decisions.answer(ItemKey::path("/tmp/c"), Decision::Replace, false);
    assert_eq!(decisions.next_unanswered(), None);
    assert!(decisions.is_complete());
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/a")),
        Some(Decision::Skip)
    );
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/b")),
        Some(Decision::KeepBoth)
    );
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/c")),
        Some(Decision::Replace)
    );
}

#[test]
fn the_first_answer_can_apply_to_all() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(ItemKey::path("/tmp/a"), Decision::KeepBoth, true);

    for item in conflicts() {
        assert_eq!(decisions.decision_for(&item), Some(Decision::KeepBoth));
    }
    assert_eq!(decisions.next_unanswered(), None);
    assert!(decisions.is_complete());
}

#[test]
fn apply_to_all_does_not_overwrite_an_earlier_answer() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(ItemKey::path("/tmp/a"), Decision::Skip, false);
    decisions.answer(ItemKey::path("/tmp/b"), Decision::Replace, true);

    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/a")),
        Some(Decision::Skip)
    );
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/b")),
        Some(Decision::Replace)
    );
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/c")),
        Some(Decision::Replace)
    );
}

#[test]
fn cancel_ends_the_questions_at_once() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(ItemKey::path("/tmp/a"), Decision::Skip, false);
    decisions.answer(ItemKey::path("/tmp/b"), Decision::Cancel, false);

    assert!(decisions.is_cancelled());
    assert!(decisions.is_complete());
    assert_eq!(decisions.next_unanswered(), None);
    assert_eq!(
        decisions.decision_for(&ItemKey::path("/tmp/c")),
        None,
        "cancel ends the questions without answering the rest"
    );
}

#[test]
fn decisions_cancel_with_apply_to_all_marks_the_run_cancelled() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(ItemKey::path("/tmp/a"), Decision::Cancel, true);
    assert!(decisions.is_cancelled());
    assert!(decisions.is_complete());
    assert_eq!(decisions.next_unanswered(), None);
}

// ---------------------------------------------------------------------------
// Planning
// ---------------------------------------------------------------------------

#[test]
fn a_copy_with_a_conflict_lists_the_target() {
    let scratch = Scratch::new("plan-conflict");
    let source = scratch.join("a.txt");
    write(&source, "new");
    fs::create_dir(scratch.join("dst")).unwrap();
    write(&scratch.join("dst/a.txt"), "old");

    let operations = FileOperations::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![source.clone()],
        to: scratch.join("dst"),
    });

    assert_eq!(plan.steps().len(), 1);
    assert_eq!(plan.steps()[0].source, Some(source));
    assert_eq!(plan.steps()[0].target, Some(scratch.join("dst/a.txt")));
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::TargetExists));
    assert!(plan.lines()[1].contains("[the target exists]"));
}

#[test]
fn missing_sources_are_refused_for_their_item() {
    let scratch = Scratch::new("plan-missing");
    fs::create_dir(scratch.join("dst")).unwrap();

    let operations = FileOperations::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![scratch.join("gone.txt")],
        to: scratch.join("dst"),
    });

    assert_eq!(plan.steps()[0].conflict, Some(Conflict::MissingSource));
    assert!(!plan.steps()[0].conflict.unwrap().needs_decision());
}

#[test]
fn renaming_to_an_invalid_or_taken_name_is_a_conflict() {
    let scratch = Scratch::new("plan-rename");
    let first = scratch.join("a.txt");
    write(&first, "a");
    write(&scratch.join("b.txt"), "b");

    let operations = FileOperations::new();
    let taken = operations.plan(&Request::Rename {
        scope: Scope::User,
        path: first.clone(),
        name: "b.txt".to_string(),
    });
    assert_eq!(taken.steps()[0].conflict, Some(Conflict::NameTaken));

    let invalid = operations.plan(&Request::Rename {
        scope: Scope::User,
        path: first,
        name: "a/b".to_string(),
    });
    assert_eq!(invalid.steps()[0].conflict, Some(Conflict::InvalidName));
}

#[test]
fn a_new_folder_with_a_taken_or_invalid_name_is_a_conflict() {
    let scratch = Scratch::new("plan-new-folder");
    fs::create_dir(scratch.join("taken")).unwrap();

    let operations = FileOperations::new();
    let taken = operations.plan(&Request::NewFolder {
        scope: Scope::User,
        name: "taken".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert_eq!(taken.steps()[0].conflict, Some(Conflict::NameTaken));

    let invalid = operations.plan(&Request::NewFolder {
        scope: Scope::User,
        name: "..".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert_eq!(invalid.steps()[0].conflict, Some(Conflict::InvalidName));
}

#[test]
fn a_new_file_with_an_invalid_name_or_any_existing_target_is_a_conflict() {
    let scratch = Scratch::new("plan-new-file");
    write(&scratch.join("file.txt"), "keep");
    fs::create_dir(scratch.join("folder")).unwrap();
    let dangling = scratch.join("dangling");
    symlink(&scratch.join("missing.txt"), &dangling);

    let operations = FileOperations::new();
    for name in ["", ".", "..", "a/b", "a\0b"] {
        let plan = operations.plan(&Request::NewFile {
            scope: Scope::User,
            name: name.to_string(),
            in_folder: scratch.path.clone(),
        });
        assert_eq!(
            plan.steps()[0].conflict,
            Some(Conflict::InvalidName),
            "{name:?}"
        );
    }

    let missing = operations.plan(&Request::NewFile {
        scope: Scope::User,
        name: "a.txt".to_string(),
        in_folder: scratch.join("missing-folder"),
    });
    assert_eq!(
        missing.steps()[0].conflict,
        Some(Conflict::MissingDestination)
    );

    for name in ["file.txt", "folder", "dangling"] {
        let plan = operations.plan(&Request::NewFile {
            scope: Scope::User,
            name: name.to_string(),
            in_folder: scratch.path.clone(),
        });
        assert_eq!(
            plan.steps()[0].conflict,
            Some(Conflict::NameTaken),
            "{name}"
        );
    }

    // The collision check neither truncated the file nor followed the link.
    assert_eq!(read(&scratch.join("file.txt")), "keep");
    assert_eq!(
        fs::read_link(&dangling).unwrap(),
        scratch.join("missing.txt")
    );
}

#[test]
fn a_new_file_is_zero_bytes_and_a_dry_run_only_reports_the_plan() {
    let scratch = Scratch::new("new-file-run");
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let request = Request::NewFile {
        scope: Scope::User,
        name: "empty.txt".to_string(),
        in_folder: scratch.path.clone(),
    };

    let dry = operations.dry_run(&request);
    assert!(dry[0].contains("create file empty.txt"));
    assert!(dry[1].contains("new-file"));
    assert!(!scratch.join("empty.txt").exists(), "dry run wrote a file");

    let report = run(
        &mut operations,
        &request,
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    let metadata = fs::metadata(scratch.join("empty.txt")).expect("created file");
    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 0);
}

#[test]
fn a_name_that_appears_after_planning_is_refused_without_touching_it() {
    let scratch = Scratch::new("new-file-race");
    let target = scratch.join("empty.txt");
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let request = Request::NewFile {
        scope: Scope::User,
        name: "empty.txt".to_string(),
        in_folder: scratch.path.clone(),
    };

    let plan = operations.plan(&request);
    assert_eq!(plan.steps()[0].conflict, None);
    write(&target, "already here");

    let report = operations
        .execute(&plan, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused(Conflict::NameTaken.text().to_string())
    );
    assert_eq!(read(&target), "already here");
}

#[test]
fn creating_a_file_leaves_the_previous_undo_entry_available() {
    let scratch = Scratch::new("new-file-undo");
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();

    run(
        &mut operations,
        &Request::NewFolder {
            scope: Scope::User,
            name: "older".to_string(),
            in_folder: scratch.path.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert_eq!(operations.undo.len(), 1);

    run(
        &mut operations,
        &Request::NewFile {
            scope: Scope::User,
            name: "empty.txt".to_string(),
            in_folder: scratch.path.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(scratch.join("empty.txt").is_file());
    assert_eq!(operations.undo.len(), 1);
    assert!(matches!(
        operations.undo[0].kind,
        RecordKind::NewFolder { .. }
    ));
    let undo = operations.plan(&Request::Undo);
    assert_eq!(
        undo.steps()[0].source.as_deref(),
        Some(scratch.join("older").as_path())
    );
}

#[test]
fn a_folder_into_itself_is_refused() {
    let scratch = Scratch::new("plan-into-itself");
    let folder = scratch.join("tree");
    fs::create_dir(&folder).unwrap();
    write(&folder.join("a.txt"), "a");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![folder.clone()],
        to: folder.clone(),
    });
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::FolderIntoItself));

    let report = operations
        .execute(&plan, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the folder is inside itself".to_string())
    );
    assert!(!folder.join("tree").exists());
}

#[test]
fn moving_into_the_folder_an_item_is_already_in_is_refused() {
    let scratch = Scratch::new("plan-already-there");
    let file = scratch.join("a.txt");
    write(&file, "a");

    let operations = FileOperations::new();
    let plan = operations.plan(&Request::Move {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![file],
        to: scratch.path.clone(),
    });
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::AlreadyThere));
}

// ---------------------------------------------------------------------------
// Dry runs and execution
// ---------------------------------------------------------------------------

#[test]
fn a_dry_run_changes_nothing_on_disk() {
    let scratch = Scratch::new("dry-run");
    let source = scratch.join("a.txt");
    write(&source, "new");
    fs::create_dir(scratch.join("dst")).unwrap();
    write(&scratch.join("dst/a.txt"), "old");

    let before = snapshot(&scratch.path);
    let operations = FileOperations::new();
    let lines = operations.dry_run(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![source],
        to: scratch.join("dst"),
    });
    let after = snapshot(&scratch.path);

    assert_eq!(before, after);
    assert!(lines[0].starts_with("dry run: copy"));
    assert!(lines[1].contains("->"));
    assert!(lines[1].contains("[the target exists]"));
}

#[test]
fn a_folder_tree_with_a_symlink_is_copied_faithfully() {
    let scratch = Scratch::new("copy-tree");
    let tree = scratch.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    write(&tree.join("file.txt"), "hello");
    write(&tree.join("sub/inner.txt"), "inner");
    symlink(Path::new("file.txt"), &tree.join("link"));
    symlink(Path::new("no-such-target"), &tree.join("dangling"));
    fs::create_dir(scratch.join("dst")).unwrap();

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let report = run(
        &mut operations,
        &Request::Copy {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![tree],
            to: scratch.join("dst"),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );

    assert!(report.all_succeeded(), "{:?}", report.lines());
    let copied = scratch.join("dst/tree");
    assert_eq!(read(&copied.join("file.txt")), "hello");
    assert_eq!(read(&copied.join("sub/inner.txt")), "inner");
    assert_eq!(
        fs::read_link(copied.join("link")).unwrap(),
        PathBuf::from("file.txt")
    );
    assert_eq!(
        fs::read_link(copied.join("dangling")).unwrap(),
        PathBuf::from("no-such-target")
    );
    assert!(
        fs::symlink_metadata(copied.join("link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn copying_into_its_own_folder_uses_copy_and_copy_2() {
    let scratch = Scratch::new("copy-own-folder");
    let file = scratch.join("a.txt");
    write(&file, "original");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();

    let first = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![file.clone()],
        to: scratch.path.clone(),
    });
    assert_eq!(first.steps()[0].conflict, None);
    assert!(first.steps()[0].keep_both);
    let report = operations
        .execute(&first, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap();
    assert!(report.all_succeeded());

    let second = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![file.clone()],
        to: scratch.path.clone(),
    });
    let report = operations
        .execute(&second, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap();
    assert!(report.all_succeeded());

    assert_eq!(read(&file), "original");
    assert_eq!(read(&scratch.join("a (copy).txt")), "original");
    assert_eq!(read(&scratch.join("a (copy 2).txt")), "original");
}

#[test]
fn skip_and_keep_both_are_applied_per_item() {
    let scratch = Scratch::new("decisions-skip-keepboth");
    let source = scratch.join("src");
    fs::create_dir(&source).unwrap();
    let a = source.join("a.txt");
    let b = source.join("b.txt");
    write(&a, "new-a");
    write(&b, "new-b");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();
    write(&destination.join("a.txt"), "old-a");
    write(&destination.join("b.txt"), "old-b");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![a.clone(), b.clone()],
        to: destination.clone(),
    });
    assert_eq!(plan.decisions().count(), 2);

    let mut decisions = Decisions::new(plan.decisions().filter_map(Step::decision_key));
    decisions.answer(key(&a), Decision::Skip, false);
    decisions.answer(key(&b), Decision::KeepBoth, false);
    let report = operations
        .execute(&plan, &decisions, &cancellable, |_| {})
        .unwrap();

    assert_eq!(report.items[0].outcome, ItemOutcome::Skipped);
    assert_eq!(report.items[1].outcome, ItemOutcome::Done);
    assert_eq!(read(&destination.join("a.txt")), "old-a");
    assert_eq!(read(&destination.join("b.txt")), "old-b");
    assert_eq!(read(&destination.join("b (copy).txt")), "new-b");
}

#[test]
fn unanswered_conflicts_refuse_the_whole_operation_and_name_the_items() {
    let scratch = Scratch::new("unanswered");
    let source = scratch.join("a.txt");
    write(&source, "new");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();
    write(&destination.join("a.txt"), "old");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![source.clone()],
        to: destination.clone(),
    });
    let error = operations
        .execute(&plan, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap_err();

    assert_eq!(
        error,
        ExecuteError::UnansweredConflicts(vec![ItemKey::path(source)])
    );
    assert!(error.to_string().contains("/tmp/gimbal-p2f-"));
    assert_eq!(read(&destination.join("a.txt")), "old");
}

#[test]
fn a_cancel_decision_runs_nothing() {
    let scratch = Scratch::new("cancel-decision");
    let source = scratch.join("a.txt");
    write(&source, "new");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();
    write(&destination.join("a.txt"), "old");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![source.clone()],
        to: destination.clone(),
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(key(&source), Decision::Cancel, false);

    let report = operations
        .execute(&plan, &decisions, &cancellable, |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items[0].outcome, ItemOutcome::Cancelled);
    assert_eq!(read(&destination.join("a.txt")), "old");
}

#[test]
fn a_cancel_partway_still_runs_nothing() {
    let scratch = Scratch::new("cancel-partway");
    let source = scratch.join("src");
    fs::create_dir(&source).unwrap();
    let a = source.join("a.txt");
    let b = source.join("b.txt");
    write(&a, "new-a");
    write(&b, "new-b");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();
    write(&destination.join("a.txt"), "old-a");
    write(&destination.join("b.txt"), "old-b");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths: vec![a.clone(), b.clone()],
        to: destination.clone(),
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(key(&a), Decision::Skip, false);
    decisions.answer(key(&b), Decision::Cancel, false);

    let report = operations
        .execute(&plan, &decisions, &cancellable, |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items.len(), 2);
    assert!(
        report
            .items
            .iter()
            .all(|item| item.outcome == ItemOutcome::Cancelled)
    );
    assert_eq!(read(&destination.join("a.txt")), "old-a");
    assert_eq!(read(&destination.join("b.txt")), "old-b");
}

#[test]
fn cancelling_stops_after_the_current_item() {
    let scratch = Scratch::new("cancel-halfway");
    let source = scratch.join("src");
    fs::create_dir(&source).unwrap();
    let mut paths = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let path = source.join(name);
        write(&path, name);
        paths.push(path);
    }
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Copy {
        scope: Scope::User,
        target_scope: Scope::User,
        paths,
        to: destination.clone(),
    });
    let trigger = cancellable.clone();
    let report = operations
        .execute(
            &plan,
            &Decisions::new(Vec::new()),
            &cancellable,
            move |progress| {
                if progress.items_done == 1 {
                    trigger.cancel();
                }
            },
        )
        .unwrap();

    assert!(report.cancelled);
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert_eq!(report.items[1].outcome, ItemOutcome::Cancelled);
    assert_eq!(report.items[2].outcome, ItemOutcome::Cancelled);
    assert!(destination.join("a.txt").exists());
    assert!(!destination.join("b.txt").exists());
    assert!(!destination.join("c.txt").exists());
}

#[test]
fn a_failed_item_does_not_stop_the_others() {
    let scratch = Scratch::new("partial-failure");
    let source = scratch.join("src");
    fs::create_dir(&source).unwrap();
    let good = source.join("good.txt");
    let bad = source.join("bad.txt");
    write(&good, "good");
    write(&bad, "bad");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o000)).unwrap();
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let report = run(
        &mut operations,
        &Request::Copy {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![good, bad.clone()],
            to: destination.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );

    fs::set_permissions(&bad, fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert!(matches!(report.items[1].outcome, ItemOutcome::Failed(_)));
    assert_eq!(read(&destination.join("good.txt")), "good");
    assert!(!destination.join("bad.txt").exists());
}

// ---------------------------------------------------------------------------
// Undo (operations that do not need the trash)
// ---------------------------------------------------------------------------

#[test]
fn undo_a_move_moves_the_item_back() {
    let scratch = Scratch::new("undo-move");
    let source = scratch.join("a.txt");
    write(&source, "a");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    run(
        &mut operations,
        &Request::Move {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![source.clone()],
            to: destination.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(!source.exists());
    assert_eq!(read(&destination.join("a.txt")), "a");

    let report = run(
        &mut operations,
        &Request::Undo,
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(report.all_succeeded());
    assert_eq!(read(&source), "a");
    assert!(!destination.join("a.txt").exists());
    assert_eq!(operations.undo_len(), 0);
}

#[test]
fn undo_a_rename_renames_it_back() {
    let scratch = Scratch::new("undo-rename");
    let original = scratch.join("a.txt");
    let renamed = scratch.join("b.txt");
    write(&original, "a");

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    run(
        &mut operations,
        &Request::Rename {
            scope: Scope::User,
            path: original.clone(),
            name: "b.txt".to_string(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(renamed.exists());

    let report = run(
        &mut operations,
        &Request::Undo,
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(report.all_succeeded());
    assert_eq!(read(&original), "a");
    assert!(!renamed.exists());
}

#[test]
fn undo_refuses_only_the_item_that_changed() {
    let scratch = Scratch::new("undo-changed");
    let source = scratch.join("src");
    fs::create_dir(&source).unwrap();
    let a = source.join("a.txt");
    let b = source.join("b.txt");
    write(&a, "a");
    write(&b, "b");
    let destination = scratch.join("dst");
    fs::create_dir(&destination).unwrap();

    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    run(
        &mut operations,
        &Request::Move {
            scope: Scope::User,
            target_scope: Scope::User,
            paths: vec![a.clone(), b.clone()],
            to: destination.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );

    // The second target changed since the move: it is gone.
    fs::remove_file(destination.join("b.txt")).unwrap();

    let report = run(
        &mut operations,
        &Request::Undo,
        &Decisions::new(Vec::new()),
        &cancellable,
    );

    assert_eq!(report.items[0].outcome, ItemOutcome::Done);
    assert_eq!(
        report.items[1].outcome,
        ItemOutcome::Refused("the source is missing".to_string())
    );
    assert!(a.exists());
    assert!(!b.exists());
}

#[test]
fn undo_with_nothing_on_the_stack_is_an_error() {
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();
    let plan = operations.plan(&Request::Undo);
    assert!(plan.steps().is_empty());
    if std::env::var("XDG_DATA_HOME").is_ok() {
        // A plan for Undo calls user_data_dir() only through trash lookups;
        // with an empty stack it does not touch it.
    }
    let error = operations
        .execute(&plan, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap_err();
    assert_eq!(error, ExecuteError::NothingToUndo);
}

#[test]
fn the_undo_stack_keeps_at_most_fifty_records() {
    let scratch = Scratch::new("undo-cap");
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();

    for index in 0..55 {
        run(
            &mut operations,
            &Request::NewFolder {
                scope: Scope::User,
                name: format!("folder-{index}"),
                in_folder: scratch.path.clone(),
            },
            &Decisions::new(Vec::new()),
            &cancellable,
        );
    }

    assert_eq!(operations.undo_len(), MAX_UNDO);
    assert!(scratch.join("folder-54").is_dir());
}

#[test]
fn new_folder_and_rename_run_and_the_plan_lists_them() {
    let scratch = Scratch::new("new-folder-rename");
    let mut operations = FileOperations::new();
    let cancellable = gio::Cancellable::new();

    let plan = operations.plan(&Request::NewFolder {
        scope: Scope::User,
        name: "made".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert!(plan.lines()[1].contains("new-folder"));
    run(
        &mut operations,
        &Request::NewFolder {
            scope: Scope::User,
            name: "made".to_string(),
            in_folder: scratch.path.clone(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(scratch.join("made").is_dir());

    let original = scratch.join("made/inner.txt");
    write(&original, "inner");
    run(
        &mut operations,
        &Request::Rename {
            scope: Scope::User,
            path: original.clone(),
            name: "other.txt".to_string(),
        },
        &Decisions::new(Vec::new()),
        &cancellable,
    );
    assert!(scratch.join("made/other.txt").exists());
}

// ---------------------------------------------------------------------------
// Trash info parsing
// ---------------------------------------------------------------------------

#[test]
fn trashinfo_percent_decodes_the_path() {
    let parsed = trashinfo::parse(
        "[Trash Info]\nPath=/tmp/a%20b/%C3%A9/%2Bplus+plus\nDeletionDate=2026-11-02T13:04:05\n",
    )
    .unwrap();
    assert_eq!(
        parsed.path,
        PathBuf::from("/tmp/a b/é/+plus+plus"),
        "percent escapes decode; a literal plus stays a plus"
    );
}

#[test]
fn trashinfo_percent_decodes_non_utf8_paths() {
    let parsed =
        trashinfo::parse("[Trash Info]\nPath=/tmp/caf%E9\nDeletionDate=2026-11-02T13:04:05\n")
            .unwrap();
    assert_eq!(
        parsed.path.as_os_str().as_encoded_bytes(),
        b"/tmp/caf\xe9".as_slice()
    );
}

#[test]
fn trashinfo_reads_both_date_forms() {
    let proper =
        trashinfo::parse("[Trash Info]\nPath=/tmp/a\nDeletionDate=2026-11-02T13:04:05\n").unwrap();
    assert_eq!(proper.deletion_date.year, 2026);
    assert_eq!(proper.deletion_date.month, 11);
    assert_eq!(proper.deletion_date.day, 2);
    assert_eq!(proper.deletion_date.hour, 13);
    assert_eq!(proper.deletion_date.minute, 4);
    assert_eq!(proper.deletion_date.second, 5);

    let example =
        trashinfo::parse("[Trash Info]\nPath=/tmp/a\nDeletionDate=20040831T22:32:08\n").unwrap();
    assert_eq!(example.deletion_date.year, 2004);
    assert_eq!(example.deletion_date.month, 8);
    assert_eq!(example.deletion_date.day, 31);

    let fractional =
        trashinfo::parse("[Trash Info]\nPath=/tmp/a\nDeletionDate=2026-11-02T13:04:05.123\n")
            .unwrap();
    assert_eq!(fractional.deletion_date.second, 5);
}

#[test]
fn trashinfo_reports_a_missing_key_header_or_bad_value() {
    assert_eq!(
        trashinfo::parse("[Trash Info]\nPath=/tmp/a\n").unwrap_err(),
        trashinfo::TrashInfoError::MissingKey("DeletionDate")
    );
    assert_eq!(
        trashinfo::parse("[Trash Info]\nDeletionDate=2026-11-02T13:04:05\n").unwrap_err(),
        trashinfo::TrashInfoError::MissingKey("Path")
    );
    assert_eq!(
        trashinfo::parse("Path=/tmp/a\nDeletionDate=2026-11-02T13:04:05\n").unwrap_err(),
        trashinfo::TrashInfoError::MissingHeader
    );
    assert_eq!(
        trashinfo::parse("[Trash Info]\nPath=/tmp/%zz\nDeletionDate=2026-11-02T13:04:05\n")
            .unwrap_err(),
        trashinfo::TrashInfoError::BadEscape(5)
    );
    assert!(matches!(
        trashinfo::parse("[Trash Info]\nPath=/tmp/a\nDeletionDate=yesterday\n").unwrap_err(),
        trashinfo::TrashInfoError::BadDate(_)
    ));
}

#[test]
fn empty_refuses_a_reused_uri_with_unknown_metadata() {
    let _scratch = Scratch::new("review-empty-reuse");
    let item = trash_item_with_fingerprint("trash:///a.txt", None, None, None);
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();

    // A later object at the same URI with the same (missing) metadata and no
    // object evidence, exactly like a backend that cannot prove sameness.
    backend.state.lock().unwrap().items[0] =
        trash_item_with_fingerprint("trash:///a.txt", None, None, None);

    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("Cannot verify this Trash item is unchanged".to_string())
    );
    assert!(backend.deleted().is_empty());
    assert_eq!(backend.remaining(), vec!["trash:///a.txt"]);
}

#[test]
fn restore_conflicts_with_same_original_need_separate_answers() {
    let scratch = Scratch::new("review-restore-same-original");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    write(&target, "existing");
    let first = trash_item("trash:///first.txt", Some(&target), None);
    let second = trash_item("trash:///second.txt", Some(&target), None);
    let (operations, _) = restored_operations(&scratch, vec![first.clone(), second.clone()]);

    let plan = operations.plan(&Request::Restore {
        items: vec![first.identity().clone(), second.identity().clone()],
    });
    assert_eq!(plan.decisions().count(), 2);
    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(first_key, Decision::Skip, false);
    assert!(
        decisions.next_unanswered().is_some(),
        "the second trashed version still needs its own answer"
    );
}

#[test]
fn same_uri_original_and_date_with_a_different_identity_is_refused() {
    let scratch = Scratch::new("empty-same-metadata-new-object");
    let original = scratch.join("same.txt");
    let item = trash_item_with_fingerprint(
        "trash:///same.txt",
        Some(&original),
        Some(1_700_000_000),
        Some("generation-1"),
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();

    backend.state.lock().unwrap().items[0] = trash_item_with_fingerprint(
        "trash:///same.txt",
        Some(&original),
        Some(1_700_000_000),
        Some("generation-2"),
    );
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the item changed in the trash".to_string())
    );
    assert!(backend.deleted().is_empty());
    assert_eq!(backend.remaining(), vec!["trash:///same.txt"]);
}

#[test]
fn a_lost_identity_is_refused_before_permanent_deletion() {
    let scratch = Scratch::new("empty-token-loss");
    let original = scratch.join("a.txt");
    let item = trash_item_with_fingerprint(
        "trash:///a.txt",
        Some(&original),
        None,
        Some("generation-1"),
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();

    backend.state.lock().unwrap().items[0] =
        trash_item_with_fingerprint("trash:///a.txt", Some(&original), None, None);
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("Cannot verify this Trash item is unchanged".to_string())
    );
    assert!(backend.deleted().is_empty());
}

#[test]
fn a_missing_display_date_with_a_stable_fingerprint_still_deletes() {
    let scratch = Scratch::new("empty-missing-date");
    let item = trash_item_with_fingerprint(
        "trash:///a.txt",
        Some(&scratch.join("a.txt")),
        None,
        Some("generation-1"),
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(backend.deleted(), vec!["trash:///a.txt"]);
}

#[test]
fn a_confirmation_for_an_older_plan_is_refused() {
    let scratch = Scratch::new("empty-stale-confirmation");
    let original = scratch.join("a.txt");
    let item = trash_item_with_fingerprint(
        "trash:///a.txt",
        Some(&original),
        Some(1_700_000_000),
        Some("generation-1"),
    );
    let backend = TestTrash::with(vec![item]);
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let old_plan = operations.plan(&Request::EmptyTrash);
    let old_confirmation = EmptyConfirmation::for_plan(&old_plan).unwrap();

    backend.state.lock().unwrap().items[0] = trash_item_with_fingerprint(
        "trash:///a.txt",
        Some(&original),
        Some(1_700_000_000),
        Some("generation-2"),
    );
    let new_plan = operations.plan(&Request::EmptyTrash);
    assert_eq!(
        operations
            .execute_empty(
                &new_plan,
                old_confirmation,
                &gio::Cancellable::new(),
                |_| {}
            )
            .unwrap_err(),
        ExecuteError::ConfirmationMismatch
    );
    assert!(backend.deleted().is_empty());
}

#[test]
fn restore_same_original_skip_then_keep_both_without_apply_all() {
    let scratch = Scratch::new("restore-same-original-skip-keep");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    write(&target, "existing");
    let first = trash_item("trash:///first.txt", Some(&target), None);
    let second = trash_item("trash:///second.txt", Some(&target), None);
    let (mut operations, backend) =
        restored_operations(&scratch, vec![first.clone(), second.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![first.identity().clone(), second.identity().clone()],
    });
    assert_eq!(plan.decisions().count(), 2);

    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    assert_eq!(first_key, ItemKey::Trash(first.uri().clone()));
    decisions.answer(first_key, Decision::Skip, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    assert_eq!(second_key, ItemKey::Trash(second.uri().clone()));
    decisions.answer(second_key, Decision::KeepBoth, false);

    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(read(&target), "existing");
    assert_eq!(read(&folder.join("same (copy).txt")), "restored second.txt");
    assert_eq!(backend.remaining(), vec!["trash:///first.txt"]);
}

#[test]
fn restore_earlier_skip_then_apply_all_keep_both_preserves_it() {
    let scratch = Scratch::new("restore-same-original-apply-all");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    write(&target, "existing");
    let first = trash_item("trash:///first.txt", Some(&target), None);
    let second = trash_item("trash:///second.txt", Some(&target), None);
    let (mut operations, backend) =
        restored_operations(&scratch, vec![first.clone(), second.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![first.identity().clone(), second.identity().clone()],
    });

    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(first_key, Decision::Skip, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(second_key, Decision::KeepBoth, true);

    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(report.items[0].outcome, ItemOutcome::Skipped);
    assert_eq!(read(&folder.join("same (copy).txt")), "restored second.txt");
    assert_eq!(backend.remaining(), vec!["trash:///first.txt"]);
}

#[test]
fn restore_distinct_replace_answers_apply_to_their_own_items() {
    let scratch = Scratch::new("restore-distinct-replace");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let first_target = folder.join("first.txt");
    let second_target = folder.join("second.txt");
    write(&first_target, "old first");
    write(&second_target, "old second");
    let first = trash_item("trash:///first.txt", Some(&first_target), None);
    let second = trash_item("trash:///second.txt", Some(&second_target), None);
    let (operations, _) = restored_operations(&scratch, vec![first.clone(), second.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![first.identity().clone(), second.identity().clone()],
    });

    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(first_key.clone(), Decision::Replace, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(second_key.clone(), Decision::Skip, false);

    assert_eq!(first_key, ItemKey::Trash(first.identity().uri().clone()));
    assert_eq!(second_key, ItemKey::Trash(second.identity().uri().clone()));
    assert_eq!(decisions.decision_for(&first_key), Some(Decision::Replace));
    assert_eq!(decisions.decision_for(&second_key), Some(Decision::Skip));
    assert_eq!(plan.steps()[0].target, Some(first_target));
    assert_eq!(plan.steps()[1].target, Some(second_target));
    // The executor's Replace path trashes the existing target through GIO,
    // which is exercised in tests/trash_browser.rs under scratch XDG data.
}

#[test]
fn restore_same_original_cancel_after_skip_runs_nothing() {
    let scratch = Scratch::new("restore-same-original-cancel");
    let folder = scratch.join("folder");
    fs::create_dir_all(&folder).unwrap();
    let target = folder.join("same.txt");
    write(&target, "existing");
    let first = trash_item("trash:///first.txt", Some(&target), None);
    let second = trash_item("trash:///second.txt", Some(&target), None);
    let (mut operations, backend) =
        restored_operations(&scratch, vec![first.clone(), second.clone()]);
    let plan = operations.plan(&Request::Restore {
        items: vec![first.identity().clone(), second.identity().clone()],
    });

    let mut decisions = Decisions::from_plan(&plan);
    let first_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(first_key, Decision::Skip, false);
    let second_key = decisions.next_unanswered().unwrap().clone();
    decisions.answer(second_key, Decision::Cancel, false);

    let report = operations
        .execute(&plan, &decisions, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(read(&target), "existing");
    assert_eq!(
        backend.remaining(),
        vec!["trash:///first.txt", "trash:///second.txt"]
    );
}

#[test]
fn trashinfo_keeps_the_first_occurrence_of_a_key() {
    let parsed = trashinfo::parse(
        "[Trash Info]\nPath=/tmp/first\nPath=/tmp/second\nDeletionDate=2026-11-02T13:04:05\nDeletionDate=2027-01-01T00:00:00\n",
    )
    .unwrap();
    assert_eq!(parsed.path, PathBuf::from("/tmp/first"));
    assert_eq!(parsed.deletion_date.year, 2026);
}

#[test]
fn cancellation_during_the_last_empty_item_is_not_a_failure() {
    let item = trash_item("trash:///cancelled.txt", None, None);
    let backend = TestTrash::with(vec![item]);
    backend.state.lock().unwrap().cancel_during_delete = true;
    let mut operations = FileOperations::with_trash_backend(Box::new(backend.clone()));
    let plan = operations.plan(&Request::EmptyTrash);
    let confirmation = EmptyConfirmation::for_plan(&plan).unwrap();
    let report = operations
        .execute_empty(&plan, confirmation, &gio::Cancellable::new(), |_| {})
        .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items[0].outcome, ItemOutcome::Cancelled);
    assert!(backend.deleted().is_empty());
    assert_eq!(backend.remaining(), vec!["trash:///cancelled.txt"]);
}

#[test]
fn every_native_operation_keeps_both_sides_scopes_and_undo_remembers_them() {
    let scratch = Scratch::new("scopes");
    let source = scratch.join("a.txt");
    write(&source, "a");
    let folder = scratch.join("dst");
    fs::create_dir(&folder).unwrap();

    let operations = FileOperations::new();
    let user = Scope::User;
    let admin = Scope::Admin;
    let requests = [
        Request::Trash {
            paths: vec![source.clone()],
            scope: admin,
        },
        Request::Copy {
            paths: vec![source.clone()],
            to: folder.clone(),
            scope: admin,
            target_scope: user,
        },
        Request::Move {
            paths: vec![source.clone()],
            to: folder.clone(),
            scope: user,
            target_scope: admin,
        },
        Request::Rename {
            path: source.clone(),
            name: "b.txt".to_string(),
            scope: admin,
        },
        Request::NewFolder {
            name: "new".to_string(),
            in_folder: folder.clone(),
            scope: admin,
        },
        Request::DeletePermanent {
            paths: vec![source.clone()],
            scope: admin,
        },
    ];
    for request in &requests {
        let plan = operations.plan(request);
        assert_eq!(plan.scope(), request.scope());
        assert_eq!(plan.target_scope(), request.target_scope());
        if request.scope() == admin || request.target_scope() == admin {
            assert!(plan.is_admin());
        }
        assert_eq!(
            crate::admin::scoped_uri(&source, request.scope()),
            Some(
                format!(
                    "{}{}",
                    if request.scope() == admin {
                        "admin://"
                    } else {
                        "file://"
                    },
                    "/tmp/"
                ) + crate::admin::scoped_uri(&source, request.scope())
                    .unwrap()
                    .split("/tmp/")
                    .nth(1)
                    .unwrap()
            )
        );
    }

    // The undo stack records the scope of the operation that produced it.
    let mut operations = FileOperations::new();
    operations.push_undo(UndoRecord {
        kind: RecordKind::Rename {
            from: scratch.join("old.txt"),
            to: scratch.join("new.txt"),
        },
        scope: admin,
        target_scope: user,
    });
    let undo = operations.plan(&Request::Undo);
    assert_eq!(undo.scope(), admin);
    assert_eq!(undo.target_scope(), user);
}

/// An administrator location has no trash (GVfs's admin backend reports
/// "Operation not supported"). Undoing an administrator New Folder deletes the
/// still-empty folder; undoing a copy into an administrator folder is refused
/// with a reason instead of deleting what may have been edited since.
#[test]
fn administrator_undo_never_plans_a_trash() {
    let scratch = Scratch::new("admin-undo");
    let folder = scratch.join("New Folder");
    fs::create_dir(&folder).expect("new folder");
    let copy = scratch.join("copy.txt");
    fs::write(&copy, "x").expect("copy");

    let undo_of = |kind: RecordKind, scope: Scope, target_scope: Scope| {
        let mut operations = FileOperations::new();
        operations.push_undo(UndoRecord {
            kind,
            scope,
            target_scope,
        });
        operations.plan(&Request::Undo)
    };
    let new_folder = || RecordKind::NewFolder {
        path: folder.clone(),
    };
    let copied = || RecordKind::Copy {
        copies: vec![copy.clone()],
        replaced: Vec::new(),
    };

    let admin = undo_of(new_folder(), Scope::Admin, Scope::Admin);
    assert_eq!(admin.steps()[0].action, Action::PermanentDelete);
    // Without a GVfs admin mount (as in this test) the emptiness check can't
    // read the folder and refuses it as not empty: the safe default.
    assert_eq!(admin.steps()[0].conflict, Some(Conflict::NotEmpty));
    let user = undo_of(new_folder(), Scope::User, Scope::User);
    assert_eq!(user.steps()[0].action, Action::Trash);

    let admin = undo_of(copied(), Scope::User, Scope::Admin);
    assert_eq!(admin.steps()[0].conflict, Some(Conflict::NoAdminTrash));
    let user = undo_of(copied(), Scope::User, Scope::User);
    assert_eq!(user.steps()[0].conflict, None);
}
