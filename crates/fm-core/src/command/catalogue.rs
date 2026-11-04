// SPDX-License-Identifier: GPL-3.0-or-later
//! The V1 catalogue. This table is the single source of truth: menus,
//! hotkeys, the agent socket and the CLI all read it, so a command exists
//! exactly when it is listed here.

use super::{ArgKind, ArgSpec, CommandSpec, ContextSource, Effect, Presence};

const SIDES: &[&str] = &["left", "right"];
const SECTIONS: &[&str] = &["recent", "devices", "remote"];
const SORT_BY: &[&str] = &["name", "modified", "size", "type"];
const SORT_ORDER: &[&str] = &["ascending", "descending"];
const SORT_SCOPE: &[&str] = &["folder", "global"];

const PATH_FROM_CURRENT_FOLDER: ArgSpec = ArgSpec {
    name: "path",
    kind: ArgKind::Path,
    presence: Presence::Context(ContextSource::CurrentFolder),
    default: None,
};

const OPTIONAL_FOLDER: ArgSpec = ArgSpec {
    name: "path",
    kind: ArgKind::Path,
    presence: Presence::Optional,
    default: None,
};

const PATHS_FROM_SELECTION: ArgSpec = ArgSpec {
    name: "paths",
    kind: ArgKind::Paths,
    presence: Presence::Context(ContextSource::Selection),
    default: None,
};

const REQUIRED_PATH: ArgSpec = ArgSpec {
    name: "path",
    kind: ArgKind::Path,
    presence: Presence::Required,
    default: None,
};

const REQUIRED_NAME: ArgSpec = ArgSpec {
    name: "name",
    kind: ArgKind::Text,
    presence: Presence::Required,
    default: None,
};

const REQUIRED_INDEX: ArgSpec = ArgSpec {
    name: "index",
    kind: ArgKind::Int {
        min: 0,
        max: i64::MAX,
    },
    presence: Presence::Required,
    default: None,
};

const REQUIRED_FROM: ArgSpec = ArgSpec {
    name: "from",
    kind: ArgKind::Int {
        min: 0,
        max: i64::MAX,
    },
    presence: Presence::Required,
    default: None,
};

const REQUIRED_TO_INDEX: ArgSpec = ArgSpec {
    name: "to",
    kind: ArgKind::Int {
        min: 0,
        max: i64::MAX,
    },
    presence: Presence::Required,
    default: None,
};

const REQUIRED_DESTINATION: ArgSpec = ArgSpec {
    name: "to",
    kind: ArgKind::Path,
    presence: Presence::Required,
    default: None,
};

const TRASH_ITEMS: ArgSpec = ArgSpec {
    name: "items",
    kind: ArgKind::TrashItems,
    presence: Presence::Required,
    default: None,
};

