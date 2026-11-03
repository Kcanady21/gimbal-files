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
//!
//! The checked top-level deletion boundary follows the approach in Thunar
//! 4.20.7 (GPL-3.0-or-later, `thunar-io-jobs.c:95-102` and
//! `thunar-io-scan-directory.c:79-90`) as an independent Rust implementation;
//! no C source was copied.

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

/// The file type recorded in a trash fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrashFileKind {
    Regular,
    Directory,
    SymbolicLink,
    Special,
    Shortcut,
    Mountable,
}

impl TrashFileKind {
    fn from_gio(kind: gio::FileType) -> Option<Self> {
        match kind {
            gio::FileType::Regular => Some(Self::Regular),
            gio::FileType::Directory => Some(Self::Directory),
            gio::FileType::SymbolicLink => Some(Self::SymbolicLink),
            gio::FileType::Special => Some(Self::Special),
            gio::FileType::Shortcut => Some(Self::Shortcut),
            gio::FileType::Mountable => Some(Self::Mountable),
            _ => None,
        }
    }
}

/// One precise Unix timestamp from a GIO attribute set.
///
/// GLib exposes seconds, microseconds and nanoseconds separately. `nanos` is
/// the nanosecond value when the backend supplies it, otherwise the
/// microsecond value converted to nanoseconds. A backend that supplies only
/// seconds still yields a usable coarser fingerprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TrashTimestamp {
    seconds: i64,
    nanos: Option<u32>,
}

impl TrashTimestamp {
    fn from_info(
        info: &gio::FileInfo,
        seconds_attribute: &str,
        usec_attribute: &str,
        nsec_attribute: &str,
    ) -> Option<Self> {
        let seconds = i64::try_from(attribute_u64(info, seconds_attribute)?).ok()?;
        let nanos = attribute_u32(info, nsec_attribute)
            .or_else(|| attribute_u32(info, usec_attribute).map(|usec| usec.saturating_mul(1000)));
        Some(Self { seconds, nanos })
    }

    /// Compare the finest precision present on both sides. A missing
    /// sub-second field is not a change; the seconds still have to match.
    fn same_instant(self, other: Self) -> bool {
        self.seconds == other.seconds
            && match (self.nanos, other.nanos) {
                (Some(left), Some(right)) => left == right,
                _ => true,
            }
    }
}

/// A structured object and revision fingerprint supplied through a trash URI.
///
/// GVfs 1.60.0 enumerates and queries top-level trash items by forwarding the
/// requested attributes to the real trashed file
/// (`daemon/gvfsbackendtrash.c:635-643`, `:783-793`). The locally observed
/// backend returned `id::file`, `id::filesystem`, `unix::device`,
/// `unix::inode`, precise `time::changed`, `etag::value` and
/// `standard::target-uri`.
///
/// The supported identity combinations are deliberately explicit:
///
/// - `id::file` plus `id::filesystem`, both present and equal; or
/// - `unix::device` plus `unix::inode`, both present and equal.
///
/// `etag::value` is supplementary revision evidence, never an object ID.
/// GLib 2.88.0 derives it only from mtime seconds/usec/nsec
/// (`gio/glocalfileinfo.c:114-138`), so two different files with the same
/// mtime can share an ETag. `standard::target-uri` names the native trashed
/// file in this backend, never an original deletion destination, and is only
/// compared when both sides provide it. Original path and deletion date are
/// display/restore metadata and are not part of this fingerprint.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TrashFingerprint {
    id_file: Option<OsString>,
    id_filesystem: Option<OsString>,
    device: Option<u64>,
    inode: Option<u64>,
    kind: Option<TrashFileKind>,
    changed: Option<TrashTimestamp>,
    modified: Option<TrashTimestamp>,
    size: Option<u64>,
    target_uri: Option<OsString>,
    etag: Option<TrashRevision>,
}

