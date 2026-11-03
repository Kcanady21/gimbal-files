// SPDX-License-Identifier: GPL-3.0-or-later
//! GIO trash identities, listing, restoration and permanent deletion.
//!
//! The trash is addressed by its `trash:///` URIs, never by a local path or
//! by a display name. GVfs owns the backend that maps those URIs to each
//! mounted volume's trash directory, so an unplugged or missing backend is a
//! clear error rather than a guessed `~/.local/share/Trash` path.
//!
//! The production adapter is [`GioTrash`]. A test may inject another
//! [`TrashBackend`] into [`FileOperations`](super::FileOperations); the plans,
//! decisions and executor are the same for both.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use gio::prelude::*;

/// The GIO trash root. It is not an item.
pub const TRASH_ROOT_URI: &str = "trash:///";

/// A validated GIO trash item URI.
///
/// The URI has exactly one percent-encoded name below `trash:///`. The root,
/// foreign schemes, extra path segments, `.`/`..`, encoded slashes, NUL bytes
/// and non-canonical escapes are refused. A display name is never an identity;
/// the decoded bytes stay in this value so a non-UTF-8 name survives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrashUri(String);

impl TrashUri {
    /// Parse and canonicalise one trash item URI.
    pub fn parse(text: &str) -> Result<Self, TrashUriError> {
        let Some(encoded) = text.strip_prefix(TRASH_ROOT_URI) else {
            return Err(TrashUriError::NotTrashRoot);
        };
        if encoded.is_empty() || encoded.contains('/') {
            return Err(TrashUriError::NotAnItem);
        }
        if encoded == "." || encoded == ".." {
            return Err(TrashUriError::Traversal);
        }

        let mut decoded = Vec::with_capacity(encoded.len());
        let bytes = encoded.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == b'%' {
                let high = bytes
                    .get(index + 1)
                    .copied()
                    .and_then(hex_digit)
                    .ok_or(TrashUriError::BadEscape)?;
                let low = bytes
                    .get(index + 2)
                    .copied()
                    .and_then(hex_digit)
                    .ok_or(TrashUriError::BadEscape)?;
                let value = (high << 4) | low;
                if value == b'/' || value == 0 {
                    return Err(TrashUriError::UnsafeName);
                }
                // A canonical URI never escapes a byte GIO leaves raw.
                if is_raw_allowed(value) {
                    return Err(TrashUriError::NonCanonical);
                }
                decoded.push(value);
                index += 3;
            } else {
                if !is_raw_allowed(byte) {
                    return Err(TrashUriError::NonCanonical);
                }
                decoded.push(byte);
                index += 1;
            }
        }

        if decoded.is_empty() || decoded == b"." || decoded == b".." {
            return Err(TrashUriError::Traversal);
        }

        let mut canonical = String::from(TRASH_ROOT_URI);
        for byte in &decoded {
            if is_raw_allowed(*byte) {
                canonical.push(*byte as char);
            } else {
                canonical.push('%');
                canonical.push_str(&format!("{byte:02X}"));
            }
        }
        if canonical != text {
            return Err(TrashUriError::NonCanonical);
        }

        Ok(Self(canonical))
    }

    /// Build the URI for one raw trash entry name. Non-UTF-8 bytes are
    /// percent-encoded, so the name is preserved exactly.
    pub fn from_name(name: &OsStr) -> Result<Self, TrashUriError> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes == b"." || bytes == b".." {
            return Err(TrashUriError::Traversal);
        }

        let mut text = String::from(TRASH_ROOT_URI);
        for byte in bytes {
            if *byte == b'/' || *byte == 0 {
                return Err(TrashUriError::UnsafeName);
            }
            if is_raw_allowed(*byte) {
                text.push(*byte as char);
            } else {
                text.push('%');
                text.push_str(&format!("{byte:02X}"));
            }
        }
        Self::parse(&text)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The decoded entry name, preserving non-UTF-8 bytes.
    pub fn name(&self) -> OsString {
        let encoded = &self.0[TRASH_ROOT_URI.len()..];
        let bytes = encoded.as_bytes();
        let mut decoded = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'%' {
                let high = hex_digit(bytes[index + 1]).unwrap_or(0);
                let low = hex_digit(bytes[index + 2]).unwrap_or(0);
                decoded.push((high << 4) | low);
                index += 3;
            } else {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
        OsString::from_vec(decoded)
    }

    /// The GIO file for this validated item.
    pub fn file(&self) -> gio::File {
        gio::File::for_uri(&self.0)
    }
}

