// SPDX-License-Identifier: GPL-3.0-or-later
//! Tests for the preview contract. Real files live only in
//! `/tmp/gimbal-preview-*` scratch folders, which each test creates and removes.

use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use super::*;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/tmp").join(format!(
            "gimbal-preview-{tag}-{}-{nanos}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.path.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn path(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn fake_encoding() -> TextEncoding {
    TextEncoding {
        bom: false,
        line_ending: Some(LineEnding::Lf),
    }
}

fn fake_revision() -> FileRevision {
    FileRevision {
        device: 7,
        inode: 11,
        size: 4,
        modified_sec: 1,
        modified_nsec: 0,
        changed_sec: 1,
        changed_nsec: 0,
        etag: Some("test-etag".to_string()),
    }
}

fn fake_document(path: &str, text: &str) -> PreviewDocument {
    PreviewDocument {
        path: PathBuf::from(path),
        name: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        size: text.len() as u64,
        revision: fake_revision(),
        access: WriteAccess {
            file_writable: true,
            parent_writable: true,
            reason: None,
        },
        content: PreviewContent::Text(TextDocument {
            text: text.to_string(),
            encoding: fake_encoding(),
        }),
    }
}

fn loaded_document(path: &Path) -> PreviewDocument {
    match load(path) {
        Loaded::Document(document) => document,
        other => panic!("expected a document, got {other:?}"),
    }
}

fn text_document(path: &Path) -> TextDocument {
    loaded_document(path).text().unwrap().clone()
}

fn save_original(document: &PreviewDocument, text: &str) -> Result<SaveSuccess, SaveError> {
    save(&SaveRequest {
        path: document.path.clone(),
        text: text.to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::Original {
            expected: SaveExpectation {
                revision: document.revision.clone(),
            },
        },
    })
}

// ---------------------------------------------------------------------------
// Selection and state
// ---------------------------------------------------------------------------

#[test]
fn selection_counts_none_one_and_many() {
    let mut names = BTreeSet::new();
    assert_eq!(
        Selection::from_names(Path::new("/f"), &names),
        Selection::None
    );
    names.insert(OsString::from("a.txt"));
    assert_eq!(
        Selection::from_names(Path::new("/f"), &names),
        Selection::file("/f/a.txt")
    );
    names.insert(OsString::from("b.txt"));
    assert_eq!(
        Selection::from_names(Path::new("/f"), &names),
        Selection::Multiple { count: 2 }
    );
}

#[test]
fn a_folder_is_a_folder_state_with_no_text_editor() {
    let scratch = Scratch::new("folder");
    let folder = scratch.path("folder");
    fs::create_dir(&folder).unwrap();
    match load(&folder) {
        Loaded::Folder(FolderInfo::Folder { path, name }) => {
            assert_eq!(path, folder);
            assert_eq!(name, "folder");
        }
        other => panic!("expected a folder, got {other:?}"),
    }
}

#[test]
fn stale_load_generations_are_ignored_even_when_a_load_races() {
    let mut session = PreviewSession::new();
    let first = session.select(Selection::file("/a.txt")).unwrap();
    let second = session.select(Selection::file("/b.txt")).unwrap();
    assert!(!session.load_finished(&first, Loaded::Document(fake_document("/a.txt", "A"))));
    assert_eq!(
        session.state(),
        &ViewState::Loading {
            path: PathBuf::from("/b.txt")
        }
    );
    assert!(session.load_finished(&second, Loaded::Document(fake_document("/b.txt", "B"))));
    assert!(
        matches!(session.state(), ViewState::Document(document) if document.path == Path::new("/b.txt"))
    );
}

#[test]
fn dirty_text_stays_pinned_across_selection_navigation_and_trash() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    assert!(session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A"))));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    assert!(session.is_dirty());

    for selection in [
        Selection::file("/b.txt"),
        Selection::Multiple { count: 3 },
        Selection::Trash,
    ] {
        assert_eq!(session.select(selection), None);
        assert!(session.is_dirty());
        assert_eq!(
            session.pinned().unwrap().document.path,
            PathBuf::from("/a.txt")
        );
        assert!(matches!(
            session.state(),
            ViewState::Document(document) if document.path == Path::new("/a.txt")
        ));
    }
}

#[test]
fn images_media_and_folders_are_not_editable() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.png")).unwrap();
    let mut document = fake_document("/a.png", "");
    document.content = PreviewContent::Image(ImageDocument {
        mime: "image/png".to_string(),
    });
    session.load_finished(&ticket, Loaded::Document(document));
    assert!(session.begin_edit(0).is_err());
    assert!(!session.is_editing());
}

#[test]
fn a_same_selection_refresh_keeps_a_clean_editing_session() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    assert!(session.is_editing());
    assert_eq!(session.select(Selection::file("/a.txt")), None);
    assert!(session.is_editing());
}

#[test]
fn save_result_revision_only_applies_to_the_starting_buffer() {
    let document = fake_document("/a.txt", "A");
    let pinned = PinnedDocument::new(document, 1, 4, false);
    assert!(pinned.save_applies_to(4));
    assert!(!pinned.save_applies_to(5));
}

#[test]
fn reconcile_after_save_returns_to_a_trash_page() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let save = session.begin_save_original().unwrap();
    session.select(Selection::Trash);
    assert_eq!(
        session.save_finished(&save, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert_eq!(session.reconcile(), None);
    assert_eq!(session.state(), &ViewState::Trash);
}

#[test]
fn a_failed_save_keeps_the_dirty_pin_and_buffer_target() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(2);
    let snapshot = session.begin_save_original().unwrap();
    assert_eq!(snapshot.path, PathBuf::from("/a.txt"));
    assert!(session.save_failed(&snapshot, "File changed on disk"));
    assert!(session.is_dirty());
    assert!(session.is_editing());
    assert!(!session.saving());
    assert_eq!(session.warning(), Some("File changed on disk"));
}

#[test]
fn save_finished_and_discard_clear_the_dirty_pin() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let save = session.begin_save_original().unwrap();
    assert_eq!(
        session.save_finished(&save, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert!(!session.is_editing());
    assert!(session.warning().is_none());

    let ticket = session.select(Selection::file("/b.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/b.txt", "B")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(2);
    let next = session.discard().unwrap();
    assert_eq!(next.path, PathBuf::from("/b.txt"));
    assert!(!session.is_dirty());
}

#[test]
fn external_change_warning_keeps_dirty_text_and_reports_clean_reloads() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    assert!(!session.note_external_change());
    assert_eq!(session.warning(), Some("File changed on disk"));
    assert!(session.is_dirty());

    let mut clean = PreviewSession::new();
    let ticket = clean.select(Selection::file("/a.txt")).unwrap();
    clean.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    assert!(clean.note_external_change());
    assert!(clean.warning().is_none());
}

#[test]
fn a_clean_external_reload_drops_the_old_pin_and_uses_the_new_revision() {
    let scratch = Scratch::new("clean-reload");
    let path = scratch.write("note.txt", b"old contents\n");
    let first = loaded_document(&path);
    let first_revision = first.revision.clone();
    assert_eq!(first.text().unwrap().text, "old contents\n");

    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file(&path)).unwrap();
    session.load_finished(&ticket, Loaded::Document(first));
    session.begin_edit(0).unwrap();
    assert!(session.is_editing());
    assert!(!session.is_dirty());

    fs::write(&path, b"new contents\n").unwrap();
    assert!(session.note_external_change());
    let ticket = session.force_load(path.clone());
    assert!(!session.is_editing(), "the old clean pin must be gone");
    let second = loaded_document(&path);
    let second_revision = second.revision.clone();
    assert!(session.load_finished(&ticket, Loaded::Document(second)));
    assert!(!session.is_editing());
    assert_eq!(
        session.current_document().unwrap().text().unwrap().text,
        "new contents\n"
    );

    session.begin_edit(0).unwrap();
    let pinned = session.pinned().unwrap();
    assert_eq!(pinned.document.text().unwrap().text, "new contents\n");
    assert!(pinned.document.revision.same_version(&second_revision));
    assert!(!pinned.document.revision.same_version(&first_revision));
    session.mark_dirty(1);
    let save = session.begin_save_original().unwrap();
    assert!(save.token.editor.revision.same_version(&second_revision));
    assert!(!save.token.editor.revision.same_version(&first_revision));
}