impl TrashFingerprint {
    /// Read the production fingerprint from one `GFileInfo`.
    ///
    /// Presence and attribute status are checked: an absent attribute is
    /// `None`, never a zero or empty value. `NOFOLLOW_SYMLINKS` on the query
    /// makes these attributes describe the trashed entry itself.
    pub fn from_file_info(info: &gio::FileInfo) -> Self {
        let kind = match info.attribute_type(gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str()) {
            gio::FileAttributeType::Uint32 | gio::FileAttributeType::Int32
                if attribute_present(info, gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str()) =>
            {
                TrashFileKind::from_gio(info.file_type())
            }
            _ => None,
        };
        Self {
            id_file: attribute_string(info, gio::FILE_ATTRIBUTE_ID_FILE.as_str())
                .map(OsString::from),
            id_filesystem: attribute_string(info, gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str())
                .map(OsString::from),
            device: attribute_u64(info, gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str()),
            inode: attribute_u64(info, gio::FILE_ATTRIBUTE_UNIX_INODE.as_str()),
            kind,
            changed: TrashTimestamp::from_info(
                info,
                gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(),
                gio::FILE_ATTRIBUTE_TIME_CHANGED_USEC.as_str(),
                TIME_CHANGED_NSEC,
            ),
            modified: TrashTimestamp::from_info(
                info,
                gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(),
                gio::FILE_ATTRIBUTE_TIME_MODIFIED_USEC.as_str(),
                TIME_MODIFIED_NSEC,
            ),
            size: attribute_u64(info, gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str()),
            target_uri: attribute_string(info, gio::FILE_ATTRIBUTE_STANDARD_TARGET_URI.as_str())
                .map(OsString::from),
            etag: attribute_string(info, gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
                .filter(|etag| !etag.is_empty())
                .map(TrashRevision::new),
        }
    }

    /// Whether this value carries one of the supported object identity
    /// combinations on its own.
    pub fn has_supported_identity(&self) -> bool {
        (self.id_file.is_some() && self.id_filesystem.is_some())
            || (self.device.is_some() && self.inode.is_some())
    }

    /// The supplementary ETag, if the backend supplied one.
    pub fn etag(&self) -> Option<&TrashRevision> {
        self.etag.as_ref()
    }

    /// Compare two fingerprints.
    ///
    /// Any identity component present on both sides must agree. Sameness
    /// requires a complete supported identity pair, not an ETag. Available
    /// change evidence is compared when both sides provide it: precise ctime
    /// first, then mtime and size. A missing optional field is not itself a
    /// change.
    pub fn compare_object(&self, fresh: &Self) -> ObjectComparison {
        if let (Some(left), Some(right)) = (&self.id_file, &fresh.id_file)
            && left != right
        {
            return ObjectComparison::Changed;
        }
        if let (Some(left), Some(right)) = (&self.id_filesystem, &fresh.id_filesystem)
            && left != right
        {
            return ObjectComparison::Changed;
        }
        if let (Some(left), Some(right)) = (self.device, fresh.device)
            && left != right
        {
            return ObjectComparison::Changed;
        }
        if let (Some(left), Some(right)) = (self.inode, fresh.inode)
            && left != right
        {
            return ObjectComparison::Changed;
        }
        if let (Some(left), Some(right)) = (self.kind, fresh.kind)
            && left != right
        {
            return ObjectComparison::Changed;
        }
        if let (Some(left), Some(right)) = (&self.target_uri, &fresh.target_uri)
            && left != right
        {
            return ObjectComparison::Changed;
        }

        let file_pair = self.id_file.is_some()
            && self.id_filesystem.is_some()
            && fresh.id_file.is_some()
            && fresh.id_filesystem.is_some();
        let device_pair = self.device.is_some()
            && self.inode.is_some()
            && fresh.device.is_some()
            && fresh.inode.is_some();
        let established = file_pair || device_pair;

        if let (Some(left), Some(right)) = (self.changed, fresh.changed) {
            if !left.same_instant(right) {
                return ObjectComparison::Changed;
            }
        } else if let (Some(left), Some(right)) = (self.modified, fresh.modified) {
            if !left.same_instant(right) {
                return ObjectComparison::Changed;
            }
            if let (Some(left), Some(right)) = (self.size, fresh.size)
                && left != right
            {
                return ObjectComparison::Changed;
            }
        }

        // ETag is only an extra revision check, never the identity. A changed
        // ETag on an otherwise matching object is still observed change.
        if let (Some(left), Some(right)) = (&self.etag, &fresh.etag)
            && left != right
        {
            return ObjectComparison::Changed;
        }

        if established {
            ObjectComparison::Same
        } else {
            ObjectComparison::Unverifiable
        }
    }
}

/// The result of comparing a snapshot fingerprint with a fresh one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectComparison {
    /// A supported identity pair and all available evidence agree.
    Same,
    /// A present identity component or available change evidence differs.
    Changed,
    /// Neither side supplies a complete supported identity pair.
    Unverifiable,
}

