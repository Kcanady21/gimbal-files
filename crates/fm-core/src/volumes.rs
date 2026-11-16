// SPDX-License-Identifier: GPL-3.0-or-later
//! The GTK-free volume inventory: which local volumes are mounted, which are
//! mountable, and the process-local identities a mount request may use.
//!
//! A backend enumerates live GIO volumes and mounts and hands in a
//! [`RawSnapshot`]. This module never opens `/dev`, never runs a command and
//! never treats a label, a path or a remote URI as a mount specification. The
//! only mountable identity is a backend-issued object serial with a native
//! unix-device identifier and `can_mount` set.
//!
//! Opaque ids are assigned here. An id and its incarnation are never reused:
//! removing a volume and inserting a different object with the same label,
//! UUID or device name produces a new id, so an old command cannot be
//! retargeted at the replacement.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

/// One mount as the backend currently sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawMount {
    /// The backend's identity for this exact live mount object. The GIO
    /// adapter changes this when the underlying object changes, even if its
    /// root, label or UUID do not.
    pub object: u64,
    pub root: PathBuf,
    pub label: String,
    /// The native unix-device identifier, when the mount has one.
    pub native: Option<String>,
    pub uuid: Option<String>,
    /// The backend mount object's own answer. This is the only unmount rule;
    /// no path list decides for a system mount.
    pub can_unmount: bool,
    /// The backend mount object's own answer. Ejecting powers the drive down
    /// where the service supports it.
    pub can_eject: bool,
}

/// One volume as the backend currently sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawVolume {
    /// The backend's identity for this exact live volume object.
    pub object: u64,
    /// GIO's `unix-device` identifier, for example `/dev/sdb1`. This is the
    /// only accepted native device identity.
    pub native: Option<String>,
    pub uuid: Option<String>,
    pub label: String,
    /// GIO's `can_mount` result.
    pub can_mount: bool,
    /// Whether the backend considers this a local device (it has a drive or a
    /// unix-device identifier). Remote/activation-only volumes are not local.
    pub has_drive: bool,
    pub mount: Option<RawMount>,
}

/// One enumeration from the backend.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawSnapshot {
    pub volumes: Vec<RawVolume>,
    /// Mounts that are not represented by one of the volumes above, including
    /// navigation-only mounts. The root mount is included here too; the
    /// inventory never makes a device row for it.
    pub mounts: Vec<RawMount>,
}

/// A process-local opaque volume id. It contains no device path and is only
/// meaningful to the process that issued it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VolumeId(String);

impl VolumeId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VolumeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a device row's mount root is, or why it cannot be mounted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Mounted {
        root: PathBuf,
        /// The backend serial of this exact live mount object. It changes when
        /// the same volume is remounted at the same root, so usage and row
        /// state cannot cross a mount replacement.
        mount_object: u64,
        /// The live mount's own capability answers.
        can_unmount: bool,
        can_eject: bool,
    },
    Unmounted {
        mountable: bool,
        /// A short, honest reason when `mountable` is false. Empty when true.
        explanation: String,
    },
}

impl DeviceState {
    pub fn is_mounted(&self) -> bool {
        matches!(self, DeviceState::Mounted { .. })
    }

    pub fn root(&self) -> Option<&Path> {
        match self {
            DeviceState::Mounted { root, .. } => Some(root),
            DeviceState::Unmounted { .. } => None,
        }
    }

    /// The live mount object's backend serial, when this entry is mounted.
    pub fn mount_object(&self) -> Option<u64> {
        match self {
            DeviceState::Mounted { mount_object, .. } => Some(*mount_object),
            DeviceState::Unmounted { .. } => None,
        }
    }

    pub fn can_unmount(&self) -> bool {
        matches!(
            self,
            DeviceState::Mounted {
                can_unmount: true,
                ..
            }
        )
    }

    pub fn can_eject(&self) -> bool {
        matches!(
            self,
            DeviceState::Mounted {
                can_eject: true,
                ..
            }
        )
    }

    pub fn mountable(&self) -> bool {
        matches!(
            self,
            DeviceState::Unmounted {
                mountable: true,
                ..
            }
        )
    }

    pub fn explanation(&self) -> Option<&str> {
        match self {
            DeviceState::Mounted { .. } => None,
            DeviceState::Unmounted {
                mountable: false,
                explanation,
            } => Some(explanation),
            DeviceState::Unmounted {
                mountable: true, ..
            } => None,
        }
    }
}

/// One row in the Devices section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceEntry {
    id: VolumeId,
    incarnation: u64,
    label: String,
    native: Option<String>,
    uuid: Option<String>,
    state: DeviceState,
    object: u64,
    volume_object: bool,
    order: u64,
}

impl DeviceEntry {
    pub fn id(&self) -> &VolumeId {
        &self.id
    }

    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn native(&self) -> Option<&str> {
        self.native.as_deref()
    }

    pub fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub fn state(&self) -> &DeviceState {
        &self.state
    }

    pub fn mount_object(&self) -> Option<u64> {
        self.state.mount_object()
    }

    pub fn can_unmount(&self) -> bool {
        self.state.can_unmount()
    }

    pub fn can_eject(&self) -> bool {
        self.state.can_eject()
    }

    /// The backend object serial. It is not a mount specification on its own;
    /// [`Inventory::mount_target`] binds it to the entry's id and incarnation.
    pub fn object(&self) -> u64 {
        self.object
    }

    pub fn is_volume(&self) -> bool {
        self.volume_object
    }
}

/// A live mount candidate. It is produced only by [`Inventory::mount_target`]
/// after checking the entry's current state and native identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountTarget {
    id: VolumeId,
    incarnation: u64,
    object: u64,
    native: String,
    uuid: Option<String>,
    label: String,
}

impl MountTarget {
    pub fn id(&self) -> &VolumeId {
        &self.id
    }

    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    pub fn object(&self) -> u64 {
        self.object
    }

    pub fn native(&self) -> &str {
        &self.native
    }

    pub fn uuid(&self) -> Option<&str> {
        self.uuid.as_deref()
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

/// The two reviewed ways to take a mounted device away.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DriveAction {
    Unmount,
    Eject,
}

impl DriveAction {
    pub fn verb(self) -> &'static str {
        match self {
            DriveAction::Unmount => "Unmount",
            DriveAction::Eject => "Eject",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            DriveAction::Unmount => "drive.unmount",
            DriveAction::Eject => "drive.eject",
        }
    }

    pub fn pending_text(self) -> &'static str {
        match self {
            DriveAction::Unmount => "Unmounting…",
            DriveAction::Eject => "Ejecting…",
        }
    }
}

impl fmt::Display for DriveAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DriveAction::Unmount => "unmount",
            DriveAction::Eject => "eject",
        })
    }
}

