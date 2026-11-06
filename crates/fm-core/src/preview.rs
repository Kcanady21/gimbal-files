// SPDX-License-Identifier: GPL-3.0-or-later
//! Preview selection, text decoding, limits, read/revision contracts and the
//! atomic save path.
//!
//! The UI owns widgets and worker threads; this module owns the typed states
//! that decide what a selection means, how bytes become text and how a save is
//! checked before it may replace anything. No GTK lives here (GIO is allowed
//! for file attributes, ETags and the atomic replacement).

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use gio::prelude::*;

/// The largest text file shown or edited. A larger file is refused before its
/// body is allocated.
pub const MAX_TEXT_BYTES: u64 = 2 * 1024 * 1024;
/// The largest image file read for a preview.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// The largest decoded image, in pixels. Checked from the loader's
/// `size-prepared` signal before pixel allocation.
pub const MAX_IMAGE_PIXELS: u64 = 64 * 1024 * 1024;
/// The default longest edge of a first-pass texture. The UI asks for a larger
/// device-pixel target when the allocated panel or a zoom level needs it.
pub const IMAGE_MAX_EDGE: i32 = 2048;
/// The longest edge of one rendered image/PDF texture. Larger requests are
/// scaled down and reported as a rendering limit rather than stretched.
pub const MAX_RENDER_EDGE: i32 = 8192;
/// The largest rendered texture in pixels. 16 MP is 64 MiB as RGBA8.
pub const MAX_RENDER_PIXELS: u64 = 16 * 1024 * 1024;
/// The largest PNG/thumb file read back from a PDF renderer or cache.
pub const MAX_RENDER_BYTES: u64 = 64 * 1024 * 1024;
/// Grid thumbnails may occupy at most this much decoded RGBA in memory.
pub const MAX_THUMBNAIL_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// A requested image decode size, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderSize {
    pub width: i32,
    pub height: i32,
}

/// Why a requested render was reduced. The UI shows this verbatim so a further
/// zoom click cannot claim to reveal pixels that were never rendered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderLimit {
    Edge,
    Pixels,
}

impl RenderLimit {
    pub fn message(self) -> String {
        match self {
            Self::Edge => {
                format!("Rendering limit: at most {MAX_RENDER_EDGE} pixels on the longest edge.")
            }
            Self::Pixels => format!(
                "Rendering limit: at most {} megapixels per render.",
                MAX_RENDER_PIXELS / 1_000_000
            ),
        }
    }
}

/// A bounded render target and whether the requested size had to be reduced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderPlan {
    pub size: RenderSize,
    pub limit: Option<RenderLimit>,
}

/// Contain `source` inside `box`, preserving aspect ratio. A non-positive box
/// falls back to the source size.
pub fn fit_within(source_w: i32, source_h: i32, box_w: i32, box_h: i32) -> (i32, i32) {
    if source_w <= 0 || source_h <= 0 {
        return (1, 1);
    }
    if box_w <= 0 || box_h <= 0 {
        return (source_w, source_h);
    }
    let scale = (f64::from(box_w) / f64::from(source_w))
        .min(f64::from(box_h) / f64::from(source_h))
        .min(1.0);
    (
        ((f64::from(source_w) * scale).round() as i32).max(1),
        ((f64::from(source_h) * scale).round() as i32).max(1),
    )
}

/// Fit a source width, keeping its aspect ratio (PDF fit-width and the initial
/// image decode both use this).
pub fn fit_width(source_w: i32, source_h: i32, width: i32) -> (i32, i32) {
    if source_w <= 0 || source_h <= 0 || width <= 0 {
        return (1, 1);
    }
    let scale = f64::from(width) / f64::from(source_w);
    (width, ((f64::from(source_h) * scale).round() as i32).max(1))
}

/// Turn a logical display size into device pixels for a monitor scale. Output
/// is bounded by [`MAX_RENDER_EDGE`] and [`MAX_RENDER_PIXELS`].
pub fn render_plan(display_w: i32, display_h: i32, device_scale: f64) -> RenderPlan {
    render_plan_bounded(display_w, display_h, device_scale, MAX_RENDER_EDGE)
}

/// [`render_plan`] with a tighter edge cap. PDF first-page rendering uses an
/// aspect-preserving square Poppler bound, so its honest display cap is lower
/// than the image edge limit; the widget must not enlarge the texture beyond
/// the raster it actually received.
pub fn render_plan_bounded(
    display_w: i32,
    display_h: i32,
    device_scale: f64,
    max_edge: i32,
) -> RenderPlan {
    let scale = if device_scale.is_finite() && device_scale > 0.0 {
        device_scale
    } else {
        1.0
    };
    let max_edge = max_edge.max(1);
    let mut width = (f64::from(display_w.max(1)) * scale).round().max(1.0) as i32;
    let mut height = (f64::from(display_h.max(1)) * scale).round().max(1.0) as i32;
    let mut limit = None;
    let longest = width.max(height);
    if longest > max_edge {
        limit = Some(if max_edge < MAX_RENDER_EDGE {
            // A PDF-style bound is driven by the square pixel cap.
            RenderLimit::Pixels
        } else {
            RenderLimit::Edge
        });
        let factor = f64::from(max_edge) / f64::from(longest);
        width = ((f64::from(width) * factor).round() as i32).max(1);
        height = ((f64::from(height) * factor).round() as i32).max(1);
    }
    let pixels = u64::from(width as u32) * u64::from(height as u32);
    if pixels > MAX_RENDER_PIXELS {
        limit = Some(RenderLimit::Pixels);
        let factor = (MAX_RENDER_PIXELS as f64 / pixels as f64).sqrt();
        width = ((f64::from(width) * factor).floor() as i32).max(1);
        height = ((f64::from(height) * factor).floor() as i32).max(1);
    }
    RenderPlan {
        size: RenderSize { width, height },
        limit,
    }
}

/// The logical size the UI should allocate for a texture rendered at
/// `device_scale`: one texture pixel per device pixel. When the render limit
/// bites, the logical size is reduced too, so the picture is never stretched
/// beyond the pixels that were actually rendered.
pub fn display_size_for_render(render: RenderSize, device_scale: f64) -> (i32, i32) {
    let scale = if device_scale.is_finite() && device_scale > 0.0 {
        device_scale
    } else {
        1.0
    };
    (
        ((f64::from(render.width) / scale).round() as i32).max(1),
        ((f64::from(render.height) / scale).round() as i32).max(1),
    )
}

/// One save at a time. GIO's conditional replacement is atomic per file; this
/// also keeps two windows from writing the same document at the same moment.
static SAVE_LOCK: Mutex<()> = Mutex::new(());

/// What the focused tab/tile has selected. `Trash` is the visible Trash page,
/// whose hidden folder selection is deliberately not previewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selection {
    None,
    Multiple { count: usize },
    Trash,
    File { path: PathBuf },
}

impl Selection {
    pub fn from_names(folder: &Path, names: &BTreeSet<OsString>) -> Self {
        match names.len() {
            0 => Self::None,
            1 => Self::File {
                path: folder.join(names.iter().next().expect("one name")),
            },
            count => Self::Multiple { count },
        }
    }

    pub fn file(path: impl Into<PathBuf>) -> Self {
        Self::File { path: path.into() }
    }

    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File { path } => Some(path),
            _ => None,
        }
    }
}

/// How the bytes end their lines. `None` means mixed or a lone CR: the file
/// may be shown but this first implementation will not edit it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }
}

/// The encoding facts needed to write the original bytes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextEncoding {
    pub bom: bool,
    pub line_ending: Option<LineEnding>,
}

impl TextEncoding {
    pub fn is_mixed(self) -> bool {
        self.line_ending.is_none()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextDecodeError {
    Binary,
    InvalidUtf8,
}

/// Device/inode identity plus the version evidence used by a save.
///
/// ETag is the GIO write precondition, not an object identity: local GIO
/// derives it from mtime (see `docs/notes/P2m.md`). Identity is device+inode,
/// as in the Trash work.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileRevision {
    pub device: u64,
    pub inode: u64,
    pub size: u64,
    pub modified_sec: i64,
    pub modified_nsec: i64,
    pub changed_sec: i64,
    pub changed_nsec: i64,
    pub etag: Option<String>,
}

impl FileRevision {
    pub fn from_metadata(metadata: &fs::Metadata, etag: Option<String>) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.size(),
            modified_sec: metadata.mtime(),
            modified_nsec: metadata.mtime_nsec(),
            changed_sec: metadata.ctime(),
            changed_nsec: metadata.ctime_nsec(),
            etag,
        }
    }

    pub fn same_object(&self, other: &Self) -> bool {
        self.device == other.device && self.inode == other.inode
    }

    pub fn same_version(&self, other: &Self) -> bool {
        self.same_object(other)
            && self.size == other.size
            && self.modified_sec == other.modified_sec
            && self.modified_nsec == other.modified_nsec
            && self.changed_sec == other.changed_sec
            && self.changed_nsec == other.changed_nsec
            && self.etag == other.etag
    }

    /// A short identity for read-only/dry-run text.
    pub fn identity_label(&self) -> String {
        format!("{}:{}", self.device, self.inode)
    }
}

/// The refusal for an agent, CLI or hotkey caller that tries to edit or save
/// an administrator editor.
pub const ADMIN_EDIT_UI_ONLY: &str = "administrator edits are UI-only";

/// Why a text preview cannot be edited. The UI shows this verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnlyReason {
    NotWritable,
    ParentNotWritable,
    Symlink,
    MultipleHardLinks,
    MixedLineEndings,
}

impl ReadOnlyReason {
    pub fn message(&self) -> &'static str {
        match self {
            Self::NotWritable => "Read-only: the file is not writable.",
            Self::ParentNotWritable => {
                "Read-only: the folder is not writable, so the file cannot be replaced safely."
            }
            Self::Symlink => "Read-only: this is a symbolic link; saving would replace the link.",
            Self::MultipleHardLinks => {
                "Read-only: this file has several hard links; saving would break the link."
            }
            Self::MixedLineEndings => {
                "Read-only: mixed line endings are previewed but not edited yet."
            }
        }
    }
}

/// File and parent replacement permission, frozen at load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WriteAccess {
    pub file_writable: bool,
    pub parent_writable: bool,
    pub reason: Option<ReadOnlyReason>,
}

impl WriteAccess {
    pub fn can_edit(&self) -> bool {
        self.reason.is_none() && self.file_writable && self.parent_writable
    }

    /// Whether this block is only the ordinary Unix permission check and the
    /// owner is somebody else. Symlinks, hard links and mixed endings are
    /// never administrator-editable: those blocks are about safe writing, not
    /// permission. A text document is always a regular file because `load`
    /// refuses every other kind before it builds one.
    pub fn admin_editable(&self, owner_uid: u32, current_uid: u32) -> bool {
        let permission_blocked = !self.file_writable || !self.parent_writable;
        let permission_only = match &self.reason {
            Some(ReadOnlyReason::NotWritable | ReadOnlyReason::ParentNotWritable) => true,
            None => permission_blocked,
            Some(
                ReadOnlyReason::Symlink
                | ReadOnlyReason::MultipleHardLinks
                | ReadOnlyReason::MixedLineEndings,
            ) => false,
        };
        permission_blocked && permission_only && crate::admin::needs_admin(owner_uid, current_uid)
    }
}