fn attribute_present(info: &gio::FileInfo, name: &str) -> bool {
    // `attribute_status` reports UNSET for attributes that are present but
    // were populated by `g_file_info_set_attribute_*` (and for some backends
    // after a query). `has_attribute` is the presence check; the typed getter
    // then rejects an absent or wrongly typed value.
    info.has_attribute(name)
}

fn attribute_string(info: &gio::FileInfo, name: &str) -> Option<String> {
    if !attribute_present(info, name) || info.attribute_type(name) != gio::FileAttributeType::String
    {
        return None;
    }
    info.attribute_string(name)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_string())
}

fn attribute_u32(info: &gio::FileInfo, name: &str) -> Option<u32> {
    if !attribute_present(info, name) {
        return None;
    }
    match info.attribute_type(name) {
        gio::FileAttributeType::Uint32 => Some(info.attribute_uint32(name)),
        gio::FileAttributeType::Int32 => u32::try_from(info.attribute_int32(name)).ok(),
        gio::FileAttributeType::Uint64 => u32::try_from(info.attribute_uint64(name)).ok(),
        _ => None,
    }
}

fn attribute_u64(info: &gio::FileInfo, name: &str) -> Option<u64> {
    if !attribute_present(info, name) {
        return None;
    }
    // GLib sets `unix::device` as uint32 but `unix::inode` and timestamps as
    // uint64. Read each attribute's actual type; never coerce an absent one.
    match info.attribute_type(name) {
        gio::FileAttributeType::Uint64 => Some(info.attribute_uint64(name)),
        gio::FileAttributeType::Uint32 => Some(u64::from(info.attribute_uint32(name))),
        gio::FileAttributeType::Int64 => u64::try_from(info.attribute_int64(name)).ok(),
        gio::FileAttributeType::Int32 => u64::try_from(info.attribute_int32(name)).ok(),
        _ => None,
    }
}

/// One supplementary ETag revision token for a trash entry.
///
/// GIO documents `etag::value` as an opaque current-version string
/// (`gio-0.22.10:src/auto/constants.rs:52-54`). It is not an object ID: GLib
/// 2.88.0's native ETag contains only mtime seconds/usec/nsec
/// (`gio/glocalfileinfo.c:114-138`), so equal mtimes can produce equal ETags
/// for different files. This type is therefore stored inside
/// [`TrashFingerprint`] and never used on its own to admit permanent deletion.
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
/// an optional structured backend fingerprint.
///
/// The original path and deletion date are display and restore metadata. They
/// are not proof that an object is unchanged: an item with the same URI,
/// original path and second-resolution deletion date can still be a later
/// replacement. Only [`TrashIdentity::same_object`] may authorise permanent
/// deletion, and it requires a complete supported fingerprint pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TrashIdentity {
    uri: TrashUri,
    original: Option<PathBuf>,
    deleted_at: Option<i64>,
    fingerprint: Option<TrashFingerprint>,
}

