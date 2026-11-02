// SPDX-License-Identifier: GPL-3.0-or-later
//! Unit tests for planning, decisions, names, reports and the trash-info
//! parser. Anything that trashes a file lives in `tests/trash.rs`, where a
//! child process points `XDG_DATA_HOME` at a scratch folder.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
            paths: vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/c")],
            to: PathBuf::from("/tmp/b"),
        }
    );
    assert_eq!(
        request_for(&check("file.rename", &[("path", "/tmp/a"), ("name", "b")]))
            .unwrap()
            .unwrap(),
        Request::Rename {
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
            name: "new".to_string(),
            in_folder: PathBuf::from("/tmp"),
        }
    );
    assert_eq!(
        request_for(&check("file.undo", &[])).unwrap().unwrap(),
        Request::Undo
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
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

fn conflicts() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/tmp/a"),
        PathBuf::from("/tmp/b"),
        PathBuf::from("/tmp/c"),
    ]
}

#[test]
fn three_conflicts_are_answered_one_by_one() {
    let mut decisions = Decisions::new(conflicts());
    assert_eq!(decisions.next_unanswered(), Some(Path::new("/tmp/a")));

    decisions.answer(Path::new("/tmp/a"), Decision::Skip, false);
    assert_eq!(decisions.next_unanswered(), Some(Path::new("/tmp/b")));

    decisions.answer(Path::new("/tmp/b"), Decision::KeepBoth, false);
    assert_eq!(decisions.next_unanswered(), Some(Path::new("/tmp/c")));

    decisions.answer(Path::new("/tmp/c"), Decision::Replace, false);
    assert_eq!(decisions.next_unanswered(), None);
    assert!(decisions.is_complete());
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/a")),
        Some(Decision::Skip)
    );
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/b")),
        Some(Decision::KeepBoth)
    );
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/c")),
        Some(Decision::Replace)
    );
}

#[test]
fn the_first_answer_can_apply_to_all() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(Path::new("/tmp/a"), Decision::KeepBoth, true);

    for item in conflicts() {
        assert_eq!(decisions.decision_for(&item), Some(Decision::KeepBoth));
    }
    assert_eq!(decisions.next_unanswered(), None);
    assert!(decisions.is_complete());
}

#[test]
fn apply_to_all_does_not_overwrite_an_earlier_answer() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(Path::new("/tmp/a"), Decision::Skip, false);
    decisions.answer(Path::new("/tmp/b"), Decision::Replace, true);

    assert_eq!(
        decisions.decision_for(Path::new("/tmp/a")),
        Some(Decision::Skip)
    );
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/b")),
        Some(Decision::Replace)
    );
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/c")),
        Some(Decision::Replace)
    );
}

#[test]
fn cancel_ends_the_questions_at_once() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(Path::new("/tmp/a"), Decision::Skip, false);
    decisions.answer(Path::new("/tmp/b"), Decision::Cancel, false);

    assert!(decisions.is_cancelled());
    assert!(decisions.is_complete());
    assert_eq!(decisions.next_unanswered(), None);
    assert_eq!(
        decisions.decision_for(Path::new("/tmp/c")),
        None,
        "cancel ends the questions without answering the rest"
    );
}

#[test]
fn decisions_cancel_with_apply_to_all_marks_the_run_cancelled() {
    let mut decisions = Decisions::new(conflicts());
    decisions.answer(Path::new("/tmp/a"), Decision::Cancel, true);
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
        path: first.clone(),
        name: "b.txt".to_string(),
    });
    assert_eq!(taken.steps()[0].conflict, Some(Conflict::NameTaken));

    let invalid = operations.plan(&Request::Rename {
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
        name: "taken".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert_eq!(taken.steps()[0].conflict, Some(Conflict::NameTaken));

    let invalid = operations.plan(&Request::NewFolder {
        name: "..".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert_eq!(invalid.steps()[0].conflict, Some(Conflict::InvalidName));
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
        paths: vec![a.clone(), b.clone()],
        to: destination.clone(),
    });
    assert_eq!(plan.decisions().count(), 2);

    let mut decisions = Decisions::new(plan.decisions().filter_map(|step| step.source.clone()));
    decisions.answer(&a, Decision::Skip, false);
    decisions.answer(&b, Decision::KeepBoth, false);
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
        paths: vec![source.clone()],
        to: destination.clone(),
    });
    let error = operations
        .execute(&plan, &Decisions::new(Vec::new()), &cancellable, |_| {})
        .unwrap_err();

    assert_eq!(error, ExecuteError::UnansweredConflicts(vec![source]));
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
        paths: vec![source.clone()],
        to: destination.clone(),
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(&source, Decision::Cancel, false);

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
        paths: vec![a.clone(), b.clone()],
        to: destination.clone(),
    });
    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(&a, Decision::Skip, false);
    decisions.answer(&b, Decision::Cancel, false);

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
        name: "made".to_string(),
        in_folder: scratch.path.clone(),
    });
    assert!(plan.lines()[1].contains("new-folder"));
    run(
        &mut operations,
        &Request::NewFolder {
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
fn trashinfo_keeps_the_first_occurrence_of_a_key() {
    let parsed = trashinfo::parse(
        "[Trash Info]\nPath=/tmp/first\nPath=/tmp/second\nDeletionDate=2026-11-02T13:04:05\nDeletionDate=2027-01-01T00:00:00\n",
    )
    .unwrap();
    assert_eq!(parsed.path, PathBuf::from("/tmp/first"));
    assert_eq!(parsed.deletion_date.year, 2026);
}
