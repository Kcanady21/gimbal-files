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
    /// The explanation shown when the entry is disabled, or on hover.
    pub tooltip: Option<&'static str>,
    /// A thin rule instead of a button. Separators carry no command.
    pub separator: bool,
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
        tooltip: None,
        separator: false,
    }
}

fn separator() -> MenuItem {
    MenuItem {
        label: String::new(),
        command: "",
        enabled: false,
        action: MenuAction::Command(Vec::new()),
        icon: None,
        tooltip: None,
        separator: true,
    }
}

/// The one explanation a missing Baobab shows on both Disk Usage entries.
pub const BAOBAB_MISSING: &str = "Install baobab to see what uses the space";

/// The menu for a right-click on selected items. `current_uid` is the user's
/// uid when it could be read; without it no administrator entry is offered.
#[cfg(test)]
pub fn item_menu(items: &[ItemInfo], current_uid: Option<u32>) -> Vec<MenuItem> {
    item_menu_in_scope(items, current_uid, false, true)
}

/// The same menu, with the tab's scope and whether Baobab is installed.
/// Administrator tabs never offer an administrator entry: the current location
/// already is one.
pub fn item_menu_in_scope(
    items: &[ItemInfo],
    current_uid: Option<u32>,
    in_admin: bool,
    baobab: bool,
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
            MenuAction::Command(vec![("path".to_string(), folder.clone())]),
        ));

        // P2af: Disk Usage is next to Properties, for exactly one folder. It
        // stays visible when Baobab is missing, with the reason on hover.
        let mut usage = item(
            "Disk Usage",
            "app.disk-usage",
            MenuAction::Command(vec![("path".to_string(), folder)]),
        );
        usage.enabled = baobab;
        usage.tooltip = (!baobab).then_some(BAOBAB_MISSING);
        menu.push(usage);
    }

    // Properties is always the last entry, for files and folders and any
    // selection size.
    menu.push(item(
        "Properties",
        "file.properties",
        MenuAction::Command(path_args(&paths)),
    ));

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

    // New Tab and New Window always open normal tabs and windows at this
    // folder, even inside an administrator tab (P2af). The handler enforces
    // that; the menu only names the ordinary commands.
    menu.push(item(
        "New Tab",
        "tab.new",
        MenuAction::Command(vec![("path".to_string(), folder_text.clone())]),
    ));
    menu.push(item(
        "New Window",
        "window.new",
        MenuAction::Command(vec![("path".to_string(), folder_text.clone())]),
    ));

    menu.push(separator());

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

    menu.push(separator());

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
        MenuAction::Command(vec![("path".to_string(), folder_text.clone())]),
    );
    usage.enabled = baobab;
    usage.tooltip = (!baobab).then_some(BAOBAB_MISSING);
    menu.push(usage);

    menu.push(item(
        "Properties",
        "file.properties",
        MenuAction::Command(vec![("paths".to_string(), folder_text)]),
    ));

    menu.push(separator());

    menu.push(item(
        "Settings…",
        "app.settings",
        MenuAction::Command(Vec::new()),
    ));

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
        menu.iter()
            .filter(|item| !item.separator)
            .map(|item| item.label.as_str())
            .collect()
    }

    /// Labels and separators in order, so the background menu's shape is one
    /// readable assertion.
    fn shape(menu: &[MenuItem]) -> Vec<String> {
        menu.iter()
            .map(|item| {
                if item.separator {
                    "|".to_string()
                } else {
                    item.label.clone()
                }
            })
            .collect()
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
                "Properties",
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
            !item_menu_in_scope(&[folder("root-docs")], Some(CURRENT_UID), true, true)
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
        let menu = item_menu_in_scope(&[unowned_file("root.txt")], Some(CURRENT_UID), true, true);
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
        assert!(labels.contains(&"Disk Usage"));
        assert_eq!(labels.last().copied(), Some("Properties"));
        let new_tab = menu.iter().find(|item| item.command == "tab.new").unwrap();
        assert_eq!(
            new_tab.action,
            MenuAction::Command(vec![("path".to_string(), "/tmp/docs".to_string())])
        );
    }

    #[test]
    fn properties_is_last_for_every_selection_and_disk_usage_needs_one_folder() {
        let selections = [
            vec![owned_file("a.txt")],
            vec![owned_file("a.txt"), owned_file("b.txt")],
            vec![folder("docs")],
            vec![folder("docs"), folder("more")],
            vec![owned_file("a.txt"), folder("docs")],
        ];
        for selection in selections {
            let menu = item_menu(&selection, Some(CURRENT_UID));
            assert_eq!(
                menu.last().map(|item| (item.label.as_str(), item.command)),
                Some(("Properties", "file.properties"))
            );
            let disk = menu
                .iter()
                .filter(|item| item.command == "app.disk-usage")
                .count();
            let expected = usize::from(selection.len() == 1 && selection[0].is_dir);
            assert_eq!(disk, expected, "{selection:?}");
        }
    }

    #[test]
    fn a_folder_disk_usage_entry_is_disabled_with_the_reason_when_baobab_is_missing() {
        let menu = item_menu_in_scope(&[folder("docs")], Some(CURRENT_UID), false, false);
        let usage = menu
            .iter()
            .find(|item| item.command == "app.disk-usage")
            .expect("Disk Usage stays visible");
        assert!(!usage.enabled);
        assert_eq!(usage.tooltip, Some(BAOBAB_MISSING));
        assert_eq!(
            usage.action,
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
    fn empty_space_gets_the_reviewed_background_menu_in_order() {
        let menu = background_menu(Path::new("/tmp"), None, true, false);
        assert_eq!(
            shape(&menu),
            [
                "New Tab",
                "New Window",
                "|",
                "New Folder…",
                "Paste",
                "|",
                "Pin This Folder",
                "Refresh",
                "Disk Usage",
                "Properties",
                "|",
                "Settings…",
            ]
        );
        let paste = menu
            .iter()
            .find(|item| item.command == "file.paste")
            .unwrap();
        assert!(
            !paste.enabled,
            "paste is insensitive with no clipboard files"
        );
        let usage = menu
            .iter()
            .find(|item| item.command == "app.disk-usage")
            .unwrap();
        assert!(usage.enabled);
        assert!(
            menu.iter()
                .filter(|item| !item.separator)
                .all(|item| COMMANDS.iter().any(|command| command.name == item.command)),
            "every entry is a registry command"
        );
    }

    #[test]
    fn new_tab_and_new_window_carry_this_folder_even_inside_an_admin_tab() {
        for in_admin in [false, true] {
            let menu =
                background_menu_in_scope(Path::new("/tmp/docs"), None, true, true, false, in_admin);
            for (command, label) in [("tab.new", "New Tab"), ("window.new", "New Window")] {
                let entry = menu
                    .iter()
                    .find(|item| item.command == command)
                    .unwrap_or_else(|| panic!("{label} inside an admin tab: {in_admin}"));
                assert_eq!(
                    entry.action,
                    MenuAction::Command(vec![("path".to_string(), "/tmp/docs".to_string())])
                );
                assert!(entry.enabled);
            }
            assert!(
                !menu.iter().any(|item| item.command == "tab.open-as-admin"),
                "New Tab and New Window are the ordinary commands"
            );
        }
    }

    #[test]
    fn undo_is_first_in_its_section_when_there_is_something_to_undo() {
        let menu = background_menu(Path::new("/tmp"), Some("restored 2 items"), true, true);
        assert_eq!(
            shape(&menu)[..5],
            [
                "New Tab",
                "New Window",
                "|",
                "Undo restored 2 items",
                "New Folder…"
            ]
        );
        let undo = menu
            .iter()
            .find(|item| item.command == "file.undo")
            .unwrap();
        assert!(undo.enabled);
    }

    #[test]
    fn disk_usage_stays_visible_and_explains_a_missing_baobab() {
        let menu = background_menu(Path::new("/tmp"), None, false, true);
        let usage = menu
            .iter()
            .find(|item| item.command == "app.disk-usage")
            .unwrap();
        assert!(!usage.enabled);
        assert_eq!(usage.tooltip, Some(BAOBAB_MISSING));
    }

    #[test]
    fn settings_is_the_last_background_entry_and_the_folder_properties_entry_is_last_before_it() {
        let menu = background_menu(Path::new("/tmp"), None, true, true);
        assert_eq!(shape(&menu).last().unwrap(), "Settings…");
        assert_eq!(menu.last().unwrap().command, "app.settings");
        let properties = menu
            .iter()
            .find(|item| item.command == "file.properties")
            .expect("the folder Properties entry");
        assert_eq!(properties.label, "Properties");
        assert_eq!(
            properties.action,
            MenuAction::Command(vec![("paths".to_string(), "/tmp".to_string())])
        );
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
            for item in menu.into_iter().filter(|item| !item.separator) {
                assert!(
                    COMMANDS.iter().any(|command| command.name == item.command),
                    "{} is not in the catalogue",
                    item.command
                );
            }
        }
    }
}