impl TrashIdentity {
    pub fn new(
        uri: TrashUri,
        original: Option<PathBuf>,
        deleted_at: Option<i64>,
        fingerprint: Option<TrashFingerprint>,
    ) -> Self {
        Self {
            uri,
            original,
            deleted_at,
            fingerprint,
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

    pub fn fingerprint(&self) -> Option<&TrashFingerprint> {
        self.fingerprint.as_ref()
    }

    /// The supplementary ETag from the fingerprint, if one was supplied.
    pub fn revision(&self) -> Option<&TrashRevision> {
        self.fingerprint.as_ref().and_then(TrashFingerprint::etag)
    }

    /// Compare this snapshot with a fresh identity through their structured
    /// fingerprints.
    pub fn compare_object(&self, fresh: &TrashIdentity) -> ObjectComparison {
        if self.uri != fresh.uri {
            return ObjectComparison::Changed;
        }
        match (&self.fingerprint, fresh.fingerprint()) {
            (Some(current), Some(fresh)) => current.compare_object(fresh),
            _ => ObjectComparison::Unverifiable,
        }
    }

    /// Whether this structured fingerprint proves that `fresh` is the same
    /// trash object. Both sides need a complete supported identity pair; a
    /// missing or changed component is not a match. Optional original/date
    /// metadata and ETag are deliberately not the proof.
    pub fn same_object(&self, fresh: &TrashIdentity) -> bool {
        self.compare_object(fresh) == ObjectComparison::Same
    }

    /// Whether a restore request still describes `fresh` closely enough to
    /// act on it. A request made only of a URI (the registry's argument) is
    /// accepted; any fingerprint or metadata it does carry must agree, and a
    /// snapshot date disappearing is a change.
    pub fn agrees_with(&self, fresh: &TrashIdentity) -> bool {
        if self.uri != fresh.uri {
            return false;
        }
        if self.fingerprint.is_some() && self.compare_object(fresh) != ObjectComparison::Same {
            return false;
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

    /// Query one item through its validated trash URI. This is the only
    /// read used by revalidation and checked deletion.
    fn query_item(&self, uri: &TrashUri) -> Result<TrashItem, TrashError>;

    /// Re-read one identity and refuse it if the URI or any evidence it
    /// carries changed. Used before a restore.
    fn revalidate(&self, item: &TrashIdentity) -> Result<TrashItem, TrashError> {
        let fresh = self.query_item(item.uri())?;
        if !item.agrees_with(fresh.identity()) {
            return Err(TrashError::Changed);
        }
        Ok(fresh)
    }

    /// Re-read one identity and require a complete supported fingerprint
    /// match. Permanent deletion uses only this.
    fn revalidate_object(&self, item: &TrashIdentity) -> Result<TrashItem, TrashError> {
        let fresh = self.query_item(item.uri())?;
        match item.compare_object(fresh.identity()) {
            ObjectComparison::Same => Ok(fresh),
            ObjectComparison::Changed => Err(TrashError::Changed),
            ObjectComparison::Unverifiable => Err(TrashError::Unverifiable),
        }
    }

    /// Move one trashed item back to its parent. The caller has checked the
    /// parent and decided any target conflict. A failure leaves the source.
    fn restore(
        &self,
        item: &TrashIdentity,
        target: &Path,
        cancellable: &gio::Cancellable,
    ) -> Result<(), String>;

    /// Revalidate the exact confirmed identity and then permanently delete
    /// that one top-level trash URI. The query and GIO delete live inside this
    /// one checked operation; there is deliberately no unconditional public
    /// production delete path for the executor to call by mistake.
    fn delete_checked(
        &self,
        item: &TrashIdentity,
        cancellable: &gio::Cancellable,
    ) -> Result<(), TrashError>;
}

/// Why the trash could not be listed or revalidated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrashError {
    /// No GVfs trash backend, or a mounted volume is unplugged.
    Unavailable(String),
    /// The item is no longer there under the same identity.
    Missing,
    /// The item's metadata or fingerprint no longer matches the snapshot.
    Changed,
    /// The backend has no complete supported identity for this item, so it
    /// can never be permanently deleted from a snapshot.
    Unverifiable,
    /// The backend honoured cancellation before completing deletion.
    Cancelled,
    Io(String),
}

impl fmt::Display for TrashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrashError::Unavailable(reason) => write!(f, "the trash is unavailable: {reason}"),
            TrashError::Missing => f.write_str("the item is no longer in the trash"),
            TrashError::Changed => f.write_str("the item changed in the trash"),
            TrashError::Unverifiable => f.write_str("Cannot verify this Trash item is unchanged"),
            TrashError::Cancelled => f.write_str("the trash operation was cancelled"),
            TrashError::Io(reason) => write!(f, "the trash request failed: {reason}"),
        }
    }
}

