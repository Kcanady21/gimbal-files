// SPDX-License-Identifier: GPL-3.0-or-later
//! The right-click menus, built as pure lists of registry commands.
//!
//! Every entry names a command from `fm-core`'s catalogue. The UI only turns
//! these values into buttons and runs the command, so a menu can never drift
//! away from the hotkeys.

use std::ffi::OsStr;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use fm_core::admin::needs_admin;

/// The lock shown on administrator entries and administrator tabs.
pub use crate::look::icons::ADMIN_LOCK_ICON;

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
    /// A symbolic icon name shown before the label.
    pub icon: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
    /// The target's owner, following a symlink. `None` means missing or broken.
    pub owner_uid: Option<u32>,
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
        icon: None,
    }
}

/// The menu for a right-click on selected items. `current_uid` is the user's
/// uid when it could be read; without it no administrator entry is offered.
#[cfg(test)]
pub fn item_menu(items: &[ItemInfo], current_uid: Option<u32>) -> Vec<MenuItem> {
    item_menu_in_scope(items, current_uid, false)
}

/// The same menu, with the tab's scope. Administrator tabs never offer an
/// administrator entry: the current location already is one.
pub fn item_menu_in_scope(
    items: &[ItemInfo],
    current_uid: Option<u32>,
    in_admin: bool,
) -> Vec<MenuItem> {
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

    // P2aa: Open With sits directly under Open, and only when every selected
    // item is a file. Folders open in Gimbal. The command has no `app`
    // argument here, so the window opens the submenu instead of launching.
    if !in_admin && items.iter().all(|item| item.is_file && !item.is_dir) {
        menu.push(item(
            "Open With",
            "file.open-with",
            MenuAction::Command(path_args(&paths)),
        ));
    }

    if !in_admin
        && single
        && items[0].is_file
        && !items[0].is_dir
        && items[0]
            .owner_uid
            .zip(current_uid)
            .is_some_and(|(owner, current)| needs_admin(owner, current))
    {
        let mut admin = item(
            "Open as Administrator",
            "file.open-as-admin",
            MenuAction::Command(path_args(&paths)),
        );
        admin.icon = Some(ADMIN_LOCK_ICON);
        menu.push(admin);
    }

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
            MenuAction::Command(vec![("path".to_string(), folder.clone())]),
        ));
        if !in_admin
            && items[0]
                .owner_uid
                .zip(current_uid)
                .is_some_and(|(owner, current)| needs_admin(owner, current))
        {
            let mut admin = item(
                "Open as Administrator",
                "tab.open-as-admin",
                MenuAction::Command(vec![("path".to_string(), folder)]),
            );
            admin.icon = Some(ADMIN_LOCK_ICON);
            menu.push(admin);
        }
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
        if in_admin {
            "Delete Permanently…"
        } else {
            "Move to Trash"
        },
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
#[cfg(test)]
pub fn background_menu(
    folder: &Path,
    undo_summary: Option<&str>,
    baobab: bool,
    clipboard_has_files: bool,
) -> Vec<MenuItem> {
    background_menu_in_scope(
        folder,
        undo_summary,
        baobab,
        clipboard_has_files,
        false,
        false,
    )
}

