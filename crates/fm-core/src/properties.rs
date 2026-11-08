// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure parts of the Properties window.
//!
//! The UI reads a file's GIO metadata and starts the existing folder-size
//! walker; this module only turns the numbers into words, so every rule can be
//! tested without a display or a filesystem.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Shown when a filesystem does not record a timestamp, or does not report the
/// attribute at all.
pub const BROKEN_LINK_PERMISSIONS: &str = "Not applicable to a broken link";
pub const UNKNOWN: &str = "Unknown";
/// Shown while the folder-size walker is running.
pub const CALCULATING: &str = "Calculating…";
/// The one note for a folder walk that hit entries it could not read.
pub const UNREADABLE_NOTE: &str = "Some items couldn't be read";

/// The words for one permission triple and the mode's other facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Permissions {
    pub owner: &'static str,
    pub group: &'static str,
    pub others: &'static str,
    /// True when a file carries at least one execute bit. Folders never say
    /// Executable, because their execute bits mean traversal.
    pub executable: bool,
    /// The low twelve mode bits, as `0644`.
    pub octal: String,
}

/// The permission words from a Unix mode. `is_dir` only decides whether the
/// execute bits are reported as "Executable"; the owner, group and other
/// words describe read and write, as the Properties window shows them.
pub fn permissions(mode: u32, is_dir: bool) -> Permissions {
    Permissions {
        owner: access_words((mode >> 6) & 0o7),
        group: access_words((mode >> 3) & 0o7),
        others: access_words(mode & 0o7),
        executable: !is_dir && mode & 0o111 != 0,
        octal: format!("{:04o}", mode & 0o7777),
    }
}

fn access_words(bits: u32) -> &'static str {
    if bits & 0o6 == 0o6 {
        "Read and write"
    } else if bits & 0o4 != 0 {
        "Read only"
    } else {
        "None"
    }
}

/// One item's facts for the multi-item summary and type tally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SummaryItem {
    pub is_dir: bool,
    /// GIO's content-type description, such as `PNG image`.
    pub type_description: String,
}

/// `N items: X files, Y folders`, with singular words for one. A selection of
/// only files or only folders mentions only that kind.
pub fn item_count(items: &[SummaryItem]) -> String {
    let files = items.iter().filter(|item| !item.is_dir).count();
    let folders = items.iter().filter(|item| item.is_dir).count();
    let count = items.len();
    let item_word = if count == 1 { "item" } else { "items" };

    let mut parts = Vec::new();
    if files > 0 {
        parts.push(count_text(files, "file"));
    }
    if folders > 0 {
        parts.push(count_text(folders, "folder"));
    }
    if parts.is_empty() {
        format!("0 {item_word}")
    } else {
        format!("{count} {item_word}: {}", parts.join(", "))
    }
}

fn count_text(count: usize, word: &str) -> String {
    if count == 1 {
        format!("1 {word}")
    } else {
        format!("{count} {word}s")
    }
}

