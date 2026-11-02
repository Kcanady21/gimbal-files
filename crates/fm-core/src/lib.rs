// SPDX-License-Identifier: GPL-3.0-or-later
//! The UI-agnostic core of Gimbal Files.
//!
//! The command registry, the workspace model and the TOML config live here, as
//! do the file operations (trash, copy, move) done through GIO. Nothing in this
//! crate may depend on GTK: the UI, the agent socket and the CLI are all thin
//! front ends over this core.

#![forbid(unsafe_code)]

pub mod command;
pub mod config;
pub(crate) mod fsutil;
pub mod session;
pub mod workspace;

/// The application ID used by the UI and, later, the agent socket.
///
/// Kyle's niri config only skips the focus-ring fill behind windows whose app
/// ID matches `^io\.github\.kcanady21\.Gimbal`, and the window blur depends on
/// that rule.
pub const APP_ID: &str = "io.github.kcanady21.GimbalFiles";

#[cfg(test)]
mod tests {
    use super::APP_ID;

    /// GLib's rule for a valid GApplication ID, as documented for
    /// `g_application_id_is_valid()` (GIO): at least two dot-separated
    /// elements, each non-empty, made of `[A-Za-z0-9_-]` and not starting
    /// with a digit, and the whole ID at most 255 characters.
    ///
    /// <https://docs.gtk.org/gio/func.Application.id_is_valid.html>
    /// The binding is `gio-sys-0.21.5/src/lib.rs:10299`.
    fn is_valid_application_id(id: &str) -> bool {
        if id.len() > 255 {
            return false;
        }

        let elements: Vec<&str> = id.split('.').collect();
        if elements.len() < 2 {
            return false;
        }

        elements.iter().all(|element| {
            !element.is_empty()
                && !element.starts_with(|c: char| c.is_ascii_digit())
                && element
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
    }

    #[test]
    fn app_id_matches_the_niri_rule_and_is_a_valid_gapplication_id() {
        assert!(APP_ID.starts_with("io.github.kcanady21.Gimbal"));
        assert!(is_valid_application_id(APP_ID));
    }
}
