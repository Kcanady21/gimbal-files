// SPDX-License-Identifier: GPL-3.0-or-later
//! Administrator locations for files the current user does not own.
//!
//! Gimbal never starts a root process. A file outside the user's ownership is
//! opened through the desktop's administrator backend: GVfs's `admin://` for
//! GIO applications, or KIO's `admin:` for KDE applications. This module holds
//! the pure decisions; the UI reads the default application's desktop entry and
//! performs the launch.

use std::path::{Path, PathBuf};

/// The current user's uid. `/proc/self` is owned by the process's real uid, so
/// reading its metadata avoids a dependency on `getuid(2)` or libc.
pub fn current_uid() -> Result<u32, String> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata("/proc/self")
        .map(|metadata| metadata.uid())
        .map_err(|error| format!("could not read the current uid: {error}"))
}

/// Whether a file owned by `owner_uid` needs the administrator backend for
/// `current_uid`. Equal owners use the ordinary user path.
pub fn needs_admin(owner_uid: u32, current_uid: u32) -> bool {
    owner_uid != current_uid
}

/// Why a selection cannot be opened through an administrator location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminFileProblem {
    /// Exactly one file may be opened at a time.
    NotOneFile,
    /// Folders belong to administrator tabs, not this file command.
    Folder,
    /// A missing path, a broken link, or something that is not a regular file.
    NotRegularFile,
    /// The current user owns the file, so it opens normally.
    OwnedByUser,
}

impl AdminFileProblem {
    /// The refusal text shown by the command handler.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotOneFile => "Open as Administrator opens one file at a time.",
            Self::Folder => "Open as Administrator works on files, not folders.",
            Self::NotRegularFile => "Open as Administrator needs a regular file.",
            Self::OwnedByUser => "You own this file; open it normally.",
        }
    }
}

/// Check a selection before launching an administrator location. `is_file`
/// and `owner_uid` come from metadata that follows symlinks, so a link is
/// judged by the target it resolves to. A broken link has no owner.
pub fn check_admin_file(
    count: usize,
    is_dir: bool,
    is_file: bool,
    owner_uid: Option<u32>,
    current_uid: u32,
) -> Result<(), AdminFileProblem> {
    if count != 1 {
        return Err(AdminFileProblem::NotOneFile);
    }
    if is_dir {
        return Err(AdminFileProblem::Folder);
    }
    if !is_file {
        return Err(AdminFileProblem::NotRegularFile);
    }
    let Some(owner_uid) = owner_uid else {
        return Err(AdminFileProblem::NotRegularFile);
    };
    if !needs_admin(owner_uid, current_uid) {
        return Err(AdminFileProblem::OwnedByUser);
    }
    Ok(())
}

/// How a default application resolves administrator URLs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppFamily {
    /// A KDE application using KIO's `admin:` handler from `kio-admin`.
    Kio,
    /// A GIO application using GVfs's `admin://` handler from `gvfs-backends`.
    Gio,
    /// An application that takes paths and cannot open an administrator URL.
    PathOnly,
    /// A Flatpak or Snap application, which cannot reach the host's backends.
    Sandboxed,
}

impl AppFamily {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kio => "kio",
            Self::Gio => "gio",
            Self::PathOnly => "path-only",
            Self::Sandboxed => "sandboxed",
        }
    }
}

/// Classify a desktop entry from the fields that decide its URL support.
///
/// `categories` is the entry's `Categories` string, `exec` its `Exec` line,
/// and the flags say whether `X-Flatpak` or `X-SnapInstanceName` is present.
pub fn app_family(
    categories: Option<&str>,
    exec: Option<&str>,
    flatpak: bool,
    snap: bool,
) -> AppFamily {
    if flatpak || snap {
        return AppFamily::Sandboxed;
    }
    if categories.is_some_and(|categories| {
        categories
            .split(';')
            .any(|category| category.trim() == "KDE")
    }) {
        return AppFamily::Kio;
    }
    if exec.is_some_and(exec_takes_uris) {
        return AppFamily::Gio;
    }
    AppFamily::PathOnly
}