/// The selection's types, largest group first and then alphabetically:
/// `3 PNG images, 1 PDF document`.
pub fn type_tally(items: &[SummaryItem]) -> String {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in items {
        let description = item.type_description.trim();
        let description = if description.is_empty() {
            UNKNOWN
        } else {
            description
        };
        *counts.entry(description.to_string()).or_default() += 1;
    }

    let mut groups: Vec<(String, usize)> = counts.into_iter().collect();
    groups.sort_by(|(left_name, left_count), (right_name, right_count)| {
        right_count
            .cmp(left_count)
            .then_with(|| left_name.to_lowercase().cmp(&right_name.to_lowercase()))
            .then_with(|| left_name.cmp(right_name))
    });

    groups
        .into_iter()
        .map(|(description, count)| format!("{count} {}", plural(&description, count)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn plural(word: &str, count: usize) -> String {
    if count == 1 {
        return word.to_string();
    }
    if word.ends_with('s') {
        format!("{word}es")
    } else {
        format!("{word}s")
    }
}

/// The folder every path shares, when they share one. Paths directly under
/// different folders have no common parent and get no Location row.
pub fn common_parent(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut parents = paths.iter().filter_map(|path| path.parent());
    let first = parents.next()?.to_path_buf();
    if parents.all(|parent| parent == first) {
        Some(first)
    } else {
        None
    }
}

/// A local date and time, or [`UNKNOWN`] when the filesystem has no value.
/// `seconds` is a Unix timestamp; GLib converts it to the live local zone.
pub fn date_text(seconds: Option<i64>) -> String {
    let Some(seconds) = seconds else {
        return UNKNOWN.to_string();
    };
    glib::DateTime::from_unix_local(seconds)
        .ok()
        .and_then(|date_time| date_time.format("%Y-%m-%d %H:%M:%S").ok())
        .map(|text| text.to_string())
        .unwrap_or_else(|| UNKNOWN.to_string())
}

/// The human size plus its exact byte count for one file.
pub fn file_size_text(bytes: u64) -> String {
    format!("{} ({} bytes)", crate::preview::format_size(bytes), bytes)
}

/// Whether a completed walk's partial reasons mean some entries could not be
/// read. Other partial causes keep their own wording from the walker.
pub fn has_unreadable_entries(reasons: &[crate::folder_size::PartialReason]) -> bool {
    reasons.iter().any(|reason| {
        matches!(
            reason,
            crate::folder_size::PartialReason::PermissionDenied
                | crate::folder_size::PartialReason::Unreadable
        )
    })
}

/// The folder size row once the walker finishes.
pub fn folder_size_text(bytes: u64, items: &str, unreadable: bool) -> String {
    let mut text = format!("{} — {items}", crate::preview::format_size(bytes));
    if unreadable {
        text.push('\n');
        text.push_str(UNREADABLE_NOTE);
    }
    text
}

/// A path's last component, for a missing or non-UTF-8 name.
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folder_size::PartialReason;

    fn item(is_dir: bool, description: &str) -> SummaryItem {
        SummaryItem {
            is_dir,
            type_description: description.to_string(),
        }
    }

    #[test]
    fn permission_words_for_the_six_reviewed_modes() {
        let file = |mode| permissions(mode, false);
        assert_eq!(file(0o644).owner, "Read and write");
        assert_eq!(file(0o644).group, "Read only");
        assert_eq!(file(0o644).others, "Read only");
        assert!(!file(0o644).executable);
        assert_eq!(file(0o644).octal, "0644");

        let six_hundred = file(0o600);
        assert_eq!(six_hundred.owner, "Read and write");
        assert_eq!(six_hundred.group, "None");
        assert_eq!(six_hundred.others, "None");
        assert!(!six_hundred.executable);
        assert_eq!(six_hundred.octal, "0600");

        let seven_fifty_five = file(0o755);
        assert_eq!(seven_fifty_five.owner, "Read and write");
        assert_eq!(seven_fifty_five.group, "Read only");
        assert_eq!(seven_fifty_five.others, "Read only");
        assert!(seven_fifty_five.executable);
        assert_eq!(seven_fifty_five.octal, "0755");

        let seven_hundred = file(0o700);
        assert_eq!(seven_hundred.owner, "Read and write");
        assert_eq!(seven_hundred.group, "None");
        assert_eq!(seven_hundred.others, "None");
        assert!(seven_hundred.executable);
        assert_eq!(seven_hundred.octal, "0700");

        let four_forty_four = file(0o444);
        assert_eq!(four_forty_four.owner, "Read only");
        assert_eq!(four_forty_four.group, "Read only");
        assert_eq!(four_forty_four.others, "Read only");
        assert!(!four_forty_four.executable);
        assert_eq!(four_forty_four.octal, "0444");

        let none = file(0o000);
        assert_eq!(none.owner, "None");
        assert_eq!(none.group, "None");
        assert_eq!(none.others, "None");
        assert!(!none.executable);
        assert_eq!(none.octal, "0000");
    }

    #[test]
    fn a_folder_never_says_executable_but_keeps_its_mode() {
        let folder = permissions(0o755, true);
        assert_eq!(folder.owner, "Read and write");
        assert_eq!(folder.group, "Read only");
        assert_eq!(folder.others, "Read only");
        assert!(!folder.executable);
        assert_eq!(folder.octal, "0755");

        let private = permissions(0o700, true);
        assert!(!private.executable);
        assert_eq!(private.octal, "0700");
    }

    #[test]
    fn the_multi_item_count_names_only_the_kinds_present() {
        assert_eq!(
            item_count(&[item(false, "Text file"), item(false, "PNG image")]),
            "2 items: 2 files"
        );
        assert_eq!(
            item_count(&[item(true, "Folder"), item(true, "Folder")]),
            "2 items: 2 folders"
        );
        assert_eq!(
            item_count(&[
                item(false, "PNG image"),
                item(false, "PNG image"),
                item(true, "Folder"),
            ]),
            "3 items: 2 files, 1 folder"
        );
        assert_eq!(item_count(&[item(false, "Text file")]), "1 item: 1 file");
        assert_eq!(item_count(&[]), "0 items");
    }

    #[test]
    fn the_type_tally_groups_plurals_and_orders_by_count_then_name() {
        let tally = type_tally(&[
            item(false, "PNG image"),
            item(false, "PDF document"),
            item(false, "PNG image"),
            item(false, "PNG image"),
        ]);
        assert_eq!(tally, "3 PNG images, 1 PDF document");

        let folders = type_tally(&[item(true, "Folder"), item(false, "PNG image")]);
        assert_eq!(folders, "1 Folder, 1 PNG image");

        assert_eq!(type_tally(&[item(false, "Text file")]), "1 Text file");
        assert_eq!(type_tally(&[]), "");
        assert_eq!(type_tally(&[item(false, "")]), "1 Unknown");
    }

    #[test]
    fn the_common_parent_is_only_reported_when_there_is_one() {
        let same = common_parent(&[
            PathBuf::from("/tmp/a/one.txt"),
            PathBuf::from("/tmp/a/two.txt"),
        ]);
        assert_eq!(same, Some(PathBuf::from("/tmp/a")));

        let different = common_parent(&[
            PathBuf::from("/tmp/a/one.txt"),
            PathBuf::from("/tmp/b/two.txt"),
        ]);
        assert_eq!(different, None);
        assert_eq!(common_parent(&[]), None);
    }

    #[test]
    fn a_missing_time_attribute_reads_unknown() {
        assert_eq!(date_text(None), UNKNOWN);
        let known = date_text(Some(1_700_000_000));
        assert_ne!(known, UNKNOWN);
        assert_eq!(known.len(), "2023-11-14 22:13:20".len());
        assert!(known.contains('-'));
        assert!(known.contains(':'));
    }

    #[test]
    fn a_folder_size_note_names_unreadable_entries_only_when_they_happened() {
        assert!(has_unreadable_entries(&[PartialReason::Unreadable]));
        assert!(has_unreadable_entries(&[PartialReason::PermissionDenied]));
        assert!(!has_unreadable_entries(&[PartialReason::TimeLimit]));
        assert!(!has_unreadable_entries(&[]));

        let text = folder_size_text(4_904, "12 files, 3 folders", true);
        assert!(text.starts_with("4.8 KiB — 12 files, 3 folders"));
        assert!(text.ends_with(UNREADABLE_NOTE));
        assert!(!folder_size_text(4_904, "12 files, 3 folders", false).contains(UNREADABLE_NOTE));
    }

    #[test]
    fn a_file_size_shows_the_human_and_exact_forms() {
        assert_eq!(file_size_text(4_904), "4.8 KiB (4904 bytes)");
        assert_eq!(file_size_text(12), "12 B (12 bytes)");
    }
}