/// A live mounted device candidate. It is produced only by
/// [`Inventory::removal_target`] for the exact live mount incarnation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalTarget {
    id: VolumeId,
    incarnation: u64,
    mount_object: u64,
    root: PathBuf,
    label: String,
    action: DriveAction,
}

impl RemovalTarget {
    pub fn id(&self) -> &VolumeId {
        &self.id
    }

    pub fn incarnation(&self) -> u64 {
        self.incarnation
    }

    pub fn mount_object(&self) -> u64 {
        self.mount_object
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn action(&self) -> DriveAction {
        self.action
    }
}

/// Why a mounted device cannot start an unmount or eject. These are refusals
/// before any backend call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalRefusal {
    UnknownId,
    NotMounted,
    Pending,
    NotSupported(DriveAction),
}

impl fmt::Display for RemovalRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemovalRefusal::UnknownId => {
                f.write_str("that volume is no longer present; refresh the Device list")
            }
            RemovalRefusal::NotMounted => f.write_str("that volume is not mounted"),
            RemovalRefusal::Pending => {
                f.write_str("an operation is already running for that volume")
            }
            RemovalRefusal::NotSupported(action) => write!(
                f,
                "the volume service does not offer to {} that volume",
                action
            ),
        }
    }
}

/// Why a device id cannot start a mount. These are refusals before any backend
/// call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountRefusal {
    UnknownId,
    Ineligible(String),
}

impl fmt::Display for MountRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MountRefusal::UnknownId => {
                f.write_str("that volume is no longer present; refresh the Device list")
            }
            MountRefusal::Ineligible(reason) => f.write_str(reason),
        }
    }
}

/// A short mount result. The detail is kept separately so a row can show a
/// one-line state while its tooltip or status area carries the full error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountFailureKind {
    Cancelled,
    Denied,
    DeviceGone,
    BackendUnavailable,
    Busy,
    /// The backend already showed its own dialog; a second status message
    /// would duplicate it.
    Handled,
    Failed,
}

impl MountFailureKind {
    pub fn short(self) -> &'static str {
        match self {
            MountFailureKind::Cancelled => "Mount cancelled",
            MountFailureKind::Denied => "Permission denied",
            MountFailureKind::DeviceGone => "Device removed",
            MountFailureKind::BackendUnavailable => "Volume service unavailable",
            MountFailureKind::Busy => "Drive busy",
            MountFailureKind::Handled => "",
            MountFailureKind::Failed => "Mount failed",
        }
    }
}

/// The shared state of one mount attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountStatus {
    /// The request has been accepted; this is not a mounted volume yet.
    Pending,
    /// The backend and the current inventory agree that this exact volume
    /// incarnation is mounted at this root, by this exact mount object.
    Mounted { root: PathBuf, mount_object: u64 },
    Failed {
        kind: MountFailureKind,
        detail: String,
    },
}

impl MountStatus {
    pub fn pending() -> Self {
        MountStatus::Pending
    }

    pub fn mounted(root: PathBuf, mount_object: u64) -> Self {
        MountStatus::Mounted { root, mount_object }
    }

    pub fn mount_object(&self) -> Option<u64> {
        match self {
            MountStatus::Mounted { mount_object, .. } => Some(*mount_object),
            _ => None,
        }
    }

    pub fn failed(kind: MountFailureKind, detail: impl Into<String>) -> Self {
        MountStatus::Failed {
            kind,
            detail: detail.into(),
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, MountStatus::Pending)
    }

    pub fn is_mounted(&self) -> bool {
        matches!(self, MountStatus::Mounted { .. })
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_pending()
    }

    /// The one-line row text. Mounted rows read their actual root separately.
    pub fn short(&self) -> &'static str {
        match self {
            MountStatus::Pending => "Mounting…",
            MountStatus::Mounted { .. } => "Mounted",
            MountStatus::Failed { kind, .. } => kind.short(),
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            MountStatus::Failed { detail, .. } => Some(detail),
            _ => None,
        }
    }
}

/// A short unmount/eject result. The detail stays separate, as with mounts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemovalFailureKind {
    Cancelled,
    Denied,
    DeviceGone,
    BackendUnavailable,
    Busy,
    Handled,
    Failed,
}

impl RemovalFailureKind {
    pub fn short(self, action: DriveAction) -> &'static str {
        match self {
            RemovalFailureKind::Cancelled => match action {
                DriveAction::Unmount => "Unmount cancelled",
                DriveAction::Eject => "Eject cancelled",
            },
            RemovalFailureKind::Denied => "Permission denied",
            RemovalFailureKind::DeviceGone => "Device removed",
            RemovalFailureKind::BackendUnavailable => "Volume service unavailable",
            RemovalFailureKind::Busy => "Drive busy",
            RemovalFailureKind::Handled => "",
            RemovalFailureKind::Failed => match action {
                DriveAction::Unmount => "Unmount failed",
                DriveAction::Eject => "Eject failed",
            },
        }
    }
}

/// The shared state of one unmount/eject attempt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalStatus {
    Pending(DriveAction),
    Unmounted,
    Ejected,
    Failed {
        action: DriveAction,
        kind: RemovalFailureKind,
        detail: String,
    },
}

impl RemovalStatus {
    pub fn pending(action: DriveAction) -> Self {
        RemovalStatus::Pending(action)
    }

    pub fn failed(
        action: DriveAction,
        kind: RemovalFailureKind,
        detail: impl Into<String>,
    ) -> Self {
        RemovalStatus::Failed {
            action,
            kind,
            detail: detail.into(),
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self, RemovalStatus::Pending(_))
    }

    pub fn is_terminal(&self) -> bool {
        !self.is_pending()
    }

    pub fn action(&self) -> Option<DriveAction> {
        match self {
            RemovalStatus::Pending(action) => Some(*action),
            RemovalStatus::Failed { action, .. } => Some(*action),
            RemovalStatus::Unmounted | RemovalStatus::Ejected => None,
        }
    }

    pub fn kind(&self) -> Option<RemovalFailureKind> {
        match self {
            RemovalStatus::Failed { kind, .. } => Some(*kind),
            _ => None,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            RemovalStatus::Failed { detail, .. } => Some(detail),
            _ => None,
        }
    }
}

/// What one reconciliation changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    pub changed: bool,
    pub added: Vec<VolumeId>,
    pub removed: Vec<VolumeId>,
}

struct Candidate {
    object: u64,
    volume_object: bool,
    label: String,
    native: Option<String>,
    uuid: Option<String>,
    state: DeviceState,
}

/// The process's one volume inventory. Rebuild it with [`Self::reconcile`];
/// every window reads the same entries.
#[derive(Debug, Default)]
pub struct Inventory {
    entries: Vec<DeviceEntry>,
    next_id: u64,
    next_incarnation: u64,
    next_order: u64,
    root_mount_object: Option<u64>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn entries(&self) -> &[DeviceEntry] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entry(&self, id: &VolumeId) -> Option<&DeviceEntry> {
        self.entries.iter().find(|entry| &entry.id == id)
    }