#[test]
fn failed_and_unsupported_reloads_leave_no_editor() {
    let scratch = Scratch::new("failed-reload");
    let path = scratch.write("note.txt", b"old\n");
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file(&path)).unwrap();
    session.load_finished(&ticket, Loaded::Document(loaded_document(&path)));
    session.begin_edit(0).unwrap();

    let ticket = session.force_load(path.clone());
    assert!(session.load_failed(&ticket, LoadFailure::Missing));
    assert!(!session.is_editing());
    assert!(session.begin_edit(0).is_err());

    let ticket = session.force_load(path.clone());
    assert!(session.load_finished(
        &ticket,
        Loaded::Unsupported {
            path: path.clone(),
            name: "note.txt".to_string(),
            reason: UnsupportedReason::Binary,
            size: Some(4),
        }
    ));
    assert!(!session.is_editing());
    assert!(session.begin_edit(0).is_err());
}

#[test]
fn a_dirty_external_change_keeps_the_pinned_buffer() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(3);
    assert!(!session.note_external_change());
    assert!(session.is_editing());
    assert!(session.is_dirty());
    assert_eq!(session.pinned().unwrap().buffer_revision, 3);
}

// ---------------------------------------------------------------------------
// Save As request binding
// ---------------------------------------------------------------------------

fn dirty_session(path: &str, revision: u64) -> PreviewSession {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file(path)).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document(path, "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(revision);
    session
}

#[test]
fn a_destination_reply_for_discarded_editor_a_cannot_start_b_through_a() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    assert_eq!(
        session
            .choose_save_as_path(request.id, PathBuf::from("/destination.txt"))
            .unwrap(),
        SaveAsRoute::NewPath
    );

    session.discard();
    let ticket = session.select(Selection::file("/b.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/b.txt", "B")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);

    assert!(
        session
            .save_as_destination(request.id, Path::new("/destination.txt"), None)
            .is_err(),
        "A's late destination reply must not name B's buffer"
    );
    assert!(!session.has_active_save());
    assert!(session.begin_save_original().is_ok());
}

#[test]
fn old_chooser_and_replace_replies_are_rejected_after_reload_or_replacement() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    session.reload_pinned().unwrap();
    assert!(
        session
            .choose_save_as_path(request.id, PathBuf::from("/dest.txt"))
            .is_err()
    );

    let mut clean = PreviewSession::new();
    let ticket = clean.select(Selection::file("/a.txt")).unwrap();
    clean.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    clean.begin_edit(0).unwrap();
    let request = clean.begin_save_as().unwrap();
    clean
        .choose_save_as_path(request.id, PathBuf::from("/dest.txt"))
        .unwrap();
    let revision = fake_revision();
    clean
        .save_as_destination(request.id, Path::new("/dest.txt"), Some(revision))
        .unwrap();
    let ticket = clean.select(Selection::file("/b.txt")).unwrap();
    clean.load_finished(&ticket, Loaded::Document(fake_document("/b.txt", "B")));
    assert!(
        clean.confirm_save_as(request.id).is_err(),
        "replacement ends the old clean editor's request"
    );
}

#[test]
fn editing_while_a_destination_check_is_pending_refuses_the_stale_reply() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    session
        .choose_save_as_path(request.id, PathBuf::from("/dest.txt"))
        .unwrap();
    session.mark_dirty(2);
    assert!(!session.save_as_pending());
    assert!(
        session
            .save_as_destination(request.id, Path::new("/dest.txt"), None)
            .is_err()
    );
    assert!(!session.has_active_save());
}

#[test]
fn duplicate_or_in_flight_save_as_requests_are_refused() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    assert!(session.begin_save_as().is_err());
    session.cancel_save_as(request.id);
    let second = session.begin_save_as().unwrap();
    assert_ne!(second.id, request.id);
    session.cancel_save_as(second.id);

    session.begin_save_original().unwrap();
    assert!(
        session.begin_save_as().is_err(),
        "a save in progress owns the panel"
    );
}

#[test]
fn saving_over_the_original_path_routes_through_the_original_checks() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    assert_eq!(
        session
            .choose_save_as_path(request.id, PathBuf::from("/a.txt"))
            .unwrap(),
        SaveAsRoute::Original
    );
    assert!(!session.save_as_pending());
    let save = session.begin_save_original().unwrap();
    assert!(!save.new_path);
    assert!(save.replace.is_none());
}

