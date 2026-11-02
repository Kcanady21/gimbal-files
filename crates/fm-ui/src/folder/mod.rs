// SPDX-License-Identifier: GPL-3.0-or-later
//! The folder view: the GIO listing, the virtualized grid and its status line.

mod sort;
mod view;

use std::ffi::OsString;
use std::path::Path;

pub use view::FolderView;

/// The status line under the grid. `selected` is the model's count, so it is
/// shown even when a live listing has not realised every name yet.
pub fn status_text(files: u64, folders: u64, selected: usize) -> String {
    let mut text = format!("Files: {files}, Folders: {folders}");
    if selected > 0 {
        text.push_str(&format!(", {selected} selected"));
    }
    text
}

/// One click's meaning, before it reaches the model. Shift carries the visible
/// order because the model has no listing of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClickRequest {
    Plain(OsString),
    Ctrl(OsString),
    Shift {
        name: OsString,
        order: Vec<OsString>,
    },
}

/// The pure click mapping. Ctrl wins if both modifiers are down, as the strip's
/// clicks do.
pub fn click_request(ctrl: bool, shift: bool, name: OsString, order: &[OsString]) -> ClickRequest {
    if ctrl {
        ClickRequest::Ctrl(name)
    } else if shift {
        ClickRequest::Shift {
            name,
            order: order.to_vec(),
        }
    } else {
        ClickRequest::Plain(name)
    }
}

/// A name starting with `.` is hidden and left out of the listing.
pub fn is_hidden_name(name: &Path) -> bool {
    name.file_name()
        .is_some_and(|name| name.as_encoded_bytes().first() == Some(&b'.'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn status_counts_and_selection_read_naturally() {
        assert_eq!(status_text(12, 4, 0), "Files: 12, Folders: 4");
        assert_eq!(status_text(12, 4, 2), "Files: 12, Folders: 4, 2 selected");
        assert_eq!(status_text(0, 0, 1), "Files: 0, Folders: 0, 1 selected");
    }

    #[test]
    fn shift_gets_the_visible_order_exactly() {
        let order: Vec<OsString> = ["b", "a", "c"]
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(
            click_request(false, true, OsString::from("a"), &order),
            ClickRequest::Shift {
                name: OsString::from("a"),
                order,
            }
        );
        assert_eq!(
            click_request(false, false, OsString::from("b"), &[]),
            ClickRequest::Plain(OsString::from("b"))
        );
        assert_eq!(
            click_request(true, true, OsString::from("c"), &[]),
            ClickRequest::Ctrl(OsString::from("c"))
        );
    }

    #[test]
    fn dot_names_are_hidden() {
        assert!(is_hidden_name(Path::new(".git")));
        assert!(is_hidden_name(Path::new("notes/.hidden")));
        assert!(!is_hidden_name(Path::new("notes")));
        assert!(!is_hidden_name(PathBuf::from("file.txt").as_path()));
    }
}