    pub fn entry_for_device(&self, device: &str) -> Option<&DeviceEntry> {
        self.entries
            .iter()
            .find(|entry| entry.id.as_str() == device)
    }

    /// The only path from a command's text to a mount. Unknown and stale ids,
    /// already mounted entries, and volumes without a native identity or
    /// `can_mount` all fail here, before the backend is called.
    pub fn mount_target(&self, device: &str) -> Result<MountTarget, MountRefusal> {
        let entry = self
            .entry_for_device(device)
            .ok_or(MountRefusal::UnknownId)?;
        match &entry.state {
            DeviceState::Mounted { .. } => Err(MountRefusal::Ineligible(
                "that volume is already mounted".to_string(),
            )),
            DeviceState::Unmounted {
                mountable: false,
                explanation,
            } => Err(MountRefusal::Ineligible(explanation.clone())),
            DeviceState::Unmounted {
                mountable: true, ..
            } => {
                let Some(native) = entry.native.clone() else {
                    return Err(MountRefusal::Ineligible(
                        "that volume has no native device identifier".to_string(),
                    ));
                };
                Ok(MountTarget {
                    id: entry.id.clone(),
                    incarnation: entry.incarnation,
                    object: entry.object,
                    native,
                    uuid: entry.uuid.clone(),
                    label: entry.label.clone(),
                })
            }
        }
    }

    /// The only path from a command's text to an unmount or eject. Unknown ids,
    /// unmounted entries, an operation already running for this id, and a
    /// mount whose own `can_unmount`/`can_eject` answer is false all fail here,
    /// before the backend is called. The main drive and `/` are never device
    /// rows, so they cannot produce a target either.
    pub fn removal_target(
        &self,
        device: &str,
        action: DriveAction,
        pending: bool,
    ) -> Result<RemovalTarget, RemovalRefusal> {
        let entry = self
            .entry_for_device(device)
            .ok_or(RemovalRefusal::UnknownId)?;
        if pending {
            return Err(RemovalRefusal::Pending);
        }
        let DeviceState::Mounted {
            root,
            mount_object,
            can_unmount,
            can_eject,
        } = &entry.state
        else {
            return Err(RemovalRefusal::NotMounted);
        };
        let supported = match action {
            DriveAction::Unmount => *can_unmount,
            DriveAction::Eject => *can_eject,
        };
        if !supported {
            return Err(RemovalRefusal::NotSupported(action));
        }
        Ok(RemovalTarget {
            id: entry.id.clone(),
            incarnation: entry.incarnation,
            mount_object: *mount_object,
            root: root.clone(),
            label: entry.label.clone(),
            action,
        })
    }

    /// Whether this exact entry incarnation is mounted now, where, and by
    /// which live mount object. A late callback uses this instead of trusting
    /// its own success.
    pub fn reconcile_mount(&self, id: &VolumeId, incarnation: u64) -> Option<(PathBuf, u64)> {
        self.entry(id)
            .filter(|entry| entry.incarnation == incarnation)
            .and_then(|entry| match &entry.state {
                DeviceState::Mounted {
                    root, mount_object, ..
                } => Some((root.clone(), *mount_object)),
                DeviceState::Unmounted { .. } => None,
            })
    }

    /// The backend serial of the root mount currently backing Main Drive, when
    /// the monitor exposes one. It is part of the main usage identity.
    pub fn root_mount_object(&self) -> Option<u64> {
        self.root_mount_object
    }

    /// Fold one backend snapshot into the inventory. A live entry keeps its
    /// id and incarnation while the backend reports the same object; anything
    /// absent loses its id permanently.
    pub fn reconcile(&mut self, snapshot: &RawSnapshot) -> ReconcileReport {
        self.root_mount_object = snapshot
            .volumes
            .iter()
            .filter_map(|volume| volume.mount.as_ref())
            .find(|mount| mount.root == Path::new("/"))
            .map(|mount| mount.object)
            .or_else(|| {
                snapshot
                    .mounts
                    .iter()
                    .find(|mount| mount.root == Path::new("/"))
                    .map(|mount| mount.object)
            });
        let candidates = candidates(snapshot);
        let old_entries = std::mem::take(&mut self.entries);
        let mut next_entries = Vec::with_capacity(candidates.len());
        let mut added = Vec::new();
        let mut changed = false;

        for candidate in candidates {
            let existing = old_entries.iter().find(|entry| {
                entry.object == candidate.object && entry.volume_object == candidate.volume_object
            });
            if let Some(old) = existing {
                let mut entry = old.clone();
                if entry.label != candidate.label
                    || entry.native != candidate.native
                    || entry.uuid != candidate.uuid
                    || entry.state != candidate.state
                {
                    changed = true;
                }
                entry.label = candidate.label;
                entry.native = candidate.native;
                entry.uuid = candidate.uuid;
                entry.state = candidate.state;
                next_entries.push(entry);
            } else {
                self.next_id += 1;
                self.next_incarnation += 1;
                self.next_order += 1;
                let id = VolumeId(format!("volume-{}", self.next_id));
                added.push(id.clone());
                next_entries.push(DeviceEntry {
                    id: id.clone(),
                    incarnation: self.next_incarnation,
                    label: candidate.label,
                    native: candidate.native,
                    uuid: candidate.uuid,
                    state: candidate.state,
                    object: candidate.object,
                    volume_object: candidate.volume_object,
                    order: self.next_order,
                });
                changed = true;
            }
        }

        let removed: Vec<VolumeId> = old_entries
            .iter()
            .filter(|old| {
                !next_entries
                    .iter()
                    .any(|entry| entry.id == old.id && entry.incarnation == old.incarnation)
            })
            .map(|entry| entry.id.clone())
            .collect();
        if !removed.is_empty() {
            changed = true;
        }

        // Rows keep the order they first appeared in; new rows append. A
        // monitor reordering cannot move an existing row under the pointer or
        // its open context menu.
        next_entries.sort_by_key(|entry| entry.order);
        self.entries = next_entries;

        ReconcileReport {
            changed,
            added,
            removed,
        }
    }
}