impl fmt::Display for TrashUri {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn is_raw_allowed(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'-' | b'.'
                | b'_'
                | b'~'
                | b'!'
                | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b'='
                | b':'
                | b'@'
        )
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Why a string is not a valid trash item URI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrashUriError {
    /// Not `trash:///` with a lowercase scheme.
    NotTrashRoot,
    /// The root itself, a nested path, or an empty name.
    NotAnItem,
    /// `.`, `..`, an encoded slash or NUL.
    Traversal,
    /// A malformed, lowercase or unnecessarily escaped byte.
    BadEscape,
    NonCanonical,
    UnsafeName,
}

impl fmt::Display for TrashUriError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrashUriError::NotTrashRoot => {
                f.write_str("expected a `trash:///` item URI, not the trash root")
            }
            TrashUriError::NotAnItem => f.write_str("the trash URI does not name one item"),
            TrashUriError::Traversal => f.write_str("the trash URI contains a traversal name"),
            TrashUriError::BadEscape => f.write_str("the trash URI has a malformed percent escape"),
            TrashUriError::NonCanonical => {
                f.write_str("the trash URI is not in canonical percent-encoded form")
            }
            TrashUriError::UnsafeName => f.write_str("the trash URI names a slash or NUL byte"),
        }
    }
}

impl std::error::Error for TrashUriError {}

/// An opaque backend object/revision token for one trash entry.
///
/// GIO documents `etag::value` as the backend's opaque identifier for the
/// current version of a file (`gio-0.22.10:src/auto/constants.rs:52-54`).
/// It is evidence about the object, not display metadata: a later replacement
/// at the same URI may carry a different token. The scratch backend uses an
/// explicit generation token. A missing token is never treated as a match.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrashRevision(OsString);

impl TrashRevision {
    pub fn new(value: impl Into<OsString>) -> Self {
        Self(value.into())
    }

    pub fn as_os_str(&self) -> &OsStr {
        &self.0
    }
}

impl fmt::Display for TrashRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0.to_string_lossy())
    }
}

/// One trash item's identity: its URI, optional display/restore metadata, and
/// an optional backend object/revision token.
///
/// The original path and deletion date are display and restore metadata. They
/// are not proof that an object is unchanged: an item with the same URI,
/// original path and second-resolution deletion date can still be a later
/// replacement. Only [`TrashIdentity::same_object`] may authorise permanent
/// deletion, and it requires matching non-empty backend tokens.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrashIdentity {
    uri: TrashUri,
    original: Option<PathBuf>,
    deleted_at: Option<i64>,
    revision: Option<TrashRevision>,
}

impl TrashIdentity {
    pub fn new(
        uri: TrashUri,
        original: Option<PathBuf>,
        deleted_at: Option<i64>,
        revision: Option<TrashRevision>,
    ) -> Self {
        Self {
            uri,
            original,
            deleted_at,
            revision,
        }
    }

    pub fn uri(&self) -> &TrashUri {
        &self.uri
    }

    pub fn original(&self) -> Option<&Path> {
        self.original.as_deref()
    }

    pub fn deleted_at(&self) -> Option<i64> {
        self.deleted_at
    }

    pub fn revision(&self) -> Option<&TrashRevision> {
        self.revision.as_ref()
    }

    /// Whether this object/revision token proves that `fresh` is the same
    /// trash object. Both sides need a token; a missing or changed token is
    /// not a match. Optional original/date metadata is deliberately not part
    /// of this proof.
    pub fn same_object(&self, fresh: &TrashIdentity) -> bool {
        if self.uri != fresh.uri {
            return false;
        }
        match (&self.revision, &fresh.revision) {
            (Some(current), Some(fresh)) => current == fresh,
            _ => false,
        }
    }

