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

use std::fmt;
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
    Failed,
}

impl MountFailureKind {
    pub fn short(self) -> &'static str {
        match self {
            MountFailureKind::Cancelled => "Mount cancelled",
            MountFailureKind::Denied => "Permission denied",
            MountFailureKind::DeviceGone => "Device removed",
            MountFailureKind::BackendUnavailable => "Volume service unavailable",
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

    /// Whether this exact entry incarnation is mounted now, where, and by
    /// which live mount object. A late callback uses this instead of trusting
    /// its own success.
    pub fn reconcile_mount(&self, id: &VolumeId, incarnation: u64) -> Option<(PathBuf, u64)> {
        self.entry(id)
            .filter(|entry| entry.incarnation == incarnation)
            .and_then(|entry| match &entry.state {
                DeviceState::Mounted { root, mount_object } => Some((root.clone(), *mount_object)),
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
}
