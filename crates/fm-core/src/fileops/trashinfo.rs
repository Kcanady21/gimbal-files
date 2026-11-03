// SPDX-License-Identifier: GPL-3.0-or-later
//! Reading freedesktop.org trash `*.trashinfo` files.
//!
//! The format is defined by the [Trash Specification 1.0], section 6,
//! "Contents of a trash directory": a `[Trash Info]` header, a `Path` key
//! holding the original pathname URL-escaped as in RFC 2396 section 2, and a
//! `DeletionDate` key in `YYYY-MM-DDThh:mm:ss` local time. Keys after the
//! first occurrence of either are ignored, as the specification requires.
//!
//! [Trash Specification 1.0]: https://specifications.freedesktop.org/trash/1.0/

use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

/// One parsed `.trashinfo` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashInfo {
    /// The original pathname, percent-decoded.
    pub path: PathBuf,
    /// When the item was trashed, in local time.
    pub deletion_date: DeletionDate,
}

/// The date and time from a `DeletionDate` key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionDate {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl DeletionDate {
    /// The instant this local date and time names, in Unix seconds.
    pub fn to_unix(self) -> Option<i64> {
        glib::DateTime::new(
            &glib::TimeZone::local(),
            self.year,
            self.month as i32,
            self.day as i32,
            self.hour as i32,
            self.minute as i32,
            f64::from(self.second),
        )
        .ok()
        .map(|date| date.to_unix())
    }
}

/// Why a `.trashinfo` file could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashInfoError {
    /// The first line was not `[Trash Info]`.
    MissingHeader,
    /// A required key was absent.
    MissingKey(&'static str),
    /// A `%` escape was incomplete or not hexadecimal.
    BadEscape(usize),
    /// The `DeletionDate` value was not a date this parser understands.
    BadDate(String),
}

impl fmt::Display for TrashInfoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrashInfoError::MissingHeader => f.write_str("the first line is not `[Trash Info]`"),
            TrashInfoError::MissingKey(key) => write!(f, "the `{key}` key is missing"),
            TrashInfoError::BadEscape(at) => {
                write!(f, "a `%` escape at byte {at} is not two hex digits")
            }
            TrashInfoError::BadDate(value) => {
                write!(f, "the deletion date `{value}` is not a date and time")
            }
        }
    }
}

impl std::error::Error for TrashInfoError {}

/// Resolve a `.trashinfo` `Path` against its trash directory. A top-directory
/// trash stores the original path relative to the mount's top directory; the
/// home trash stores it absolute. The `$topdir/.Trash/$uid` form has one extra
/// path component, so its top directory is two levels up, not one.
pub fn resolve_original(trash_dir: &Path, home_trash: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() || trash_dir == home_trash {
        return path.to_path_buf();
    }

    let under_dot_trash = trash_dir.file_name().and_then(|name| name.to_str()) == Some(".Trash")
        || trash_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            == Some(".Trash");
    let topdir = if under_dot_trash {
        trash_dir.parent().and_then(Path::parent)
    } else {
        trash_dir.parent()
    };
    topdir.map_or_else(|| path.to_path_buf(), |topdir| topdir.join(path))
}

/// Parse one `.trashinfo` file.
pub fn parse(text: &str) -> Result<TrashInfo, TrashInfoError> {
    let mut lines = text.lines();
    let header = lines.next().unwrap_or("").trim_end_matches('\r');
    if header != "[Trash Info]" {
        return Err(TrashInfoError::MissingHeader);
    }

    let mut path = None;
    let mut deletion_date = None;

    for line in lines {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key {
            "Path" if path.is_none() => path = Some(percent_decode(value)?),
            "DeletionDate" if deletion_date.is_none() => {
                deletion_date = Some(parse_deletion_date(value)?);
            }
            _ => {}
        }
    }

    Ok(TrashInfo {
        path: path.ok_or(TrashInfoError::MissingKey("Path"))?,
        deletion_date: deletion_date.ok_or(TrashInfoError::MissingKey("DeletionDate"))?,
    })
}

/// Percent-decode a `Path` value into bytes, then into a `PathBuf`, so a
/// non-UTF-8 original path survives.
fn percent_decode(value: &str) -> Result<PathBuf, TrashInfoError> {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = bytes
                .get(index + 1)
                .copied()
                .and_then(hex_digit)
                .ok_or(TrashInfoError::BadEscape(index))?;
            let low = bytes
                .get(index + 2)
                .copied()
                .and_then(hex_digit)
                .ok_or(TrashInfoError::BadEscape(index))?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }

    Ok(PathBuf::from(OsString::from_vec(decoded)))
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Parse `YYYY-MM-DDThh:mm:ss`. The specification's own example uses
/// `YYYYMMDDThh:mm:ss`, so that form is accepted too. A fractional second is
/// ignored.
fn parse_deletion_date(value: &str) -> Result<DeletionDate, TrashInfoError> {
    let (date, time) = value
        .split_once('T')
        .ok_or_else(|| TrashInfoError::BadDate(value.to_string()))?;

    let (year, month, day) = if date.len() == 10 && date.as_bytes()[4] == b'-' {
        (
            parse_digits(&date[0..4], value)?,
            parse_digits(&date[5..7], value)?,
            parse_digits(&date[8..10], value)?,
        )
    } else if date.len() == 8 {
        (
            parse_digits(&date[0..4], value)?,
            parse_digits(&date[4..6], value)?,
            parse_digits(&date[6..8], value)?,
        )
    } else {
        return Err(TrashInfoError::BadDate(value.to_string()));
    };

    let time = time.split_once('.').map_or(time, |(whole, _)| whole);
    let parts: Vec<&str> = time.split(':').collect();
    if parts.len() != 3 {
        return Err(TrashInfoError::BadDate(value.to_string()));
    }
    let hour = parse_digits(parts[0], value)?;
    let minute = parse_digits(parts[1], value)?;
    let second = parse_digits(parts[2], value)?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return Err(TrashInfoError::BadDate(value.to_string()));
    }

    Ok(DeletionDate {
        year: year as i32,
        month,
        day,
        hour,
        minute,
        second,
    })
}

fn parse_digits(text: &str, whole: &str) -> Result<u32, TrashInfoError> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(TrashInfoError::BadDate(whole.to_string()));
    }
    text.parse()
        .map_err(|_| TrashInfoError::BadDate(whole.to_string()))
}