    /// Whether a restore request still describes `fresh` closely enough to
    /// act on it. A request made only of a URI (the registry's argument) is
    /// accepted; any metadata or token it does carry must agree, and a
    /// snapshot date disappearing is a change.
    pub fn agrees_with(&self, fresh: &TrashIdentity) -> bool {
        if self.uri != fresh.uri {
            return false;
        }
        match (&self.revision, &fresh.revision) {
            (Some(current), Some(fresh)) if current != fresh => return false,
            (Some(_), None) => return false,
            _ => {}
        }
        if let Some(original) = &self.original
            && fresh.original.as_ref() != Some(original)
        {
            return false;
        }
        if let Some(date) = self.deleted_at
            && fresh.deleted_at != Some(date)
        {
            return false;
        }
        true
    }
}

impl fmt::Display for TrashIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.uri.as_str())
    }
}

/// One entry in the trash, as listed by a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrashItem {
    identity: TrashIdentity,
    display_name: String,
    is_dir: bool,
    icon_name: Option<String>,
}

impl TrashItem {
    pub fn new(
        identity: TrashIdentity,
        display_name: String,
        is_dir: bool,
        icon_name: Option<String>,
    ) -> Self {
        Self {
            identity,
            display_name,
            is_dir,
            icon_name,
        }
    }

    pub fn identity(&self) -> &TrashIdentity {
        &self.identity
    }

    pub fn uri(&self) -> &TrashUri {
        self.identity.uri()
    }

    pub fn display_name(&self) -> &str {
        &self.display_name
    }

    pub fn is_dir(&self) -> bool {
        self.is_dir
    }

    pub fn icon_name(&self) -> Option<&str> {
        self.icon_name.as_deref()
    }

    pub fn original(&self) -> Option<&Path> {
        self.identity.original()
    }

    pub fn deleted_at(&self) -> Option<i64> {
        self.identity.deleted_at()
    }

    pub fn can_restore(&self) -> bool {
        self.identity.original().is_some()
    }
}

/// A backend for the GIO trash namespace. The production implementation is
/// [`GioTrash`]; tests inject a scratch implementation.
pub trait TrashBackend: Send {
    fn list(&self) -> Result<Vec<TrashItem>, TrashError>;

    /// Re-read one identity and refuse it if the URI or any evidence it
    /// carries changed. Used before a restore.
    fn revalidate(&self, item: &TrashIdentity) -> Result<TrashItem, TrashError>;

    /// Re-read one identity and require proof that it is the same object.
    /// Permanent deletion uses only this. A backend that cannot produce an
    /// object/revision token refuses instead of guessing from metadata.
    fn revalidate_object(&self, item: &TrashIdentity) -> Result<TrashItem, TrashError> {
        let fresh = self.revalidate(item)?;
        if item.same_object(fresh.identity()) {
            return Ok(fresh);
        }
        Err(
            if item.revision().is_none() || fresh.identity().revision().is_none() {
                TrashError::Unverifiable
            } else {
                TrashError::Changed
            },
        )
    }

    /// Move one trashed item back to its parent. The caller has checked the
    /// parent and decided any target conflict. A failure leaves the source.
    fn restore(
        &self,
        item: &TrashIdentity,
        target: &Path,
        cancellable: &gio::Cancellable,
    ) -> Result<(), String>;

    /// Permanently delete exactly one trash entry through the backend.
    fn delete(&self, item: &TrashIdentity, cancellable: &gio::Cancellable) -> Result<(), String>;
}

/// Why the trash could not be listed or revalidated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashError {
    /// No GVfs trash backend, or a mounted volume is unplugged.
    Unavailable(String),
    /// The item is no longer there under the same identity.
    Missing,
    /// The item's metadata or object token no longer matches the snapshot.
    Changed,
    /// The backend has no object/revision evidence for this item, so it can
    /// never be permanently deleted from a snapshot.
    Unverifiable,
    Io(String),
}

