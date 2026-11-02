// SPDX-License-Identifier: GPL-3.0-or-later
//! The local socket that agents use to talk to the running app.
//!
//! It depends on `fm-core` so that every request it carries is a command from
//! the core registry, exactly like the UI and the CLI.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// The socket path inside the caller's XDG runtime directory.
///
/// The caller passes `$XDG_RUNTIME_DIR` in, so this stays pure and testable;
/// the function never reads the environment.
pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("gimbal-files").join("agent.sock")
}

#[cfg(test)]
mod tests {
    use super::socket_path;
    use std::path::Path;

    #[test]
    fn socket_lives_under_the_runtime_directory() {
        assert_eq!(
            socket_path(Path::new("/run/user/1000")),
            Path::new("/run/user/1000/gimbal-files/agent.sock")
        );
    }
}
