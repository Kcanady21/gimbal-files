// SPDX-License-Identifier: GPL-3.0-or-later
//! What Save decides, and what SaveFiles names.
//!
//! Everything here is pure apart from two small filesystem helpers
//! ([`inspect`] and [`folder_writable`]): the decision functions take a
//! `lookup` closure, so their tests need no files. The picker supplies
//! [`inspect`] in the running dialog.
//!
//! The model for the overwrite and collision questions is Nautilus's Save
//! dialog (`gimbal-refs/nautilus-50.2.2/src/nautilus-file-chooser.c:183-343`,
//! `:411-510`) and its portal results (`nautilus-portal.c:236-262`,
//! `:485-520`). The `name (2).ext` collision naming follows fm-core's
//! `free_copy_name` (`fm-core/src/fileops/mod.rs:340-360`) with "(2)" rather
//! than "(copy)": nothing is being copied.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use gtk::gio;
use gtk::prelude::*;

use fm_core::fileops::is_valid_name;

/// What a path is, as far as Save cares. A symlink is followed for the first
/// two: a link to a regular file is a file to overwrite, and a link to a folder
/// is a folder to enter. A dangling link, socket or device is `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Existing {
    Missing,
    Folder,
    File,
    Other,
}

/// What one Save should do. `Enter` is not an answer: it navigates and leaves
/// the dialog open, exactly as GTK and Nautilus do when the typed name is an
/// existing folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    Refused(&'static str),
    Enter(PathBuf),
    Overwrite(PathBuf),
    Save(PathBuf),
}

/// The empty or whitespace-only name's refusal.
pub const TYPE_A_NAME: &str = "Type a file name";
/// A name that is not one legal path component.
pub const BAD_NAME: &str = "That name can't be used";
/// A name taken by something that is not a regular file.
pub const TAKEN: &str = "That name is taken by something that isn't a file";
/// The folder cannot be written to.
pub const NOT_WRITABLE: &str = "You can't save in this folder";
/// An absolute or `~/` path whose folder is not there.
pub const NO_SUCH_FOLDER: &str = "That folder doesn't exist";
/// A relative name with a slash in it.
pub const NAME_HAS_SLASH: &str = "A name can't contain /";

/// The whole SaveFile decision. Empty names are refused before anything is
/// looked up; names keep their exact bytes, with no trimming beyond the
/// empty-name check and no case folding.
pub fn save_target(
    folder: &Path,
    typed: &str,
    folder_writable: bool,
    lookup: &dyn Fn(&Path) -> Existing,
) -> SaveTarget {
    if typed.trim().is_empty() {
        return SaveTarget::Refused(TYPE_A_NAME);
    }
    if !is_valid_name(typed) {
        return SaveTarget::Refused(BAD_NAME);
    }
    let target = folder.join(typed);
    match lookup(&target) {
        // Entering a folder changes where the next Save lands, not the answer.
        Existing::Folder => SaveTarget::Enter(target),
        // A read-only mount or /usr refuses every save, even over a file the
        // user can read.
        _ if !folder_writable => SaveTarget::Refused(NOT_WRITABLE),
        Existing::File => SaveTarget::Overwrite(target),
        Existing::Other => SaveTarget::Refused(TAKEN),
        Existing::Missing => SaveTarget::Save(target),
    }
}

/// The bytes of a `ay` option up to its first NUL. The portal sends a
/// NUL-terminated string; a missing NUL is tolerated, as
/// [`super::service::folder_from_bytes`] does.
pub fn field_bytes(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    &value[..end]
}

/// Whether one path component is legal, byte for byte. This is
/// `fm_core::fileops::is_valid_name` in its byte form: Linux names need not be
/// UTF-8, and SaveFiles carries bytes.
fn valid_component(name: &OsStr) -> bool {
    let bytes = name.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 255
        && bytes != b"."
        && bytes != b".."
        && !bytes.contains(&b'/')
        && !bytes.contains(&0)
}