fn candidates(snapshot: &RawSnapshot) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    let mut claimed_mount_objects: Vec<u64> = Vec::new();
    let mut claimed_roots: Vec<PathBuf> = Vec::new();

    for volume in &snapshot.volumes {
        let local = volume.native.is_some() || volume.has_drive;
        if !local {
            continue;
        }
        if let Some(mount) = &volume.mount {
            claimed_mount_objects.push(mount.object);
            claimed_roots.push(mount.root.clone());
            if mount.root == Path::new("/") {
                continue;
            }
            candidates.push(Candidate {
                object: volume.object,
                volume_object: true,
                label: first_label(&[&volume.label, &mount.label])
                    .unwrap_or_else(|| fallback_label(volume.native.as_deref(), None)),
                native: volume.native.clone(),
                uuid: volume.uuid.clone(),
                state: DeviceState::Mounted {
                    root: mount.root.clone(),
                    mount_object: mount.object,
                    can_unmount: mount.can_unmount,
                    can_eject: mount.can_eject,
                },
            });
            continue;
        }

        let (mountable, explanation) = match (&volume.native, volume.can_mount) {
            (Some(_), true) => (true, String::new()),
            (Some(_), false) => (
                false,
                "This desktop's volume service does not offer to mount it.".to_string(),
            ),
            (None, _) => (
                false,
                "This local volume has no mountable unix device identifier.".to_string(),
            ),
        };
        candidates.push(Candidate {
            object: volume.object,
            volume_object: true,
            label: first_label(&[&volume.label])
                .unwrap_or_else(|| fallback_label(volume.native.as_deref(), None)),
            native: volume.native.clone(),
            uuid: volume.uuid.clone(),
            state: DeviceState::Unmounted {
                mountable,
                explanation,
            },
        });
    }

    for mount in &snapshot.mounts {
        if mount.root == Path::new("/") {
            continue;
        }
        if claimed_mount_objects.contains(&mount.object) || claimed_roots.contains(&mount.root) {
            continue;
        }
        if candidates
            .iter()
            .any(|candidate| candidate.state.root() == Some(mount.root.as_path()))
        {
            continue;
        }
        claimed_mount_objects.push(mount.object);
        claimed_roots.push(mount.root.clone());
        candidates.push(Candidate {
            object: mount.object,
            volume_object: false,
            label: first_label(&[&mount.label])
                .unwrap_or_else(|| fallback_label(mount.native.as_deref(), Some(&mount.root))),
            native: mount.native.clone(),
            uuid: mount.uuid.clone(),
            state: DeviceState::Mounted {
                root: mount.root.clone(),
                mount_object: mount.object,
                can_unmount: mount.can_unmount,
                can_eject: mount.can_eject,
            },
        });
    }

    candidates
}