/// Decoded text plus the facts needed to write it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextDocument {
    pub text: String,
    pub encoding: TextEncoding,
}

impl TextDocument {
    pub fn edit_block_reason(&self, access: &WriteAccess) -> Option<ReadOnlyReason> {
        if let Some(reason) = &access.reason {
            return Some(reason.clone());
        }
        if !access.file_writable {
            return Some(ReadOnlyReason::NotWritable);
        }
        if !access.parent_writable {
            return Some(ReadOnlyReason::ParentNotWritable);
        }
        if self.encoding.is_mixed() {
            return Some(ReadOnlyReason::MixedLineEndings);
        }
        None
    }

    pub fn can_edit(&self, access: &WriteAccess) -> bool {
        self.edit_block_reason(access).is_none()
    }
}

/// The GIO thumbnail sizes. A larger cached thumbnail is preferred when the
/// requested panel or grid cell needs its detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ThumbnailKind {
    Normal,
    Large,
    XLarge,
    XXLarge,
}

impl ThumbnailKind {
    pub fn path_attribute(self) -> &'static str {
        match self {
            Self::Normal => "thumbnail::path-normal",
            Self::Large => "thumbnail::path-large",
            Self::XLarge => "thumbnail::path-xlarge",
            Self::XXLarge => "thumbnail::path-xxlarge",
        }
    }

    pub fn valid_attribute(self) -> &'static str {
        match self {
            Self::Normal => "thumbnail::is-valid-normal",
            Self::Large => "thumbnail::is-valid-large",
            Self::XLarge => "thumbnail::is-valid-xlarge",
            Self::XXLarge => "thumbnail::is-valid-xxlarge",
        }
    }

    /// The size GIO promises for this cache kind. Used only as a preference;
    /// the UI still checks the decoded dimensions.
    pub fn nominal_edge(self) -> i32 {
        match self {
            Self::Normal => 128,
            Self::Large => 256,
            Self::XLarge => 512,
            Self::XXLarge => 1024,
        }
    }
}

/// A GIO thumbnail cache file that was validated as a regular, bounded file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailSource {
    pub path: PathBuf,
    pub size: u64,
    pub kind: ThumbnailKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageDocument {
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaDocument {
    pub mime: String,
    pub thumbnail: Option<ThumbnailSource>,
    /// Every valid cache size, largest first, so a corrupt candidate falls
    /// through to the next.
    pub thumbnails: Vec<ThumbnailSource>,
}

/// A PDF preview. Page 1 may be rendered by the UI's optional Poppler helper;
/// the cached thumbnail is only a labelled fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfDocument {
    pub mime: String,
    pub thumbnail: Option<ThumbnailSource>,
    /// Every valid cache size, largest first, used only as a labelled fallback.
    pub thumbnails: Vec<ThumbnailSource>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewContent {
    Text(TextDocument),
    Image(ImageDocument),
    Media(MediaDocument),
    Pdf(PdfDocument),
}

/// One regular file that may be previewed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewDocument {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    pub revision: FileRevision,
    pub access: WriteAccess,
    pub content: PreviewContent,
}

impl PreviewDocument {
    pub fn text(&self) -> Option<&TextDocument> {
        match &self.content {
            PreviewContent::Text(text) => Some(text),
            _ => None,
        }
    }

    pub fn image(&self) -> Option<&ImageDocument> {
        match &self.content {
            PreviewContent::Image(image) => Some(image),
            _ => None,
        }
    }

    pub fn media(&self) -> Option<&MediaDocument> {
        match &self.content {
            PreviewContent::Media(media) => Some(media),
            _ => None,
        }
    }

    pub fn pdf(&self) -> Option<&PdfDocument> {
        match &self.content {
            PreviewContent::Pdf(pdf) => Some(pdf),
            _ => None,
        }
    }

    pub fn can_edit(&self) -> bool {
        self.text().is_some_and(|text| text.can_edit(&self.access))
    }

    pub fn edit_block_reason(&self) -> Option<ReadOnlyReason> {
        self.text()
            .and_then(|text| text.edit_block_reason(&self.access))
    }

    /// Whether Alt-click may enter administrator edit mode. The permission
    /// block must be the only one, the text must be editable apart from
    /// permission, and the owner must be another user.
    pub fn admin_editable(&self, owner_uid: u32, current_uid: u32) -> bool {
        self.text().is_some_and(|text| !text.encoding.is_mixed())
            && self.access.admin_editable(owner_uid, current_uid)
    }
}

/// The user name for an owner uid, for the preview's administrator banner and
/// warning. `root` is the common case; unknown ids fall back to the number.
pub fn owner_name(uid: u32) -> String {
    if uid == 0 {
        return "root".to_string();
    }
    let passwd = fs::read_to_string("/etc/passwd").unwrap_or_default();
    for line in passwd.lines() {
        let mut fields = line.split(':');
        let Some(name) = fields.next() else {
            continue;
        };
        if fields.next().is_none() {
            continue;
        }
        if !name.is_empty() && fields.next().and_then(|id| id.parse::<u32>().ok()) == Some(uid) {
            return name.to_string();
        }
    }
    uid.to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnsupportedReason {
    TextTooLarge { size: u64 },
    ImageTooLarge { size: u64 },
    PdfTooLarge { size: u64 },
    ImageDimensions { width: u32, height: u32 },
    Binary,
    InvalidUtf8,
    MediaNoThumbnail,
    SpecialFile,
}

impl UnsupportedReason {
    pub fn message(&self) -> String {
        match self {
            Self::TextTooLarge { size } => format!(
                "This text file is {}; text previews are limited to {}.",
                format_size(*size),
                format_size(MAX_TEXT_BYTES)
            ),
            Self::ImageTooLarge { size } => format!(
                "This image is {}; image previews are limited to {}.",
                format_size(*size),
                format_size(MAX_IMAGE_BYTES)
            ),
            Self::PdfTooLarge { size } => format!(
                "This PDF is {}; PDF previews are limited to {}.",
                format_size(*size),
                format_size(PDF_MAX_INPUT_BYTES)
            ),
            Self::ImageDimensions { width, height } => format!(
                "Image too large to preview ({width} × {height} pixels; limit {} megapixels).",
                MAX_IMAGE_PIXELS / 1_000_000
            ),
            Self::Binary => "Binary file: preview and editing are not available.".to_string(),
            Self::InvalidUtf8 => {
                "This file is not valid UTF-8; preview and editing are not available.".to_string()
            }
            Self::MediaNoThumbnail => "Preview not available.".to_string(),
            Self::SpecialFile => "Special files are not previewed.".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadFailure {
    Missing,
    PermissionDenied,
    UnavailableVolume,
    ChangedWhileLoading,
    Io(String),
}

impl LoadFailure {
    pub fn message(&self) -> String {
        match self {
            Self::Missing => "This file is no longer available.".to_string(),
            Self::PermissionDenied => "Permission denied while reading this file.".to_string(),
            Self::UnavailableVolume => "The volume holding this file is not available.".to_string(),
            Self::ChangedWhileLoading => {
                "The file changed while it was being read; select it again.".to_string()
            }
            Self::Io(message) => format!("Could not read this file: {message}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderInfo {
    Folder { path: PathBuf, name: String },
}

/// The result of one classified read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    Document(PreviewDocument),
    Folder(FolderInfo),
    Unsupported {
        path: PathBuf,
        name: String,
        reason: UnsupportedReason,
        size: Option<u64>,
    },
    Failed {
        path: PathBuf,
        name: String,
        failure: LoadFailure,
    },
}

/// The clean preview state. A pinned dirty document lives beside this in
/// [`PreviewSession`] and is never replaced by a selection change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ViewState {
    Empty,
    Multiple {
        count: usize,
    },
    Trash,
    Loading {
        path: PathBuf,
    },
    Document(PreviewDocument),
    Folder(FolderInfo),
    Unsupported {
        path: PathBuf,
        name: String,
        reason: UnsupportedReason,
        size: Option<u64>,
    },
    Failed {
        path: PathBuf,
        name: String,
        failure: LoadFailure,
    },
}

/// A load request tagged with the generation it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadTicket {
    pub generation: u64,
    pub path: PathBuf,
}

/// The immutable identity of the file behind an editor. A new editor always
/// gets a new id, so a reply for an older editor can never authorize a write
/// against the current buffer.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EditorToken {
    pub id: u64,
    pub path: PathBuf,
    pub revision: FileRevision,
}

/// An editor identity plus the exact buffer revision that made a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActionToken {
    pub editor: EditorToken,
    pub buffer_revision: u64,
}

/// The text being edited in this window. It stays attached to its document
/// while the user browses elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedDocument {
    pub document: PreviewDocument,
    /// Allocated once per editing session. Never reused.
    pub editor_id: u64,
    pub dirty: bool,
    pub saving: bool,
    /// Entered by Alt-click on a file owned by another user. Every save asks
    /// first and writes through GVfs's administrator backend.
    pub admin: bool,
    /// The buffer revision captured when a save starts. A newer edit makes the
    /// result stale instead of marking unseen bytes as saved.
    pub buffer_revision: u64,
    pub saved_buffer_revision: u64,
}

impl PinnedDocument {
    fn new(document: PreviewDocument, editor_id: u64, buffer_revision: u64, admin: bool) -> Self {
        Self {
            document,
            editor_id,
            dirty: false,
            saving: false,
            admin,
            buffer_revision,
            saved_buffer_revision: buffer_revision,
        }
    }

    pub fn editor_token(&self) -> EditorToken {
        EditorToken {
            id: self.editor_id,
            path: self.document.path.clone(),
            revision: self.document.revision.clone(),
        }
    }

    pub fn action_token(&self) -> ActionToken {
        ActionToken {
            editor: self.editor_token(),
            buffer_revision: self.buffer_revision,
        }
    }

    pub fn mark_dirty(&mut self, revision: u64) {
        self.buffer_revision = revision;
        self.dirty = revision != self.saved_buffer_revision;
    }

    pub fn save_applies_to(&self, revision: u64) -> bool {
        self.buffer_revision == revision
    }
}

/// A stable key for one buffer of one editor. Quit coordination uses it to
/// avoid asking twice while still asking again after a new edit.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EditorIdentity {
    pub editor_id: u64,
    pub path: PathBuf,
    pub revision: FileRevision,
    pub buffer_revision: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorActivity {
    pub identity: EditorIdentity,
    pub dirty: bool,
    pub saving: bool,
}

impl PinnedDocument {
    fn identity(&self) -> EditorIdentity {
        EditorIdentity {
            editor_id: self.editor_id,
            path: self.document.path.clone(),
            revision: self.document.revision.clone(),
            buffer_revision: self.buffer_revision,
        }
    }
}

/// Where a Save As request is in its chooser/destination/Replace flow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveAsStage {
    Choosing,
    CheckingDestination,
    AwaitingReplace,
    Starting,
}

/// One Save As request. Its token freezes the editor and buffer revision that
/// opened the chooser, so every later reply can be checked against the same
/// editor before any write or status change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveAsRequest {
    pub id: u64,
    pub token: ActionToken,
    pub path: Option<PathBuf>,
    pub stage: SaveAsStage,
    pub replace: Option<ReplaceConfirmation>,
}