/// SaveFiles' names: each `files` entry by its last path component. One that
/// is not a valid name after that fails the whole call, naming its index.
pub fn save_files_names(files: &[Vec<u8>]) -> Result<Vec<OsString>, usize> {
    let mut names = Vec::with_capacity(files.len());
    for (index, field) in files.iter().enumerate() {
        let bytes = field_bytes(field);
        if bytes.is_empty() {
            return Err(index);
        }
        let path = Path::new(OsStr::from_bytes(bytes));
        let Some(name) = path.file_name() else {
            return Err(index);
        };
        if !valid_component(name) {
            return Err(index);
        }
        names.push(name.to_os_string());
    }
    Ok(names)
}

/// The entries that clash: a target that already exists in `folder`, or a name
/// that appeared earlier in this same answer.
pub fn clashing(
    folder: &Path,
    names: &[OsString],
    lookup: &dyn Fn(&Path) -> Existing,
) -> Vec<usize> {
    let mut seen: HashSet<OsString> = HashSet::new();
    let mut clashes = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let exists = !matches!(lookup(&folder.join(name)), Existing::Missing);
        let duplicate = !seen.insert(name.clone());
        if exists || duplicate {
            clashes.push(index);
        }
    }
    clashes
}

/// How SaveFiles answers an existing target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CollisionChoice {
    /// Every clashing name gets the first free `name (2).ext`.
    KeepBoth,
    /// Existing names stay; only duplicates within the answer are numbered.
    Replace,
}

/// The paths SaveFiles answers with, in the order it was sent. `reserved` is
/// every name this answer has already handed out, so one answer can never name
/// the same path twice.
pub fn resolve_names(
    folder: &Path,
    names: &[OsString],
    choice: CollisionChoice,
    lookup: &dyn Fn(&Path) -> Existing,
) -> Vec<PathBuf> {
    let mut reserved: HashSet<OsString> = HashSet::new();
    let mut paths = Vec::with_capacity(names.len());
    for name in names {
        let exists = !matches!(lookup(&folder.join(name)), Existing::Missing);
        let duplicate = reserved.contains(name);
        let rename = match choice {
            CollisionChoice::KeepBoth => exists || duplicate,
            CollisionChoice::Replace => duplicate,
        };
        let chosen = if rename {
            free_save_name(folder, name, &reserved, lookup)
        } else {
            name.clone()
        };
        reserved.insert(chosen.clone());
        paths.push(folder.join(chosen));
    }
    paths
}

/// The first free `name (2).ext`, `name (3).ext`, … in `folder`, avoiding names
/// this answer already reserved.
pub fn free_save_name(
    folder: &Path,
    name: &OsStr,
    reserved: &HashSet<OsString>,
    lookup: &dyn Fn(&Path) -> Existing,
) -> OsString {
    for number in 2..10_000 {
        let candidate = numbered_name(name, number);
        if !reserved.contains(&candidate)
            && matches!(lookup(&folder.join(&candidate)), Existing::Missing)
        {
            return candidate;
        }
    }
    numbered_name(name, 10_000)
}

/// `name (n).ext`, with the extension kept after the number. `file_stem`'s rule
/// is the one rename and New Folder use, so `archive.tar.gz` becomes
/// `archive.tar (2).gz`.
fn numbered_name(name: &OsStr, number: usize) -> OsString {
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or(name);
    let mut result = stem.to_os_string();
    result.push(format!(" ({number})"));
    if let Some(extension) = path.extension() {
        result.push(".");
        result.push(extension);
    }
    result
}

/// The number of characters to select in the Name entry: the stem, as
/// `rename_prefill` selects it (`fileops/dialogs.rs:188-199`). `Untitled.odt`
/// selects 8, `archive.tar.gz` selects 11, and `.bashrc` selects all 7.
pub fn stem_selection(name: &str) -> i32 {
    let stem = Path::new(name).file_stem().unwrap_or(OsStr::new(name));
    i32::try_from(stem.to_string_lossy().chars().count()).unwrap_or(i32::MAX)
}

/// The SaveFiles footer's line: up to three names, then how many were left
/// out.
pub fn save_files_summary(names: &[OsString]) -> String {
    if names.is_empty() {
        return "No files to save".to_string();
    }
    let shown: Vec<String> = names
        .iter()
        .take(3)
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let mut listed = shown.join(", ");
    if names.len() > shown.len() {
        listed.push_str(&format!(" and {} more", names.len() - shown.len()));
    }
    format!(
        "Saving {} {}: {listed}",
        names.len(),
        if names.len() == 1 { "file" } else { "files" }
    )
}

