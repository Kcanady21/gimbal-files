// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure part of `org.freedesktop.FileManager1`: turning the URIs that
//! browsers, the desktop portal and terminals send into local paths.
//!
//! The D-Bus object and the activation file live in `fm-ui` and `packaging`;
//! nothing here is GTK or D-Bus. A refused URI never stops the rest of a call:
//! its reason is kept for the one `filemanager1:` log line.

use std::collections::HashSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// The interface accepts at most 64 entries. Anything past the 64th is
/// refused with [`RefusalReason::TooMany`] rather than silently dropped.
pub const MAX_URIS: usize = 64;

/// Why one URI did not become a path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// The URI's scheme is not `file`.
    UnsupportedScheme,
    /// The text is a path, a bare name, or a `file:` URI without `//`.
    RelativePath,
    /// The URI or its decoded path contains a NUL byte.
    NulByte,
    /// The entry is past [`MAX_URIS`].
    TooMany,
    /// A `file://` URI names a host other than the local machine.
    NotLocal,
    /// GLib could not decode the URI.
    InvalidUri,
}

impl RefusalReason {
    /// The short text used in the `filemanager1:` log line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnsupportedScheme => "unsupported scheme",
            Self::RelativePath => "relative path",
            Self::NulByte => "NUL byte",
            Self::TooMany => "past the 64-URI limit",
            Self::NotLocal => "not a local path",
            Self::InvalidUri => "invalid URI",
        }
    }
}

/// One URI that was not turned into a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    pub uri: String,
    pub reason: RefusalReason,
}

impl Refused {
    pub fn new(uri: &str, reason: RefusalReason) -> Self {
        Self {
            uri: uri.to_string(),
            reason,
        }
    }
}

/// Decode `file://` URIs into paths. The return value keeps the accepted
/// paths in call order and lists every refusal with its reason.
///
/// Only `file://` URIs with a local path are accepted. Percent escapes are
/// decoded by `glib::filename_from_uri` (`glib-0.22.10:src/auto/functions.rs:272`),
/// so non-UTF-8 bytes in the URI become the same non-UTF-8 bytes in the
/// `PathBuf`. Exact duplicate URI strings are dropped, keeping the first.
pub fn uris_to_paths(uris: &[String]) -> (Vec<PathBuf>, Vec<Refused>) {
    let mut paths = Vec::new();
    let mut refused = Vec::new();
    let mut seen = HashSet::new();

    for (index, uri) in uris.iter().enumerate() {
        if index >= MAX_URIS {
            refused.push(Refused::new(uri, RefusalReason::TooMany));
            continue;
        }
        if !seen.insert(uri.as_str()) {
            continue;
        }
        match path_from_uri(uri) {
            Ok(path) => paths.push(path),
            Err(reason) => refused.push(Refused::new(uri, reason)),
        }
    }

    (paths, refused)
}

/// One URI to one path. The checks are ordered so a `sftp://` or `trash:///`
/// URI is reported as a scheme problem, not as an invalid file URI.
fn path_from_uri(uri: &str) -> Result<PathBuf, RefusalReason> {
    if uri.as_bytes().contains(&0) {
        return Err(RefusalReason::NulByte);
    }

    let Some((scheme, rest)) = uri.split_once(':') else {
        return Err(RefusalReason::RelativePath);
    };
    if !is_scheme(scheme) {
        return Err(RefusalReason::RelativePath);
    }
    if !scheme.eq_ignore_ascii_case("file") {
        return Err(RefusalReason::UnsupportedScheme);
    }
    if !rest.starts_with("//") {
        return Err(RefusalReason::RelativePath);
    }

    let (path, host) = glib::filename_from_uri(uri).map_err(|_| RefusalReason::InvalidUri)?;
    if let Some(host) = host
        && !host.is_empty()
        && host != "localhost"
    {
        return Err(RefusalReason::NotLocal);
    }
    if !path.is_absolute() {
        return Err(RefusalReason::RelativePath);
    }
    if path.as_os_str().as_bytes().contains(&0) {
        return Err(RefusalReason::NulByte);
    }
    Ok(path)
}