/// Which save route a chosen path uses. Saving over the original path is
/// always the original Save, with its loaded-version checks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveAsRoute {
    Original,
    NewPath,
}

/// The immutable context of one accepted save. The UI carries it through the
/// worker result; the session accepts only the matching active save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveTicket {
    pub id: u64,
    pub token: ActionToken,
    pub path: PathBuf,
    pub new_path: bool,
    pub replace: Option<ReplaceConfirmation>,
    pub encoding: TextEncoding,
    /// Written through GVfs's administrator backend on this exact editor.
    pub admin: bool,
}

/// What a save result did to the session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveCompletion {
    Applied,
    EditorChanged,
    Stale,
}

/// A close/quit continuation bound to one buffer. `save` names the save job
/// that must complete first; `None` means discard has already abandoned the
/// edits and the UI may close after revalidation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CloseRequest {
    pub token: ActionToken,
    pub save: Option<u64>,
}

/// Who asked for a selection. A model refresh may reapply the same old
/// selection after Save As; an explicit click/keyboard selection is a user
/// intent and always releases a saved preview override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionOrigin {
    Model,
    User,
}

/// A clean preview of the path a successful Save As committed. The browser's
/// own selection remains on the old item; passive model reapplications of that
/// same selection must not pull the preview back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedPreviewOverride {
    pub path: PathBuf,
    pub revision: FileRevision,
}

/// Pure editor/preview state. The UI mirrors it with widgets and worker jobs;
/// all selection, stale-load, dirty-pin and discard transitions live here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewSession {
    candidate: Selection,
    state: ViewState,
    generation: u64,
    pinned: Option<PinnedDocument>,
    warning: Option<String>,
    next_editor: u64,
    next_request: u64,
    save_as: Option<SaveAsRequest>,
    active_save: Option<SaveTicket>,
    close: Option<CloseRequest>,
    saved_override: Option<SavedPreviewOverride>,
    last_model_selection: Option<Selection>,
}

impl Default for PreviewSession {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewSession {
    pub fn new() -> Self {
        Self {
            candidate: Selection::None,
            state: ViewState::Empty,
            generation: 0,
            pinned: None,
            warning: None,
            next_editor: 0,
            next_request: 0,
            save_as: None,
            active_save: None,
            close: None,
            saved_override: None,
            last_model_selection: None,
        }
    }

    pub fn candidate(&self) -> &Selection {
        &self.candidate
    }