/// The V1 catalogue, in the order menus and `gimbal-files-cli commands` show
/// it. Do not add or drop commands here without Kyle's review.
pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        name: "app.quit",
        summary: "Quit Gimbal Files.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "app.disk-usage",
        summary: "Show a folder's disk usage in the system analyser.",
        effect: Effect::Launch,
        undo: false,
        arguments: &[PATH_FROM_CURRENT_FOLDER],
    },
    CommandSpec {
        name: "window.new",
        summary: "Open a new window, optionally at a folder.",
        effect: Effect::View,
        undo: false,
        arguments: &[OPTIONAL_FOLDER],
    },
    CommandSpec {
        name: "window.close",
        summary: "Close the current window.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "tab.new",
        summary: "Open a new tab, optionally at a folder.",
        effect: Effect::View,
        undo: false,
        arguments: &[OPTIONAL_FOLDER],
    },
    CommandSpec {
        name: "tab.close",
        summary: "Close the current tab.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "tab.close-selected",
        summary: "Close the selected tabs.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "tab.focus",
        summary: "Focus the tab at an index.",
        effect: Effect::View,
        undo: false,
        arguments: &[REQUIRED_INDEX],
    },
    CommandSpec {
        name: "tab.move",
        summary: "Move a tab to another index.",
        effect: Effect::View,
        undo: false,
        arguments: &[REQUIRED_FROM, REQUIRED_TO_INDEX],
    },
    CommandSpec {
        name: "tab.open-in-new-window",
        summary: "Open the current tab in a new window.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "group.split",
        summary: "Split the selected tabs into a tiled group.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "group.close",
        summary: "Close the current group.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "group.rename",
        summary: "Rename the current group.",
        effect: Effect::View,
        undo: false,
        arguments: &[REQUIRED_NAME],
    },
    CommandSpec {
        name: "group.add-tab",
        summary: "Add the current tab to a group.",
        effect: Effect::View,
        undo: false,
        arguments: &[ArgSpec {
            name: "group",
            kind: ArgKind::Int {
                min: 0,
                max: i64::MAX,
            },
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "nav.go",
        summary: "Go to a folder.",
        effect: Effect::View,
        undo: false,
        arguments: &[REQUIRED_PATH],
    },
    CommandSpec {
        name: "nav.back",
        summary: "Go back in this tab's history.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "nav.forward",
        summary: "Go forward in this tab's history.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "nav.up",
        summary: "Go to the parent folder.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "nav.refresh",
        summary: "Reload the current folder.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "nav.focus-path",
        summary: "Focus the path bar.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "search.open",
        summary: "Open search, optionally with a query.",
        effect: Effect::View,
        undo: false,
        arguments: &[ArgSpec {
            name: "query",
            kind: ArgKind::Text,
            presence: Presence::Optional,
            default: None,
        }],
    },
    CommandSpec {
        name: "selection.all",
        summary: "Select every item.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "selection.none",
        summary: "Clear the selection.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "file.open",
        summary: "Open files with their default application.",
        effect: Effect::Launch,
        undo: false,
        arguments: &[PATHS_FROM_SELECTION],
    },
    CommandSpec {
        name: "file.trash",
        summary: "Move files to the trash.",
        effect: Effect::Files,
        undo: true,
        arguments: &[PATHS_FROM_SELECTION],
    },
    CommandSpec {
        name: "file.copy",
        summary: "Copy files into a folder.",
        effect: Effect::Files,
        undo: true,
        arguments: &[PATHS_FROM_SELECTION, REQUIRED_DESTINATION],
    },
    CommandSpec {
        name: "file.move",
        summary: "Move files into a folder.",
        effect: Effect::Files,
        undo: true,
        arguments: &[PATHS_FROM_SELECTION, REQUIRED_DESTINATION],
    },
    CommandSpec {
        name: "file.rename",
        summary: "Rename one file in place.",
        effect: Effect::Files,
        undo: true,
        arguments: &[
            ArgSpec {
                name: "path",
                kind: ArgKind::Path,
                presence: Presence::Context(ContextSource::Selection),
                default: None,
            },
            REQUIRED_NAME,
        ],
    },
    CommandSpec {
        name: "file.new-folder",
        summary: "Create a new folder.",
        effect: Effect::Files,
        undo: true,
        arguments: &[
            REQUIRED_NAME,
            ArgSpec {
                name: "in",
                kind: ArgKind::Path,
                presence: Presence::Context(ContextSource::CurrentFolder),
                default: None,
            },
        ],
    },
    CommandSpec {
        name: "file.undo",
        summary: "Undo the last file operation.",
        effect: Effect::Files,
        undo: false,
        arguments: &[],
    },
    // Clipboard commands (Kyle approved adding them, Nov 2). Cut and copy only
    // put the files on the clipboard; paste is the file operation.
    CommandSpec {
        name: "file.cut",
        summary: "Put files on the clipboard to move them.",
        effect: Effect::View,
        undo: false,
        arguments: &[PATHS_FROM_SELECTION],
    },
    CommandSpec {
        name: "file.copy-to-clipboard",
        summary: "Put files on the clipboard to copy them.",
        effect: Effect::View,
        undo: false,
        arguments: &[PATHS_FROM_SELECTION],
    },
    CommandSpec {
        name: "file.paste",
        summary: "Copy or move the clipboard's files into a folder.",
        effect: Effect::Files,
        undo: true,
        arguments: &[ArgSpec {
            name: "in",
            kind: ArgKind::Path,
            presence: Presence::Context(ContextSource::CurrentFolder),
            default: None,
        }],
    },
    CommandSpec {
        name: "trash.open",
        summary: "Open the trash.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "trash.restore",
        summary: "Restore trash items to their original locations.",
        effect: Effect::Files,
        undo: true,
        arguments: &[TRASH_ITEMS],
    },
    // Kyle approved permanent emptying on Nov 2. It stays an explicitly
    // clicked UI operation: the registry refuses every other caller.
    CommandSpec {
        name: "trash.empty",
        summary: "Permanently delete the confirmed trash snapshot.",
        effect: Effect::Files,
        undo: false,
        arguments: &[],
    },
    // Kyle approved permanent deletion of selected Trash items on Nov 3.
    // The items are required and validated; this command never means the whole
    // Trash. It stays an explicitly clicked UI operation: the registry refuses
    // every other caller.
    CommandSpec {
        name: "trash.delete-permanently",
        summary: "Permanently delete the selected Trash items.",
        effect: Effect::Files,
        undo: false,
        arguments: &[TRASH_ITEMS],
    },
    CommandSpec {
        name: "preview.edit",
        summary: "Edit the file shown in the preview.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "preview.discard",
        summary: "Discard edits to the previewed file.",
        effect: Effect::View,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "preview.save",
        summary: "Save edits to the previewed file.",
        effect: Effect::Files,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "preview.save-as",
        summary: "Save the previewed file to a new path.",
        effect: Effect::Files,
        undo: false,
        arguments: &[REQUIRED_PATH],
    },
    CommandSpec {
        name: "view.set-scale",
        summary: "Set the interface scale.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[ArgSpec {
            name: "scale",
            kind: ArgKind::Number {
                min: 50,
                max: 100,
                step: Some(5),
            },
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "view.zoom-in",
        summary: "Increase the interface scale by one step.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "view.zoom-out",
        summary: "Decrease the interface scale by one step.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[],
    },
    CommandSpec {
        name: "view.set-opacity",
        summary: "Set the window background opacity.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[ArgSpec {
            name: "opacity",
            kind: ArgKind::Number {
                min: 0,
                max: 100,
                step: None,
            },
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "view.toggle-sidebar",
        summary: "Show or hide a sidebar.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[ArgSpec {
            name: "side",
            kind: ArgKind::Choice(SIDES),
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "view.toggle-section",
        summary: "Expand or collapse a sidebar section.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[ArgSpec {
            name: "section",
            kind: ArgKind::Choice(SECTIONS),
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "view.set-sort",
        summary: "Set the sort order.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[
            ArgSpec {
                name: "by",
                kind: ArgKind::Choice(SORT_BY),
                presence: Presence::Required,
                default: None,
            },
            ArgSpec {
                name: "order",
                kind: ArgKind::Choice(SORT_ORDER),
                presence: Presence::Required,
                default: None,
            },
            ArgSpec {
                name: "scope",
                kind: ArgKind::Choice(SORT_SCOPE),
                presence: Presence::Optional,
                default: Some("folder"),
            },
        ],
    },
    // Kyle approved removing a folder's sort exception on Nov 4. It acts on
    // the focused tab's folder and falls back to the global sort.
    CommandSpec {
        name: "view.clear-folder-sort",
        summary: "Clear this folder's sort rule and use the global one.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[],
    },
    // Kyle approved one Settings key for local automation on Nov 3. It is a
    // UI-only switch: the registry refuses Agent, Cli and Hotkey before any
    // handler or config write. The switch itself is the only caller.
    CommandSpec {
        name: "agent.set-enabled",
        summary: "Allow local scripts and AI agents to control Gimbal.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[ArgSpec {
            name: "enabled",
            kind: ArgKind::Bool,
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "pins.add",
        summary: "Pin a folder.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[PATH_FROM_CURRENT_FOLDER],
    },
    CommandSpec {
        name: "pins.remove",
        summary: "Unpin a folder.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[REQUIRED_PATH],
    },
    CommandSpec {
        name: "pins.move",
        summary: "Move a pinned folder to another position.",
        effect: Effect::Settings,
        undo: false,
        arguments: &[REQUIRED_FROM, REQUIRED_TO_INDEX],
    },
    CommandSpec {
        name: "drive.mount",
        summary: "Mount a drive.",
        effect: Effect::System,
        undo: false,
        arguments: &[ArgSpec {
            name: "device",
            kind: ArgKind::Text,
            presence: Presence::Required,
            default: None,
        }],
    },
    // Kyle approved both on Nov 4. They take the same opaque device id as
    // drive.mount and stay UI-only through the command's System effect.
    CommandSpec {
        name: "drive.unmount",
        summary: "Unmount a mounted drive.",
        effect: Effect::System,
        undo: false,
        arguments: &[ArgSpec {
            name: "device",
            kind: ArgKind::Text,
            presence: Presence::Required,
            default: None,
        }],
    },
    CommandSpec {
        name: "drive.eject",
        summary: "Eject a mounted drive.",
        effect: Effect::System,
        undo: false,
        arguments: &[ArgSpec {
            name: "device",
            kind: ArgKind::Text,
            presence: Presence::Required,
            default: None,
        }],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn drive_command(name: &str) -> &'static CommandSpec {
        COMMANDS
            .iter()
            .find(|spec| spec.name == name)
            .expect("the reviewed drive command")
    }

    #[test]
    fn unmount_and_eject_are_system_commands_with_the_opaque_device_argument() {
        for name in ["drive.unmount", "drive.eject"] {
            let spec = drive_command(name);
            assert_eq!(spec.effect, Effect::System);
            assert!(!spec.undo, "{name} has no undo");
            assert_eq!(spec.arguments.len(), 1);
            let argument = &spec.arguments[0];
            assert_eq!(argument.name, "device");
            assert!(matches!(argument.kind, ArgKind::Text));
            assert!(matches!(argument.presence, Presence::Required));
            assert!(!spec.summary.is_empty());
        }
    }
}