/// The collision dialog's body, naming the folder and up to three clashing
/// names.
pub fn collision_text(folder: &Path, names: &[OsString], clashes: &[usize]) -> String {
    let mut unique: Vec<&OsStr> = Vec::new();
    for index in clashes {
        let Some(name) = names.get(*index).map(OsString::as_os_str) else {
            continue;
        };
        if !unique.contains(&name) {
            unique.push(name);
        }
    }
    let count = clashes.len();
    let shown: Vec<String> = unique
        .iter()
        .take(3)
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    let mut listed = shown.join(", ");
    if unique.len() > shown.len() {
        listed.push_str(&format!(" and {} more", unique.len() - shown.len()));
    }
    format!(
        "{} of these files already {} in “{}”: {}",
        count,
        if count == 1 { "exists" } else { "exist" },
        folder.display(),
        listed
    )
}

/// What one path is, for the running dialog.
pub fn inspect(path: &Path) -> Existing {
    match std::fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Existing::Missing,
        Err(_) => Existing::Other,
        Ok(_) => {
            if path.is_dir() {
                Existing::Folder
            } else if path.is_file() {
                Existing::File
            } else {
                Existing::Other
            }
        }
    }
}

/// Whether the folder's `access::can-write` attribute is true. GIO accounts for
/// the mount and ACLs, which a permission-bit check does not; a failed query is
/// a refusal, never a silent attempt.
pub fn folder_writable(folder: &Path) -> bool {
    gio::File::for_path(folder)
        .query_info(
            "access::can-write",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .map(|info| info.boolean("access::can-write"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nothing_exists(_: &Path) -> Existing {
        Existing::Missing
    }

    fn exists(name: &str) -> impl Fn(&Path) -> Existing {
        let name = name.to_string();
        move |path: &Path| {
            if path.file_name().is_some_and(|have| have == name.as_str()) {
                Existing::File
            } else {
                Existing::Missing
            }
        }
    }

    fn os(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn an_empty_or_whitespace_only_name_is_refused_before_a_lookup() {
        for name in ["", " ", "\t\n"] {
            assert_eq!(
                save_target(Path::new("/tmp"), name, true, &|_| panic!("no lookup")),
                SaveTarget::Refused(TYPE_A_NAME)
            );
        }
    }

    #[test]
    fn an_invalid_name_is_refused_and_a_long_one_too() {
        for name in ["a/b", ".", "..", "a\0b"] {
            assert_eq!(
                save_target(Path::new("/tmp"), name, true, &nothing_exists),
                SaveTarget::Refused(BAD_NAME),
                "{name:?}"
            );
        }
        let long = "x".repeat(256);
        assert_eq!(
            save_target(Path::new("/tmp"), &long, true, &nothing_exists),
            SaveTarget::Refused(BAD_NAME)
        );
        let fit = "x".repeat(255);
        assert_eq!(
            save_target(Path::new("/tmp"), &fit, true, &nothing_exists),
            SaveTarget::Save(Path::new("/tmp").join(fit))
        );
    }

    #[test]
    fn an_existing_folder_is_entered_not_answered() {
        let folder = Path::new("/tmp/work");
        let target = folder.join("sub");
        assert_eq!(
            save_target(folder, "sub", true, &|path| {
                assert_eq!(path, target);
                Existing::Folder
            }),
            SaveTarget::Enter(target)
        );
    }

    #[test]
    fn an_existing_file_or_a_link_to_one_asks_before_overwriting() {
        let folder = Path::new("/tmp/work");
        let target = folder.join("notes.txt");
        assert_eq!(
            save_target(folder, "notes.txt", true, &|_| Existing::File),
            SaveTarget::Overwrite(target.clone())
        );
        // A symlink to a file is a file: `inspect` follows it.
        assert_eq!(
            save_target(folder, "notes.txt", true, &|_| Existing::File),
            SaveTarget::Overwrite(target)
        );
    }

    #[test]
    fn something_else_with_the_name_is_refused() {
        assert_eq!(
            save_target(Path::new("/tmp"), "sock", true, &|_| Existing::Other),
            SaveTarget::Refused(TAKEN)
        );
    }

    #[test]
    fn a_folder_that_cannot_be_written_refuses_both_new_and_existing_files() {
        assert_eq!(
            save_target(Path::new("/usr"), "new.txt", false, &nothing_exists),
            SaveTarget::Refused(NOT_WRITABLE)
        );
        assert_eq!(
            save_target(Path::new("/usr"), "existing.txt", false, &|_| {
                Existing::File
            }),
            SaveTarget::Refused(NOT_WRITABLE)
        );
        // A folder is still enterable, even from a place you cannot write.
        assert_eq!(
            save_target(Path::new("/usr"), "bin", false, &|_| Existing::Folder),
            SaveTarget::Enter(PathBuf::from("/usr/bin"))
        );
    }

    #[test]
    fn a_missing_name_saves_in_the_folder() {
        assert_eq!(
            save_target(Path::new("/tmp/out"), "new file.txt", true, &nothing_exists),
            SaveTarget::Save(PathBuf::from("/tmp/out/new file.txt"))
        );
    }

    #[test]
    fn a_name_keeps_its_spaces_and_its_case() {
        assert_eq!(
            save_target(Path::new("/tmp"), "  report.TXT ", true, &nothing_exists),
            SaveTarget::Save(PathBuf::from("/tmp/  report.TXT "))
        );
    }

    #[test]
    fn inspect_follows_a_link_to_a_file_and_refuses_a_dangling_one() {
        let scratch = std::env::temp_dir().join(format!(
            "gimbal-save-inspect-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&scratch).expect("a scratch folder");
        let file = scratch.join("real.txt");
        std::fs::write(&file, b"x").expect("a file");
        std::os::unix::fs::symlink(&file, scratch.join("link.txt")).expect("a link");
        std::os::unix::fs::symlink(scratch.join("gone"), scratch.join("dangling")).expect("a link");
        std::fs::create_dir_all(scratch.join("folder")).expect("a folder");

        assert_eq!(inspect(&file), Existing::File);
        assert_eq!(inspect(&scratch.join("link.txt")), Existing::File);
        assert_eq!(inspect(&scratch.join("folder")), Existing::Folder);
        assert_eq!(inspect(&scratch.join("dangling")), Existing::Other);
        assert_eq!(inspect(&scratch.join("missing")), Existing::Missing);

        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn a_symlink_is_overwritten_and_a_dangling_link_is_refused() {
        let scratch = std::env::temp_dir().join(format!(
            "gimbal-save-link-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&scratch).expect("a scratch folder");
        let real = scratch.join("real.txt");
        std::fs::write(&real, b"x").expect("a file");
        let link = scratch.join("link.txt");
        std::os::unix::fs::symlink(&real, &link).expect("a link");
        assert_eq!(
            save_target(&scratch, "link.txt", true, &inspect),
            SaveTarget::Overwrite(link)
        );

        let dangling = scratch.join("dangling");
        std::os::unix::fs::symlink(scratch.join("gone"), &dangling).expect("a link");
        assert_eq!(
            save_target(&scratch, "dangling", true, &inspect),
            SaveTarget::Refused(TAKEN)
        );
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn savefiles_names_take_the_last_component_and_reject_an_invalid_one() {
        assert_eq!(
            save_files_names(&[b"/tmp/a.txt\0".to_vec(), b"b.png\0".to_vec()]),
            Ok(os(&["a.txt", "b.png"]))
        );
        // A missing NUL is tolerated, as the folder option tolerates one.
        assert_eq!(
            save_files_names(&[b"/tmp/a.txt".to_vec()]),
            Ok(os(&["a.txt"]))
        );
        assert_eq!(
            save_files_names(&[b"a/b/c.txt\0".to_vec()]),
            Ok(os(&["c.txt"]))
        );
        assert_eq!(save_files_names(&[b"\0".to_vec()]), Err(0));
        assert_eq!(save_files_names(&[b"/\0".to_vec()]), Err(0));
        assert_eq!(
            save_files_names(&[b"ok.txt\0".to_vec(), b"..\0".to_vec()]),
            Err(1)
        );
        assert_eq!(
            save_files_names(&[b"ok.txt\0".to_vec(), b"a/b/..\0".to_vec()]),
            Err(1)
        );
    }

    #[test]
    fn clashes_are_a_folder_hit_or_an_earlier_duplicate() {
        let folder = Path::new("/tmp");
        let names = os(&["a.txt", "b.txt", "a.txt", "c.txt"]);
        let lookup = exists("b.txt");
        assert_eq!(clashing(folder, &names, &lookup), vec![1, 2]);
    }

    #[test]
    fn keep_both_numbers_every_clash_and_replace_only_duplicates() {
        let folder = Path::new("/tmp");
        let names = os(&["a.txt", "b.txt", "a.txt"]);
        let lookup = exists("b.txt");
        assert_eq!(
            resolve_names(folder, &names, CollisionChoice::KeepBoth, &lookup),
            vec![
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/b (2).txt"),
                PathBuf::from("/tmp/a (2).txt"),
            ]
        );
        assert_eq!(
            resolve_names(folder, &names, CollisionChoice::Replace, &lookup),
            vec![
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/b.txt"),
                PathBuf::from("/tmp/a (2).txt"),
            ]
        );
    }

    #[test]
    fn two_clashes_with_the_same_name_get_two_numbers() {
        let folder = Path::new("/tmp");
        let names = os(&["a.txt", "a.txt", "a.txt"]);
        assert_eq!(
            resolve_names(folder, &names, CollisionChoice::Replace, &nothing_exists),
            vec![
                PathBuf::from("/tmp/a.txt"),
                PathBuf::from("/tmp/a (2).txt"),
                PathBuf::from("/tmp/a (3).txt"),
            ]
        );
        // Keep Both numbers the first one too, because the folder holds it.
        assert_eq!(
            resolve_names(folder, &names, CollisionChoice::KeepBoth, &exists("a.txt")),
            vec![
                PathBuf::from("/tmp/a (2).txt"),
                PathBuf::from("/tmp/a (3).txt"),
                PathBuf::from("/tmp/a (4).txt"),
            ]
        );
    }

    #[test]
    fn numbers_keep_the_extension_and_skip_names_already_reserved() {
        let folder = Path::new("/tmp");
        let names = os(&["a.tar.gz", "a.tar.gz"]);
        assert_eq!(
            resolve_names(folder, &names, CollisionChoice::Replace, &nothing_exists),
            vec![
                PathBuf::from("/tmp/a.tar.gz"),
                PathBuf::from("/tmp/a.tar (2).gz"),
            ]
        );
    }

    #[test]
    fn the_stem_selection_follows_rename() {
        assert_eq!(stem_selection("Untitled.odt"), 8);
        assert_eq!(stem_selection("archive.tar.gz"), 11);
        assert_eq!(stem_selection(".bashrc"), 7);
        assert_eq!(stem_selection("notes"), 5);
    }

    #[test]
    fn the_footer_lists_three_names_and_then_a_count() {
        assert_eq!(save_files_summary(&[]), "No files to save");
        assert_eq!(save_files_summary(&os(&["a.txt"])), "Saving 1 file: a.txt");
        assert_eq!(
            save_files_summary(&os(&["a.txt", "b.png", "c.pdf"])),
            "Saving 3 files: a.txt, b.png, c.pdf"
        );
        assert_eq!(
            save_files_summary(&os(&["a.txt", "b.png", "c.pdf", "d.doc", "e.odt"])),
            "Saving 5 files: a.txt, b.png, c.pdf and 2 more"
        );
    }

    #[test]
    fn the_collision_text_names_the_count_the_folder_and_the_names() {
        let folder = Path::new("/tmp/out");
        let names = os(&["a.txt", "b.png", "c.pdf"]);
        assert_eq!(
            collision_text(folder, &names, &[0, 1]),
            "2 of these files already exist in “/tmp/out”: a.txt, b.png"
        );
        assert_eq!(
            collision_text(folder, &names, &[2]),
            "1 of these files already exists in “/tmp/out”: c.pdf"
        );
    }

    #[test]
    fn a_field_stops_at_its_first_nul() {
        assert_eq!(field_bytes(b"/tmp/a\0junk"), b"/tmp/a");
        assert_eq!(field_bytes(b"/tmp/a"), b"/tmp/a");
        assert_eq!(field_bytes(b"\0/tmp"), b"");
    }
}
