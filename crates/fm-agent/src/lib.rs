// SPDX-License-Identifier: GPL-3.0-or-later
//! The local socket that agents use to talk to the running app.
//!
//! It depends on `fm-core` so that every request it carries is a command from
//! the core registry, exactly like the UI and the CLI. The transport chooses
//! `Caller::Agent` for every connection; the wire schema has no caller field.
//! `fm-agent` is GTK-free and contains no `unsafe` code: the only GIO use is
//! the safe `gio::Socket::from_fd` credential read on a transport worker.

#![forbid(unsafe_code)]

pub mod client;
mod log;
pub mod protect;
pub mod protocol;
pub mod transport;

use std::path::{Path, PathBuf};

pub use protect::{ProtectedPathError, ProtectedRoots};
pub use protocol::{
    ArgumentInfo, CommandInfo, Data, ErrorBody, JobInfo, NameInfo, Operation, Reply, Request,
    RunKind, SocketInfo, Status, WindowInfo,
};
pub use transport::{EndpointError, Inbound, Server, ServerConfig};

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