fn first_label(values: &[&String]) -> Option<String> {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

fn fallback_label(native: Option<&str>, root: Option<&Path>) -> String {
    native
        .and_then(|native| Path::new(native).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .or_else(|| {
            root.and_then(|root| root.file_name())
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Drive".to_string())
}

// -------------------------------------------------------------------------
// Internal drives: fstab-mounted disks the volume monitor hides
// -------------------------------------------------------------------------
//
// udisks2 and GIO's unix-mount code treat a boot mount outside `/media`,
// `/run/media/$USER` or the home folder as system-internal and hide it from
// the volume monitor (GLib's `g_unix_is_mount_path_system_internal` and
// `g_unix_mount_entry_guess_should_display` in `gio/gunixmounts.c`). Such a
// disk is real user storage, so the sidebar reads the kernel's own table
// instead. The parsing and every decision here are pure: the UI hands in the
// `/proc/self/mountinfo` text, the mount paths the Devices section already
// shows, and a device-to-label map, so no test needs a real disk.

/// How often the volume service re-reads `/proc/self/mountinfo` between
/// monitor events. This is a polling period, not an animation, so it stays
/// outside `look::motion`.
pub const MOUNTINFO_POLL_SECS: u64 = 5;

/// The filesystem types that count as disk filesystems. Anything else
/// (tmpfs, overlay, squashfs, `fuse.*`, nfs, cifs, …) is not a drive.
const DISK_FILESYSTEMS: [&str; 14] = [
    "bcachefs", "btrfs", "exfat", "ext2", "ext3", "ext4", "f2fs", "jfs", "ntfs", "ntfs3",
    "reiserfs", "vfat", "xfs", "zfs",
];

/// Mount points that are the system's own, by exact name.
const SYSTEM_MOUNT_EXACT: [&str; 4] = ["/", "/home", "/boot", "/efi"];

/// Mount points that are the system's own, and everything under them.
/// `/run/media` is the exception Devices already covers.
const SYSTEM_MOUNT_PREFIXES: [&str; 12] = [
    "/boot", "/efi", "/usr", "/var", "/opt", "/srv", "/nix", "/snap", "/proc", "/sys", "/dev",
    "/run",
];

/// One fstab-mounted data disk the Devices section does not show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InternalDrive {
    pub mount_path: PathBuf,
    /// The kernel's mount source, for example `/dev/sda1`.
    pub device: String,
    pub fs_type: String,
    /// The filesystem label from `/dev/disk/by-label`, when there is one.
    pub label: Option<String>,
}

impl InternalDrive {
    /// The row name: the label, else the mount folder's name.
    pub fn name(&self) -> String {
        match &self.label {
            Some(label) if !label.trim().is_empty() => label.clone(),
            _ => self
                .mount_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.mount_path.display().to_string()),
        }
    }

    /// The row tooltip: the mount path and the device.
    pub fn tooltip(&self) -> String {
        format!("{} — {}", self.mount_path.display(), self.device)
    }
}

/// One line of `/proc/self/mountinfo` with only the fields the filter reads
/// (`proc_pid_mountinfo(5)`). Octal escapes are already decoded.
#[derive(Clone, Debug, PartialEq, Eq)]
struct MountRecord {
    /// The block device's major:minor, when the line carries one. btrfs
    /// subvolumes share one anonymous (0, n) pair per filesystem, so this —
    /// not only the source string — identifies the root filesystem's device.
    device_id: Option<(u64, u64)>,
    mount_path: PathBuf,
    fs_type: String,
    source: String,
    /// The mount options and the super options, comma-joined as the kernel
    /// writes them.
    options: String,
}

/// Decodes mountinfo's octal escapes (`\040` for a space, `\011`, `\012` and
/// `\134` for a backslash). Mount paths contain spaces on real machines.
fn decode_mountinfo_field(field: &str) -> Vec<u8> {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..index + 4]
                .iter()
                .all(|digit| (b'0'..=b'7').contains(digit))
        {
            let value = (bytes[index + 1] - b'0') * 64
                + (bytes[index + 2] - b'0') * 8
                + (bytes[index + 3] - b'0');
            decoded.push(value);
            index += 4;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    decoded
}

fn decode_mountinfo_path(field: &str) -> PathBuf {
    PathBuf::from(OsString::from_vec(decode_mountinfo_field(field)))
}

fn decode_mountinfo_string(field: &str) -> String {
    String::from_utf8_lossy(&decode_mountinfo_field(field)).into_owned()
}

/// Parses mountinfo text. Whitespace never appears literally inside a field
/// (it is octal-escaped), so splitting on whitespace is exact.
fn parse_mountinfo(text: &str) -> Vec<MountRecord> {
    let mut records = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 7 {
            continue;
        }
        let device_id = fields[2]
            .split_once(':')
            .and_then(|(major, minor)| Some((major.parse().ok()?, minor.parse().ok()?)));
        // Fields 6.. are the optional tag-value fields up to the "-"
        // separator, then fstype, source and the super options.
        let Some(offset) = fields[6..].iter().position(|field| *field == "-") else {
            continue;
        };
        let tail = &fields[6 + offset + 1..];
        if tail.len() < 3 {
            continue;
        }
        records.push(MountRecord {
            device_id,
            mount_path: decode_mountinfo_path(fields[4]),
            fs_type: decode_mountinfo_string(tail[0]),
            source: decode_mountinfo_string(tail[1]),
            options: format!("{},{}", fields[5], tail[2]),
        });
    }
    records
}

fn is_disk_filesystem(fs_type: &str) -> bool {
    DISK_FILESYSTEMS.contains(&fs_type)
}

/// Whether the mount point belongs to the system's own layout. `/run/media`
/// and everything under it stay eligible: they are Devices' territory and the
/// Devices-duplication rule removes them, they are not hidden here.
fn is_system_mount_path(path: &Path) -> bool {
    if SYSTEM_MOUNT_EXACT
        .iter()
        .any(|name| path == Path::new(name))
    {
        return true;
    }
    if path.starts_with("/run/media") {
        return false;
    }
    SYSTEM_MOUNT_PREFIXES
        .iter()
        .any(|prefix| path.starts_with(prefix))
}

fn has_option(options: &str, option: &str) -> bool {
    options.split(',').any(|field| field == option)
}

/// Whether one mount path is the shorter of a device's mounts — fewest
/// bytes, and byte order breaks an equal-length tie deterministically.
fn mount_path_is_shorter(candidate: &Path, kept: &Path) -> bool {
    let candidate = candidate.as_os_str().as_bytes();
    let kept = kept.as_os_str().as_bytes();
    (candidate.len(), candidate) < (kept.len(), kept)
}

/// The Internal Drives rows for one mountinfo table.
///
/// `devices_roots` are the mount paths the Devices section already shows; a
/// mount at one of them never appears in both sections. `by_label` maps a
/// canonical `/dev/…` node to its `/dev/disk/by-label` name; reading the
/// symlinks is the UI's job, deciding here stays pure.
///
/// A mount qualifies when every rule holds: the source is a `/dev/…` block
/// device; the filesystem is a disk filesystem; the mount path is absolute
/// and not the system's own; the device is not the root filesystem's device
/// (this skips btrfs subvolumes of the system disk); the options do not carry
/// `x-gvfs-hide`; and Devices does not already show the path. One device
/// mounted twice yields one row at the shortest mount path.
pub fn internal_drives(
    mountinfo: &str,
    devices_roots: &[PathBuf],
    by_label: &BTreeMap<String, String>,
) -> Vec<InternalDrive> {
    let records = parse_mountinfo(mountinfo);
    let root = records
        .iter()
        .find(|record| record.mount_path == Path::new("/"));
    let mut shortest: BTreeMap<&str, &MountRecord> = BTreeMap::new();
    for record in &records {
        if !is_disk_filesystem(&record.fs_type)
            || !record.source.starts_with("/dev/")
            || !record.mount_path.is_absolute()
            || is_system_mount_path(&record.mount_path)
            || has_option(&record.options, "x-gvfs-hide")
            || devices_roots.contains(&record.mount_path)
        {
            continue;
        }
        if let Some(root) = root {
            let same_source = record.source == root.source;
            let same_device = record
                .device_id
                .is_some_and(|id| root.device_id == Some(id));
            if same_source || same_device {
                continue;
            }
        }
        let take = match shortest.get(record.source.as_str()) {
            Some(kept) => mount_path_is_shorter(&record.mount_path, &kept.mount_path),
            None => true,
        };
        if take {
            shortest.insert(record.source.as_str(), record);
        }
    }
    let mut drives: Vec<InternalDrive> = shortest
        .values()
        .map(|record| InternalDrive {
            mount_path: record.mount_path.clone(),
            device: record.source.clone(),
            fs_type: record.fs_type.clone(),
            label: by_label
                .get(&record.source)
                .map(|label| label.trim().to_string())
                .filter(|label| !label.is_empty()),
        })
        .collect();
    drives.sort_by(|a, b| a.mount_path.cmp(&b.mount_path));
    drives
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(object: u64, root: &str, label: &str) -> RawMount {
        RawMount {
            object,
            root: PathBuf::from(root),
            label: label.to_string(),
            native: None,
            uuid: None,
            can_unmount: true,
            can_eject: true,
        }
    }

    fn volume(object: u64, native: Option<&str>, label: &str) -> RawVolume {
        RawVolume {
            object,
            native: native.map(str::to_string),
            uuid: Some(format!("uuid-{object}")),
            label: label.to_string(),
            can_mount: true,
            has_drive: true,
            mount: None,
        }
    }

    fn snapshot(volumes: Vec<RawVolume>, mounts: Vec<RawMount>) -> RawSnapshot {
        RawSnapshot { volumes, mounts }
    }

    #[test]
    fn mounted_unmounted_and_navigation_mounts_dedupe_and_root_is_never_a_device() {
        let mut mounted = volume(2, Some("/dev/sdb1"), "Camera");
        mounted.mount = Some(RawMount {
            object: 20,
            root: PathBuf::from("/media/camera"),
            label: "Camera".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-2".to_string()),
            can_unmount: true,
            can_eject: true,
        });
        let snapshot = snapshot(
            vec![volume(1, Some("/dev/sda2"), "USB"), mounted],
            vec![
                mount(20, "/media/camera", "Camera"),
                mount(30, "/media/other", "Other"),
                mount(40, "/", "Root"),
            ],
        );
        let mut inventory = Inventory::new();
        let report = inventory.reconcile(&snapshot);
        assert!(report.changed);
        assert_eq!(inventory.entries().len(), 3);

        let usb = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "USB")
            .unwrap();
        assert!(usb.state().mountable());
        assert_eq!(usb.state().explanation(), None);

        let camera = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "Camera")
            .unwrap();
        assert_eq!(camera.state().root(), Some(Path::new("/media/camera")));

        let other = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "Other")
            .unwrap();
        assert!(!other.is_volume());
        assert_eq!(other.state().root(), Some(Path::new("/media/other")));

        assert!(
            inventory
                .entries()
                .iter()
                .all(|entry| entry.state().root() != Some(Path::new("/")))
        );
    }

    #[test]
    fn duplicate_labels_are_not_identity() {
        let snapshot = snapshot(
            vec![
                volume(1, Some("/dev/sdb1"), "USB"),
                volume(2, Some("/dev/sdc1"), "USB"),
            ],
            vec![],
        );
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot);
        let ids: Vec<&VolumeId> = inventory.entries().iter().map(DeviceEntry::id).collect();
        assert_eq!(ids.len(), 2);
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn reinsert_with_the_same_device_uuid_and_label_gets_a_new_id() {
        let first = snapshot(vec![volume(1, Some("/dev/sdb1"), "USB")], vec![]);
        let mut inventory = Inventory::new();
        inventory.reconcile(&first);
        let old_id = inventory.entries()[0].id().clone();
        assert!(inventory.mount_target(old_id.as_str()).is_ok());

        let removed = inventory.reconcile(&snapshot(vec![], vec![]));
        assert_eq!(removed.removed, vec![old_id.clone()]);
        assert_eq!(
            inventory.mount_target(old_id.as_str()),
            Err(MountRefusal::UnknownId)
        );

        let replacement = snapshot(vec![volume(2, Some("/dev/sdb1"), "USB")], vec![]);
        inventory.reconcile(&replacement);
        let new_id = inventory.entries()[0].id().clone();
        assert_ne!(old_id, new_id);
        assert_eq!(
            inventory.mount_target(old_id.as_str()),
            Err(MountRefusal::UnknownId)
        );
        assert!(inventory.mount_target(new_id.as_str()).is_ok());
    }

    #[test]
    fn stable_objects_keep_their_id_and_order_across_unrelated_rebuilds() {
        let first_snapshot = snapshot(
            vec![
                volume(1, Some("/dev/sdb1"), "One"),
                volume(2, Some("/dev/sdc1"), "Two"),
            ],
            vec![],
        );
        let mut inventory = Inventory::new();
        inventory.reconcile(&first_snapshot);
        let ids: Vec<VolumeId> = inventory
            .entries()
            .iter()
            .map(|entry| entry.id().clone())
            .collect();

        // Reordered enumeration, a new label and unchanged objects.
        let mut second = volume(2, Some("/dev/sdc1"), "Renamed two");
        second.uuid = Some("uuid-2".to_string());
        let mut first = volume(1, Some("/dev/sdb1"), "One");
        first.uuid = Some("uuid-1".to_string());
        let report = inventory.reconcile(&snapshot(vec![second, first], vec![]));
        assert!(report.changed, "an updated label is an update");
        let after: Vec<VolumeId> = inventory
            .entries()
            .iter()
            .map(|entry| entry.id().clone())
            .collect();
        assert_eq!(ids, after);
        assert_eq!(inventory.entries()[1].label(), "Renamed two");
    }

    #[test]
    fn unsupported_and_non_native_volumes_explain_the_refusal() {
        let mut no_mount = volume(1, Some("/dev/sdb1"), "Locked");
        no_mount.can_mount = false;
        let no_native = volume(2, None, "Mystery");
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![no_mount, no_native], vec![]));

        assert_eq!(inventory.entries().len(), 2);
        let locked = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "Locked")
            .unwrap();
        assert!(!locked.state().mountable());
        assert!(
            inventory
                .mount_target(locked.id().as_str())
                .unwrap_err()
                .to_string()
                .contains("does not offer")
        );
        let mystery = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "Mystery")
            .unwrap();
        assert!(!mystery.state().mountable());
        assert!(
            inventory
                .mount_target(mystery.id().as_str())
                .unwrap_err()
                .to_string()
                .contains("unix device")
        );
    }

    #[test]
    fn remote_volume_without_a_drive_or_native_id_is_not_a_device_row() {
        let mut remote = volume(1, None, "Network share");
        remote.has_drive = false;
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![remote], vec![]));
        assert!(inventory.entries().is_empty());
    }

    #[test]
    fn mount_targets_bind_the_live_incarnation_and_reconcile_only_that_one() {
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![volume(1, Some("/dev/sdb1"), "USB")], vec![]));
        let target = inventory
            .mount_target(inventory.entries()[0].id().as_str())
            .unwrap();
        assert_eq!(target.object(), 1);
        assert_eq!(target.native(), "/dev/sdb1");

        let mount = RawMount {
            object: 9,
            root: PathBuf::from("/media/usb"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: true,
        };
        let mut mounted = volume(1, Some("/dev/sdb1"), "USB");
        mounted.mount = Some(mount);
        inventory.reconcile(&snapshot(vec![mounted], vec![]));
        assert_eq!(
            inventory.reconcile_mount(target.id(), target.incarnation()),
            Some((PathBuf::from("/media/usb"), 9))
        );
        assert_eq!(
            inventory.reconcile_mount(target.id(), target.incarnation() + 999),
            None
        );
        assert_eq!(
            inventory.mount_target(target.id().as_str()),
            Err(MountRefusal::Ineligible(
                "that volume is already mounted".to_string()
            ))
        );
    }

    #[test]
    fn same_volume_same_root_mount_replacement_carries_the_new_mount_object() {
        let mut first = volume(1, Some("/dev/sdb1"), "USB");
        first.mount = Some(RawMount {
            object: 20,
            root: PathBuf::from("/media/usb"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: true,
        });
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![first], vec![]));
        let old = inventory.entries()[0].clone();
        assert_eq!(old.mount_object(), Some(20));
        assert_eq!(
            inventory.reconcile_mount(old.id(), old.incarnation()),
            Some((PathBuf::from("/media/usb"), 20))
        );

        // The backend reports the same volume and root, but a different live
        // mount object: this is a remount, not the same evidence.
        let mut second = volume(1, Some("/dev/sdb1"), "USB");
        second.mount = Some(RawMount {
            object: 33,
            root: PathBuf::from("/media/usb"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: true,
        });
        let report = inventory.reconcile(&snapshot(vec![second], vec![]));
        assert!(report.changed, "a new mount object changes the row state");
        let new = inventory.entries()[0].clone();
        assert_eq!(new.id(), old.id(), "the volume object kept its opaque id");
        assert_eq!(new.incarnation(), old.incarnation());
        assert_eq!(new.label(), old.label());
        assert_eq!(new.mount_object(), Some(33));
        assert_eq!(
            inventory.reconcile_mount(new.id(), new.incarnation()),
            Some((PathBuf::from("/media/usb"), 33))
        );
    }

    #[test]
    fn root_mount_identity_is_reported_for_main_drive() {
        let mut root_volume = volume(10, Some("/dev/nvme0n1p2"), "Root");
        root_volume.mount = Some(RawMount {
            object: 90,
            root: PathBuf::from("/"),
            label: "Root".to_string(),
            native: Some("/dev/nvme0n1p2".to_string()),
            uuid: Some("uuid-root".to_string()),
            can_unmount: false,
            can_eject: false,
        });
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![root_volume.clone()], vec![]));
        assert!(
            inventory.entries().is_empty(),
            "the root stays the separate Main Drive row"
        );
        assert_eq!(inventory.root_mount_object(), Some(90));

        root_volume.mount.as_mut().unwrap().object = 91;
        inventory.reconcile(&snapshot(vec![root_volume], vec![]));
        assert_eq!(
            inventory.root_mount_object(),
            Some(91),
            "a same-root root remount has a new main usage identity"
        );

        inventory.reconcile(&snapshot(vec![], vec![]));
        assert_eq!(inventory.root_mount_object(), None);
    }

    #[test]
    fn empty_or_missing_labels_fall_back_without_becoming_identity() {
        let snapshot = snapshot(vec![volume(1, Some("/dev/disk/by-id/abc"), "")], vec![]);
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot);
        assert_eq!(inventory.entries()[0].label(), "abc");
        assert_eq!(inventory.entries()[0].native(), Some("/dev/disk/by-id/abc"));
    }

    #[test]
    fn removal_targets_obey_the_mount_capabilities_and_pending_state() {
        let mut removable = volume(1, Some("/dev/sdb1"), "USB");
        removable.mount = Some(RawMount {
            object: 20,
            root: PathBuf::from("/run/media/kyle/USB"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: false,
        });
        let unmounted = volume(2, Some("/dev/sdc1"), "Waiting");
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![removable, unmounted], vec![]));
        let usb = inventory
            .entries()
            .iter()
            .find(|entry| entry.label() == "USB")
            .unwrap();

        let target = inventory
            .removal_target(usb.id().as_str(), DriveAction::Unmount, false)
            .expect("the mount can be unmounted");
        assert_eq!(target.mount_object(), 20);
        assert_eq!(target.root(), Path::new("/run/media/kyle/USB"));
        assert_eq!(
            inventory.removal_target(usb.id().as_str(), DriveAction::Eject, false),
            Err(RemovalRefusal::NotSupported(DriveAction::Eject)),
            "eject follows the mount's own answer, not a path list"
        );
        assert_eq!(
            inventory.removal_target(usb.id().as_str(), DriveAction::Unmount, true),
            Err(RemovalRefusal::Pending)
        );
        assert_eq!(
            inventory.removal_target(
                inventory.entries()[1].id().as_str(),
                DriveAction::Unmount,
                false
            ),
            Err(RemovalRefusal::NotMounted)
        );
        assert_eq!(
            inventory.removal_target("volume-99999", DriveAction::Unmount, false),
            Err(RemovalRefusal::UnknownId)
        );
    }

    #[test]
    fn root_and_system_mounts_never_become_removal_targets() {
        let mut root_volume = volume(1, Some("/dev/nvme0n1p2"), "Root");
        root_volume.mount = Some(RawMount {
            object: 90,
            root: PathBuf::from("/"),
            label: "Root".to_string(),
            native: Some("/dev/nvme0n1p2".to_string()),
            uuid: Some("uuid-root".to_string()),
            can_unmount: false,
            can_eject: false,
        });
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![root_volume], vec![]));
        assert!(inventory.entries().is_empty());
        assert_eq!(
            inventory.removal_target("volume-1", DriveAction::Unmount, false),
            Err(RemovalRefusal::UnknownId)
        );

        // A navigation-only system mount is a visible row only when it has a
        // live mount; its capability answers still decide.
        let system_mount = RawMount {
            object: 40,
            root: PathBuf::from("/home"),
            label: "Home".to_string(),
            native: None,
            uuid: None,
            can_unmount: false,
            can_eject: false,
        };
        inventory.reconcile(&snapshot(vec![], vec![system_mount]));
        let home = inventory.entries()[0].clone();
        assert_eq!(
            inventory.removal_target(home.id().as_str(), DriveAction::Unmount, false),
            Err(RemovalRefusal::NotSupported(DriveAction::Unmount))
        );
        assert_eq!(
            inventory.removal_target(home.id().as_str(), DriveAction::Eject, false),
            Err(RemovalRefusal::NotSupported(DriveAction::Eject))
        );
    }

    #[test]
    fn same_root_replacement_gets_a_new_removal_mount_object() {
        let mut first = volume(1, Some("/dev/sdb1"), "USB");
        first.mount = Some(RawMount {
            object: 20,
            root: PathBuf::from("/run/media/kyle/USB"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: true,
        });
        let mut inventory = Inventory::new();
        inventory.reconcile(&snapshot(vec![first], vec![]));
        let id = inventory.entries()[0].id().clone();
        let old = inventory
            .removal_target(id.as_str(), DriveAction::Unmount, false)
            .unwrap();
        assert_eq!(old.mount_object(), 20);

        let mut second = volume(1, Some("/dev/sdb1"), "USB");
        second.mount = Some(RawMount {
            object: 21,
            root: PathBuf::from("/run/media/kyle/USB"),
            label: "USB".to_string(),
            native: Some("/dev/sdb1".to_string()),
            uuid: Some("uuid-1".to_string()),
            can_unmount: true,
            can_eject: true,
        });
        inventory.reconcile(&snapshot(vec![second], vec![]));
        let new = inventory
            .removal_target(id.as_str(), DriveAction::Eject, false)
            .unwrap();
        assert_eq!(new.mount_object(), 21);
        assert_ne!(old.mount_object(), new.mount_object());
        assert_eq!(old.id(), new.id(), "the volume object kept its opaque id");
    }

    // -----------------------------------------------------------------
    // Internal drives: the mountinfo parser and filter
    // -----------------------------------------------------------------

    /// Kyle's machine (Nov 16, 2026): the `/`, `/home`, `/tmp` and
    /// `/boot/efi` lines plus the two fstab data disks, copied from
    /// `cat /proc/self/mountinfo`.
    const KYLES_MOUNTINFO: &str = concat!(
        "47 1 0:37 /@ / rw,noatime shared:1 - btrfs /dev/nvme1n1p2 rw,ssd,discard=async,space_cache=v2,autodefrag,subvolid=256,subvol=/@\n",
        "58 47 0:52 / /tmp rw,noatime shared:125 - tmpfs tmpfs rw,inode64\n",
        "63 47 0:37 /@home /home rw,noatime shared:129 - btrfs /dev/nvme1n1p2 rw,ssd,discard=async,space_cache=v2,autodefrag,subvolid=257,subvol=/@home\n",
        "67 47 0:51 / /mnt/nvme-storage rw,noatime shared:133 - btrfs /dev/nvme0n1p1 rw,ssd,discard=async,space_cache=v2,subvolid=5,subvol=/\n",
        "71 47 0:53 / /mnt/wd-blue rw,noatime shared:137 - btrfs /dev/sda1 rw,ssd,discard=async,space_cache=v2,subvolid=5,subvol=/\n",
        "62 47 259:4 / /boot/efi rw,relatime shared:141 - vfat /dev/nvme1n1p1 rw,fmask=0022,dmask=0022,codepage=437,iocharset=iso8859-1,shortname=mixed,errors=remount-ro\n",
    );

    fn mount_paths(drives: &[InternalDrive]) -> Vec<String> {
        drives
            .iter()
            .map(|drive| drive.mount_path.display().to_string())
            .collect()
    }

    fn no_labels() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn kyles_mountinfo_gives_exactly_the_two_data_drives() {
        let drives = internal_drives(KYLES_MOUNTINFO, &[], &no_labels());
        assert_eq!(
            mount_paths(&drives),
            vec!["/mnt/nvme-storage", "/mnt/wd-blue"],
            "sorted by mount path, no system mounts"
        );
        assert_eq!(drives[0].device, "/dev/nvme0n1p1");
        assert_eq!(drives[0].fs_type, "btrfs");
        assert_eq!(drives[0].label, None);
        assert_eq!(drives[0].name(), "nvme-storage");
        assert_eq!(drives[1].device, "/dev/sda1");
        assert_eq!(drives[1].name(), "wd-blue");
        assert_eq!(drives[1].tooltip(), "/mnt/wd-blue — /dev/sda1");
    }

    #[test]
    fn every_skip_rule_loses_its_own_mount() {
        let table = concat!(
            "1 0 8:2 / / rw,relatime - ext4 /dev/sda2 rw\n",
            "2 1 0:40 / /tmp rw - tmpfs tmpfs rw\n",
            "3 1 0:41 / /mnt/box rw - overlay overlay rw,lowerdir=/a,upperdir=/b\n",
            "4 1 0:42 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw\n",
            "5 1 0:43 / /mnt/nas rw - nfs server:/export rw\n",
            "6 1 0:44 / /mnt/smb rw - cifs //server/share rw\n",
            "7 1 8:9 / /boot/efi rw - vfat /dev/sda1 rw\n",
            "8 1 8:2 /@home /home rw - ext4 /dev/sda2 rw\n",
            "9 1 8:2 /@snap /home/kyle/snap rw - ext4 /dev/sda2 rw\n",
            "10 1 8:3 / /mnt/hidden rw - ext4 /dev/sda3 rw,noatime,x-gvfs-hide\n",
            "11 1 8:30 / /mnt/also-hidden ro - ext4 /dev/sda30 ro,x-gvfs-hide,relatime\n",
            "12 1 8:4 / /srv/data rw - xfs /dev/sda4 rw\n",
            "13 1 8:5 / /mnt/kept rw - xfs /dev/sdb1 rw\n",
            "14 1 0:9 / /mnt/devnode rw - ext4 /dev/xsdfake rw\n",
            "15 1 8:6 / media/relative rw - ext4 /dev/sdc1 rw\n",
        );
        // `/home/kyle/snap` sits outside every prefix rule but shares the
        // root device; the two `x-gvfs-hide` mounts differ in option
        // position and read-only state. `/mnt/devnode` stays: `/dev/xsdfake`
        // is not a real node on this machine, but the rule only reads text.
        let drives = internal_drives(table, &[], &no_labels());
        assert_eq!(
            mount_paths(&drives),
            vec!["/mnt/devnode", "/mnt/kept"],
            "only /dev-sourced disk filesystems outside system paths stay"
        );
    }

    #[test]
    fn a_devices_shown_path_and_a_run_media_share_the_dedup_rules() {
        let table = concat!(
            "1 0 0:37 / / rw - btrfs /dev/nvme1n1p2 rw\n",
            "2 1 8:1 / /mnt/wd-blue rw - btrfs /dev/sda1 rw\n",
            "3 1 8:7 / /run/media/kyle/USB rw - vfat /dev/sdc1 rw\n",
            "4 1 8:7 / /mnt/usb-copy rw - vfat /dev/sdc1 rw\n",
        );
        // Devices shows the stick at its /run/media root and nothing else.
        let devices_roots = [PathBuf::from("/run/media/kyle/USB")];
        let drives = internal_drives(table, &devices_roots, &no_labels());
        assert_eq!(
            mount_paths(&drives),
            vec!["/mnt/usb-copy", "/mnt/wd-blue"],
            "one device mounted twice yields one row; /run/media is dropped\n             with the Devices-duplication rule, not the prefix rule"
        );
    }

    #[test]
    fn a_second_device_mount_of_the_same_device_keeps_the_shortest_path() {
        let table = concat!(
            "1 0 8:2 / / rw - ext4 /dev/sda2 rw\n",
            "2 1 8:5 / /mnt/deep/mirror/of/disk rw - ext4 /dev/sdb1 rw\n",
            "3 1 8:5 / /mnt/disk rw - ext4 /dev/sdb1 rw\n",
        );
        let drives = internal_drives(table, &[], &no_labels());
        assert_eq!(mount_paths(&drives), vec!["/mnt/disk"]);
    }

    #[test]
    fn an_escaped_mount_path_with_a_space_decodes() {
        let table = concat!(
            "1 0 8:2 / / rw - ext4 /dev/sda2 rw\n",
            "2 1 8:8 / /mnt/My\\040Disk rw - ext4 /dev/sda8 rw\n",
            "3 1 8:9 / /mnt/back\\134slash rw - ext4 /dev/sda9 rw\n",
        );
        let drives = internal_drives(table, &[], &no_labels());
        let names: Vec<String> = drives.iter().map(InternalDrive::name).collect();
        assert_eq!(names, vec!["My Disk", "back\\slash"]);
        assert_eq!(drives[0].mount_path, Path::new("/mnt/My Disk"));
    }

    #[test]
    fn labels_come_from_by_label_and_blank_ones_fall_back() {
        let mut by_label = BTreeMap::new();
        by_label.insert("/dev/sda1".to_string(), "WD-Blue-SSD".to_string());
        by_label.insert("/dev/nvme0n1p1".to_string(), " ".to_string());
        let drives = internal_drives(KYLES_MOUNTINFO, &[], &by_label);
        assert_eq!(drives[0].name(), "nvme-storage", "a blank label is none");
        assert_eq!(drives[0].label, None);
        assert_eq!(drives[1].name(), "WD-Blue-SSD");
        assert_eq!(drives[1].label.as_deref(), Some("WD-Blue-SSD"));
        assert_eq!(drives[1].tooltip(), "/mnt/wd-blue — /dev/sda1");
    }

    #[test]
    fn without_a_root_entry_every_qualifying_mount_still_shows() {
        // A synthetic table with no `/` line: the root-device rule has
        // nothing to compare against, so it must not swallow the data disks.
        let table = concat!(
            "67 47 0:51 / /mnt/nvme-storage rw - btrfs /dev/nvme0n1p1 rw\n",
            "71 47 0:53 / /mnt/wd-blue rw - btrfs /dev/sda1 rw\n",
        );
        let drives = internal_drives(table, &[], &no_labels());
        assert_eq!(mount_paths(&drives).len(), 2);
    }
}