/// Whether an `Exec` line has an unescaped `%u` or `%U` field code. `%%` is a
/// literal percent sign, so `%%u` does not count.
fn exec_takes_uris(exec: &str) -> bool {
    let bytes = exec.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 1 < bytes.len() {
            match bytes[index + 1] {
                b'u' | b'U' => return true,
                _ => index += 2,
            }
        } else {
            index += 1;
        }
    }
    false
}

/// The administrator URL for `path`, or `None` for a family that cannot use
/// one. Both backends take the same percent-encoded absolute path; only the
/// scheme spelling differs. `glib::filename_to_uri` performs the encoding,
/// including non-UTF-8 bytes.
pub fn admin_uri(family: AppFamily, path: &Path) -> Option<String> {
    let prefix = match family {
        AppFamily::Gio => "admin://",
        AppFamily::Kio => "admin:",
        AppFamily::PathOnly | AppFamily::Sandboxed => return None,
    };
    let file_uri = glib::filename_to_uri(path, None).ok()?;
    let encoded = file_uri.strip_prefix("file://")?;
    Some(format!("{prefix}{encoded}"))
}

/// The content type whose default application opens the file. GIO types every
/// empty file `application/x-zerosize` regardless of its name, and no
/// application handles that type, so an empty file falls back to the type its
/// name suggests.
pub fn lookup_type<'a>(queried: &'a str, by_name: &'a str) -> &'a str {
    if queried == "application/x-zerosize" {
        by_name
    } else {
        queried
    }
}

/// The metadata the file menu and command need for one item. It follows
/// symlinks, so a link is judged by its target and a broken link gets no owner.
pub fn file_facts(path: &Path) -> (bool, bool, Option<u32>) {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            use std::os::unix::fs::MetadataExt;
            (metadata.is_dir(), metadata.is_file(), Some(metadata.uid()))
        }
        Err(_) => (false, false, None),
    }
}

