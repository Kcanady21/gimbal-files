// SPDX-License-Identifier: GPL-3.0-or-later
//! Command-line arguments as tabs.
//!
//! A folder argument opens that folder. File arguments are grouped by their
//! parent folder, so several files in one folder open one tab with all of
//! them selected (P2l item 5). The grouping is pure; the caller supplies the
//! folder test so it can be unit-tested without touching the disk.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// One tab's worth of a command line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Argument {
    Folder(PathBuf),
    Files {
        folder: PathBuf,
        names: Vec<OsString>,
    },
}

/// Group paths in command-line order. Consecutive files from the same folder
/// join one `Files` group; a folder argument is its own group.
pub fn group_arguments(paths: &[PathBuf], is_dir: &dyn Fn(&Path) -> bool) -> Vec<Argument> {
    let mut groups: Vec<Argument> = Vec::new();
    for path in paths {
        if is_dir(path) {
            groups.push(Argument::Folder(path.clone()));
            continue;
        }
        let Some(name) = path.file_name() else {
            groups.push(Argument::Folder(path.clone()));
            continue;
        };
        let folder = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
        match groups.last_mut() {
            Some(Argument::Files {
                folder: existing,
                names,
            }) if *existing == folder => names.push(name.to_os_string()),
            _ => groups.push(Argument::Files {
                folder,
                names: vec![name.to_os_string()],
            }),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn no_folders(_: &Path) -> bool {
        false
    }

    #[test]
    fn one_file_opens_its_parent_with_that_name() {
        let groups = group_arguments(&[file("/tmp/a/one.txt")], &no_folders);
        assert_eq!(
            groups,
            vec![Argument::Files {
                folder: PathBuf::from("/tmp/a"),
                names: vec![OsString::from("one.txt")],
            }]
        );
    }

    #[test]
    fn several_files_in_one_folder_make_one_group() {
        let groups = group_arguments(
            &[file("/tmp/a/one.txt"), file("/tmp/a/two.txt")],
            &no_folders,
        );
        assert_eq!(
            groups,
            vec![Argument::Files {
                folder: PathBuf::from("/tmp/a"),
                names: vec![OsString::from("one.txt"), OsString::from("two.txt")],
            }]
        );
    }

    #[test]
    fn files_in_two_folders_make_two_groups_in_order() {
        let groups = group_arguments(
            &[
                file("/tmp/a/one.txt"),
                file("/tmp/b/two.txt"),
                file("/tmp/a/three.txt"),
            ],
            &no_folders,
        );
        assert_eq!(
            groups,
            vec![
                Argument::Files {
                    folder: PathBuf::from("/tmp/a"),
                    names: vec![OsString::from("one.txt")],
                },
                Argument::Files {
                    folder: PathBuf::from("/tmp/b"),
                    names: vec![OsString::from("two.txt")],
                },
                Argument::Files {
                    folder: PathBuf::from("/tmp/a"),
                    names: vec![OsString::from("three.txt")],
                },
            ]
        );
    }

    #[test]
    fn a_folder_argument_is_its_own_group() {
        let is_dir = |path: &Path| path == Path::new("/tmp/folder");
        let groups = group_arguments(&[file("/tmp/a/one.txt"), file("/tmp/folder")], &is_dir);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0], {
            Argument::Files {
                folder: PathBuf::from("/tmp/a"),
                names: vec![OsString::from("one.txt")],
            }
        });
        assert_eq!(groups[1], Argument::Folder(PathBuf::from("/tmp/folder")));
    }
}