    pub fn state(&self) -> &ViewState {
        &self.state
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn pinned(&self) -> Option<&PinnedDocument> {
        self.pinned.as_ref()
    }

    pub fn pinned_mut(&mut self) -> Option<&mut PinnedDocument> {
        self.pinned.as_mut()
    }

    pub fn is_dirty(&self) -> bool {
        self.pinned.as_ref().is_some_and(|pinned| pinned.dirty)
    }

    pub fn is_editing(&self) -> bool {
        self.pinned.is_some()
    }

    /// Whether the pinned editor is in administrator mode.
    pub fn is_admin_editing(&self) -> bool {
        self.pinned.as_ref().is_some_and(|pinned| pinned.admin)
    }

    pub fn saving(&self) -> bool {
        self.pinned.as_ref().is_some_and(|pinned| pinned.saving)
    }

    pub fn has_active_save(&self) -> bool {
        self.active_save.is_some()
    }

    pub fn active_save(&self) -> Option<&SaveTicket> {
        self.active_save.as_ref()
    }

    /// One stable record per editor, used by Quit to ask again only when the
    /// document or buffer revision actually changed.
    pub fn activity(&self) -> Option<EditorActivity> {
        self.pinned.as_ref().map(|pinned| EditorActivity {
            identity: pinned.identity(),
            dirty: pinned.dirty,
            saving: pinned.saving,
        })
    }

    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    pub fn set_warning(&mut self, warning: Option<String>) {
        self.warning = warning;
    }

    /// The document the UI should show: the dirty/editing one wins over the
    /// clean candidate.
    pub fn current_document(&self) -> Option<&PreviewDocument> {
        self.pinned
            .as_ref()
            .map(|pinned| &pinned.document)
            .or(match &self.state {
                ViewState::Document(document) => Some(document),
                _ => None,
            })
    }

    /// A selection change. It is only a load request when no dirty or saving
    /// editor is pinned; otherwise it becomes the pending candidate.
    pub fn select(&mut self, selection: Selection) -> Option<LoadTicket> {
        self.select_origin(selection, SelectionOrigin::User)
    }

    /// A passive model refresh. It must not steal a successful Save As's new
    /// preview while it merely reapplies the same old browser selection. Any
    /// different selection is a genuine navigation and releases the override.
    pub fn apply_model_selection(&mut self, selection: Selection) -> Option<LoadTicket> {
        let stale = self.saved_override.is_some()
            && self
                .last_model_selection
                .as_ref()
                .is_some_and(|last| *last == selection);
        self.last_model_selection = Some(selection.clone());
        if stale {
            return None;
        }
        self.saved_override = None;
        self.select_origin(selection, SelectionOrigin::Model)
    }

    /// An explicit click or keyboard selection. Even selecting the old
    /// already-selected item releases a Save As override and shows that item.
    pub fn select_user(&mut self, selection: Selection) -> Option<LoadTicket> {
        self.last_model_selection = Some(selection.clone());
        self.select_origin(selection, SelectionOrigin::User)
    }

    pub fn saved_override(&self) -> Option<&SavedPreviewOverride> {
        self.saved_override.as_ref()
    }

    /// A navigation or focus change explicitly releases Save As ownership.
    /// Returns a load ticket when the saved new document is currently shown
    /// and the actual candidate is a different file.
    pub fn release_saved_override(&mut self) -> Option<LoadTicket> {
        let override_path = self.saved_override.take()?;
        if self.pinned.is_some() {
            return None;
        }
        if let ViewState::Document(document) = &self.state
            && document.path == override_path.path
        {
            return match self.candidate.clone() {
                Selection::File { path } if path != override_path.path => {
                    self.last_model_selection = Some(Selection::File { path: path.clone() });
                    Some(self.force_load(path))
                }
                _ => None,
            };
        }
        None
    }

    fn select_origin(
        &mut self,
        selection: Selection,
        origin: SelectionOrigin,
    ) -> Option<LoadTicket> {
        let same = self.candidate == selection;
        if origin == SelectionOrigin::User {
            self.last_model_selection = Some(selection.clone());
        }
        self.candidate = selection.clone();
        let releasing_saved = same && self.saved_override.is_some();
        self.saved_override = None;

        if same && !releasing_saved {
            return None;
        }
        if self
            .pinned
            .as_ref()
            .is_some_and(|pinned| pinned.dirty || pinned.saving)
        {
            // Cancel any load that was already in flight; the pinned buffer
            // remains the only document this window may show or save.
            self.generation = self.generation.wrapping_add(1);
            return None;
        }
        if self.pinned.take().is_some() {
            // A clean editing session ends when the user selects elsewhere.
            self.save_as = None;
            self.generation = self.generation.wrapping_add(1);
            self.state = ViewState::Empty;
        }

        self.generation = self.generation.wrapping_add(1);
        self.warning = None;
        match selection {
            Selection::None => {
                self.state = ViewState::Empty;
                None
            }
            Selection::Multiple { count } => {
                self.state = ViewState::Multiple { count };
                None
            }
            Selection::Trash => {
                self.state = ViewState::Trash;
                None
            }
            Selection::File { path } => {
                if origin == SelectionOrigin::User && same && releasing_saved {
                    // Re-selecting the old already-selected file after Save As
                    // must actually reload it, not leave the new path visible.
                    return Some(self.force_load(path));
                }
                self.state = ViewState::Loading { path: path.clone() };
                Some(LoadTicket {
                    generation: self.generation,
                    path,
                })
            }
        }
    }

    /// Start a load even when the selection did not change (Reload, monitor,
    /// or reconciliation after Save/Discard). A clean editing session is
    /// finished first: the old clean pin can no longer describe the new load.
    pub fn force_load(&mut self, path: PathBuf) -> LoadTicket {
        if self
            .saved_override
            .as_ref()
            .is_some_and(|saved| saved.path != path)
        {
            self.saved_override = None;
        }
        if self
            .pinned
            .as_ref()
            .is_some_and(|pinned| !pinned.dirty && !pinned.saving)
        {
            self.pinned = None;
            self.save_as = None;
        }
        self.generation = self.generation.wrapping_add(1);
        self.state = ViewState::Loading { path: path.clone() };
        self.warning = None;
        LoadTicket {
            generation: self.generation,
            path,
        }
    }

    /// Reload the current candidate after Save or Discard. A successful Save As
    /// owns the displayed saved document until the user explicitly selects
    /// elsewhere; reconcile must not pull the preview back to the older
    /// browser candidate. A clean document with the same path is kept rather
    /// than re-read.
    pub fn reconcile(&mut self) -> Option<LoadTicket> {
        if self.pinned.is_some() || self.saved_override.is_some() {
            return None;
        }
        match self.candidate.clone() {
            Selection::File { path } => {
                if let ViewState::Document(document) = &self.state
                    && document.path == path
                {
                    return None;
                }
                Some(self.force_load(path))
            }
            Selection::None => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Empty;
                None
            }
            Selection::Multiple { count } => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Multiple { count };
                None
            }
            Selection::Trash => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Trash;
                None
            }
        }
    }

    pub fn load_finished(&mut self, ticket: &LoadTicket, loaded: Loaded) -> bool {
        if ticket.generation != self.generation {
            return false;
        }
        // A clean pin cannot survive a different load: its old revision would
        // otherwise authorize a later save against stale metadata.
        if self
            .pinned
            .as_ref()
            .is_some_and(|pinned| !pinned.dirty && !pinned.saving)
        {
            self.pinned = None;
            self.save_as = None;
        }
        self.state = match loaded {
            Loaded::Document(document) => {
                if let Some(saved) = self.saved_override.as_mut()
                    && saved.path == document.path
                {
                    saved.revision = document.revision.clone();
                }
                ViewState::Document(document)
            }
            Loaded::Folder(folder) => ViewState::Folder(folder),
            Loaded::Unsupported {
                path,
                name,
                reason,
                size,
            } => ViewState::Unsupported {
                path,
                name,
                reason,
                size,
            },
            Loaded::Failed {
                path,
                name,
                failure,
            } => ViewState::Failed {
                path,
                name,
                failure,
            },
        };
        true
    }

    pub fn load_failed(&mut self, ticket: &LoadTicket, failure: LoadFailure) -> bool {
        if ticket.generation != self.generation {
            return false;
        }
        let name = display_name(&ticket.path);
        self.state = ViewState::Failed {
            path: ticket.path.clone(),
            name,
            failure,
        };
        true
    }

    /// Alt-click eligibility. A non-text or blocked document stays read-only.
    pub fn begin_edit(&mut self, buffer_revision: u64) -> Result<(), String> {
        if self.pinned.is_some() {
            return Ok(());
        }
        let Some(document) = (match &self.state {
            ViewState::Document(document) => Some(document.clone()),
            _ => None,
        }) else {
            return Err("Only a text preview can be edited.".to_string());
        };
        if document.text().is_none() {
            return Err("Only a text preview can be edited.".to_string());
        }
        if let Some(reason) = document.edit_block_reason() {
            return Err(reason.message().to_string());
        }
        self.next_editor = self.next_editor.wrapping_add(1);
        self.pinned = Some(PinnedDocument::new(
            document,
            self.next_editor,
            buffer_revision,
            false,
        ));
        self.warning = None;
        Ok(())
    }

    /// Enter administrator edit mode. The document must be editable apart
    /// from ordinary Unix permission and owned by another user.
    pub fn begin_admin_edit(
        &mut self,
        buffer_revision: u64,
        owner_uid: u32,
        current_uid: u32,
    ) -> Result<(), String> {
        if self.pinned.is_some() {
            return Ok(());
        }
        let Some(document) = (match &self.state {
            ViewState::Document(document) => Some(document.clone()),
            _ => None,
        }) else {
            return Err("Only a text preview can be edited.".to_string());
        };
        if !document.admin_editable(owner_uid, current_uid) {
            return Err(document
                .edit_block_reason()
                .map(|reason| reason.message().to_string())
                .unwrap_or_else(|| {
                    "Only an editable text preview can use administrator mode.".into()
                }));
        }
        self.next_editor = self.next_editor.wrapping_add(1);
        self.pinned = Some(PinnedDocument::new(
            document,
            self.next_editor,
            buffer_revision,
            true,
        ));
        self.warning = None;
        Ok(())
    }

    /// The caller-aware Alt-click entry. Only the UI may turn an
    /// administrator-editable document into an administrator editor; an
    /// ordinary writable document keeps P2m's UI-or-hotkey rule.
    pub fn begin_edit_for(
        &mut self,
        caller: crate::command::Caller,
        buffer_revision: u64,
        owner_uid: u32,
        current_uid: u32,
    ) -> Result<(), String> {
        if self
            .current_document()
            .is_some_and(|document| document.admin_editable(owner_uid, current_uid))
        {
            if caller != crate::command::Caller::Ui {
                return Err(ADMIN_EDIT_UI_ONLY.to_string());
            }
            return self.begin_admin_edit(buffer_revision, owner_uid, current_uid);
        }
        self.begin_edit(buffer_revision)
    }

    pub fn mark_dirty(&mut self, buffer_revision: u64) {
        let changed = self
            .pinned
            .as_mut()
            .map(|pinned| {
                let changed = pinned.buffer_revision != buffer_revision;
                pinned.mark_dirty(buffer_revision);
                changed
            })
            .unwrap_or(false);
        if changed {
            // A destination check or Replace question for the old buffer is
            // stale now. The user invokes Save As again after editing.
            self.save_as = None;
        }
    }

    /// Open a Save As chooser. Refused while another chooser, destination
    /// check, Replace question or save owns this panel.
    pub fn begin_save_as(&mut self) -> Result<SaveAsRequest, String> {
        if self.save_as.is_some() {
            return Err("A Save As is already in progress.".to_string());
        }
        let Some(pinned) = self.pinned.as_ref() else {
            return Err("Alt-click a writable text preview before Save As.".to_string());
        };
        if pinned.admin {
            return Err("Save As isn't available while editing as administrator.".to_string());
        }
        if pinned.saving || self.active_save.is_some() {
            return Err("Wait for the current save to finish.".to_string());
        }
        if pinned.document.text().is_none() {
            return Err("Only editable text can be saved.".to_string());
        }
        self.next_request = self.next_request.wrapping_add(1);
        let request = SaveAsRequest {
            id: self.next_request,
            token: pinned.action_token(),
            path: None,
            stage: SaveAsStage::Choosing,
            replace: None,
        };
        self.save_as = Some(request.clone());
        Ok(request)
    }

    /// Validate a chooser/destination/Replace reply against the editor and
    /// buffer revision that opened the request.
    pub fn check_save_as(&self, id: u64, stage: SaveAsStage) -> Result<&SaveAsRequest, String> {
        let Some(request) = self.save_as.as_ref() else {
            return Err("That Save As is no longer active.".to_string());
        };
        if request.id != id || request.stage != stage {
            return Err("That Save As reply is stale; start Save As again.".to_string());
        }
        if !self
            .pinned
            .as_ref()
            .is_some_and(|pinned| pinned.action_token() == request.token)
        {
            return Err("The editor changed; start Save As again.".to_string());
        }
        Ok(request)
    }

    pub fn save_as_pending(&self) -> bool {
        self.save_as.is_some()
    }

    /// Record the chooser's chosen path. Saving over the original path is
    /// routed back through the original Save's loaded-version checks.
    pub fn choose_save_as_path(&mut self, id: u64, path: PathBuf) -> Result<SaveAsRoute, String> {
        self.check_save_as(id, SaveAsStage::Choosing)?;
        let original = self
            .save_as
            .as_ref()
            .is_some_and(|request| request.token.editor.path == path);
        if original {
            self.save_as = None;
            return Ok(SaveAsRoute::Original);
        }
        let request = self
            .save_as
            .as_mut()
            .expect("the checked request is still current");
        request.path = Some(path);
        request.stage = SaveAsStage::CheckingDestination;
        Ok(SaveAsRoute::NewPath)
    }

    /// Record a finished destination check. `revision` is present when the
    /// destination already exists and Replace must confirm that exact version.
    pub fn save_as_destination(
        &mut self,
        id: u64,
        path: &Path,
        revision: Option<FileRevision>,
    ) -> Result<SaveAsRequest, String> {
        self.check_save_as(id, SaveAsStage::CheckingDestination)?;
        let request = self
            .save_as
            .as_mut()
            .expect("the checked request is still current");
        if request.path.as_deref() != Some(path) {
            return Err("That destination reply belongs to another path.".to_string());
        }
        request.replace = revision.map(|revision| ReplaceConfirmation {
            path: path.to_path_buf(),
            revision,
        });
        request.stage = SaveAsStage::AwaitingReplace;
        Ok(request.clone())
    }

    /// Accept the Replace answer (or the missing-destination path) and freeze
    /// the save context. No write happens until `start_save_as`.
    pub fn confirm_save_as(&mut self, id: u64) -> Result<SaveAsRequest, String> {
        self.check_save_as(id, SaveAsStage::AwaitingReplace)?;
        let request = self
            .save_as
            .as_mut()
            .expect("the checked request is still current");
        if request.path.is_none() {
            return Err("Save As has no chosen path.".to_string());
        }
        request.stage = SaveAsStage::Starting;
        Ok(request.clone())
    }

    pub fn cancel_save_as(&mut self, id: u64) {
        if self
            .save_as
            .as_ref()
            .is_some_and(|request| request.id == id)
        {
            self.save_as = None;
        }
    }

    /// Start the save for an accepted Save As request. The ticket captures the
    /// editor, its document revision and the buffer revision for the result.
    pub fn start_save_as(&mut self, id: u64) -> Result<SaveTicket, String> {
        self.check_save_as(id, SaveAsStage::Starting)?;
        let request = self
            .save_as
            .as_ref()
            .expect("the checked request is still current")
            .clone();
        let path = request
            .path
            .clone()
            .ok_or_else(|| "Save As has no chosen path.".to_string())?;
        self.begin_save_from(request.token, path, true, request.replace)
    }

    /// Start the normal Save. It always targets the pinned document path and
    /// uses its load-time identity/version.
    pub fn begin_save_original(&mut self) -> Result<SaveTicket, String> {
        if self.save_as.is_some() {
            return Err("Finish or cancel the current Save As first.".to_string());
        }
        let Some(pinned) = self.pinned.as_ref() else {
            return Err("There are no preview edits to save.".to_string());
        };
        if pinned.saving {
            return Err("This preview is already being saved.".to_string());
        }
        if !pinned.dirty {
            return Err("There are no unsaved preview edits.".to_string());
        }
        let token = pinned.action_token();
        let path = pinned.document.path.clone();
        self.begin_save_from(token, path, false, None)
    }

    /// The caller-aware Save. An administrator editor may only be saved by the
    /// UI, even though ordinary preview saves still accept a configured
    /// hotkey.
    pub fn begin_save_original_for(
        &mut self,
        caller: crate::command::Caller,
    ) -> Result<SaveTicket, String> {
        if self.is_admin_editing() && caller != crate::command::Caller::Ui {
            return Err(ADMIN_EDIT_UI_ONLY.to_string());
        }
        self.begin_save_original()
    }

    fn begin_save_from(
        &mut self,
        token: ActionToken,
        path: PathBuf,
        new_path: bool,
        replace: Option<ReplaceConfirmation>,
    ) -> Result<SaveTicket, String> {
        if self.active_save.is_some() {
            return Err("This preview is already being saved.".to_string());
        }
        let Some(pinned) = self.pinned.as_ref() else {
            return Err("There are no preview edits to save.".to_string());
        };
        if pinned.action_token() != token {
            return Err("The editor changed; start the save again.".to_string());
        }
        let encoding = pinned
            .document
            .text()
            .ok_or_else(|| "Only editable text can be saved.".to_string())?
            .encoding;
        let admin = pinned.admin;
        self.next_request = self.next_request.wrapping_add(1);
        let ticket = SaveTicket {
            id: self.next_request,
            token,
            path,
            new_path,
            replace,
            encoding,
            admin,
        };
        let pinned = self.pinned.as_mut().expect("checked above");
        pinned.saving = true;
        self.active_save = Some(ticket.clone());
        self.save_as = None;
        Ok(ticket)
    }

    /// Apply a save result only when it is the active save and the editor has
    /// not changed since the ticket was frozen.
    pub fn save_finished(
        &mut self,
        ticket: &SaveTicket,
        document: PreviewDocument,
    ) -> SaveCompletion {
        if self.active_save.as_ref() != Some(ticket) {
            return SaveCompletion::Stale;
        }
        if document.path != ticket.path {
            self.active_save = None;
            if let Some(pinned) = self.pinned.as_mut() {
                pinned.saving = false;
            }
            self.close = None;
            return SaveCompletion::Stale;
        }
        let matches = self.pinned.as_ref().is_some_and(|pinned| {
            pinned.editor_id == ticket.token.editor.id
                && pinned.document.path == ticket.token.editor.path
                && pinned
                    .document
                    .revision
                    .same_version(&ticket.token.editor.revision)
        });
        if !matches {
            self.active_save = None;
            if let Some(pinned) = self.pinned.as_mut() {
                pinned.saving = false;
            }
            self.close = None;
            return SaveCompletion::Stale;
        }
        if self
            .pinned
            .as_ref()
            .is_none_or(|pinned| pinned.buffer_revision != ticket.token.buffer_revision)
        {
            self.active_save = None;
            if let Some(pinned) = self.pinned.as_mut() {
                pinned.saving = false;
            }
            self.close = None;
            return SaveCompletion::EditorChanged;
        }
        self.pinned = None;
        self.active_save = None;
        self.save_as = None;
        self.generation = self.generation.wrapping_add(1);
        self.state = ViewState::Document(document.clone());
        self.warning = None;
        if ticket.new_path {
            self.saved_override = Some(SavedPreviewOverride {
                path: document.path.clone(),
                revision: document.revision.clone(),
            });
        } else if self
            .saved_override
            .as_ref()
            .is_some_and(|saved| saved.path == document.path)
        {
            // Saving the document that Save As already owns is its ordinary
            // Save: keep ownership and record the new saved revision. The
            // passive browser candidate is still the older file.
            self.saved_override = Some(SavedPreviewOverride {
                path: document.path.clone(),
                revision: document.revision.clone(),
            });
        } else {
            self.saved_override = None;
        }
        SaveCompletion::Applied
    }

    /// A failed save keeps the pinned buffer and its dirty state. The close
    /// continuation requested for it cannot be fulfilled, so it is dropped.
    pub fn save_failed(&mut self, ticket: &SaveTicket, message: impl Into<String>) -> bool {
        if self.active_save.as_ref() != Some(ticket) {
            return false;
        }
        self.active_save = None;
        self.close = None;
        if let Some(pinned) = self.pinned.as_mut() {
            if pinned.editor_id == ticket.token.editor.id && pinned.action_token() == ticket.token {
                pinned.saving = false;
                self.warning = Some(message.into());
                return true;
            }
            pinned.saving = false;
        }
        false
    }

    /// Discard explicitly, then reload the current candidate if it is still
    /// selected. It never deletes the file and never touches model undo.
    pub fn discard(&mut self) -> Option<LoadTicket> {
        self.save_as = None;
        self.close = None;
        self.discard_inner()
    }

    /// The close dialog's Discard branch. The close request survives long
    /// enough for the idle to revalidate it before the window may close.
    pub fn discard_preserving_close(&mut self) -> Option<LoadTicket> {
        let close = self.close.take();
        self.save_as = None;
        let ticket = self.discard_inner();
        self.close = close;
        ticket
    }

    fn discard_inner(&mut self) -> Option<LoadTicket> {
        if self.pinned.as_ref().is_some_and(|pinned| pinned.saving) {
            return None;
        }
        // Discard an editor that owns the displayed saved document reloads
        // that document. The browser's older candidate must not be retargeted
        // by a discard/conflict action belonging to the new file.
        let owned_path = self.pinned.as_ref().and_then(|pinned| {
            self.saved_override
                .as_ref()
                .filter(|saved| saved.path == pinned.document.path)
                .map(|_| pinned.document.path.clone())
        });
        if let Some(path) = owned_path {
            self.pinned = None;
            self.active_save = None;
            self.warning = None;
            return Some(self.force_load(path));
        }
        self.pinned = None;
        self.active_save = None;
        self.saved_override = None;
        self.generation = self.generation.wrapping_add(1);
        self.warning = None;
        match self.candidate.clone() {
            Selection::File { path } => Some(self.force_load(path)),
            Selection::None => {
                self.state = ViewState::Empty;
                None
            }
            Selection::Multiple { count } => {
                self.state = ViewState::Multiple { count };
                None
            }
            Selection::Trash => {
                self.state = ViewState::Trash;
                None
            }
        }
    }

    /// Reload the document currently pinned, used by "Reload" in the external
    /// change dialog. It abandons edits and every pending action bound to them,
    /// then reconciles to the unchanged pending selection: the dirty pin may be
    /// A while the user is browsing B, Trash, nothing or several files. When the
    /// pinned editor owns the displayed Save As document, reload that document
    /// instead of the older browser candidate.
    pub fn reload_pinned(&mut self) -> Option<LoadTicket> {
        if self.pinned.as_ref().is_some_and(|pinned| pinned.saving) {
            return None;
        }
        // Reload only exists while an editor is pinned.
        let pinned = self.pinned.as_ref()?;
        let owned_path = self
            .saved_override
            .as_ref()
            .filter(|saved| saved.path == pinned.document.path)
            .map(|_| pinned.document.path.clone());
        self.pinned = None;
        self.active_save = None;
        self.save_as = None;
        self.close = None;
        self.warning = None;
        if let Some(path) = owned_path {
            return Some(self.force_load(path));
        }
        self.saved_override = None;
        // Do not write the old pinned path back into `candidate`: Reload
        // abandons those edits, it does not select their file again.
        match self.candidate.clone() {
            Selection::File { path } => Some(self.force_load(path)),
            Selection::None => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Empty;
                None
            }
            Selection::Multiple { count } => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Multiple { count };
                None
            }
            Selection::Trash => {
                self.generation = self.generation.wrapping_add(1);
                self.state = ViewState::Trash;
                None
            }
        }
    }

    /// A monitor says the disk changed. Dirty or saving text stays; the UI
    /// shows the warning. A clean preview may be reloaded.
    pub fn note_external_change(&mut self) -> bool {
        if self
            .pinned
            .as_ref()
            .is_some_and(|pinned| pinned.dirty || pinned.saving)
        {
            self.warning = Some("File changed on disk".to_string());
            false
        } else {
            true
        }
    }

    /// Capture the buffer that a close/quit continuation belongs to. Repeated
    /// requests return the same continuation instead of competing with it.
    pub fn arm_close(&mut self) -> Result<CloseRequest, String> {
        if let Some(close) = &self.close {
            return Ok(close.clone());
        }
        let Some(pinned) = self.pinned.as_ref() else {
            return Err("There are no unsaved preview edits.".to_string());
        };
        if !pinned.dirty && !pinned.saving {
            return Err("There are no unsaved preview edits.".to_string());
        }
        let close = CloseRequest {
            token: pinned.action_token(),
            save: if pinned.saving {
                self.active_save.as_ref().map(|ticket| ticket.id)
            } else {
                None
            },
        };
        self.close = Some(close.clone());
        Ok(close)
    }

    pub fn bind_close_to_save(&mut self, ticket: &SaveTicket) {
        if let Some(close) = self.close.as_mut()
            && close.token == ticket.token
        {
            close.save = Some(ticket.id);
        }
    }

    pub fn close_satisfied_by(&self, ticket: &SaveTicket) -> bool {
        self.active_save.is_none()
            && !self.saving()
            && self
                .close
                .as_ref()
                .is_some_and(|close| close.save == Some(ticket.id) && close.token == ticket.token)
    }

    pub fn has_close(&self) -> bool {
        self.close.is_some()
    }

    pub fn close_request(&self) -> Option<&CloseRequest> {
        self.close.as_ref()
    }

    /// Revalidate a discard-based close. A new edit or a newly started save
    /// invalidates it, so fresh unsaved work is never closed silently.
    pub fn close_ready(&self, close: &CloseRequest) -> bool {
        self.close.as_ref() == Some(close)
            && !self.is_dirty()
            && !self.saving()
            && self.active_save.is_none()
    }

    pub fn take_close(&mut self) -> Option<CloseRequest> {
        self.close.take()
    }

    pub fn invalidate_close(&mut self) {
        self.close = None;
    }

    /// Drop pending chooser/destination/Replace/close state. Used when a
    /// document is replaced or the window is closing.
    pub fn invalidate_actions(&mut self) {
        self.save_as = None;
        self.close = None;
    }
}

