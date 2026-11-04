// SPDX-License-Identifier: GPL-3.0-or-later
//! The right-click menus, built as pure lists of registry commands.
//!
//! Every entry names a command from `fm-core`'s catalogue. The UI only turns
//! these values into buttons and runs the command, so a menu can never drift
//! away from the hotkeys.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// What the UI does when an entry is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuAction {
    /// Dispatch this command with these arguments.
    Command(Vec<(String, String)>),
    /// Open the rename popover for one item.
    Rename { path: PathBuf },
    /// Open the new-folder popover in this folder.
    NewFolder { folder: PathBuf },
    /// Choose a folder, then dispatch `command` with these sources and `to=`.
    ChooseFolder {
        command: &'static str,
        paths: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItem {
    pub label: String,
    pub command: &'static str,
    pub enabled: bool,
    pub action: MenuAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub path: PathBuf,
    pub is_dir: bool,
}

fn path_args(paths: &[PathBuf]) -> Vec<(String, String)> {
    paths
        .iter()
        .map(|path| ("paths".to_string(), path.to_string_lossy().into_owned()))
        .collect()
}

fn item(label: &str, command: &'static str, action: MenuAction) -> MenuItem {
    MenuItem {
        label: label.to_string(),
        command,
        enabled: true,
        action,
    }
}

/// The menu for a right-click on selected items.
pub fn item_menu(items: &[ItemInfo]) -> Vec<MenuItem> {
    if items.is_empty() {
        return Vec::new();
    }

    let paths: Vec<PathBuf> = items.iter().map(|item| item.path.clone()).collect();
    let single = items.len() == 1;
    let single_folder = single.then(|| items.first().is_some_and(|item| item.is_dir));

    let mut menu = vec![item(
        "Open",
        "file.open",
        MenuAction::Command(path_args(&paths)),
    )];

    if items.len() >= 2 && items.iter().all(|item| item.is_dir) {
        menu.push(item(
            "Open in New Group",
            "group.open",
            MenuAction::Command(path_args(&paths)),
        ));
    }

    if single_folder == Some(true) {
        let folder = items[0].path.to_string_lossy().into_owned();
        menu.push(item(
            "Open in New Tab",
            "tab.new",
            MenuAction::Command(vec![("path".to_string(), folder)]),
        ));
    }

    if single {
        menu.push(item(
            "Rename…",
            "file.rename",
            MenuAction::Rename {
                path: items[0].path.clone(),
            },
        ));
    }

    menu.push(item(
        "Cut",
        "file.cut",
        MenuAction::Command(path_args(&paths)),
    ));
    menu.push(item(
        "Copy",
        "file.copy-to-clipboard",
        MenuAction::Command(path_args(&paths)),
    ));
    menu.push(item(
        "Copy To…",
        "file.copy",
        MenuAction::ChooseFolder {
            command: "file.copy",
            paths: paths.clone(),
        },
    ));
    menu.push(item(
        "Move To…",
        "file.move",
        MenuAction::ChooseFolder {
            command: "file.move",
            paths: paths.clone(),
        },
    ));
    menu.push(item(
        "Move to Trash",
        "file.trash",
        MenuAction::Command(path_args(&paths)),
    ));

    if single_folder == Some(true) {
        let folder = items[0].path.to_string_lossy().into_owned();
        menu.push(item(
            "Pin",
            "pins.add",
            MenuAction::Command(vec![("path".to_string(), folder)]),
        ));
    }

    menu
}

/// The menu for a right-click on empty grid space.
pub fn background_menu(
    folder: &Path,
    undo_summary: Option<&str>,
    baobab: bool,
    clipboard_has_files: bool,
) -> Vec<MenuItem> {
    let folder_text = folder.to_string_lossy().into_owned();
    let mut menu = Vec::new();

    if let Some(summary) = undo_summary {
        menu.push(item(
            &format!("Undo {summary}"),
            "file.undo",
            MenuAction::Command(Vec::new()),
        ));
    }

    menu.push(item(
        "New Folder…",
        "file.new-folder",
        MenuAction::NewFolder {
            folder: folder.to_path_buf(),
        },
    ));

    let mut paste = item(
        "Paste",
        "file.paste",
        MenuAction::Command(vec![("in".to_string(), folder_text.clone())]),
    );
    paste.enabled = clipboard_has_files;
    menu.push(paste);

    menu.push(item(
        "Pin This Folder",
        "pins.add",
        MenuAction::Command(vec![("path".to_string(), folder_text.clone())]),
    ));
    menu.push(item(
        "Refresh",
        "nav.refresh",
        MenuAction::Command(Vec::new()),
    ));

    let mut usage = item(
        "Disk Usage",
        "app.disk-usage",
        MenuAction::Command(vec![("path".to_string(), folder_text)]),
    );
    usage.enabled = baobab;
    menu.push(usage);

    menu
}

/// Find one executable on a `PATH`-shaped string. Used for `baobab`, which the
/// Disk Usage entry runs when it is installed.
pub fn find_on_path(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    let path = path?;
    for directory in path.to_string_lossy().split(':') {
        if directory.is_empty() {
            continue;
        }
        let candidate = Path::new(directory).join(name);
        let Ok(metadata) = std::fs::metadata(&candidate) else {
            continue;
        };
        if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm_core::command::COMMANDS;

    fn file(name: &str) -> ItemInfo {
        ItemInfo {
            path: PathBuf::from(format!("/tmp/{name}")),
            is_dir: false,
        }
    }

    fn folder(name: &str) -> ItemInfo {
        ItemInfo {
            path: PathBuf::from(format!("/tmp/{name}")),
            is_dir: true,
        }
    }

    fn labels(menu: &[MenuItem]) -> Vec<&str> {
        menu.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn one_file_has_no_folder_only_entries() {
        let menu = item_menu(&[file("a.txt")]);
        assert_eq!(
            labels(&menu),
            [
                "Open",
                "Rename…",
                "Cut",
                "Copy",
                "Copy To…",
                "Move To…",
                "Move to Trash",
            ]
        );
        assert!(menu.iter().all(|item| item.enabled));
    }

    #[test]
    fn several_files_cannot_be_renamed_or_pinned() {
        let menu = item_menu(&[file("a.txt"), file("b.txt")]);
        let labels = labels(&menu);
        assert!(!labels.contains(&"Rename…"));
        assert!(!labels.contains(&"Open in New Tab"));
        assert!(!labels.contains(&"Pin"));
        assert!(labels.contains(&"Move to Trash"));
    }

    #[test]
    fn one_folder_gets_open_in_new_tab_and_pin() {
        let menu = item_menu(&[folder("docs")]);
        let labels = labels(&menu);
        assert!(labels.contains(&"Open in New Tab"));
        assert!(labels.contains(&"Pin"));
        let new_tab = menu.iter().find(|item| item.command == "tab.new").unwrap();
        assert_eq!(
            new_tab.action,
            MenuAction::Command(vec![("path".to_string(), "/tmp/docs".to_string())])
        );
    }

    #[test]
    fn several_folders_get_open_in_new_group_after_open_in_view_order() {
        let menu = item_menu(&[folder("a"), folder("b"), folder("c")]);
        assert_eq!(labels(&menu)[..2], ["Open", "Open in New Group"]);
        let entry = menu
            .iter()
            .find(|item| item.command == "group.open")
            .unwrap();
        assert_eq!(
            entry.action,
            MenuAction::Command(vec![
                ("paths".to_string(), "/tmp/a".to_string()),
                ("paths".to_string(), "/tmp/b".to_string()),
                ("paths".to_string(), "/tmp/c".to_string()),
            ])
        );
    }

    #[test]
    fn a_file_in_the_selection_has_no_open_in_new_group() {
        let menu = item_menu(&[folder("docs"), file("a.txt"), folder("more")]);
        assert!(!labels(&menu).contains(&"Open in New Group"));
    }

    #[test]
    fn one_folder_alone_has_no_open_in_new_group() {
        let menu = item_menu(&[folder("docs")]);
        assert!(!labels(&menu).contains(&"Open in New Group"));
    }

    #[test]
    fn a_mix_has_no_single_item_entries() {
        let menu = item_menu(&[file("a.txt"), folder("docs")]);
        let labels = labels(&menu);
        assert!(!labels.contains(&"Rename…"));
        assert!(!labels.contains(&"Open in New Tab"));
        assert!(!labels.contains(&"Pin"));
        assert!(labels.contains(&"Cut"));
    }

    #[test]
    fn empty_space_gets_the_background_menu() {
        let menu = background_menu(Path::new("/tmp"), None, true, false);
        assert_eq!(
            labels(&menu),
            [
                "New Folder…",
                "Paste",
                "Pin This Folder",
                "Refresh",
                "Disk Usage",
            ]
        );
        assert!(
            !menu[1].enabled,
            "paste is insensitive with no clipboard files"
        );
        assert!(menu[4].enabled);
        assert!(
            menu.iter()
                .all(|item| COMMANDS.iter().any(|command| command.name == item.command)),
            "every entry is a registry command"
        );
    }

    #[test]
    fn undo_is_first_when_there_is_something_to_undo() {
        let menu = background_menu(Path::new("/tmp"), Some("restored 2 items"), true, true);
        assert_eq!(menu[0].label, "Undo restored 2 items");
        assert_eq!(menu[0].command, "file.undo");
        assert!(menu[1].enabled);
    }

    #[test]
    fn disk_usage_is_insensitive_when_baobab_is_missing() {
        let menu = background_menu(Path::new("/tmp"), None, false, true);
        let usage = menu
            .iter()
            .find(|item| item.command == "app.disk-usage")
            .unwrap();
        assert!(!usage.enabled);
    }

    #[test]
    fn every_entry_is_a_registry_command() {
        let menus = [
            item_menu(&[file("a.txt")]),
            item_menu(&[folder("docs")]),
            item_menu(&[file("a.txt"), folder("docs")]),
            background_menu(Path::new("/tmp"), Some("restored 1 item"), true, true),
        ];
        for menu in menus {
            for item in menu {
                assert!(
                    COMMANDS.iter().any(|command| command.name == item.command),
                    "{} is not in the catalogue",
                    item.command
                );
            }
        }
    }
}
