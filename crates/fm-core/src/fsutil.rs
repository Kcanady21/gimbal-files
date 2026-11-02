// SPDX-License-Identifier: GPL-3.0-or-later
//! File-system helpers shared by the config and the session.
//!
//! [`save_atomic`] writes a whole document through a uniquely named temp file
//! in the same folder, fsyncs it, then renames it over the original. The
//! original permissions are kept and the folder is created when missing.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::CONFIG_FILE;

/// Write a file atomically: a temp file in the same folder, fsync, then rename
/// over the original. The original permissions are kept, and the folder is
/// created when missing.
pub(crate) fn save_atomic(path: &Path, contents: &str) -> io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    let sequence = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| CONFIG_FILE.to_string());
    let temp_path = parent.join(format!(
        ".{name}.tmp-{}-{nanos}-{sequence}",
        std::process::id()
    ));

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp_path)?;

    if let Err(error) = file
        .write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
    {
        drop(file);
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    if let Some(permissions) = permissions
        && let Err(error) = fs::set_permissions(&temp_path, permissions)
    {
        drop(file);
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    drop(file);

    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error);
    }

    // Make the rename itself durable where the platform allows opening a
    // directory for fsync. Failure here does not invalidate the save.
    if let Ok(directory) = File::open(parent) {
        let _ = directory.sync_all();
    }

    Ok(())
}