// ---------------------------------------------------------------------------
// Limits and text
// ---------------------------------------------------------------------------

/// Logical line count: a trailing newline ends a new, empty numbered line.
pub fn logical_line_count(text: &str) -> usize {
    text.as_bytes()
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

/// Whether a decoded image is allowed and how much it must be scaled down
/// before pixel allocation. `None` means the source exceeds the 64 MP limit.
pub fn image_target_size(width: i32, height: i32) -> Option<(i32, i32)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let pixels = u64::from(width as u32) * u64::from(height as u32);
    if pixels > MAX_IMAGE_PIXELS {
        return None;
    }
    let longest = width.max(height);
    if longest <= IMAGE_MAX_EDGE {
        return Some((width, height));
    }
    let scale = f64::from(IMAGE_MAX_EDGE) / f64::from(longest);
    Some((
        ((f64::from(width) * scale).round() as i32).max(1),
        ((f64::from(height) * scale).round() as i32).max(1),
    ))
}

/// Decode strict UTF-8, strip a BOM and record the original line-ending
/// convention. Mixed endings are shown read-only; they are never written back.
pub fn decode_text(bytes: &[u8]) -> Result<(String, TextEncoding), TextDecodeError> {
    let (bytes, bom) = if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (rest, true)
    } else {
        (bytes, false)
    };
    if bytes.contains(&0) {
        return Err(TextDecodeError::Binary);
    }
    let line_ending = detect_line_ending(bytes);
    let mut text = std::str::from_utf8(bytes)
        .map_err(|_| TextDecodeError::InvalidUtf8)?
        .to_string();
    if text.contains('\r') {
        // Display only: CRLF and lone CR become buffer newlines. The encoding
        // keeps `None` for mixed/lone-CR files, so saving is refused.
        text = text.replace("\r\n", "\n").replace('\r', "\n");
    }
    Ok((text, TextEncoding { bom, line_ending }))
}

/// The bytes to commit for `text`, preserving the BOM and uniform line ending.
pub fn encode_text(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, SaveError> {
    let Some(line_ending) = encoding.line_ending else {
        return Err(SaveError::ReadOnly(ReadOnlyReason::MixedLineEndings));
    };
    if text.contains('\r') {
        return Err(SaveError::InvalidText);
    }
    let body = match line_ending {
        LineEnding::Lf => text.as_bytes().to_vec(),
        LineEnding::Crlf => {
            let mut body = Vec::with_capacity(text.len() + text.matches('\n').count());
            for byte in text.as_bytes() {
                if *byte == b'\n' {
                    body.push(b'\r');
                }
                body.push(*byte);
            }
            body
        }
    };
    if encoding.bom {
        let mut with_bom = Vec::with_capacity(body.len() + 3);
        with_bom.extend_from_slice(&[0xef, 0xbb, 0xbf]);
        with_bom.extend_from_slice(&body);
        Ok(with_bom)
    } else {
        Ok(body)
    }
}

fn detect_line_ending(bytes: &[u8]) -> Option<LineEnding> {
    let mut lf = 0usize;
    let mut crlf = 0usize;
    let mut lone_cr = 0usize;
    let mut index = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'\r' => {
                if bytes.get(index + 1) == Some(&b'\n') {
                    crlf += 1;
                    index += 2;
                    continue;
                }
                lone_cr += 1;
            }
            b'\n' => lf += 1,
            _ => {}
        }
        index += 1;
    }
    if lone_cr > 0 || (lf > 0 && crlf > 0) {
        None
    } else if crlf > 0 {
        Some(LineEnding::Crlf)
    } else {
        Some(LineEnding::Lf)
    }
}

// ---------------------------------------------------------------------------
// Inspection and loading
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Inspected {
    path: PathBuf,
    name: String,
    metadata: fs::Metadata,
    revision: FileRevision,
    access: WriteAccess,
    content_type: Option<String>,
    thumbnails: Vec<ThumbnailSource>,
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.display().to_string())
}