/// An RFC 3986 scheme: ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ).
fn is_scheme(text: &str) -> bool {
    let mut bytes = text.bytes();
    match bytes.next() {
        Some(first) if first.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

/// The one-line summary of refused reasons, never a list longer than eight.
pub fn refusal_summary(refused: &[Refused]) -> String {
    if refused.is_empty() {
        return String::new();
    }
    let mut summary = String::new();
    for (index, refused) in refused.iter().take(8).enumerate() {
        if index > 0 {
            summary.push_str(", ");
        }
        summary.push_str(refused.reason.as_str());
    }
    if refused.len() > 8 {
        summary.push_str(", …");
    }
    summary
}

/// The body of the one `filemanager1:` log line for a call.
///
/// `caller` is the caller's unique bus name, `method` the interface member,
/// and the counts cover every URI the call carried. Refused URIs contribute
/// only their reasons, so a large call never prints a long list.
pub fn call_log_line(caller: &str, method: &str, accepted: usize, refused: &[Refused]) -> String {
    let summary = refusal_summary(refused);
    if summary.is_empty() {
        format!("{caller} called {method}: {accepted} accepted, 0 refused")
    } else {
        format!(
            "{caller} called {method}: {accepted} accepted, {} refused ({summary})",
            refused.len()
        )
    }
}

/// A path is a folder for the ShowFolders/ShowItems distinction. Kept here so
/// the UI's pure mapping can be tested with no filesystem.
pub fn is_folder(path: &Path, is_dir: &dyn Fn(&Path) -> bool) -> bool {
    is_dir(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uris(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn paths(values: &[&str]) -> Vec<PathBuf> {
        uris_to_paths(&uris(values)).0
    }

    #[test]
    fn plain_file_uri_becomes_its_path() {
        assert_eq!(
            paths(&["file:///tmp/gimbal/a"]),
            vec![PathBuf::from("/tmp/gimbal/a")]
        );
    }

    #[test]
    fn percent_encoding_spaces_and_parentheses_are_decoded() {
        assert_eq!(
            paths(&["file:///tmp/gimbal-p2x/a%20b%20%28c%29/file.txt"]),
            vec![PathBuf::from("/tmp/gimbal-p2x/a b (c)/file.txt")]
        );
    }

    #[test]
    fn non_utf8_bytes_are_kept() {
        let decoded = paths(&["file:///tmp/caf%C3%A9/na%C3%AFve%FF.txt"]);
        assert_eq!(decoded.len(), 1);
        assert_eq!(
            decoded[0].as_os_str().as_bytes(),
            b"/tmp/caf\xc3\xa9/na\xc3\xafve\xff.txt"
        );
    }

    #[test]
    fn other_schemes_are_refused_with_their_reason() {
        let (decoded, refused) = uris_to_paths(&uris(&["sftp://host/tmp/a", "trash:///a"]));
        assert!(decoded.is_empty());
        assert_eq!(refused.len(), 2);
        assert!(
            refused
                .iter()
                .all(|entry| entry.reason == RefusalReason::UnsupportedScheme)
        );
    }

    #[test]
    fn relative_paths_empty_text_and_nul_are_refused() {
        let (decoded, refused) =
            uris_to_paths(&uris(&["tmp/a", "", "file:relative/a", "file:///tmp/a\0b"]));
        assert!(decoded.is_empty());
        assert_eq!(refused.len(), 4);
        assert_eq!(refused[0].reason, RefusalReason::RelativePath);
        assert_eq!(refused[1].reason, RefusalReason::RelativePath);
        assert_eq!(refused[2].reason, RefusalReason::RelativePath);
        assert_eq!(refused[3].reason, RefusalReason::NulByte);
    }

    #[test]
    fn a_non_local_host_is_refused() {
        let (decoded, refused) = uris_to_paths(&uris(&["file://other-host/tmp/a"]));
        assert!(decoded.is_empty());
        assert_eq!(refused.len(), 1);
        assert_eq!(refused[0].reason, RefusalReason::NotLocal);
    }

    #[test]
    fn past_the_64th_entry_is_refused() {
        let many: Vec<String> = (0..66)
            .map(|index| format!("file:///tmp/{index}"))
            .collect();
        let (decoded, refused) = uris_to_paths(&many);
        assert_eq!(decoded.len(), 64);
        assert_eq!(refused.len(), 2);
        assert!(
            refused
                .iter()
                .all(|entry| entry.reason == RefusalReason::TooMany)
        );
    }

    #[test]
    fn exact_duplicates_are_dropped_in_order() {
        let decoded = paths(&[
            "file:///tmp/b",
            "file:///tmp/a",
            "file:///tmp/b",
            "file:///tmp/a",
        ]);
        assert_eq!(
            decoded,
            vec![PathBuf::from("/tmp/b"), PathBuf::from("/tmp/a")]
        );
    }

    #[test]
    fn the_log_line_keeps_counts_and_at_most_eight_reasons() {
        let refused: Vec<Refused> = (0..10)
            .map(|index| {
                Refused::new(
                    &format!("sftp://host/{index}"),
                    RefusalReason::UnsupportedScheme,
                )
            })
            .collect();
        let line = call_log_line(":1.42", "ShowItems", 3, &refused);
        assert!(line.contains(":1.42"));
        assert!(line.contains("ShowItems"));
        assert!(line.contains("3 accepted"));
        assert!(line.contains("10 refused"));
        assert!(line.contains("unsupported scheme"));
        assert!(line.contains("…"));
        assert!(!line.contains("sftp://"));
    }

    #[test]
    fn the_log_line_has_no_summary_without_refusals() {
        assert_eq!(
            call_log_line(":1.2", "ShowFolders", 1, &[]),
            ":1.2 called ShowFolders: 1 accepted, 0 refused"
        );
    }
}