impl fmt::Display for TrashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrashError::Unavailable(reason) => write!(f, "the trash is unavailable: {reason}"),
            TrashError::Missing => f.write_str("the item is no longer in the trash"),
            TrashError::Changed => f.write_str("the item changed in the trash"),
            TrashError::Unverifiable => f.write_str("Cannot verify this Trash item is unchanged"),
            TrashError::Io(reason) => write!(f, "the trash request failed: {reason}"),
        }
    }
}

impl std::error::Error for TrashError {}

const LIST_ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::icon,trash::orig-path,trash::deletion-date,etag::value";

/// The production GIO/GVfs trash backend.
#[derive(Debug, Default, Clone, Copy)]
pub struct GioTrash;

impl GioTrash {
    pub fn new() -> Self {
        Self
    }

    fn root() -> gio::File {
        gio::File::for_uri(TRASH_ROOT_URI)
    }

    fn item_from_info(&self, root: &gio::File, info: &gio::FileInfo) -> Option<TrashItem> {
        let name = info.name();
        let child = root.child(&name);
        let uri = TrashUri::parse(child.uri().as_str()).ok()?;
        let original = info
            .attribute_byte_string(gio::FILE_ATTRIBUTE_TRASH_ORIG_PATH.as_str())
            .map(|text| PathBuf::from(OsString::from_vec(text.as_bytes().to_vec())))
            .filter(|path| path.is_absolute());
        let deleted_at = info.deletion_date().map(|date| date.to_unix());
        let revision = info
            .etag()
            .filter(|etag| !etag.as_str().is_empty())
            .map(|etag| TrashRevision::new(etag.as_str()));
        let icon_name = info.icon().and_then(|icon| {
            icon.downcast::<gio::ThemedIcon>()
                .ok()
                .and_then(|icon| icon.names().first().map(|name| name.to_string()))
        });
        Some(TrashItem::new(
            TrashIdentity::new(uri, original, deleted_at, revision),
            info.display_name().to_string(),
            info.file_type() == gio::FileType::Directory,
            icon_name,
        ))
    }
}

impl TrashBackend for GioTrash {
    fn list(&self) -> Result<Vec<TrashItem>, TrashError> {
        let root = Self::root();
        let enumerator = root
            .enumerate_children(
                LIST_ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )
            .map_err(|error| TrashError::Unavailable(error.to_string()))?;
        let mut items = Vec::new();
        while let Some(info) = enumerator
            .next_file(gio::Cancellable::NONE)
            .map_err(|error| TrashError::Io(error.to_string()))?
        {
            if let Some(item) = self.item_from_info(&root, &info) {
                items.push(item);
            }
        }
        items.sort_by(|left, right| {
            right
                .deleted_at()
                .cmp(&left.deleted_at())
                .then_with(|| left.display_name().cmp(right.display_name()))
        });
        Ok(items)
    }

    fn revalidate(&self, item: &TrashIdentity) -> Result<TrashItem, TrashError> {
        let root = Self::root();
        let info = item
            .uri()
            .file()
            .query_info(
                LIST_ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )
            .map_err(|error| match error.matches(gio::IOErrorEnum::NotFound) {
                true => TrashError::Missing,
                false => TrashError::Unavailable(error.to_string()),
            })?;
        let fresh = self
            .item_from_info(&root, &info)
            .ok_or(TrashError::Changed)?;
        if !item.agrees_with(fresh.identity()) {
            return Err(TrashError::Changed);
        }
        Ok(fresh)
    }

    fn restore(
        &self,
        item: &TrashIdentity,
        target: &Path,
        cancellable: &gio::Cancellable,
    ) -> Result<(), String> {
        // Nautilus restores by moving the `trash:///` item to the original
        // path (nautilus-file-undo-operations.c:1811). No fallback: a restore
        // must be a move, not a copy that leaves a second trash entry behind.
        item.uri()
            .file()
            .move_(
                &gio::File::for_path(target),
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE,
                Some(cancellable),
                None,
            )
            .map_err(|error| error.to_string())
    }

