// SPDX-License-Identifier: GPL-3.0-or-later
//! What a double-click or Enter on a grid item means.
//!
//! The decision is pure so it can be tested without launching anything. The
//! caller supplies `Path::is_dir()` and `Path::is_file()`, both of which follow
//! symlinks, so a symlink opens its target.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenDecision {
    /// A folder: navigate into it with `nav.go`.
    Navigate,
    /// A file: launch it with `file.open`.
    Launch,
    /// A broken symlink or a path that vanished between the click and here.
    Missing,
}

/// The pure decision: a directory navigates, a regular file launches.
pub fn decision(is_dir: bool, is_file: bool) -> OpenDecision {
    if is_dir {
        OpenDecision::Navigate
    } else if is_file {
        OpenDecision::Launch
    } else {
        OpenDecision::Missing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_navigates_and_a_file_launches() {
        assert_eq!(decision(true, false), OpenDecision::Navigate);
        assert_eq!(decision(false, true), OpenDecision::Launch);
        assert_eq!(decision(false, false), OpenDecision::Missing);
        // A symlink to a folder reports is_dir() only; a symlink to a file
        // reports is_file() only. Both therefore follow the same decision.
        assert_eq!(decision(true, false), OpenDecision::Navigate);
        assert_eq!(decision(false, true), OpenDecision::Launch);
    }
}
