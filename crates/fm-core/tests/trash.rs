// SPDX-License-Identifier: GPL-3.0-or-later
//! Trash-dependent file-operation tests.
//!
//! GIO refuses to trash anything on `/tmp` ("Trashing on system internal
//! mounts is not supported"), and a test must never touch Kyle's real
//! `~/.local/share/Trash`. Each parent test therefore re-runs this test binary
//! as a child with `XDG_DATA_HOME` pointed inside
//! `~/.cache/gimbal-tests/p2f-<pid>-<n>/data`, so the child's scratch files and
//! its trash are both on the home filesystem. The parent checks the outcome,
//! compares Kyle's real trash before and after, and removes the whole scratch
//! folder.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use fm_core::fileops::{Conflict, Decision, Decisions, FileOperations, ItemOutcome, Request};

const CHILD_ENV: &str = "GIMBAL_P2F_CHILD";
const PRINT_ENV: &str = "GIMBAL_P2F_PRINT";
const DEST_ENV: &str = "GIMBAL_P2F_DEST";

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Harness {
    base: PathBuf,
    tmp_dest: PathBuf,
}

impl Harness {
    fn new(test: &str) -> Self {
        let number = COUNTER.fetch_add(1, Ordering::Relaxed);
        let cache = cache_root();
        let base = cache.join(format!("p2f-{}-{number}-{test}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("data")).unwrap();
        fs::create_dir_all(base.join("scratch")).unwrap();

        let tmp_dest = PathBuf::from("/tmp").join(format!(
            "gimbal-p2f-{}-{number}-{test}-dest",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&tmp_dest);

        Self { base, tmp_dest }
    }

    fn finish(&self) {
        let _ = fs::remove_dir_all(&self.base);
        let _ = fs::remove_dir_all(&self.tmp_dest);
        // Only removes it when the other tests' folders are gone too.
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
        .env(DEST_ENV, &harness.tmp_dest)
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

    if std::env::var(PRINT_ENV).ok().as_deref() == Some("1") {
        print!("{stdout}{stderr}");
    }

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
fn trash_and_its_undo_use_the_scratch_trash() {
    parent("trash-undo", "child_trash_and_undo");
}

#[test]
fn replace_trashes_the_old_target_and_undo_restores_it() {
    parent("replace", "child_replace_copy_and_undo");
}

#[test]
fn every_decision_runs_together_in_one_mix() {
    parent("mixed", "child_mixed_decisions");
}

#[test]
fn new_folder_undo_trashes_only_an_empty_folder() {
    parent("new-folder", "child_new_folder_undo");
}

#[test]
fn a_cross_filesystem_folder_move_trashes_its_source() {
    parent("cross-fs", "child_cross_filesystem_move");
}

#[test]
fn an_untrashable_source_keeps_both_copies() {
    parent("untrashable", "child_untrashable_source");
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

fn data_trash() -> PathBuf {
    PathBuf::from(std::env::var_os("XDG_DATA_HOME").expect("XDG_DATA_HOME is set")).join("Trash")
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn trash_count() -> usize {
    names(&data_trash().join("files")).len()
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap();
}

fn print_enabled() -> bool {
    std::env::var(PRINT_ENV).ok().as_deref() == Some("1")
}

fn print_lines(title: &str, lines: &[String]) {
    if print_enabled() {
        println!("== {title} ==");
        for line in lines {
            println!("{line}");
        }
    }
}

fn execute(
    operations: &mut FileOperations,
    plan: &fm_core::fileops::Plan,
    decisions: &Decisions,
) -> fm_core::fileops::Report {
    let cancellable = gio::Cancellable::new();
    operations
        .execute(plan, decisions, &cancellable, |_| {})
        .unwrap()
}

#[test]
#[ignore]
fn child_trash_and_undo() {
    let scratch = scratch();
    let one = scratch.join("one.txt");
    let folder = scratch.join("folder");
    let two = folder.join("two.txt");
    fs::create_dir(&folder).unwrap();
    write(&one, "one");
    write(&two, "two");

    let mut operations = FileOperations::new();
    let request = Request::Trash {
        scope: fm_core::admin::Scope::User,
        paths: vec![one.clone(), folder.clone()],
    };
    let plan = operations.plan(&request);
    assert!(plan.steps().iter().all(|step| step.conflict.is_none()));
    print_lines("trash dry run", &plan.lines());

    let report = execute(&mut operations, &plan, &Decisions::new(Vec::new()));
    print_lines("trash report", &report.lines());
    assert!(report.all_succeeded());
    assert!(!one.exists());
    assert!(!folder.exists());
    assert_eq!(trash_count(), 2, "both items are in the scratch trash");
    assert_eq!(operations.undo_len(), 1);

    let undo = operations.plan(&Request::Undo);
    print_lines("undo dry run", &undo.lines());
    let report = execute(&mut operations, &undo, &Decisions::new(Vec::new()));
    print_lines("undo report", &report.lines());
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&one).unwrap(), "one");
    assert_eq!(fs::read_to_string(&two).unwrap(), "two");
    assert_eq!(trash_count(), 0);
    assert_eq!(names(&data_trash().join("info")).len(), 0);
    assert_eq!(operations.undo_len(), 0);
}

#[test]
#[ignore]
fn child_replace_copy_and_undo() {
    let scratch = scratch();
    fs::create_dir_all(scratch.join("src")).unwrap();
    fs::create_dir_all(scratch.join("dst")).unwrap();
    let source = scratch.join("src/a.txt");
    let target = scratch.join("dst/a.txt");
    write(&source, "new");
    write(&target, "old");

    let mut operations = FileOperations::new();
    let request = Request::Copy {
        scope: fm_core::admin::Scope::User,
        target_scope: fm_core::admin::Scope::User,
        paths: vec![source.clone()],
        to: scratch.join("dst"),
    };
    let plan = operations.plan(&request);
    assert_eq!(plan.steps()[0].conflict, Some(Conflict::TargetExists));
    print_lines("copy dry run (conflict)", &plan.lines());

    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(&source, Decision::Replace, false);
    let report = execute(&mut operations, &plan, &decisions);
    print_lines("copy report", &report.lines());
    assert!(report.all_succeeded());
    assert_eq!(fs::read_to_string(&source).unwrap(), "new");
    assert_eq!(fs::read_to_string(&target).unwrap(), "new");
    assert_eq!(trash_count(), 1, "the old target went to the trash");

    let undo = operations.plan(&Request::Undo);
    print_lines("copy undo dry run", &undo.lines());
    let report = execute(&mut operations, &undo, &Decisions::new(Vec::new()));
    print_lines("copy undo report", &report.lines());
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert_eq!(fs::read_to_string(&target).unwrap(), "old");
    assert_eq!(fs::read_to_string(&source).unwrap(), "new");
    assert_eq!(trash_count(), 1, "the copy itself was trashed by the undo");
    assert_eq!(operations.undo_len(), 0);
}

#[test]
#[ignore]
fn child_mixed_decisions() {
    let scratch = scratch();
    let source = scratch.join("src");
    let destination = scratch.join("dst");
    fs::create_dir(&source).unwrap();
    fs::create_dir(&destination).unwrap();

    let mut paths = Vec::new();
    for name in ["a.txt", "b.txt", "c.txt"] {
        let path = source.join(name);
        write(&path, &format!("new-{name}"));
        write(&destination.join(name), &format!("old-{name}"));
        paths.push(path);
    }

    let mut operations = FileOperations::new();
    let plan = operations.plan(&Request::Copy {
        scope: fm_core::admin::Scope::User,
        target_scope: fm_core::admin::Scope::User,
        paths: paths.clone(),
        to: destination.clone(),
    });
    assert_eq!(plan.decisions().count(), 3);
    print_lines("mixed dry run", &plan.lines());

    let mut decisions = Decisions::from_plan(&plan);
    decisions.answer(&paths[0], Decision::Skip, false);
    decisions.answer(&paths[1], Decision::KeepBoth, false);
    decisions.answer(&paths[2], Decision::Replace, false);

    let report = execute(&mut operations, &plan, &decisions);
    print_lines("mixed report", &report.lines());
    assert_eq!(report.items[0].outcome, ItemOutcome::Skipped);
    assert_eq!(report.items[1].outcome, ItemOutcome::Done);
    assert_eq!(report.items[2].outcome, ItemOutcome::Done);

    assert_eq!(
        fs::read_to_string(destination.join("a.txt")).unwrap(),
        "old-a.txt"
    );
    assert_eq!(
        fs::read_to_string(destination.join("b.txt")).unwrap(),
        "old-b.txt"
    );
    assert_eq!(
        fs::read_to_string(destination.join("b (copy).txt")).unwrap(),
        "new-b.txt"
    );
    assert_eq!(
        fs::read_to_string(destination.join("c.txt")).unwrap(),
        "new-c.txt"
    );
    assert_eq!(trash_count(), 1, "only Replace trashed a target");
}

#[test]
#[ignore]
fn child_new_folder_undo() {
    let scratch = scratch();
    let empty = scratch.join("empty");
    let full = scratch.join("full");

    let mut operations = FileOperations::new();
    let request = Request::NewFolder {
        scope: fm_core::admin::Scope::User,
        name: "empty".to_string(),
        in_folder: scratch.clone(),
    };
    let plan = operations.plan(&request);
    let report = execute(&mut operations, &plan, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded());
    assert!(empty.is_dir());

    let request = Request::NewFolder {
        scope: fm_core::admin::Scope::User,
        name: "full".to_string(),
        in_folder: scratch.clone(),
    };
    let plan = operations.plan(&request);
    let report = execute(&mut operations, &plan, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded());
    write(&full.join("keep.txt"), "keep");

    // The newest record is `full`, which is not empty: refuse it.
    let undo = operations.plan(&Request::Undo);
    print_lines("new folder undo (not empty)", &undo.lines());
    let report = execute(&mut operations, &undo, &Decisions::new(Vec::new()));
    print_lines("new folder undo report", &report.lines());
    assert_eq!(
        report.items[0].outcome,
        ItemOutcome::Refused("the folder is not empty".to_string())
    );
    assert!(full.is_dir());
    assert_eq!(fs::read_to_string(full.join("keep.txt")).unwrap(), "keep");
    assert_eq!(trash_count(), 0);

    // The next record is the empty folder: trashing it is fine.
    let undo = operations.plan(&Request::Undo);
    let report = execute(&mut operations, &undo, &Decisions::new(Vec::new()));
    assert!(report.all_succeeded(), "{:?}", report.lines());
    assert!(!empty.exists());
    assert_eq!(trash_count(), 1);
}

#[test]
#[ignore]
fn child_cross_filesystem_move() {
    let base = child_setup();
    let scratch = base.join("scratch");
    let tree = scratch.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    write(&tree.join("a.txt"), "a");
    write(&tree.join("sub/b.txt"), "b");

    let destination =
        PathBuf::from(std::env::var_os(DEST_ENV).expect("the parent sets the /tmp destination"));
    assert!(
        destination
            .to_string_lossy()
            .starts_with("/tmp/gimbal-p2f-"),
        "the destination is a /tmp/gimbal-p2f-* folder"
    );
    fs::create_dir_all(&destination).unwrap();

    let mut operations = FileOperations::new();
    let plan = operations.plan(&Request::Move {
        scope: fm_core::admin::Scope::User,
        target_scope: fm_core::admin::Scope::User,
        paths: vec![tree.clone()],
        to: destination.clone(),
    });
    assert!(plan.steps().iter().all(|step| step.conflict.is_none()));
    print_lines("cross-filesystem move dry run", &plan.lines());

    let report = execute(&mut operations, &plan, &Decisions::new(Vec::new()));
    print_lines("cross-filesystem move report", &report.lines());
    assert!(report.all_succeeded(), "{:?}", report.lines());

    let moved = destination.join("tree");
    assert_eq!(fs::read_to_string(moved.join("a.txt")).unwrap(), "a");
    assert_eq!(fs::read_to_string(moved.join("sub/b.txt")).unwrap(), "b");
    assert!(!tree.exists(), "the source folder is gone");
    assert_eq!(
        trash_count(),
        1,
        "the source folder was trashed, not deleted"
    );
    assert_eq!(operations.undo_len(), 1);
}

#[test]
#[ignore = "runs only as a child of an_untrashable_source_keeps_both_copies"]
fn child_untrashable_source() {
    // A folder moved from /tmp (which GIO refuses to trash) to the home
    // filesystem: the copy succeeds, trashing the source fails, and neither
    // copy may be deleted (Claude's P2f review fix).
    let scratch = scratch();
    let source_root =
        PathBuf::from(std::env::var_os(DEST_ENV).expect("the parent sets the /tmp folder"));
    assert!(
        source_root
            .to_string_lossy()
            .starts_with("/tmp/gimbal-p2f-"),
        "the source is in a /tmp/gimbal-p2f-* folder"
    );
    let tree = source_root.join("tree");
    fs::create_dir_all(&tree).unwrap();
    write(&tree.join("a.txt"), "a");

    let mut operations = FileOperations::new();
    let plan = operations.plan(&Request::Move {
        scope: fm_core::admin::Scope::User,
        target_scope: fm_core::admin::Scope::User,
        paths: vec![tree.clone()],
        to: scratch.clone(),
    });
    let report = execute(&mut operations, &plan, &Decisions::new(Vec::new()));
    print_lines("untrashable source report", &report.lines());

    assert!(!report.all_succeeded());
    assert!(
        report
            .lines()
            .iter()
            .any(|line| line.contains("both copies remain")),
        "{:?}",
        report.lines()
    );
    assert_eq!(fs::read_to_string(tree.join("a.txt")).unwrap(), "a");
    assert_eq!(fs::read_to_string(scratch.join("tree/a.txt")).unwrap(), "a");
    assert_eq!(trash_count(), 0);
}
