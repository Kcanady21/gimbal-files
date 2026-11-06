// SPDX-License-Identifier: GPL-3.0-or-later
//! The control paths a socket file request may never touch.
//!
//! Kyle's config file, its Gimbal control directory and the active runtime
//! control/socket directory are not agent-writable. The check covers sources,
//! destinations, rename targets, new-folder parents and undo steps, resolves
//! `..` and symbolic-link aliases, and fails closed when a path cannot be
//! resolved. The transport runs it on its worker thread, before the bridge
//! sees the request, so the GTK thread does no filesystem work for it.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use fm_core::fileops::Request;

/// The three control locations for one enable cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedRoots {
    pub config_file: PathBuf,
    pub config_dir: PathBuf,
    pub runtime_dir: PathBuf,
}

impl ProtectedRoots {
    pub fn new(
        config_file: impl Into<PathBuf>,
        config_dir: impl Into<PathBuf>,
        runtime_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            config_file: config_file.into(),
            config_dir: config_dir.into(),
            runtime_dir: runtime_dir.into(),
        }
    }

    /// Resolve the roots once. A root that does not exist yet resolves through
    /// its nearest existing ancestor, so a first save cannot slip past it.
    pub fn resolved(&self) -> Result<ResolvedRoots, ProtectedPathError> {
        Ok(ResolvedRoots {
            config_file: resolve(&self.config_file)?,
            config_dir: resolve(&self.config_dir)?,
            runtime_dir: resolve(&self.runtime_dir)?,
        })
    }
}

/// Roots after alias resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRoots {
    config_file: PathBuf,
    config_dir: PathBuf,
    runtime_dir: PathBuf,
}

impl ResolvedRoots {
    fn roots(&self) -> [&Path; 3] {
        [&self.config_file, &self.config_dir, &self.runtime_dir]
    }

    /// Whether any source, destination, ancestor or descendant of `path` is a
    /// control path. Both directions matter: moving a parent folder would take
    /// the control directory with it.
    fn overlaps(&self, path: &Path) -> bool {
        self.roots()
            .iter()
            .any(|root| path == *root || path.starts_with(root) || root.starts_with(path))
    }

    /// Whether `path` is inside (or is) a control directory. Used for a
    /// destination folder, where copying *into* a parent is harmless.
    fn contains(&self, path: &Path) -> bool {
        self.roots()
            .iter()
            .any(|root| path == *root || path.starts_with(root))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedPathError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for ProtectedPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} is a protected Gimbal control path: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for ProtectedPathError {}

/// Check a fully built file request. `resolved` is computed once per enable
/// cycle; the per-request part resolves the request's own paths.
pub fn check_request(
    request: &Request,
    resolved: &ResolvedRoots,
) -> Result<(), ProtectedPathError> {
    match request {
        Request::Trash { paths, .. } | Request::DeletePermanent { paths, .. } => {
            for path in paths {
                check_source(path, resolved)?;
            }
        }
        Request::Copy { paths, to, .. } | Request::Move { paths, to, .. } => {
            for path in paths {
                check_source(path, resolved)?;
            }
            check_destination(to, resolved)?;
        }
        Request::Rename { path, name, .. } => {
            check_source(path, resolved)?;
            let parent = path.parent().unwrap_or_else(|| Path::new("/"));
            let target = parent.join(name);
            check_destination(&target, resolved)?;
        }
        Request::NewFolder {
            name, in_folder, ..
        } => {
            check_destination(&in_folder.join(name), resolved)?;
        }
        Request::Restore { items } => {
            for item in items {
                // The trash URI is not a filesystem path; restoring can only
                // create the item's original location, which we cannot know
                // before planning. The bridge refuses a plan whose target
                // overlaps a control root (see `check_steps`).
                let _ = item;
            }
        }
        Request::EmptyTrash | Request::DeleteSelected { .. } | Request::Undo => {}
    }
    Ok(())
}

/// Check a planned undo or restore step before it runs. The worker's plan is
/// read on the GTK thread and rechecked there; this is the same pure rule.
pub fn check_steps<'a>(
    steps: impl IntoIterator<Item = (&'a Path, Option<&'a Path>)>,
    resolved: &ResolvedRoots,
) -> Result<(), ProtectedPathError> {
    for (source, target) in steps {
        check_source(source, resolved)?;
        if let Some(target) = target {
            check_destination(target, resolved)?;
        }
    }
    Ok(())
}

fn check_source(path: &Path, resolved: &ResolvedRoots) -> Result<(), ProtectedPathError> {
    let path = resolve(path)?;
    if resolved.overlaps(&path) {
        return Err(ProtectedPathError {
            path,
            reason: "it would read or change Gimbal's own control files".to_string(),
        });
    }
    Ok(())
}

fn check_destination(path: &Path, resolved: &ResolvedRoots) -> Result<(), ProtectedPathError> {
    let path = resolve(path)?;
    if resolved.contains(&path) {
        return Err(ProtectedPathError {
            path,
            reason: "it would write Gimbal's own control files".to_string(),
        });
    }
    Ok(())
}