/// The same menu with the folder's ownership and the tab's scope.
pub fn background_menu_in_scope(
    folder: &Path,
    undo_summary: Option<&str>,
    baobab: bool,
    clipboard_has_files: bool,
    folder_needs_admin: bool,
    in_admin: bool,
) -> Vec<MenuItem> {
    let folder_text = folder.to_string_lossy().into_owned();
    let mut menu = Vec::new();

    if !in_admin && folder_needs_admin {
        let mut admin = item(
            "Open as Administrator",
            "tab.open-as-admin",
            MenuAction::Command(vec![("path".to_string(), folder_text.clone())]),
        );
        admin.icon = Some(ADMIN_LOCK_ICON);
        menu.push(admin);
    }

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

    const CURRENT_UID: u32 = 1000;

    fn owned_file(name: &str) -> ItemInfo {
        ItemInfo {
            path: PathBuf::from(format!("/tmp/{name}")),
            is_dir: false,
            is_file: true,
            owner_uid: Some(CURRENT_UID),
        }
    }

    fn unowned_file(name: &str) -> ItemInfo {
        ItemInfo {
            path: PathBuf::from(format!("/tmp/{name}")),
            is_dir: false,
            is_file: true,
            owner_uid: Some(0),
        }
    }

    fn folder(name: &str) -> ItemInfo {
        ItemInfo {
            path: PathBuf::from(format!("/tmp/{name}")),
            is_dir: true,
            is_file: false,
            owner_uid: Some(CURRENT_UID),
        }
    }

    fn labels(menu: &[MenuItem]) -> Vec<&str> {
        menu.iter().map(|item| item.label.as_str()).collect()
    }

    #[test]
    fn one_file_has_no_folder_only_entries() {
        let menu = item_menu(&[owned_file("a.txt")], Some(CURRENT_UID));
        assert_eq!(
            labels(&menu),
            [
                "Open",
                "Open With",
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
    fn open_with_sits_under_open_for_files_and_not_for_folders_or_mixes() {
        let one = item_menu(&[owned_file("a.txt")], Some(CURRENT_UID));
        assert_eq!(labels(&one)[..2], ["Open", "Open With"]);
        let entry = one
            .iter()
            .find(|item| item.command == "file.open-with")
            .expect("the Open With entry");
        assert_eq!(
            entry.action,
            MenuAction::Command(vec![("paths".to_string(), "/tmp/a.txt".to_string())])
        );

        let many = item_menu(
            &[owned_file("a.txt"), owned_file("b.txt")],
            Some(CURRENT_UID),
        );
        assert!(labels(&many).contains(&"Open With"));

        let folder_menu = item_menu(&[folder("docs")], Some(CURRENT_UID));
        assert!(!labels(&folder_menu).contains(&"Open With"));

        let mixed = item_menu(&[owned_file("a.txt"), folder("docs")], Some(CURRENT_UID));
        assert!(!labels(&mixed).contains(&"Open With"));
    }

    #[test]
    fn a_file_the_user_does_not_own_gets_open_as_administrator_after_open_with() {
        let menu = item_menu(&[unowned_file("root-owned.txt")], Some(CURRENT_UID));
        assert_eq!(
            labels(&menu)[..3],
            ["Open", "Open With", "Open as Administrator"]
        );
        let admin = menu
            .iter()
            .find(|item| item.command == "file.open-as-admin")
            .expect("the administrator entry");
        assert_eq!(admin.icon, Some(ADMIN_LOCK_ICON));
        assert_eq!(
            admin.action,
            MenuAction::Command(vec![(
                "paths".to_string(),
                "/tmp/root-owned.txt".to_string()
            )])
        );
    }

    #[test]
    fn a_file_the_user_owns_has_no_administrator_entry() {
        let menu = item_menu(&[owned_file("mine.txt")], Some(CURRENT_UID));
        assert!(!labels(&menu).contains(&"Open as Administrator"));
    }

    #[test]
    fn a_folder_or_several_files_have_no_administrator_entry() {
        let folder_menu = item_menu(&[folder("docs")], Some(CURRENT_UID));
        assert!(!labels(&folder_menu).contains(&"Open as Administrator"));

        let mut root_folder = folder("root-docs");
        root_folder.owner_uid = Some(0);
        let owned_folder_menu = item_menu(&[root_folder], Some(CURRENT_UID));
        let entry = owned_folder_menu
            .iter()
            .find(|item| item.command == "tab.open-as-admin")
            .expect("a folder the user does not own opens as administrator");
        assert_eq!(entry.icon, Some(ADMIN_LOCK_ICON));
        assert_eq!(
            entry.action,
            MenuAction::Command(vec![("path".to_string(), "/tmp/root-docs".to_string())])
        );
        assert!(
            !item_menu_in_scope(&[folder("root-docs")], Some(CURRENT_UID), true)
                .iter()
                .any(|item| item.command == "tab.open-as-admin"),
            "no administrator entry inside an administrator tab"
        );

        let many = item_menu(
            &[unowned_file("a.txt"), unowned_file("b.txt")],
            Some(CURRENT_UID),
        );
        assert!(!labels(&many).contains(&"Open as Administrator"));
    }

    #[test]
    fn an_unknown_current_uid_does_not_offer_an_administrator_entry() {
        let menu = item_menu(&[unowned_file("root-owned.txt")], None);
        assert!(!labels(&menu).contains(&"Open as Administrator"));
    }

    #[test]
    fn a_broken_link_has_no_administrator_entry() {
        let broken = ItemInfo {
            path: PathBuf::from("/tmp/broken-link"),
            is_dir: false,
            is_file: false,
            owner_uid: None,
        };
        let menu = item_menu(&[broken], Some(CURRENT_UID));
        assert!(!labels(&menu).contains(&"Open as Administrator"));
    }

    #[test]
    fn an_administrator_tab_hides_open_with_and_the_admin_entry() {
        let menu = item_menu_in_scope(&[unowned_file("root.txt")], Some(CURRENT_UID), true);
        assert!(!labels(&menu).contains(&"Open With"));
        assert!(!labels(&menu).contains(&"Open as Administrator"));
    }

    #[test]
    fn several_files_cannot_be_renamed_or_pinned() {
        let menu = item_menu(
            &[owned_file("a.txt"), owned_file("b.txt")],
            Some(CURRENT_UID),
        );
        let labels = labels(&menu);
        assert!(!labels.contains(&"Rename…"));
        assert!(!labels.contains(&"Open in New Tab"));
        assert!(!labels.contains(&"Pin"));
        assert!(labels.contains(&"Move to Trash"));
    }

    #[test]
    fn one_folder_gets_open_in_new_tab_and_pin() {
        let menu = item_menu(&[folder("docs")], Some(CURRENT_UID));
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
        let menu = item_menu(&[folder("a"), folder("b"), folder("c")], Some(CURRENT_UID));
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
        let menu = item_menu(
            &[folder("docs"), owned_file("a.txt"), folder("more")],
            Some(CURRENT_UID),
        );
        assert!(!labels(&menu).contains(&"Open in New Group"));
    }

    #[test]
    fn one_folder_alone_has_no_open_in_new_group() {
        let menu = item_menu(&[folder("docs")], Some(CURRENT_UID));
        assert!(!labels(&menu).contains(&"Open in New Group"));
    }

    #[test]
    fn a_mix_has_no_single_item_entries() {
        let menu = item_menu(&[owned_file("a.txt"), folder("docs")], Some(CURRENT_UID));
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
            item_menu(&[owned_file("a.txt")], Some(CURRENT_UID)),
            item_menu(&[unowned_file("root-owned.txt")], Some(CURRENT_UID)),
            item_menu(&[folder("docs")], Some(CURRENT_UID)),
            item_menu(&[owned_file("a.txt"), folder("docs")], Some(CURRENT_UID)),
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