    fn delete(&self, item: &TrashIdentity, cancellable: &gio::Cancellable) -> Result<(), String> {
        // Nautilus empties the trash with g_file_delete on top-level items and
        // relies on the trash backend to recurse without modifying entries
        // (nautilus-file-operations.c:7342-7393). We ask the same backend and
        // never fall back to a local path.
        item.uri()
            .file()
            .delete(Some(cancellable))
            .map_err(|error| error.to_string())
    }
}

/// List the production trash.
pub fn list_trash() -> Result<Vec<TrashItem>, TrashError> {
    GioTrash::new().list()
}

/// List through an injected backend. Used by the core tests and by a caller
/// that owns a scratch backend.
pub fn list_trash_with(backend: &dyn TrashBackend) -> Result<Vec<TrashItem>, TrashError> {
    backend.list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trash_uris_accept_one_canonical_item_and_reject_the_root() {
        let uri = TrashUri::parse("trash:///a%20file.txt").expect("a valid item");
        assert_eq!(uri.as_str(), "trash:///a%20file.txt");
        assert_eq!(uri.name(), "a file.txt");

        let raw = TrashUri::parse("trash:///a:b@c!d,e=f").expect("GIO leaves pchar raw");
        assert_eq!(raw.name(), "a:b@c!d,e=f");
        assert!(TrashUri::parse("trash:///a%3Ab").is_err());

        for bad in [
            "trash:///",
            "trash://",
            "trash:////",
            "trash:///a/b",
            "file:///tmp/a",
            "TRASH:///a",
            "trash:///.",
            "trash:///..",
            "trash:///a%2Fb",
            "trash:///a%00b",
            "trash:///a%2fb",
            "trash:///a%41",
            "trash:///a b",
            "trash:///a%zz",
            "trash:///a?x=1",
            "trash:///a#frag",
        ] {
            assert!(TrashUri::parse(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn encoded_non_utf8_names_survive_round_trip() {
        let name = OsString::from_vec(vec![b'f', 0xFF, b'.', b't', b'x', b't']);
        let uri = TrashUri::from_name(&name).expect("a non-UTF-8 name is encodable");
        assert_eq!(uri.as_str(), "trash:///f%FF.txt");
        assert_eq!(uri.name(), name);
        assert_eq!(TrashUri::parse(uri.as_str()), Ok(uri));
    }

    #[test]
    fn a_missing_date_does_not_make_an_identity_changed() {
        let uri = TrashUri::parse("trash:///a.txt").unwrap();
        let revision = TrashRevision::new("rev-1");
        let snapshot = TrashIdentity::new(
            uri.clone(),
            Some(PathBuf::from("/home/k/a.txt")),
            None,
            Some(revision.clone()),
        );
        let fresh = TrashIdentity::new(
            uri.clone(),
            Some(PathBuf::from("/home/k/a.txt")),
            Some(1_700_000_000),
            Some(revision.clone()),
        );
        assert!(snapshot.agrees_with(&fresh));
        assert!(snapshot.same_object(&fresh), "the token is stable");

        let changed = TrashIdentity::new(
            uri.clone(),
            Some(PathBuf::from("/home/k/other.txt")),
            None,
            Some(revision),
        );
        assert!(!snapshot.agrees_with(&changed));
        assert!(
            snapshot.same_object(&changed),
            "the token still names the same object; metadata is not the proof"
        );
    }

    #[test]
    fn a_lost_or_changed_token_is_not_object_evidence() {
        let uri = TrashUri::parse("trash:///a.txt").unwrap();
        let snapshot =
            TrashIdentity::new(uri.clone(), None, None, Some(TrashRevision::new("rev-1")));
        let lost = TrashIdentity::new(uri.clone(), None, None, None);
        let replaced = TrashIdentity::new(uri, None, None, Some(TrashRevision::new("rev-2")));
        assert!(!snapshot.same_object(&lost));
        assert!(!snapshot.same_object(&replaced));
        assert!(!snapshot.agrees_with(&lost));
        assert!(!snapshot.agrees_with(&replaced));
    }
}