fn inspect(path: &Path) -> Result<Inspected, LoadFailure> {
    let symlink_metadata = fs::symlink_metadata(path).map_err(io_load_failure)?;
    let symlink = symlink_metadata.file_type().is_symlink();
    let metadata = if symlink {
        fs::metadata(path).map_err(io_load_failure)?
    } else {
        symlink_metadata
    };
    let name = display_name(path);
    let info = query_info(path, INFO_ATTRIBUTES).map_err(gio_load_failure)?;
    let etag = info
        .attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());
    let revision = FileRevision::from_metadata(&metadata, etag);
    let file_writable = info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE.as_str());
    let parent_writable = parent_writable(path);
    let access = WriteAccess {
        file_writable,
        parent_writable,
        reason: if symlink {
            Some(ReadOnlyReason::Symlink)
        } else if metadata.nlink() > 1 {
            Some(ReadOnlyReason::MultipleHardLinks)
        } else if !file_writable {
            Some(ReadOnlyReason::NotWritable)
        } else if !parent_writable {
            Some(ReadOnlyReason::ParentNotWritable)
        } else {
            None
        },
    };
    let content_type = info
        .attribute_string(gio::FILE_ATTRIBUTE_STANDARD_CONTENT_TYPE.as_str())
        .map(|value| value.to_string());
    let thumbnails = thumbnail_candidates(&info);
    Ok(Inspected {
        path: path.to_path_buf(),
        name,
        metadata,
        revision,
        access,
        content_type,
        thumbnails,
    })
}

const INFO_ATTRIBUTES: &str = "standard::content-type,standard::size,access::can-write,etag::value,unix::device,unix::inode,thumbnail::path,thumbnail::path-normal,thumbnail::path-large,thumbnail::path-xlarge,thumbnail::path-xxlarge,thumbnail::is-valid,thumbnail::is-valid-normal,thumbnail::is-valid-large,thumbnail::is-valid-xlarge,thumbnail::is-valid-xxlarge";

fn query_info(path: &Path, attributes: &str) -> Result<gio::FileInfo, glib::Error> {
    gio::File::for_path(path).query_info(
        attributes,
        gio::FileQueryInfoFlags::NONE,
        None::<&gio::Cancellable>,
    )
}

fn parent_writable(path: &Path) -> bool {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    query_info(parent, gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE.as_str())
        .map(|info| info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE.as_str()))
        .unwrap_or(false)
}

/// Every valid, regular, bounded GIO thumbnail cache file for one listing
/// entry, largest first. `thumbnail::is-valid-*` must be present and true;
/// the generic path is only accepted through its matching validity flag.
pub fn thumbnail_candidates(info: &gio::FileInfo) -> Vec<ThumbnailSource> {
    thumbnail_candidates_unchecked(info)
        .into_iter()
        .filter(|candidate| {
            let Ok(metadata) = fs::symlink_metadata(&candidate.path) else {
                return false;
            };
            !metadata.file_type().is_symlink()
                && metadata.is_file()
                && metadata.len() <= MAX_IMAGE_BYTES
        })
        .map(|candidate| {
            let size = fs::symlink_metadata(&candidate.path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            ThumbnailSource { size, ..candidate }
        })
        .collect()
}

/// Attribute-only candidates for a worker to validate by decoding. This does
/// no filesystem access, so a grid bind may call it on the main thread.
pub fn thumbnail_candidates_unchecked(info: &gio::FileInfo) -> Vec<ThumbnailSource> {
    let mut candidates: Vec<ThumbnailSource> = Vec::new();
    for (attribute, kind) in [
        (
            ThumbnailKind::XXLarge.path_attribute(),
            ThumbnailKind::XXLarge,
        ),
        (
            ThumbnailKind::XLarge.path_attribute(),
            ThumbnailKind::XLarge,
        ),
        (ThumbnailKind::Large.path_attribute(), ThumbnailKind::Large),
        (
            ThumbnailKind::Normal.path_attribute(),
            ThumbnailKind::Normal,
        ),
        (
            gio::FILE_ATTRIBUTE_THUMBNAIL_PATH.as_str(),
            ThumbnailKind::Normal,
        ),
    ] {
        if attribute != gio::FILE_ATTRIBUTE_THUMBNAIL_PATH.as_str() {
            let valid =
                info.has_attribute(kind.valid_attribute()) && info.boolean(kind.valid_attribute());
            if !valid {
                continue;
            }
        } else if !info.has_attribute(gio::FILE_ATTRIBUTE_THUMBNAIL_IS_VALID.as_str())
            || !info.boolean(gio::FILE_ATTRIBUTE_THUMBNAIL_IS_VALID.as_str())
        {
            continue;
        }
        // GIO stores thumbnail paths as byte strings; asking for the other
        // type logs a GLib-GIO-CRITICAL, so read only the stored type.
        let path = match info.attribute_type(attribute) {
            gio::FileAttributeType::ByteString => info
                .attribute_byte_string(attribute)
                .map(|value| PathBuf::from(OsString::from_vec(value.as_bytes().to_vec()))),
            gio::FileAttributeType::String => info
                .attribute_string(attribute)
                .map(|value| PathBuf::from(value.as_str())),
            _ => None,
        };
        let Some(path) = path else {
            continue;
        };
        if candidates.iter().any(|candidate| candidate.path == path) {
            continue;
        }
        candidates.push(ThumbnailSource {
            path,
            size: 0,
            kind,
        });
    }
    candidates
}

fn io_load_failure(error: io::Error) -> LoadFailure {
    match error.kind() {
        io::ErrorKind::NotFound => LoadFailure::Missing,
        io::ErrorKind::PermissionDenied => LoadFailure::PermissionDenied,
        _ => LoadFailure::Io(error.to_string()),
    }
}

fn gio_load_failure(error: glib::Error) -> LoadFailure {
    match error.kind::<gio::IOErrorEnum>() {
        Some(gio::IOErrorEnum::NotFound) => LoadFailure::Missing,
        Some(gio::IOErrorEnum::PermissionDenied) => LoadFailure::PermissionDenied,
        Some(gio::IOErrorEnum::NotMounted) => LoadFailure::UnavailableVolume,
        _ => LoadFailure::Io(error.to_string()),
    }
}

fn image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "gif"
                    | "webp"
                    | "bmp"
                    | "tif"
                    | "tiff"
                    | "ico"
                    | "avif"
                    | "jxl"
            )
        })
}

fn media_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "pdf" | "mp4" | "m4v" | "webm" | "mkv" | "mov" | "avi" | "ogv" | "ogg"
            )
        })
}

fn is_image(inspected: &Inspected) -> bool {
    image_extension(&inspected.path)
        || inspected
            .content_type
            .as_deref()
            .is_some_and(|mime| mime.starts_with("image/"))
}

fn is_pdf(inspected: &Inspected) -> bool {
    inspected
        .path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
        || inspected
            .content_type
            .as_deref()
            .is_some_and(|mime| mime == "application/pdf")
}

fn is_media(inspected: &Inspected) -> bool {
    (!is_pdf(inspected) && media_extension(&inspected.path))
        || inspected
            .content_type
            .as_deref()
            .is_some_and(|mime| mime.starts_with("video/"))
}

/// Classify and read one selected path. This is synchronous and bounded; the
/// UI calls it from its preview worker, never the GTK main thread.
pub fn load(path: &Path) -> Loaded {
    let inspected = match inspect(path) {
        Ok(inspected) => inspected,
        Err(failure) => {
            return Loaded::Failed {
                path: path.to_path_buf(),
                name: display_name(path),
                failure,
            };
        }
    };
    let name = inspected.name.clone();
    if inspected.metadata.is_dir() {
        return Loaded::Folder(FolderInfo::Folder {
            path: inspected.path,
            name,
        });
    }
    if !inspected.metadata.is_file() {
        return Loaded::Unsupported {
            path: inspected.path,
            name,
            reason: UnsupportedReason::SpecialFile,
            size: None,
        };
    }

    if is_image(&inspected) {
        if inspected.metadata.len() > MAX_IMAGE_BYTES {
            return Loaded::Unsupported {
                size: Some(inspected.metadata.len()),
                path: inspected.path,
                name,
                reason: UnsupportedReason::ImageTooLarge {
                    size: inspected.metadata.len(),
                },
            };
        }
        let mime = inspected
            .content_type
            .clone()
            .unwrap_or_else(|| "image/*".to_string());
        return Loaded::Document(PreviewDocument {
            path: inspected.path,
            name,
            size: inspected.metadata.len(),
            revision: inspected.revision,
            access: inspected.access,
            content: PreviewContent::Image(ImageDocument { mime }),
        });
    }

    if is_pdf(&inspected) {
        if inspected.metadata.len() > MAX_IMAGE_BYTES {
            return Loaded::Unsupported {
                size: Some(inspected.metadata.len()),
                path: inspected.path,
                name,
                reason: UnsupportedReason::PdfTooLarge {
                    size: inspected.metadata.len(),
                },
            };
        }
        let mime = inspected
            .content_type
            .clone()
            .unwrap_or_else(|| "application/pdf".to_string());
        return Loaded::Document(PreviewDocument {
            path: inspected.path,
            name,
            size: inspected.metadata.len(),
            revision: inspected.revision,
            access: inspected.access,
            content: PreviewContent::Pdf(PdfDocument {
                mime,
                thumbnail: inspected.thumbnails.first().cloned(),
                thumbnails: inspected.thumbnails,
            }),
        });
    }

    if is_media(&inspected) {
        let mime = inspected
            .content_type
            .clone()
            .unwrap_or_else(|| "application/octet-stream".to_string());
        return Loaded::Document(PreviewDocument {
            path: inspected.path,
            name,
            size: inspected.metadata.len(),
            revision: inspected.revision,
            access: inspected.access,
            content: PreviewContent::Media(MediaDocument {
                mime,
                thumbnail: inspected.thumbnails.first().cloned(),
                thumbnails: inspected.thumbnails,
            }),
        });
    }

    if inspected.metadata.len() > MAX_TEXT_BYTES {
        return Loaded::Unsupported {
            size: Some(inspected.metadata.len()),
            path: inspected.path,
            name,
            reason: UnsupportedReason::TextTooLarge {
                size: inspected.metadata.len(),
            },
        };
    }

    let bytes = match read_text_bounded(&inspected.path, &inspected.metadata) {
        Ok(bytes) => bytes,
        Err(failure) => {
            return Loaded::Failed {
                path: inspected.path,
                name,
                failure,
            };
        }
    };
    if bytes.len() as u64 > MAX_TEXT_BYTES {
        let size = bytes.len() as u64;
        return Loaded::Unsupported {
            path: inspected.path,
            name,
            reason: UnsupportedReason::TextTooLarge { size },
            size: Some(size),
        };
    }
    match decode_text(&bytes) {
        Ok((text, encoding)) => Loaded::Document(PreviewDocument {
            path: inspected.path,
            name,
            size: bytes.len() as u64,
            revision: inspected.revision,
            access: inspected.access,
            content: PreviewContent::Text(TextDocument { text, encoding }),
        }),
        Err(TextDecodeError::Binary) => Loaded::Unsupported {
            path: inspected.path,
            name,
            reason: UnsupportedReason::Binary,
            size: Some(inspected.metadata.len()),
        },
        Err(TextDecodeError::InvalidUtf8) => Loaded::Unsupported {
            path: inspected.path,
            name,
            reason: UnsupportedReason::InvalidUtf8,
            size: Some(inspected.metadata.len()),
        },
    }
}