impl std::error::Error for TrashError {}

/// GIO 0.22.10 has no typed constants for the nanosecond fields, although
/// GLib 2.88.0 supplies `time::changed-nsec` and `time::modified-nsec`.
const TIME_CHANGED_NSEC: &str = "time::changed-nsec";
const TIME_MODIFIED_NSEC: &str = "time::modified-nsec";

const LIST_ATTRIBUTES: &str = "standard::name,standard::display-name,standard::type,standard::size,standard::icon,standard::target-uri,trash::orig-path,trash::deletion-date,id::file,id::filesystem,unix::device,unix::inode,time::changed,time::changed-usec,time::changed-nsec,time::modified,time::modified-usec,time::modified-nsec,etag::value";

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
        let child = root.child(info.name());
        let uri = TrashUri::parse(child.uri().as_str()).ok()?;
        Some(self.item_for_uri(uri, info))
    }

    /// A query's native metadata name need not equal the escaped Trash name
    /// supplied by root enumeration. Preserve the URI that was queried.
    fn item_for_uri(&self, uri: TrashUri, info: &gio::FileInfo) -> TrashItem {
        let original = info
            .attribute_byte_string(gio::FILE_ATTRIBUTE_TRASH_ORIG_PATH.as_str())
            .map(|text| PathBuf::from(OsString::from_vec(text.as_bytes().to_vec())))
            .filter(|path| path.is_absolute());
        let deleted_at = info.deletion_date().map(|date| date.to_unix());
        let fingerprint = TrashFingerprint::from_file_info(info);
        let icon_name = info.icon().and_then(|icon| {
            icon.downcast::<gio::ThemedIcon>()
                .ok()
                .and_then(|icon| icon.names().first().map(|name| name.to_string()))
        });
        TrashItem::new(
            TrashIdentity::new(uri, original, deleted_at, Some(fingerprint)),
            info.display_name().to_string(),
            info.file_type() == gio::FileType::Directory,
            icon_name,
        )
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

    fn query_item(&self, uri: &TrashUri) -> Result<TrashItem, TrashError> {
        let info = uri
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
        Ok(self.item_for_uri(uri.clone(), &info))
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

    fn delete_checked(
        &self,
        item: &TrashIdentity,
        cancellable: &gio::Cancellable,
    ) -> Result<(), TrashError> {
        // Thunar's worker deletes a trashed top-level item with
        // `g_file_delete` and relies on GVfs to remove that item's contents
        // (thunar-io-jobs.c:95-102, thunar-io-scan-directory.c:79-90). The
        // query immediately before the same call is the final best-effort
        // identity check; GIO has no atomic compare-and-delete.
        self.revalidate_object(item)?;
        item.uri()
            .file()
            .delete(Some(cancellable))
            .map_err(deletion_error)
    }
}

fn deletion_error(error: glib::Error) -> TrashError {
    if error.matches(gio::IOErrorEnum::Cancelled) {
        TrashError::Cancelled
    } else if error.matches(gio::IOErrorEnum::NotFound) {
        TrashError::Missing
    } else {
        TrashError::Io(error.to_string())
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
    use glib::translate::IntoGlib;
    use std::fs::{self, File, FileTimes};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, UNIX_EPOCH};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    struct MetadataScratch {
        path: PathBuf,
    }

    impl MetadataScratch {
        fn new(name: &str) -> Self {
            let number = COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir)
                .join(".cache/gimbal-tests")
                .join(format!(
                    "p2k2-r3-metadata-{}-{number}-{name}",
                    std::process::id()
                ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for MetadataScratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn queried_metadata_preserves_the_requested_trash_uri() {
        // GVfs root enumeration supplies its escaped Trash name, but an
        // item query forwards the native FileInfo name. External-volume and
        // specially escaped home entries must retain the requested URI.
        let requested =
            TrashUri::from_name(OsStr::new(r"\run\media\test\drive\.Trash-1000\files\a.txt"))
                .unwrap();
        let info = gio::FileInfo::new();
        info.set_name("a.txt");
        info.set_display_name("a.txt");
        info.set_file_type(gio::FileType::Regular);
        info.set_icon(&gio::ThemedIcon::new("text-x-generic"));
        let item = GioTrash::new().item_for_uri(requested.clone(), &info);
        assert_eq!(item.uri(), &requested);
    }

    #[test]
    fn a_cancelled_gio_delete_retains_its_cancellation_outcome() {
        let error = glib::Error::new(gio::IOErrorEnum::Cancelled, "cancelled");
        assert_eq!(deletion_error(error), TrashError::Cancelled);
    }

    fn test_info() -> gio::FileInfo {
        gio::FileInfo::new()
    }

    fn synthetic_fingerprint(generation: &str) -> TrashFingerprint {
        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), generation);
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 1);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), 42);
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(gio::FILE_ATTRIBUTE_TIME_CHANGED_USEC.as_str(), 123_456);
        info.set_attribute_uint32(TIME_CHANGED_NSEC, 123_456_789);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(gio::FILE_ATTRIBUTE_TIME_MODIFIED_USEC.as_str(), 123_456);
        info.set_attribute_uint32(TIME_MODIFIED_NSEC, 123_456_789);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(), 7);
        info.set_attribute_string(
            gio::FILE_ATTRIBUTE_STANDARD_TARGET_URI.as_str(),
            "file:///tmp/a",
        );
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str(), generation);
        TrashFingerprint::from_file_info(&info)
    }

    fn query_native(path: &Path) -> TrashFingerprint {
        let info = gio::File::for_path(path)
            .query_info(
                LIST_ATTRIBUTES,
                gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
            )
            .unwrap();
        TrashFingerprint::from_file_info(&info)
    }

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
    fn supported_fingerprint_combinations_are_explicit() {
        let uri = TrashUri::parse("trash:///a.txt").unwrap();
        let file_and_fs = synthetic_fingerprint("l1:42");
        assert!(file_and_fs.has_supported_identity());
        let same = TrashIdentity::new(uri.clone(), None, None, Some(file_and_fs.clone()));
        let fresh = TrashIdentity::new(uri.clone(), None, None, Some(file_and_fs.clone()));
        assert_eq!(same.compare_object(&fresh), ObjectComparison::Same);

        // Device and inode alone are the documented fallback pair.
        let info = test_info();
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 7);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), 99);
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(TIME_CHANGED_NSEC, 5);
        let dev_inode = TrashFingerprint::from_file_info(&info);
        assert!(dev_inode.has_supported_identity());
        assert_eq!(dev_inode.compare_object(&dev_inode), ObjectComparison::Same);

        // Different pair kinds do not establish sameness just because each
        // side has some supported identity on its own.
        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), "l1:42");
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        let file_pair_only = TrashFingerprint::from_file_info(&info);
        let info = test_info();
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_DEVICE.as_str(), 1);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_UNIX_INODE.as_str(), 42);
        let device_pair_only = TrashFingerprint::from_file_info(&info);
        assert_eq!(
            file_pair_only.compare_object(&device_pair_only),
            ObjectComparison::Unverifiable
        );

        // A lone ETag or a partial pair is not identity.
        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str(), "same-etag");
        let etag_only = TrashFingerprint::from_file_info(&info);
        assert!(!etag_only.has_supported_identity());
        assert_eq!(
            etag_only.compare_object(&etag_only),
            ObjectComparison::Unverifiable
        );

        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), "l1:42");
        let file_only = TrashFingerprint::from_file_info(&info);
        assert!(!file_only.has_supported_identity());
        assert_eq!(
            file_only.compare_object(&file_only),
            ObjectComparison::Unverifiable
        );
    }

    #[test]
    fn equal_identity_requires_equal_precise_change_evidence() {
        let base = synthetic_fingerprint("l1:42");
        assert_eq!(base.compare_object(&base), ObjectComparison::Same);

        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), "l1:42");
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_CHANGED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(TIME_CHANGED_NSEC, 1);
        let changed = TrashFingerprint::from_file_info(&info);
        assert_eq!(base.compare_object(&changed), ObjectComparison::Changed);
    }

    #[test]
    fn mtime_and_size_are_the_fallback_when_ctime_is_absent() {
        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), "l1:42");
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(TIME_MODIFIED_NSEC, 9);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(), 7);
        let base = TrashFingerprint::from_file_info(&info);
        assert_eq!(base.compare_object(&base), ObjectComparison::Same);

        let info = test_info();
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILE.as_str(), "l1:42");
        info.set_attribute_string(gio::FILE_ATTRIBUTE_ID_FILESYSTEM.as_str(), "l1");
        info.set_attribute_uint32(
            gio::FILE_ATTRIBUTE_STANDARD_TYPE.as_str(),
            u32::try_from(gio::FileType::Regular.into_glib()).unwrap(),
        );
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_TIME_MODIFIED.as_str(), 1_700_000_000);
        info.set_attribute_uint32(TIME_MODIFIED_NSEC, 9);
        info.set_attribute_uint64(gio::FILE_ATTRIBUTE_STANDARD_SIZE.as_str(), 8);
        let resized = TrashFingerprint::from_file_info(&info);
        assert_eq!(base.compare_object(&resized), ObjectComparison::Changed);
    }

    #[test]
    fn equal_etags_and_mtimes_on_different_native_objects_do_not_match() {
        let scratch = MetadataScratch::new("same-mtime");
        let a = scratch.path.join("a.txt");
        let b = scratch.path.join("b.txt");
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();
        let stamp = UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789);
        for path in [&a, &b] {
            File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(FileTimes::new().set_modified(stamp))
                .unwrap();
        }

        let first = query_native(&a);
        let second = query_native(&b);
        assert_eq!(
            first.etag(),
            second.etag(),
            "GLib derives a native ETag from mtime, so these collide"
        );
        assert_ne!(
            first.id_file, second.id_file,
            "the object IDs and inodes still differ"
        );

        let uri = TrashUri::parse("trash:///reused.txt").unwrap();
        let snapshot = TrashIdentity::new(uri.clone(), None, None, Some(first));
        let replacement = TrashIdentity::new(uri, None, None, Some(second));
        assert!(!snapshot.same_object(&replacement));
    }

    #[test]
    fn a_missing_date_does_not_make_an_identity_changed() {
        let uri = TrashUri::parse("trash:///a.txt").unwrap();
        let fingerprint = synthetic_fingerprint("l1:42");
        let snapshot = TrashIdentity::new(
            uri.clone(),
            Some(PathBuf::from("/home/k/a.txt")),
            None,
            Some(fingerprint.clone()),
        );
        let fresh = TrashIdentity::new(
            uri.clone(),
            Some(PathBuf::from("/home/k/a.txt")),
            Some(1_700_000_000),
            Some(fingerprint.clone()),
        );
        assert!(snapshot.agrees_with(&fresh));
        assert!(snapshot.same_object(&fresh), "the fingerprint is stable");

        let changed = TrashIdentity::new(
            uri,
            Some(PathBuf::from("/home/k/other.txt")),
            None,
            Some(fingerprint),
        );
        assert!(!snapshot.agrees_with(&changed));
        assert!(
            snapshot.same_object(&changed),
            "the fingerprint still names the same object; metadata is not the proof"
        );
    }

    #[test]
    fn a_lost_or_changed_fingerprint_is_not_object_evidence() {
        let uri = TrashUri::parse("trash:///a.txt").unwrap();
        let snapshot = TrashIdentity::new(
            uri.clone(),
            None,
            None,
            Some(synthetic_fingerprint("l1:42")),
        );
        let lost = TrashIdentity::new(uri.clone(), None, None, None);
        let replaced = TrashIdentity::new(uri, None, None, Some(synthetic_fingerprint("l1:43")));
        assert!(!snapshot.same_object(&lost));
        assert!(!snapshot.same_object(&replaced));
        assert!(!snapshot.agrees_with(&lost));
        assert!(!snapshot.agrees_with(&replaced));
    }
}