#[test]
fn invalidating_actions_on_close_drops_a_pending_save_as() {
    let mut session = dirty_session("/a.txt", 1);
    let request = session.begin_save_as().unwrap();
    let old_editor = request.token.editor.id;
    session.invalidate_actions();
    assert!(!session.save_as_pending());
    assert!(
        session
            .choose_save_as_path(request.id, PathBuf::from("/dest.txt"))
            .is_err()
    );
    assert!(!session.has_active_save());

    // Reopening the editor gives a new identity; A's id cannot authorize B.
    session.discard();
    let ticket = session.select(Selection::file("/b.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(fake_document("/b.txt", "B")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    assert!(
        session
            .choose_save_as_path(request.id, PathBuf::from("/dest.txt"))
            .is_err()
    );
    let reopened = session.begin_save_as().unwrap();
    assert_ne!(reopened.token.editor.id, old_editor);
}

// ---------------------------------------------------------------------------
// Close continuations
// ---------------------------------------------------------------------------

#[test]
fn close_waits_for_the_accepted_save_and_document() {
    let mut session = dirty_session("/a.txt", 1);
    let close = session.arm_close().unwrap();
    let save = session.begin_save_original().unwrap();
    session.bind_close_to_save(&save);
    assert!(!session.close_satisfied_by(&save));
    assert_eq!(
        session.close_request().unwrap().save,
        Some(save.id),
        "the continuation names the save job it accepted"
    );
    assert_eq!(
        session.save_finished(&save, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert!(session.close_satisfied_by(&save));
    let taken = session.take_close().unwrap();
    assert_eq!(taken.save, Some(save.id));
    assert_eq!(taken.token, close.token);
}

#[test]
fn a_failed_conflict_reload_cannot_close_a_later_edit_and_save() {
    let mut session = dirty_session("/a.txt", 1);
    session.arm_close().unwrap();
    let save = session.begin_save_original().unwrap();
    session.bind_close_to_save(&save);
    assert!(session.save_failed(&save, "File changed on disk"));
    assert!(!session.has_close(), "a failed save abandons the close");
    let reload = session.reload_pinned().unwrap();
    session.load_finished(&reload, Loaded::Document(fake_document("/a.txt", "A2")));

    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let later = session.begin_save_original().unwrap();
    assert!(!session.close_satisfied_by(&later));
    assert_eq!(
        session.save_finished(&later, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert!(session.take_close().is_none());
}

#[test]
fn cancel_and_repeated_close_requests_do_not_leave_a_competing_continuation() {
    let mut session = dirty_session("/a.txt", 1);
    let first = session.arm_close().unwrap();
    let repeated = session.arm_close().unwrap();
    assert_eq!(first, repeated);
    session.invalidate_close();
    assert!(!session.has_close());

    session.mark_dirty(2);
    let save = session.begin_save_original().unwrap();
    assert!(!session.close_satisfied_by(&save));
    assert_eq!(
        session.save_finished(&save, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert!(session.take_close().is_none());
}

#[test]
fn close_while_saving_captures_the_in_flight_job() {
    let mut session = dirty_session("/a.txt", 1);
    let save = session.begin_save_original().unwrap();
    assert!(session.saving());
    let close = session.arm_close().unwrap();
    assert_eq!(close.save, Some(save.id));
    assert_eq!(
        session.save_finished(&save, fake_document("/a.txt", "saved")),
        SaveCompletion::Applied
    );
    assert!(session.close_satisfied_by(&save));
}

#[test]
fn a_close_discard_is_ready_only_while_no_new_edit_exists() {
    let mut session = dirty_session("/a.txt", 1);
    let close = session.arm_close().unwrap();
    session.discard_preserving_close().unwrap();
    assert!(session.close_ready(&close));

    let mut edited = dirty_session("/a.txt", 1);
    let close = edited.arm_close().unwrap();
    let reload = edited.discard_preserving_close().unwrap();
    edited.load_finished(&reload, Loaded::Document(fake_document("/a.txt", "A")));
    edited.begin_edit(0).unwrap();
    edited.mark_dirty(1);
    assert!(
        !edited.close_ready(&close),
        "a fresh edit invalidates the idle close"
    );
}

// ---------------------------------------------------------------------------
// Reload preserves the pending selection
// ---------------------------------------------------------------------------

fn dirty_a_with_candidate(selection: Selection) -> (PreviewSession, LoadTicket) {
    let mut session = PreviewSession::new();
    let ticket = session
        .select(Selection::file("/a.txt"))
        .expect("A is a file selection");
    session.load_finished(&ticket, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    assert_eq!(session.select(selection), None);
    (session, ticket)
}

#[test]
fn reload_of_dirty_a_previews_the_selected_b_instead_of_pulling_a_back() {
    let (mut session, old_a_ticket) = dirty_a_with_candidate(Selection::file("/b.txt"));
    let request = session.begin_save_as().unwrap();
    session
        .choose_save_as_path(request.id, PathBuf::from("/copy.txt"))
        .unwrap();
    assert!(session.save_as_pending());
    session.arm_close().unwrap();
    assert!(session.has_close());

    let reload = session.reload_pinned().expect("B is selected");
    assert_eq!(reload.path, PathBuf::from("/b.txt"));
    assert_eq!(session.candidate(), &Selection::file("/b.txt"));
    assert!(!session.save_as_pending());
    assert!(!session.has_close());
    assert!(!session.is_editing());

    // A's old load and an A result that races the new load cannot replace B.
    assert!(!session.load_finished(
        &old_a_ticket,
        Loaded::Document(fake_document("/a.txt", "old A"))
    ));
    assert!(!session.load_finished(
        &LoadTicket {
            generation: reload.generation.wrapping_sub(1),
            path: PathBuf::from("/a.txt"),
        },
        Loaded::Document(fake_document("/a.txt", "late A"))
    ));
    assert!(session.load_finished(&reload, Loaded::Document(fake_document("/b.txt", "B"))));
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(PathBuf::from("/b.txt"))
    );
    assert_eq!(session.candidate(), &Selection::file("/b.txt"));
}

#[test]
fn reload_of_dirty_a_on_the_trash_page_stays_neutral_and_clears_actions() {
    let (mut session, _) = dirty_a_with_candidate(Selection::Trash);
    let request = session.begin_save_as().unwrap();
    session
        .choose_save_as_path(request.id, PathBuf::from("/copy.txt"))
        .unwrap();
    session.arm_close().unwrap();

    assert!(session.reload_pinned().is_none());
    assert_eq!(session.candidate(), &Selection::Trash);
    assert_eq!(session.state(), &ViewState::Trash);
    assert_eq!(session.current_document(), None);
    assert!(!session.is_editing());
    assert!(!session.is_dirty());
    assert!(!session.save_as_pending());
    assert!(!session.has_close());
}

#[test]
fn reload_with_no_or_multiple_selection_keeps_that_neutral_state() {
    for (selection, expected) in [
        (Selection::None, ViewState::Empty),
        (
            Selection::Multiple { count: 4 },
            ViewState::Multiple { count: 4 },
        ),
    ] {
        let (mut session, _) = dirty_a_with_candidate(selection.clone());
        assert!(session.reload_pinned().is_none());
        assert_eq!(session.candidate(), &selection);
        assert_eq!(session.state(), &expected);
        assert_eq!(session.current_document(), None);
        assert!(!session.is_editing());
        assert!(!session.save_as_pending());
    }
}

#[test]
fn reloading_the_still_selected_file_uses_a_fresh_load_of_that_file() {
    let scratch = Scratch::new("reload-current");
    let path = scratch.write("note.txt", b"old contents\n");
    let first = loaded_document(&path);
    let first_revision = first.revision.clone();

    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file(&path)).unwrap();
    session.load_finished(&ticket, Loaded::Document(first));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);

    fs::write(&path, b"new contents\n").unwrap();
    let reload = session.reload_pinned().expect("A is still selected");
    assert_eq!(reload.path, path);
    assert_eq!(session.candidate(), &Selection::file(&path));
    assert!(!session.is_editing());
    assert!(!session.is_dirty());

    let second = loaded_document(&path);
    let second_revision = second.revision.clone();
    assert!(session.load_finished(&reload, Loaded::Document(second)));
    let current = session.current_document().unwrap();
    assert_eq!(current.path, path);
    assert_eq!(current.text().unwrap().text, "new contents\n");
    assert!(current.revision.same_version(&second_revision));
    assert!(!current.revision.same_version(&first_revision));
    assert!(!session.save_as_pending());
    assert!(!session.has_close());
}

// ---------------------------------------------------------------------------
// Text decoding and limits
// ---------------------------------------------------------------------------

#[test]
fn bom_and_uniform_line_endings_round_trip() {
    let scratch = Scratch::new("encodings");
    let crlf = scratch.write("crlf.txt", b"\xef\xbb\xbfone\r\ntwo\r\n");
    let document = text_document(&crlf);
    assert_eq!(document.text, "one\ntwo\n");
    assert_eq!(
        document.encoding,
        TextEncoding {
            bom: true,
            line_ending: Some(LineEnding::Crlf)
        }
    );
    assert_eq!(
        encode_text(&document.text, document.encoding).unwrap(),
        b"\xef\xbb\xbfone\r\ntwo\r\n"
    );

    let lf = scratch.write("lf.txt", b"one\ntwo\n");
    let document = text_document(&lf);
    assert_eq!(document.encoding.line_ending, Some(LineEnding::Lf));
    assert_eq!(
        encode_text(&document.text, document.encoding).unwrap(),
        b"one\ntwo\n"
    );
}

#[test]
fn empty_files_tabs_unicode_and_trailing_newlines_are_valid() {
    let scratch = Scratch::new("valid");
    for (name, bytes, expected) in [
        ("empty.txt", b"".as_slice(), ""),
        ("tabs.txt", b"a\tb\n".as_slice(), "a\tb\n"),
        ("unicode.txt", "héllo ✓\n".as_bytes(), "héllo ✓\n"),
        ("trailing.txt", b"last\n\n".as_slice(), "last\n\n"),
    ] {
        let path = scratch.write(name, bytes);
        let document = text_document(&path);
        assert_eq!(document.text, expected, "{name}");
        assert_eq!(
            encode_text(&document.text, document.encoding).unwrap(),
            bytes,
            "{name}"
        );
    }
}

#[test]
fn mixed_line_endings_preview_but_refuse_saving() {
    let scratch = Scratch::new("mixed");
    let path = scratch.write("mixed.txt", b"a\r\nb\nc\rd");
    let document = text_document(&path);
    assert_eq!(document.text, "a\nb\nc\nd");
    assert!(document.encoding.is_mixed());
    assert_eq!(
        encode_text(&document.text, document.encoding),
        Err(SaveError::ReadOnly(ReadOnlyReason::MixedLineEndings))
    );
    let loaded = loaded_document(&path);
    assert_eq!(
        loaded.text().unwrap().edit_block_reason(&loaded.access),
        Some(ReadOnlyReason::MixedLineEndings)
    );
}

#[test]
fn binary_and_invalid_utf8_are_never_decoded_lossily() {
    let scratch = Scratch::new("binary");
    let nul = scratch.write("nul.bin", b"abc\0def");
    assert!(matches!(
        load(&nul),
        Loaded::Unsupported {
            reason: UnsupportedReason::Binary,
            ..
        }
    ));
    let invalid = scratch.write("invalid.txt", b"abc\xffdef");
    assert!(matches!(
        load(&invalid),
        Loaded::Unsupported {
            reason: UnsupportedReason::InvalidUtf8,
            ..
        }
    ));
}

#[test]
fn text_size_limit_is_checked_before_the_body_is_shown() {
    let scratch = Scratch::new("text-limit");
    let path = scratch.write("big.txt", &vec![b'a'; MAX_TEXT_BYTES as usize + 1]);
    match load(&path) {
        Loaded::Unsupported {
            reason: UnsupportedReason::TextTooLarge { size },
            ..
        } => assert_eq!(size, MAX_TEXT_BYTES + 1),
        other => panic!("expected a size refusal, got {other:?}"),
    }
}

#[test]
fn image_dimension_limits_are_checked_before_decoding() {
    assert_eq!(image_target_size(800, 600), Some((800, 600)));
    assert_eq!(image_target_size(4000, 2000), Some((2048, 1024)));
    assert_eq!(image_target_size(9000, 9000), None);
    assert_eq!(image_target_size(0, 100), None);
}

#[test]
fn logical_lines_count_blank_and_final_empty_lines() {
    assert_eq!(logical_line_count(""), 1);
    assert_eq!(logical_line_count("a"), 1);
    assert_eq!(logical_line_count("a\n"), 2);
    assert_eq!(logical_line_count("\n\n"), 3);
    assert_eq!(logical_line_count("a\nb\nc"), 3);
}

#[test]
fn non_utf8_names_are_kept_as_paths_and_only_displayed_lossily() {
    let scratch = Scratch::new("names");
    let name = OsString::from_vec(vec![b'b', 0xff, b'.', b't', b'x', b't']);
    let path = scratch.path.join(&name);
    fs::write(&path, b"safe text\n").unwrap();
    match load(&path) {
        Loaded::Document(document) => {
            assert_eq!(document.path, path);
            assert!(document.name.contains('\u{fffd}'));
            assert_eq!(document.text().unwrap().text, "safe text\n");
        }
        other => panic!("expected a document, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Save
// ---------------------------------------------------------------------------

#[test]
fn original_save_round_trips_and_preserves_mode() {
    let scratch = Scratch::new("save");
    let path = scratch.write("note.txt", b"old\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let document = loaded_document(&path);
    let before = document.revision.clone();

    let success = save_original(&document, "new\nline\n").unwrap();
    assert_eq!(fs::read(&path).unwrap(), b"new\nline\n");
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
    assert_eq!(success.size, 9);
    assert!(!success.revision.same_version(&before));
}

#[test]
fn changed_replacement_and_missing_originals_are_refused_without_writing() {
    let scratch = Scratch::new("changed");
    let path = scratch.write("note.txt", b"original\n");
    let document = loaded_document(&path);

    fs::write(&path, b"external\n").unwrap();
    assert_eq!(
        save_original(&document, "mine\n"),
        Err(SaveError::ChangedOnDisk)
    );
    assert_eq!(fs::read(&path).unwrap(), b"external\n");

    let replacement = scratch.write("replacement.txt", b"replaced\n");
    let replacement_document = loaded_document(&replacement);
    fs::remove_file(&replacement).unwrap();
    assert_eq!(
        save_original(&replacement_document, "mine\n"),
        Err(SaveError::Missing)
    );
    assert!(!replacement.exists());
}

#[test]
fn wrong_etag_follows_the_changed_on_disk_path() {
    let scratch = Scratch::new("etag");
    let path = scratch.write("note.txt", b"original\n");
    let document = loaded_document(&path);
    let mut expected = document.revision.clone();
    expected.etag = Some("not-the-current-etag".to_string());
    let result = save(&SaveRequest {
        path: path.clone(),
        text: "mine\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::Original {
            expected: SaveExpectation { revision: expected },
        },
    });
    assert_eq!(result, Err(SaveError::ChangedOnDisk));
    assert_eq!(fs::read(&path).unwrap(), b"original\n");
}

#[test]
fn an_unwritable_file_is_read_only_and_save_leaves_it_alone() {
    let scratch = Scratch::new("readonly");
    let path = scratch.write("note.txt", b"original\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o440)).unwrap();
    let document = loaded_document(&path);
    assert_eq!(
        document.edit_block_reason(),
        Some(ReadOnlyReason::NotWritable)
    );
    assert_eq!(
        save_original(&document, "mine\n"),
        Err(SaveError::ReadOnly(ReadOnlyReason::NotWritable))
    );
    assert_eq!(fs::read(&path).unwrap(), b"original\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
}

#[test]
fn symlinks_and_hard_links_are_presented_read_only_with_a_reason() {
    let scratch = Scratch::new("links");
    let target = scratch.write("target.txt", b"target\n");
    let link = scratch.path("link.txt");
    symlink(&target, &link).unwrap();
    let document = loaded_document(&link);
    assert_eq!(document.access.reason, Some(ReadOnlyReason::Symlink));
    assert!(!document.can_edit());

    let hard = scratch.path("hard.txt");
    fs::hard_link(&target, &hard).unwrap();
    let document = loaded_document(&hard);
    assert_eq!(
        document.access.reason,
        Some(ReadOnlyReason::MultipleHardLinks)
    );
    assert!(!document.can_edit());
}

#[test]
fn only_permission_blocks_are_administrator_editable() {
    let owner = 0;
    let current = 1000;
    let access = |file_writable, parent_writable, reason| WriteAccess {
        file_writable,
        parent_writable,
        reason,
    };

    // NotWritable alone, ParentNotWritable alone, both, and the recorded
    // reason spelling of either.
    assert!(access(false, true, None).admin_editable(owner, current));
    assert!(access(true, false, None).admin_editable(owner, current));
    assert!(access(false, false, None).admin_editable(owner, current));
    assert!(access(false, true, Some(ReadOnlyReason::NotWritable)).admin_editable(owner, current));
    assert!(
        access(true, false, Some(ReadOnlyReason::ParentNotWritable)).admin_editable(owner, current)
    );
    assert!(access(false, false, Some(ReadOnlyReason::NotWritable)).admin_editable(owner, current));

    // Every other reason is a safe-writing block, not a permission block.
    for reason in [
        ReadOnlyReason::Symlink,
        ReadOnlyReason::MultipleHardLinks,
        ReadOnlyReason::MixedLineEndings,
    ] {
        assert!(!access(false, true, Some(reason)).admin_editable(owner, current));
    }

    // Nothing is blocked: ordinary editing already works.
    assert!(!access(true, true, None).admin_editable(owner, current));
    // The owner is the current user.
    assert!(!access(false, true, None).admin_editable(current, current));
}

#[test]
fn mixed_line_endings_are_never_administrator_editable() {
    let mut document = fake_document("/a.txt", "A");
    document.access = WriteAccess {
        file_writable: false,
        parent_writable: true,
        reason: Some(ReadOnlyReason::NotWritable),
    };
    document.content = PreviewContent::Text(TextDocument {
        text: "A\r\nB\n".to_string(),
        encoding: TextEncoding {
            bom: false,
            line_ending: None,
        },
    });
    assert!(!document.admin_editable(0, 1000));
}

#[test]
fn administrator_owners_are_named_for_the_warning() {
    assert_eq!(owner_name(0), "root");
    assert!(!owner_name(crate::admin::current_uid().unwrap_or(1000)).is_empty());
}

fn admin_document() -> PreviewDocument {
    let mut document = fake_document("/a.txt", "A");
    document.access = WriteAccess {
        file_writable: false,
        parent_writable: true,
        reason: Some(ReadOnlyReason::NotWritable),
    };
    document
}

#[test]
fn administrator_mode_is_ui_only_and_save_as_is_refused() {
    let mut session = PreviewSession::new();
    let ticket = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&ticket, Loaded::Document(admin_document()));

    assert_eq!(
        session.begin_edit_for(crate::command::Caller::Hotkey, 0, 0, 1000),
        Err(ADMIN_EDIT_UI_ONLY.to_string())
    );
    assert!(!session.is_editing());

    session
        .begin_edit_for(crate::command::Caller::Ui, 0, 0, 1000)
        .unwrap();
    assert!(session.is_admin_editing());
    assert_eq!(
        session.begin_save_as(),
        Err("Save As isn't available while editing as administrator.".to_string())
    );

    session.mark_dirty(1);
    assert_eq!(
        session.begin_save_original_for(crate::command::Caller::Hotkey),
        Err(ADMIN_EDIT_UI_ONLY.to_string())
    );
    assert!(
        session
            .begin_save_original_for(crate::command::Caller::Ui)
            .is_ok()
    );
}

#[test]
fn save_as_refuses_a_file_that_appeared_after_the_check() {
    let scratch = Scratch::new("save-as-race");
    let source = scratch.write("source.txt", b"source\n");
    let document = loaded_document(&source);
    let target = scratch.path("new.txt");
    assert_eq!(inspect_destination(&target).unwrap(), Destination::Missing);
    fs::write(&target, b"appeared\n").unwrap();

    let result = save(&SaveRequest {
        path: target.clone(),
        text: "mine\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::NewPath { replace: None },
    });
    assert_eq!(result, Err(SaveError::DestinationAppeared));
    assert_eq!(fs::read(&target).unwrap(), b"appeared\n");
}

#[test]
fn save_as_existing_requires_a_confirmation_bound_to_that_revision() {
    let scratch = Scratch::new("save-as-existing");
    let source = scratch.write("source.txt", b"source\n");
    let document = loaded_document(&source);
    let target = scratch.write("target.txt", b"old\n");
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let destination = inspect_destination(&target).unwrap();
    let Destination::Existing {
        revision, writable, ..
    } = destination
    else {
        panic!("expected an existing destination");
    };
    assert!(writable);

    let without = SaveRequest {
        path: target.clone(),
        text: "mine\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::NewPath { replace: None },
    };
    assert_eq!(save(&without), Err(SaveError::DestinationAppeared));

    fs::write(&target, b"changed after confirmation\n").unwrap();
    let stale = SaveRequest {
        target: SaveTarget::NewPath {
            replace: Some(ReplaceConfirmation {
                path: target.clone(),
                revision,
            }),
        },
        ..without.clone()
    };
    assert_eq!(save(&stale), Err(SaveError::DestinationChanged));
    assert_eq!(fs::read(&target).unwrap(), b"changed after confirmation\n");

    let fresh = match inspect_destination(&target).unwrap() {
        Destination::Existing { revision, .. } => revision,
        other => panic!("expected an existing destination, got {other:?}"),
    };
    let replace = SaveRequest {
        target: SaveTarget::NewPath {
            replace: Some(ReplaceConfirmation {
                path: target.clone(),
                revision: fresh,
            }),
        },
        ..without
    };
    save(&replace).unwrap();
    assert_eq!(fs::read(&target).unwrap(), b"mine\n");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn save_as_new_path_leaves_the_original_unchanged() {
    let scratch = Scratch::new("save-as-new");
    let source = scratch.write("source.txt", b"source\n");
    let document = loaded_document(&source);
    let target = scratch.path("copy.txt");
    let success = save(&SaveRequest {
        path: target.clone(),
        text: "copy\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::NewPath { replace: None },
    })
    .unwrap();
    assert_eq!(success.path, target);
    assert_eq!(fs::read(&source).unwrap(), b"source\n");
    assert_eq!(fs::read(&target).unwrap(), b"copy\n");
    assert_eq!(
        fs::metadata(&target).unwrap().permissions().mode() & 0o111,
        0
    );
}

#[test]
fn dry_run_plan_does_not_touch_the_file() {
    let scratch = Scratch::new("dry-run");
    let path = scratch.write("note.txt", b"original\n");
    let document = loaded_document(&path);
    let request = SaveRequest {
        path: path.clone(),
        text: "mine\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::Original {
            expected: SaveExpectation {
                revision: document.revision.clone(),
            },
        },
    };
    let lines = save_plan(&request);
    assert!(lines.iter().any(|line| line.contains("Save")));
    assert_eq!(fs::read(&path).unwrap(), b"original\n");
}

#[test]
fn a_failed_new_path_write_removes_only_its_own_placeholder() {
    let scratch = Scratch::new("placeholder");
    let path = scratch.write("taken.txt", b"keep\n");
    let document = loaded_document(&scratch.write("source.txt", b"source\n"));
    // A non-empty destination is refused before a placeholder is created.
    let result = save(&SaveRequest {
        path: path.clone(),
        text: "mine\n".to_string(),
        encoding: document.text().unwrap().encoding,
        target: SaveTarget::NewPath { replace: None },
    });
    assert_eq!(result, Err(SaveError::DestinationAppeared));
    assert_eq!(fs::read(&path).unwrap(), b"keep\n");
}

#[test]
fn revision_identity_and_size_formatting_are_stable() {
    let scratch = Scratch::new("revision");
    let path = scratch.write("note.txt", b"abc");
    let first = inspect_revision(&path).unwrap();
    assert_eq!(first.size, 3);
    let second = inspect_revision(&path).unwrap();
    assert!(first.same_version(&second));
    fs::write(&path, b"abcd").unwrap();
    let third = inspect_revision(&path).unwrap();
    assert!(!first.same_version(&third));
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(1536), "1.5 KiB");
    assert_eq!(document_title("a.txt", 1536), "a.txt (1.5 KiB)");
}

#[test]
fn a_missing_destination_is_missing_not_an_error() {
    let scratch = Scratch::new("missing-destination");
    assert_eq!(
        inspect_destination(&scratch.path("none.txt")).unwrap(),
        Destination::Missing
    );
}

// ---------------------------------------------------------------------------
// P2m-r3: rendering bounds, PDF requests and Save As ownership
// ---------------------------------------------------------------------------

#[test]
fn fit_and_render_bounds_preserve_aspect_and_report_the_limit() {
    assert_eq!(fit_within(4000, 2000, 800, 800), (800, 400));
    assert_eq!(fit_within(100, 50, 800, 800), (100, 50));
    assert_eq!(fit_width(300, 500, 600), (600, 1000));
    let plan = render_plan(1000, 500, 2.0);
    assert_eq!(
        plan.size,
        RenderSize {
            width: 2000,
            height: 1000
        }
    );
    assert_eq!(plan.limit, None);
    let limited = render_plan(100_000, 10_000, 2.0);
    assert_eq!(limited.limit, Some(RenderLimit::Edge));
    assert!(limited.size.width <= MAX_RENDER_EDGE);
    assert!(limited.size.height <= MAX_RENDER_EDGE);
    let mut pixels = render_plan(MAX_RENDER_EDGE, MAX_RENDER_EDGE, 1.0);
    pixels.limit = if u64::from(pixels.size.width as u32) * u64::from(pixels.size.height as u32)
        > MAX_RENDER_PIXELS
    {
        Some(RenderLimit::Pixels)
    } else {
        None
    };
    assert!(
        u64::from(pixels.size.width as u32) * u64::from(pixels.size.height as u32)
            <= MAX_RENDER_PIXELS
    );
}

#[test]
fn pdf_arguments_are_individual_os_strings_for_page_one_only() {
    let request = PdfRenderRequest {
        input: PathBuf::from("/tmp/gimbal-preview-pdf-fixture/leading space - dash.pdf"),
        output_dir: PathBuf::from("/tmp/gimbal-preview-pdf-job"),
        target: RenderSize {
            width: 640,
            height: 900,
        },
    };
    let args: Vec<String> = request
        .arguments()
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args[..5].to_vec(),
        vec![
            "-f".to_string(),
            "1".to_string(),
            "-l".to_string(),
            "1".to_string(),
            "-singlefile".to_string()
        ]
    );
    assert!(args.contains(&"-png".to_string()));
    assert!(args.contains(&"-scale-to".to_string()));
    assert!(!args.contains(&"-scale-to-x".to_string()));
    assert!(!args.contains(&"-scale-to-y".to_string()));
    // The square box is the longest requested axis, capped before Poppler.
    assert!(args.contains(&"900".to_string()));
    assert_eq!(args.last().unwrap(), "/tmp/gimbal-preview-pdf-job/page");
    assert!(
        args.iter()
            .any(|argument| argument.ends_with("leading space - dash.pdf"))
    );
}

#[test]
fn pdf_raster_plans_bound_both_axes_and_pixels_before_spawning() {
    let small = pdf_raster_plan(RenderSize {
        width: 400,
        height: 400,
    });
    assert_eq!(small.scale_to, 400);
    assert_eq!(small.limit, None);
    let tall = pdf_raster_plan(RenderSize {
        width: 400,
        height: 8192,
    });
    assert_eq!(tall.scale_to, PDF_MAX_RASTER_EDGE);
    assert_eq!(tall.limit, Some(RenderLimit::Pixels));
    assert!(u64::from(tall.scale_to as u32) * u64::from(tall.scale_to as u32) <= MAX_RENDER_PIXELS);
    assert!(tall.scale_to <= MAX_RENDER_EDGE);
    let ordinary = pdf_raster_plan(RenderSize {
        width: 640,
        height: 900,
    });
    assert_eq!(ordinary.scale_to, 900);
    // The same plan is what the production argv carries.
    let request = PdfRenderRequest {
        input: PathBuf::from("/tmp/gimbal-preview-pdf-fixture/one.pdf"),
        output_dir: PathBuf::from("/tmp/gimbal-preview-pdf-job"),
        target: RenderSize {
            width: 640,
            height: 900,
        },
    };
    let args: Vec<String> = request
        .arguments()
        .into_iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let position = args.iter().position(|arg| arg == "-scale-to").unwrap();
    assert_eq!(args[position + 1], "900");
}

#[test]
fn png_output_dimensions_are_checked_before_texture_creation() {
    let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    bytes.extend_from_slice(&640u32.to_be_bytes());
    bytes.extend_from_slice(&480u32.to_be_bytes());
    assert_eq!(
        png_dimensions(&bytes),
        Some(RenderSize {
            width: 640,
            height: 480
        })
    );
    assert_eq!(pdf_output_allowed(&bytes).unwrap().height, 480);
    let mut huge = bytes.clone();
    huge[16..20].copy_from_slice(&(MAX_RENDER_EDGE as u32 + 1).to_be_bytes());
    assert!(pdf_output_allowed(&huge).is_err());
    assert_eq!(png_dimensions(b"not a png"), None);
}

#[test]
fn thumbnail_candidates_prefer_larger_valid_files_and_skip_missing_ones() {
    let scratch = Scratch::new("thumbnail-candidates");
    let normal = scratch.write("normal.png", b"normal");
    let large = scratch.write("large.png", b"large");
    let info = gio::FileInfo::new();
    info.set_attribute_string(
        ThumbnailKind::Normal.path_attribute(),
        normal.to_string_lossy().as_ref(),
    );
    info.set_attribute_boolean(ThumbnailKind::Normal.valid_attribute(), true);
    info.set_attribute_string(
        ThumbnailKind::Large.path_attribute(),
        large.to_string_lossy().as_ref(),
    );
    info.set_attribute_boolean(ThumbnailKind::Large.valid_attribute(), true);
    info.set_attribute_string(
        ThumbnailKind::XLarge.path_attribute(),
        "/tmp/gimbal-preview-thumbnail-candidates/missing.png",
    );
    info.set_attribute_boolean(ThumbnailKind::XLarge.valid_attribute(), true);
    let candidates = thumbnail_candidates(&info);
    assert_eq!(
        candidates
            .iter()
            .map(|candidate| candidate.kind)
            .collect::<Vec<_>>(),
        vec![ThumbnailKind::Large, ThumbnailKind::Normal]
    );
    assert!(candidates.iter().all(|candidate| candidate.size > 0));
}

#[test]
fn a_successful_save_as_keeps_the_new_preview_through_a_passive_reapply() {
    let scratch = Scratch::new("save-as-ownership");
    let source = scratch.write("source.txt", b"source\n");
    let target = scratch.path("copy.txt");
    let mut session = PreviewSession::new();
    let load = session.select(Selection::file(&source)).unwrap();
    session.load_finished(&load, Loaded::Document(loaded_document(&source)));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let request = session.begin_save_as().unwrap();
    assert_eq!(
        session
            .choose_save_as_path(request.id, target.clone())
            .unwrap(),
        SaveAsRoute::NewPath
    );
    session
        .save_as_destination(request.id, &target, None)
        .unwrap();
    session.confirm_save_as(request.id).unwrap();
    let ticket = session.start_save_as(request.id).unwrap();
    let saved = save(&SaveRequest {
        path: target.clone(),
        text: "copy\n".to_string(),
        encoding: ticket.encoding,
        target: SaveTarget::NewPath { replace: None },
    })
    .unwrap();
    let document = loaded_document(&target);
    assert_eq!(
        session.save_finished(&ticket, document),
        SaveCompletion::Applied
    );
    assert_eq!(
        session.saved_override().map(|saved| saved.path.clone()),
        Some(target.clone())
    );
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(target.clone())
    );
    assert_eq!(fs::read(&target).unwrap(), b"copy\n");
    assert_eq!(fs::read(&source).unwrap(), b"source\n");

    // The production passive refresh reapplies the old browser selection; it
    // must not steal the saved preview.
    assert!(
        session
            .apply_model_selection(Selection::file(&source))
            .is_none()
    );
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(target.clone())
    );

    // An explicit user click on the old already-selected item does release it.
    let reload = session
        .select_user(Selection::file(&source))
        .expect("re-selecting the old item reloads it");
    assert_eq!(reload.path, source);
    assert_eq!(
        session.state(),
        &ViewState::Loading {
            path: source.clone()
        }
    );
    assert!(session.saved_override().is_none());
    assert_eq!(saved.size, 5);

    // The saved new path becomes the next editor's real save target.
    let reloaded = loaded_document(&target);
    let load = session.select_user(Selection::file(&target)).unwrap();
    session.load_finished(&load, Loaded::Document(reloaded));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let original = session.begin_save_original().unwrap();
    assert_eq!(original.path, target);
    assert!(!original.new_path);
    let success =
        save_original(&session.current_document().unwrap().clone(), "edited new\n").unwrap();
    assert_eq!(success.path, target);
    assert_eq!(
        session.save_finished(&original, loaded_document(&target)),
        SaveCompletion::Applied
    );
    assert_eq!(fs::read(&target).unwrap(), b"edited new\n");

    // A Trash selection is a genuine navigation and releases any override.
    session.select_user(Selection::Trash);
    assert!(session.saved_override().is_none());
    assert_eq!(session.state(), &ViewState::Trash);
}

#[test]
fn a_save_as_owner_survives_an_ordinary_save_and_reconcile_until_explicit_reselection() {
    let scratch = Scratch::new("save-as-owner-chain");
    let source = scratch.write("source.txt", b"source bytes stay\n");
    let target = scratch.path("owned.txt");
    let mut session = PreviewSession::new();

    // Select A, edit it and Save As B through the real atomic save path.
    let load = session.select_user(Selection::file(&source)).unwrap();
    session.load_finished(&load, Loaded::Document(loaded_document(&source)));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let request = session.begin_save_as().unwrap();
    assert_eq!(
        session
            .choose_save_as_path(request.id, target.clone())
            .unwrap(),
        SaveAsRoute::NewPath
    );
    session
        .save_as_destination(request.id, &target, None)
        .unwrap();
    session.confirm_save_as(request.id).unwrap();
    let first = session.start_save_as(request.id).unwrap();
    let first_success = save(&SaveRequest {
        path: first.path.clone(),
        text: "owned v1\n".to_string(),
        encoding: first.encoding,
        target: SaveTarget::NewPath { replace: None },
    })
    .unwrap();
    assert_eq!(
        session.save_finished(&first, loaded_document(&target)),
        SaveCompletion::Applied
    );
    let first_revision = first_success.revision.clone();
    assert_eq!(
        session.saved_override().map(|saved| saved.revision.clone()),
        Some(first_revision.clone())
    );

    // The production refresh reapplies the old browser candidate A.
    assert!(
        session
            .apply_model_selection(Selection::file(&source))
            .is_none()
    );
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(target.clone())
    );

    // Edit the owned document B and run an ordinary Save.
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let ticket = session.begin_save_original().unwrap();
    assert_eq!(ticket.path, target);
    assert!(!ticket.new_path);
    let second_success = save(&SaveRequest {
        path: ticket.path.clone(),
        text: "owned v2\n".to_string(),
        encoding: ticket.encoding,
        target: SaveTarget::Original {
            expected: SaveExpectation {
                revision: ticket.token.editor.revision.clone(),
            },
        },
    })
    .unwrap();
    assert_eq!(
        session.save_finished(&ticket, loaded_document(&target)),
        SaveCompletion::Applied
    );
    assert!(session.saved_override().is_some());
    assert_eq!(
        session.saved_override().map(|saved| saved.revision.clone()),
        Some(second_success.revision.clone()),
        "the owned document keeps its updated saved revision"
    );
    assert_eq!(fs::read(&target).unwrap(), b"owned v2\n");
    assert_eq!(fs::read(&source).unwrap(), b"source bytes stay\n");
    assert!(
        session.reconcile().is_none(),
        "reconcile respects ownership"
    );
    assert!(
        session
            .apply_model_selection(Selection::file(&source))
            .is_none()
    );
    // A own-write monitor/force reload of B is not a user navigation.
    let monitor = session.force_load(target.clone());
    session.load_finished(&monitor, Loaded::Document(loaded_document(&target)));
    assert!(session.saved_override().is_some());
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(target.clone())
    );
    assert!(!session.save_as_pending());
    assert!(session.active_save().is_none());
    assert!(session.pinned().is_none());
    assert!(!session.has_close());

    // A conflict Reload or Discard of the owned editor reloads B, not the
    // older browser candidate A.
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let reload = session.reload_pinned().expect("the owned editor reloads");
    assert_eq!(reload.path, target);
    session.load_finished(&reload, Loaded::Document(loaded_document(&target)));
    assert!(session.saved_override().is_some());
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let discarded = session.discard().expect("the owned editor discards");
    assert_eq!(discarded.path, target);
    session.load_finished(&discarded, Loaded::Document(loaded_document(&target)));
    assert!(session.saved_override().is_some());
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(target.clone())
    );

    // Explicitly selecting A releases ownership and reloads it normally.
    let explicit = session.select_user(Selection::file(&source)).unwrap();
    assert_eq!(explicit.path, source);
    session.load_finished(&explicit, Loaded::Document(loaded_document(&source)));
    assert!(session.saved_override().is_none());
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(source.clone())
    );
    assert_eq!(fs::read(&target).unwrap(), b"owned v2\n");
    assert_eq!(fs::read(&source).unwrap(), b"source bytes stay\n");
}

#[test]
fn a_save_as_in_another_window_does_not_take_over_this_preview() {
    let dirty = |path: &str| {
        let mut session = PreviewSession::new();
        let load = session.select(Selection::file(path)).unwrap();
        session.load_finished(&load, Loaded::Document(fake_document(path, "A")));
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        session
    };
    let mut this = dirty("/this.txt");
    let mut other = dirty("/other.txt");
    let request = other.begin_save_as().unwrap();
    let target = PathBuf::from("/other-new.txt");
    other
        .choose_save_as_path(request.id, target.clone())
        .unwrap();
    other
        .save_as_destination(request.id, &target, None)
        .unwrap();
    other.confirm_save_as(request.id).unwrap();
    let ticket = other.start_save_as(request.id).unwrap();
    assert_eq!(
        other.save_finished(&ticket, fake_document("/other-new.txt", "other")),
        SaveCompletion::Applied
    );
    assert_eq!(
        other
            .current_document()
            .map(|document| document.path.clone()),
        Some(target)
    );
    assert!(this.saved_override().is_none());
    assert_eq!(
        this.current_document()
            .map(|document| document.path.clone()),
        Some(PathBuf::from("/this.txt"))
    );
    assert!(
        this.apply_model_selection(Selection::file("/this.txt"))
            .is_none()
    );
    assert_eq!(
        this.current_document()
            .map(|document| document.path.clone()),
        Some(PathBuf::from("/this.txt"))
    );
}

#[test]
fn a_stale_save_as_completion_does_not_create_an_override() {
    let mut session = PreviewSession::new();
    let load = session.select(Selection::file("/a.txt")).unwrap();
    session.load_finished(&load, Loaded::Document(fake_document("/a.txt", "A")));
    session.begin_edit(0).unwrap();
    session.mark_dirty(1);
    let request = session.begin_save_as().unwrap();
    let path = PathBuf::from("/b.txt");
    session
        .choose_save_as_path(request.id, path.clone())
        .unwrap();
    session
        .save_as_destination(request.id, &path, None)
        .unwrap();
    session.confirm_save_as(request.id).unwrap();
    let ticket = session.start_save_as(request.id).unwrap();
    assert_eq!(
        session.save_finished(&ticket, fake_document("/c.txt", "C")),
        SaveCompletion::Stale
    );
    assert!(session.saved_override().is_none());
    assert_eq!(
        session
            .current_document()
            .map(|document| document.path.clone()),
        Some(PathBuf::from("/a.txt")),
        "a stale result must not retarget the existing preview"
    );
}

struct FakeAdminWriter {
    calls: Vec<String>,
    fail: Option<gio::IOErrorEnum>,
}

impl AdminWriter for FakeAdminWriter {
    fn replace_contents(
        &mut self,
        file: &gio::File,
        bytes: &[u8],
        etag: Option<&str>,
        make_backup: bool,
    ) -> Result<Option<String>, glib::Error> {
        self.calls.push(format!(
            "replace {} etag={etag:?} make_backup={make_backup} bytes={}",
            file.uri(),
            bytes.len()
        ));
        if let Some(kind) = self.fail {
            return Err(glib::Error::new(kind, "fake administrator failure"));
        }
        Ok(Some("new-etag".to_string()))
    }

    fn set_attribute_uint32(
        &mut self,
        _file: &gio::File,
        attribute: &str,
        value: u32,
    ) -> Result<(), glib::Error> {
        self.calls.push(format!("set {attribute}={value}"));
        Ok(())
    }
}

#[test]
fn administrator_save_uses_the_admin_scope_load_etag_and_no_backup() {
    let scratch = Scratch::new("admin-save");
    let path = scratch.write("root.conf", b"line one\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    let metadata = fs::metadata(&path).unwrap();
    let expected = SaveExpectation {
        revision: FileRevision::from_metadata(&metadata, Some("load-etag".to_string())),
    };
    let mut fake = FakeAdminWriter {
        calls: Vec::new(),
        fail: None,
    };
    let success = save_admin_with(&path, b"line two\n", &expected, &mut fake).unwrap();
    let admin_uri = crate::admin::gfile(&path, crate::admin::Scope::Admin)
        .uri()
        .to_string();
    assert_eq!(
        fake.calls[0],
        format!("replace {admin_uri} etag=Some(\"load-etag\") make_backup=false bytes=9")
    );
    assert_eq!(
        fake.calls[1],
        format!("set unix::uid={}", metadata.uid()),
        "the owner is restored before the group"
    );
    assert_eq!(
        fake.calls[2],
        format!("set unix::gid={}", metadata.gid()),
        "the group is restored before the mode"
    );
    assert_eq!(
        fake.calls[3],
        format!("set unix::mode={}", metadata.permissions().mode() & 0o7777),
        "the mode is restored before success is reported"
    );
    assert_eq!(success.mode, 0o640);
    assert_eq!(success.size, 9);
    assert!(success.revision.etag.is_some());
}

#[test]
fn administrator_save_reports_a_wrong_etag_as_a_conflict() {
    let scratch = Scratch::new("admin-etag");
    let path = scratch.write("root.conf", b"line one\n");
    let metadata = fs::metadata(&path).unwrap();
    let expected = SaveExpectation {
        revision: FileRevision::from_metadata(&metadata, Some("load-etag".to_string())),
    };
    let mut fake = FakeAdminWriter {
        calls: Vec::new(),
        fail: Some(gio::IOErrorEnum::WrongEtag),
    };
    assert_eq!(
        save_admin_with(&path, b"line two\n", &expected, &mut fake),
        Err(SaveError::ChangedOnDisk)
    );
    assert_eq!(fake.calls.len(), 1, "no metadata write follows a conflict");
    assert_eq!(fs::read(&path).unwrap(), b"line one\n");
}