fn read_text_bounded(path: &Path, before: &fs::Metadata) -> Result<Vec<u8>, LoadFailure> {
    let mut file = File::open(path).map_err(io_load_failure)?;
    let mut bytes = Vec::with_capacity(64 * 1024);
    std::io::Read::by_ref(&mut file)
        .take(MAX_TEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_load_failure)?;
    let after = file.metadata().map_err(io_load_failure)?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.ctime() != after.ctime()
        || before.ctime_nsec() != after.ctime_nsec()
    {
        return Err(LoadFailure::ChangedWhileLoading);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Save contract
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveError {
    ReadOnly(ReadOnlyReason),
    Missing,
    ChangedOnDisk,
    DestinationAppeared,
    DestinationChanged,
    IsDirectory,
    NotRegular,
    InvalidText,
    NoParent,
    Io(String),
}

impl SaveError {
    pub fn message(&self) -> String {
        match self {
            Self::ReadOnly(reason) => reason.message().to_string(),
            Self::Missing => "The file is no longer there.".to_string(),
            Self::ChangedOnDisk => "File changed on disk.".to_string(),
            Self::DestinationAppeared => {
                "A file appeared at that path after the chooser opened.".to_string()
            }
            Self::DestinationChanged => {
                "That file changed after you confirmed Replace. Choose Save As again.".to_string()
            }
            Self::IsDirectory => "That path is a folder.".to_string(),
            Self::NotRegular => "That path is not a regular file.".to_string(),
            Self::InvalidText => {
                "The editor contains a carriage return that cannot be saved.".to_string()
            }
            Self::NoParent => "That path has no folder to save into.".to_string(),
            Self::Io(message) => format!("Could not save: {message}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveExpectation {
    pub revision: FileRevision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplaceConfirmation {
    pub path: PathBuf,
    pub revision: FileRevision,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SaveTarget {
    Original {
        expected: SaveExpectation,
    },
    NewPath {
        replace: Option<ReplaceConfirmation>,
    },
    /// Replace the same path through GVfs's `admin://` backend. The ETag is
    /// the load-time write precondition, exactly as for an ordinary Save.
    Admin {
        expected: SaveExpectation,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveRequest {
    pub path: PathBuf,
    pub text: String,
    pub encoding: TextEncoding,
    pub target: SaveTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveSuccess {
    pub path: PathBuf,
    pub revision: FileRevision,
    pub size: u64,
    pub mode: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Missing,
    Existing {
        revision: FileRevision,
        writable: bool,
        reason: Option<ReadOnlyReason>,
    },
}

struct ExistingFile {
    revision: FileRevision,
    mode: u32,
    writable: bool,
    reason: Option<ReadOnlyReason>,
}

/// Inspect a Save As destination without changing it. Called on the preview
/// worker before any Replace question is shown.
pub fn inspect_destination(path: &Path) -> Result<Destination, SaveError> {
    match existing_file(path) {
        Ok(existing) => Ok(Destination::Existing {
            revision: existing.revision,
            writable: existing.writable,
            reason: existing.reason,
        }),
        Err(SaveError::Missing) => Ok(Destination::Missing),
        Err(error) => Err(error),
    }
}

/// Re-read the current revision for the monitor. No contents are read.
pub fn inspect_revision(path: &Path) -> Result<FileRevision, SaveError> {
    existing_file(path).map(|existing| existing.revision)
}

fn existing_file(path: &Path) -> Result<ExistingFile, SaveError> {
    let link = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(SaveError::Missing);
        }
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => {
            return Err(SaveError::ReadOnly(ReadOnlyReason::NotWritable));
        }
        Err(error) => return Err(SaveError::Io(error.to_string())),
    };
    if link.file_type().is_symlink() {
        return Err(SaveError::ReadOnly(ReadOnlyReason::Symlink));
    }
    let metadata = if link.is_dir() {
        return Err(SaveError::IsDirectory);
    } else {
        fs::metadata(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => SaveError::Missing,
            io::ErrorKind::PermissionDenied => SaveError::ReadOnly(ReadOnlyReason::NotWritable),
            _ => SaveError::Io(error.to_string()),
        })?
    };
    if !metadata.is_file() {
        return Err(SaveError::NotRegular);
    }
    if metadata.nlink() > 1 {
        return Err(SaveError::ReadOnly(ReadOnlyReason::MultipleHardLinks));
    }
    let info = query_info(
        path,
        "access::can-write,etag::value,unix::device,unix::inode",
    )
    .map_err(|error| match error.kind::<gio::IOErrorEnum>() {
        Some(gio::IOErrorEnum::NotFound) => SaveError::Missing,
        Some(gio::IOErrorEnum::PermissionDenied) => {
            SaveError::ReadOnly(ReadOnlyReason::NotWritable)
        }
        _ => SaveError::Io(error.to_string()),
    })?;
    let etag = info
        .attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());
    let file_writable = info.boolean(gio::FILE_ATTRIBUTE_ACCESS_CAN_WRITE.as_str());
    let parent_writable = parent_writable(path);
    let reason = if !file_writable {
        Some(ReadOnlyReason::NotWritable)
    } else if !parent_writable {
        Some(ReadOnlyReason::ParentNotWritable)
    } else {
        None
    };
    Ok(ExistingFile {
        revision: FileRevision::from_metadata(&metadata, etag),
        mode: metadata.permissions().mode(),
        writable: reason.is_none(),
        reason,
    })
}

/// The small GIO surface an administrator save needs. Production uses the
/// real `admin://` file; tests use a fake that records every call.
trait AdminWriter {
    fn replace_contents(
        &mut self,
        file: &gio::File,
        bytes: &[u8],
        etag: Option<&str>,
        make_backup: bool,
    ) -> Result<Option<String>, glib::Error>;

    fn set_attribute_uint32(
        &mut self,
        file: &gio::File,
        attribute: &str,
        value: u32,
    ) -> Result<(), glib::Error>;
}

struct GioAdminWriter;

impl AdminWriter for GioAdminWriter {
    fn replace_contents(
        &mut self,
        file: &gio::File,
        bytes: &[u8],
        etag: Option<&str>,
        make_backup: bool,
    ) -> Result<Option<String>, glib::Error> {
        file.replace_contents(
            bytes,
            etag,
            make_backup,
            gio::FileCreateFlags::NONE,
            None::<&gio::Cancellable>,
        )
        .map(|etag| etag.map(|value| value.to_string()))
    }

    fn set_attribute_uint32(
        &mut self,
        file: &gio::File,
        attribute: &str,
        value: u32,
    ) -> Result<(), glib::Error> {
        file.set_attribute_uint32(
            attribute,
            value,
            gio::FileQueryInfoFlags::NONE,
            None::<&gio::Cancellable>,
        )
    }
}

/// Write through GVfs's `admin://` backend. GVfs's `do_replace` calls
/// `g_file_replace` while running as root, so the replacement file is created
/// with root's ownership and the process umask rather than the original
/// metadata. Gimbal therefore puts the original uid, gid and mode back through
/// the same administrator file before reporting success.
fn save_admin(
    path: &Path,
    bytes: &[u8],
    expected: &SaveExpectation,
) -> Result<SaveSuccess, SaveError> {
    save_admin_with(path, bytes, expected, &mut GioAdminWriter)
}

fn save_admin_with(
    path: &Path,
    bytes: &[u8],
    expected: &SaveExpectation,
    writer: &mut impl AdminWriter,
) -> Result<SaveSuccess, SaveError> {
    let link = fs::symlink_metadata(path).map_err(map_admin_metadata_error)?;
    if link.file_type().is_symlink() {
        return Err(SaveError::ReadOnly(ReadOnlyReason::Symlink));
    }
    let metadata = fs::metadata(path).map_err(map_admin_metadata_error)?;
    if !metadata.is_file() {
        return Err(SaveError::NotRegular);
    }
    if metadata.nlink() > 1 {
        return Err(SaveError::ReadOnly(ReadOnlyReason::MultipleHardLinks));
    }
    let owner_uid = metadata.uid();
    let group_gid = metadata.gid();
    let mode = metadata.permissions().mode() & 0o7777;
    let file = crate::admin::gfile(path, crate::admin::Scope::Admin);
    let new_etag = writer
        .replace_contents(&file, bytes, expected.revision.etag.as_deref(), false)
        .map_err(map_admin_error)?;
    for (attribute, value) in [
        (gio::FILE_ATTRIBUTE_UNIX_UID.as_str(), owner_uid),
        (gio::FILE_ATTRIBUTE_UNIX_GID.as_str(), group_gid),
        (gio::FILE_ATTRIBUTE_UNIX_MODE.as_str(), mode),
    ] {
        writer
            .set_attribute_uint32(&file, attribute, value)
            .map_err(map_admin_error)?;
    }
    let metadata = fs::metadata(path).map_err(map_admin_metadata_error)?;
    let etag = query_info(path, gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
        .ok()
        .and_then(|info| {
            info.attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
                .map(|value| value.to_string())
        })
        .filter(|value| !value.is_empty())
        .or(new_etag);
    Ok(SaveSuccess {
        path: path.to_path_buf(),
        revision: FileRevision::from_metadata(&metadata, etag),
        size: metadata.len(),
        mode,
    })
}

fn map_admin_metadata_error(error: io::Error) -> SaveError {
    match error.kind() {
        io::ErrorKind::NotFound => SaveError::Missing,
        io::ErrorKind::PermissionDenied => SaveError::ReadOnly(ReadOnlyReason::NotWritable),
        _ => SaveError::Io(error.to_string()),
    }
}

fn map_admin_error(error: glib::Error) -> SaveError {
    match error.kind::<gio::IOErrorEnum>() {
        Some(gio::IOErrorEnum::WrongEtag) => SaveError::ChangedOnDisk,
        Some(gio::IOErrorEnum::NotFound) => SaveError::Missing,
        Some(gio::IOErrorEnum::PermissionDenied) => {
            SaveError::ReadOnly(ReadOnlyReason::NotWritable)
        }
        _ => SaveError::Io(error.to_string()),
    }
}

/// Write through GIO's conditional replacement. `expected` is both the object
/// identity and the ETag write precondition. The file is never truncated in
/// place; GIO writes a sibling and renames it into place.
pub fn save(request: &SaveRequest) -> Result<SaveSuccess, SaveError> {
    let _guard = SAVE_LOCK
        .lock()
        .map_err(|_| SaveError::Io("the save lock is poisoned".to_string()))?;
    let bytes = encode_text(&request.text, request.encoding)?;
    match &request.target {
        SaveTarget::Original { expected } => {
            let existing = existing_file(&request.path)?;
            if !expected.revision.same_object(&existing.revision)
                || !expected.revision.same_version(&existing.revision)
            {
                return Err(SaveError::ChangedOnDisk);
            }
            if !existing.writable {
                return Err(SaveError::ReadOnly(
                    existing.reason.unwrap_or(ReadOnlyReason::NotWritable),
                ));
            }
            replace_existing(&request.path, &bytes, &existing.revision, existing.mode)
        }
        SaveTarget::NewPath { replace } => match replace {
            Some(confirmation) => {
                if confirmation.path != request.path {
                    return Err(SaveError::DestinationChanged);
                }
                let existing = existing_file(&request.path)?;
                if !confirmation.revision.same_object(&existing.revision)
                    || !confirmation.revision.same_version(&existing.revision)
                {
                    return Err(SaveError::DestinationChanged);
                }
                if !existing.writable {
                    return Err(SaveError::ReadOnly(
                        existing.reason.unwrap_or(ReadOnlyReason::NotWritable),
                    ));
                }
                replace_existing(&request.path, &bytes, &existing.revision, existing.mode)
            }
            None => save_as_new(&request.path, &bytes),
        },
        SaveTarget::Admin { expected } => save_admin(&request.path, &bytes, expected),
    }
}

fn save_as_new(path: &Path, bytes: &[u8]) -> Result<SaveSuccess, SaveError> {
    let _parent = path.parent().ok_or(SaveError::NoParent)?;
    if !parent_writable(path) {
        return Err(SaveError::ReadOnly(ReadOnlyReason::ParentNotWritable));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => return Err(SaveError::DestinationAppeared),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(SaveError::Io(error.to_string())),
    }

    // Create the destination exclusively. This is the no-overwrite check: a
    // file that appeared after the chooser makes create_new fail. The zero-byte
    // placeholder is then replaced atomically with the committed bytes.
    let placeholder = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => SaveError::DestinationAppeared,
            io::ErrorKind::PermissionDenied => {
                SaveError::ReadOnly(ReadOnlyReason::ParentNotWritable)
            }
            _ => SaveError::Io(error.to_string()),
        })?;
    drop(placeholder);

    let placeholder_metadata =
        fs::metadata(path).map_err(|error| SaveError::Io(error.to_string()))?;
    let placeholder_revision = FileRevision::from_metadata(&placeholder_metadata, None);
    let created = match existing_file(path) {
        Ok(existing) => existing,
        Err(error) => {
            remove_own_placeholder(path, &placeholder_revision);
            return Err(error);
        }
    };
    match replace_existing(path, bytes, &created.revision, created.mode) {
        Ok(success) => Ok(success),
        Err(error) => {
            remove_own_placeholder(path, &created.revision);
            Err(error)
        }
    }
}

fn remove_own_placeholder(path: &Path, created: &FileRevision) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_file() && metadata.len() == 0 {
        let current = FileRevision::from_metadata(&metadata, None);
        if current.same_object(created) {
            let _ = fs::remove_file(path);
        }
    }
}

fn replace_existing(
    path: &Path,
    bytes: &[u8],
    expected: &FileRevision,
    mode: u32,
) -> Result<SaveSuccess, SaveError> {
    let file = gio::File::for_path(path);
    let result = file.replace_contents(
        bytes,
        expected.etag.as_deref(),
        false,
        gio::FileCreateFlags::NONE,
        None::<&gio::Cancellable>,
    );
    if let Err(error) = result {
        return Err(match error.kind::<gio::IOErrorEnum>() {
            Some(gio::IOErrorEnum::WrongEtag) => SaveError::ChangedOnDisk,
            Some(gio::IOErrorEnum::NotFound) => SaveError::Missing,
            Some(gio::IOErrorEnum::PermissionDenied) => {
                SaveError::ReadOnly(ReadOnlyReason::NotWritable)
            }
            _ => SaveError::Io(error.to_string()),
        });
    }

    // GIO replaces atomically. Restore the exact original mode afterwards so
    // an executable bit is never introduced or removed by the temp file.
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    let metadata = fs::metadata(path).map_err(|error| SaveError::Io(error.to_string()))?;
    let info = query_info(path, gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
        .map_err(|error| SaveError::Io(error.to_string()))?;
    let etag = info
        .attribute_string(gio::FILE_ATTRIBUTE_ETAG_VALUE.as_str())
        .map(|value| value.to_string())
        .filter(|value| !value.is_empty());
    Ok(SaveSuccess {
        path: path.to_path_buf(),
        revision: FileRevision::from_metadata(&metadata, etag),
        size: metadata.len(),
        mode,
    })
}

/// A read-only description of what `save` would do. No filesystem access.
pub fn save_plan(request: &SaveRequest) -> Vec<String> {
    let mut lines = Vec::new();
    let ending = match request.encoding.line_ending {
        Some(ending) => ending.as_str(),
        None => {
            lines.push("Refuse: mixed line endings are read-only.".to_string());
            return lines;
        }
    };
    let bytes = request.text.len()
        + if matches!(request.encoding.line_ending, Some(LineEnding::Crlf)) {
            request.text.matches('\n').count()
        } else {
            0
        }
        + if request.encoding.bom { 3 } else { 0 };
    lines.push(format!(
        "{} {} ({} bytes, UTF-8{}, {})",
        match &request.target {
            SaveTarget::Original { .. } => "Save",
            SaveTarget::Admin { .. } => "Save as Administrator",
            SaveTarget::NewPath { .. } => "Save As",
        },
        request.path.display(),
        bytes,
        if request.encoding.bom {
            " with BOM"
        } else {
            ""
        },
        ending
    ));
    match &request.target {
        SaveTarget::Original { expected } | SaveTarget::Admin { expected } => {
            lines.push(format!(
                "Expected object {} (ETag {})",
                expected.revision.identity_label(),
                expected.revision.etag.as_deref().unwrap_or("not provided")
            ));
        }
        SaveTarget::NewPath {
            replace: Some(confirmation),
        } => {
            lines.push(format!(
                "Replace confirmed object {} (ETag {})",
                confirmation.revision.identity_label(),
                confirmation
                    .revision
                    .etag
                    .as_deref()
                    .unwrap_or("not provided")
            ));
        }
        SaveTarget::NewPath { replace: None } => {
            lines.push("Refuse if any file exists at the destination.".to_string());
        }
    }
    lines
}

pub fn format_size(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024 * 1024 {
        return format!("{:.1} KiB", bytes as f64 / KIB);
    }
    format!("{:.1} MiB", bytes as f64 / MIB)
}

pub fn document_title(name: &str, size: u64) -> String {
    format!("{name} ({})", format_size(size))
}

// ---------------------------------------------------------------------------
// PDF first-page render requests
// ---------------------------------------------------------------------------

/// PDFs are read only when they are this small. The renderer output has its
/// own byte and pixel bounds below.
pub const PDF_MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
/// One PDF renderer gets this long to finish before its own child is killed.
pub const PDF_RENDER_TIMEOUT_MS: u64 = 10_000;
/// How many Poppler children may run at once across the application.
pub const PDF_MAX_ACTIVE_RENDERS: usize = 2;
/// The largest square box `pdftoppm -scale-to N` may be given. `-scale-to`
/// preserves aspect within an N×N box, so a square bound also guarantees the
/// [`MAX_RENDER_PIXELS`] bound before Poppler allocates the raster. The
/// installed CLI's `-scale-to-x`/`-scale-to-y` options scale one axis only (the
/// other keeps the default DPI), so they are not used.
pub const PDF_MAX_RASTER_EDGE: i32 = 4096;
/// The largest PNG read back from the renderer, before texture creation.
pub const PDF_MAX_OUTPUT_BYTES: u64 = MAX_RENDER_BYTES;

/// An aspect-preserving, two-axis bounded Poppler request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PdfRasterPlan {
    /// The value for `pdftoppm -scale-to`.
    pub scale_to: i32,
    pub limit: Option<RenderLimit>,
}

/// Fit `target`'s longest axis inside the conservative square raster bound.
/// Poppler's `-scale-to` then preserves the page aspect inside that square, so
/// no raster edge can exceed `scale_to` and no raster can exceed
/// [`MAX_RENDER_PIXELS`]. The actual page geometry is reported back to the UI
/// after the render; this plan deliberately does not need to parse the PDF.
pub fn pdf_raster_plan(target: RenderSize) -> PdfRasterPlan {
    let requested = target.width.max(target.height).max(1);
    let cap = PDF_MAX_RASTER_EDGE.clamp(1, MAX_RENDER_EDGE);
    let scale_to = requested.min(cap);
    PdfRasterPlan {
        scale_to,
        limit: (requested > cap).then_some(RenderLimit::Pixels),
    }
}

/// One first-page PDF render. All paths are absolute and are passed as
/// individual `OsString` arguments; no shell is involved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfRenderRequest {
    pub input: PathBuf,
    /// An exclusively created, job-owned directory.
    pub output_dir: PathBuf,
    pub target: RenderSize,
}

impl PdfRenderRequest {
    pub fn prefix(&self) -> PathBuf {
        self.output_dir.join("page")
    }

    pub fn output(&self) -> PathBuf {
        self.output_dir.join("page.png")
    }

    /// The verified Poppler 26.01.0 options: page 1 only, single file, PNG,
    /// scaled inside an aspect-preserving square box. `-scale-to` is the only
    /// installed option that bounds both raster axes without stretching; the
    /// post-render PNG checks remain defense and error reporting.
    pub fn arguments(&self) -> Vec<OsString> {
        let plan = pdf_raster_plan(self.target);
        let mut arguments: Vec<OsString> = Vec::with_capacity(12);
        for flag in ["-f", "1", "-l", "1", "-singlefile", "-png"] {
            arguments.push(OsString::from(flag));
        }
        arguments.push(OsString::from("-scale-to"));
        arguments.push(OsString::from(plan.scale_to.to_string()));
        arguments.push(self.input.clone().into_os_string());
        arguments.push(self.prefix().into_os_string());
        arguments
    }
}

/// A PDF input is accepted only when it is a regular local file within the
/// byte bound. This is checked before the child is spawned.
pub fn pdf_input_allowed(path: &Path) -> Result<u64, String> {
    let metadata =
        fs::symlink_metadata(path).map_err(|error| format!("PDF not available: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("PDF not available: not a regular file.".to_string());
    }
    if metadata.len() > PDF_MAX_INPUT_BYTES {
        return Err(format!(
            "This PDF is {}; PDF previews are limited to {}.",
            format_size(metadata.len()),
            format_size(PDF_MAX_INPUT_BYTES)
        ));
    }
    Ok(metadata.len())
}

/// The PNG's IHDR gives the rendered dimensions without decoding or
/// allocating pixels. A malformed or non-PNG file is refused.
pub fn png_dimensions(bytes: &[u8]) -> Option<RenderSize> {
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return None;
    }
    Some(RenderSize {
        width: width as i32,
        height: height as i32,
    })
}

/// Check rendered PNG bytes before GTK sees them.
pub fn pdf_output_allowed(bytes: &[u8]) -> Result<RenderSize, String> {
    if bytes.len() as u64 > PDF_MAX_OUTPUT_BYTES {
        return Err(format!(
            "Rendered PDF is larger than {}.",
            format_size(PDF_MAX_OUTPUT_BYTES)
        ));
    }
    let size = png_dimensions(bytes).ok_or_else(|| "Rendered PDF is not a PNG.".to_string())?;
    if size.width > MAX_RENDER_EDGE || size.height > MAX_RENDER_EDGE {
        return Err(RenderLimit::Edge.message());
    }
    if u64::from(size.width as u32) * u64::from(size.height as u32) > MAX_RENDER_PIXELS {
        return Err(RenderLimit::Pixels.message());
    }
    Ok(size)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