/// The one path a selection would open as administrator, or the first refusal.
pub fn selected_admin_file(
    paths: &[PathBuf],
    current_uid: u32,
) -> Result<PathBuf, AdminFileProblem> {
    let (is_dir, is_file, owner_uid) = paths
        .first()
        .map(|path| file_facts(path))
        .unwrap_or((false, false, None));
    check_admin_file(paths.len(), is_dir, is_file, owner_uid, current_uid)?;
    paths.first().cloned().ok_or(AdminFileProblem::NotOneFile)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    #[test]
    fn an_empty_file_is_looked_up_by_its_name() {
        assert_eq!(
            lookup_type("application/x-zerosize", "text/plain"),
            "text/plain"
        );
        assert_eq!(lookup_type("text/x-csrc", "text/plain"), "text/x-csrc");
    }

    #[test]
    fn owners_that_differ_need_an_administrator() {
        assert!(needs_admin(0, 1000));
        assert!(needs_admin(1000, 0));
        assert!(!needs_admin(1000, 1000));
    }

    #[test]
    fn kates_real_entry_is_kio() {
        let categories = "Qt;KDE;Utility;TextEditor;Development;";
        let exec = "kate -b %U";
        assert_eq!(
            app_family(Some(categories), Some(exec), false, false),
            AppFamily::Kio
        );
    }

    #[test]
    fn gnome_text_editor_is_gio() {
        let categories = "GNOME;GTK;";
        let exec = "gnome-text-editor %U";
        assert_eq!(
            app_family(Some(categories), Some(exec), false, false),
            AppFamily::Gio
        );
    }

    #[test]
    fn a_percent_f_application_is_path_only() {
        assert_eq!(
            app_family(Some("GTK;"), Some("editor %f"), false, false),
            AppFamily::PathOnly
        );
        assert_eq!(
            app_family(Some("GTK;"), Some("editor %F"), false, false),
            AppFamily::PathOnly
        );
        assert_eq!(
            app_family(Some("GTK;"), Some("editor"), false, false),
            AppFamily::PathOnly
        );
        // %% is a literal percent, not a URI field code.
        assert_eq!(
            app_family(Some("GTK;"), Some("editor %%u"), false, false),
            AppFamily::PathOnly
        );
    }

    #[test]
    fn a_flatpak_entry_is_sandboxed() {
        assert_eq!(
            app_family(Some("GNOME;GTK;"), Some("editor %U"), true, false),
            AppFamily::Sandboxed
        );
    }

    #[test]
    fn a_snap_entry_is_sandboxed() {
        assert_eq!(
            app_family(Some("KDE;"), Some("kate %U"), false, true),
            AppFamily::Sandboxed
        );
    }

    #[test]
    fn a_kde_app_that_is_also_a_flatpak_is_sandboxed() {
        assert_eq!(
            app_family(Some("KDE;"), Some("kate %U"), true, false),
            AppFamily::Sandboxed
        );
    }

    #[test]
    fn admin_uris_percent_encode_the_absolute_path() {
        let path = Path::new("/tmp/a b#%()z");
        assert_eq!(
            admin_uri(AppFamily::Gio, path).as_deref(),
            Some("admin:///tmp/a%20b%23%25()z")
        );
        assert_eq!(
            admin_uri(AppFamily::Kio, path).as_deref(),
            Some("admin:/tmp/a%20b%23%25()z")
        );
        assert_eq!(admin_uri(AppFamily::PathOnly, path), None);
        assert_eq!(admin_uri(AppFamily::Sandboxed, path), None);
    }

    #[test]
    fn admin_uris_keep_non_utf8_bytes_as_percent_encoded_bytes() {
        let path = PathBuf::from(OsString::from_vec(b"/tmp/nonutf8-\xff.txt".to_vec()));
        assert_eq!(
            admin_uri(AppFamily::Gio, &path).as_deref(),
            Some("admin:///tmp/nonutf8-%FF.txt")
        );
        assert_eq!(
            admin_uri(AppFamily::Kio, &path).as_deref(),
            Some("admin:/tmp/nonutf8-%FF.txt")
        );
    }

    #[test]
    fn a_selection_refuses_more_than_one_file() {
        let paths = [PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")];
        assert_eq!(
            selected_admin_file(&paths, 1000),
            Err(AdminFileProblem::NotOneFile)
        );
        assert_eq!(
            check_admin_file(2, false, true, Some(0), 1000),
            Err(AdminFileProblem::NotOneFile)
        );
    }

    #[test]
    fn a_selection_refuses_a_folder_and_a_file_the_user_owns() {
        assert_eq!(
            check_admin_file(1, true, false, Some(0), 1000),
            Err(AdminFileProblem::Folder)
        );
        assert_eq!(
            check_admin_file(1, false, true, Some(1000), 1000),
            Err(AdminFileProblem::OwnedByUser)
        );
        assert_eq!(
            check_admin_file(1, false, false, None, 1000),
            Err(AdminFileProblem::NotRegularFile)
        );
        assert_eq!(check_admin_file(1, false, true, Some(0), 1000), Ok(()));
    }

    #[test]
    fn file_facts_follows_a_symlink_and_rejects_a_broken_one() {
        struct Scratch(PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        let root = PathBuf::from(format!(
            "/tmp/gimbal-p2y-admin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("scratch root");
        let scratch = Scratch(root.clone());

        let target = root.join("target.txt");
        std::fs::write(&target, b"target").expect("target file");
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("target link");
        let (is_dir, is_file, owner) = file_facts(&link);
        assert!(!is_dir);
        assert!(is_file);
        assert_eq!(owner, Some(current_uid().expect("current uid")));

        let broken = root.join("broken.txt");
        std::os::unix::fs::symlink(root.join("missing.txt"), &broken).expect("broken link");
        let (is_dir, is_file, owner) = file_facts(&broken);
        assert!(!is_dir);
        assert!(!is_file);
        assert_eq!(owner, None);

        drop(scratch);
    }
}