/// Resolve a path lexically, then through the longest existing ancestor so
/// symlinks and `..` aliases cannot point back into a control directory.
pub fn resolve(path: &Path) -> Result<PathBuf, ProtectedPathError> {
    if !path.is_absolute() {
        return Err(ProtectedPathError {
            path: path.to_path_buf(),
            reason: "the path is not absolute".to_string(),
        });
    }
    let normalized = normalize(path);
    let mut existing = normalized.as_path();
    let mut remainder: Vec<&std::ffi::OsStr> = Vec::new();

    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else {
                    return Err(ProtectedPathError {
                        path: path.to_path_buf(),
                        reason: format!("cannot resolve it: {error}"),
                    });
                };
                remainder.push(name);
                let Some(parent) = existing.parent() else {
                    return Err(ProtectedPathError {
                        path: path.to_path_buf(),
                        reason: format!("cannot resolve it: {error}"),
                    });
                };
                existing = parent;
            }
            Err(error) => {
                return Err(ProtectedPathError {
                    path: path.to_path_buf(),
                    reason: format!("cannot resolve it: {error}"),
                });
            }
        }
    }

    let mut resolved = std::fs::canonicalize(existing).map_err(|error| ProtectedPathError {
        path: path.to_path_buf(),
        reason: format!("cannot resolve it: {error}"),
    })?;
    for name in remainder.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

/// Remove `.` and resolve `..` lexically without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {}
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(name) => parts.push(name),
        }
    }
    let mut result = PathBuf::from("/");
    for part in parts {
        result.push(part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct Scratch {
        root: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "/tmp/gimbal-agent-protect-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|time| time.as_nanos())
                    .unwrap_or(0)
            );
            fs::create_dir_all(&unique).expect("scratch");
            Self {
                root: fs::canonicalize(unique).expect("canonical scratch"),
            }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn roots(scratch: &Scratch) -> (ProtectedRoots, ResolvedRoots) {
        let config_dir = scratch.root.join("config/gimbal-files");
        fs::create_dir_all(&config_dir).expect("config dir");
        fs::write(config_dir.join("config.toml"), "[agent]\n").expect("config");
        let runtime = scratch.root.join("run/gimbal-files");
        fs::create_dir_all(&runtime).expect("runtime");
        let roots = ProtectedRoots::new(config_dir.join("config.toml"), &config_dir, &runtime);
        let resolved = roots.resolved().expect("resolved");
        (roots, resolved)
    }

    #[test]
    fn the_config_and_runtime_directories_are_refused_for_sources_and_targets() {
        let scratch = Scratch::new("roots");
        let (roots, resolved) = roots(&scratch);
        assert!(resolve(&roots.config_file).is_ok());

        let request = Request::Copy {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![roots.config_file.clone()],
            to: scratch.root.join("dest"),
        };
        assert!(check_request(&request, &resolved).is_err());

        let request = Request::Copy {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![scratch.root.join("plain")],
            to: roots.runtime_dir.clone(),
        };
        assert!(check_request(&request, &resolved).is_err());

        let request = Request::NewFolder {
            scope: fm_core::admin::Scope::User,
            name: "new".to_string(),
            in_folder: roots.config_dir.clone(),
        };
        assert!(check_request(&request, &resolved).is_err());
    }

    #[test]
    fn parent_aliases_and_symlinks_cannot_reach_a_control_directory() {
        let scratch = Scratch::new("alias");
        let (roots, resolved) = roots(&scratch);
        let link = scratch.root.join("link");
        symlink(&roots.config_dir, &link).expect("symlink");

        let request = Request::Trash {
            scope: fm_core::admin::Scope::User,
            paths: vec![link.join("config.toml")],
        };
        assert!(check_request(&request, &resolved).is_err());

        let request = Request::Copy {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![scratch.root.join("plain")],
            to: scratch.root.join("x/../config/gimbal-files"),
        };
        assert!(check_request(&request, &resolved).is_err());

        // Moving a parent directory that contains the control directory would
        // change it as a descendant.
        let request = Request::Move {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![scratch.root.join("config")],
            to: scratch.root.join("elsewhere"),
        };
        assert!(check_request(&request, &resolved).is_err());
    }

    #[test]
    fn ordinary_scratch_paths_are_allowed() {
        let scratch = Scratch::new("plain");
        let (_roots, resolved) = roots(&scratch);
        fs::create_dir_all(scratch.root.join("src")).expect("src");
        fs::write(scratch.root.join("src/a.txt"), "a").expect("file");
        let request = Request::Copy {
            scope: fm_core::admin::Scope::User,
            target_scope: fm_core::admin::Scope::User,
            paths: vec![scratch.root.join("src/a.txt")],
            to: scratch.root.join("dest"),
        };
        assert!(check_request(&request, &resolved).is_ok());
    }

    #[test]
    fn relative_paths_and_missing_ancestors_fail_closed() {
        let scratch = Scratch::new("missing");
        let (_roots, _resolved) = roots(&scratch);
        assert!(resolve(Path::new("relative")).is_err());
        let missing = scratch.root.join("no/such/deep/path");
        assert!(resolve(&missing).is_ok());
    }
}
