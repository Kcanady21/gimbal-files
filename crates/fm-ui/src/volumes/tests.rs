// SPDX-License-Identifier: GPL-3.0-or-later
//! Injected volume backends and production-path tests. No real device is
//! opened, mounted, unmounted or ejected.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fm_core::volumes::{
    DriveAction, MountFailureKind, MountRefusal, MountStatus, MountTarget, RawMount, RawSnapshot,
    RawVolume, RemovalFailureKind, RemovalRefusal, RemovalStatus, RemovalTarget, VolumeId,
};
use fm_core::workspace::WindowId;

use super::{
    Backend, DriveGate, MountBegin, MountDone, RemovalBegin, RemovalNotice, UsageDone, UsageValue,
    Volumes,
};

#[derive(Default)]
struct FakeInner {
    volumes: Vec<RawVolume>,
    mounts: Vec<RawMount>,
    next_object: u64,
    mount_calls: Vec<MountTarget>,
    pending_mounts: Vec<(VolumeId, gio::Cancellable, MountDone)>,
    fail_next_mount: Option<(gio::IOErrorEnum, String)>,
    removal_calls: Vec<RemovalTarget>,
    pending_removals: Vec<(VolumeId, gio::Cancellable, MountDone)>,
    order: Rc<RefCell<Vec<String>>>,
    snapshot_calls: u64,
    usage_calls: Vec<PathBuf>,
    pending_usage: Vec<(PathBuf, gio::Cancellable, UsageDone)>,
    block_usage: bool,
    usage_result: Option<Result<(u64, u64), String>>,
}

pub(crate) struct FakeBackend {
    inner: RefCell<FakeInner>,
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self {
            inner: RefCell::new(FakeInner {
                usage_result: Some(Ok((1_000_000, 250_000))),
                ..FakeInner::default()
            }),
        }
    }
}

impl FakeBackend {
    pub(crate) fn add_volume(&self, native: &str, label: &str) -> u64 {
        let mut inner = self.inner.borrow_mut();
        inner.next_object += 1;
        let object = inner.next_object;
        inner.volumes.push(RawVolume {
            object,
            native: Some(native.to_string()),
            uuid: Some(format!("uuid-{object}")),
            label: label.to_string(),
            can_mount: true,
            has_drive: true,
            mount: None,
        });
        object
    }

    pub(crate) fn add_unmountable_volume(&self, native: Option<&str>, label: &str) -> u64 {
        let object = self.add_volume(native.unwrap_or("/dev/null"), label);
        let mut inner = self.inner.borrow_mut();
        let volume = inner
            .volumes
            .iter_mut()
            .find(|volume| volume.object == object)
            .expect("just added");
        volume.can_mount = false;
        volume.native = native.map(str::to_string);
        object
    }

    pub(crate) fn add_plain_mount(&self, root: &str, label: &str) -> u64 {
        let mut inner = self.inner.borrow_mut();
        inner.next_object += 1;
        let object = inner.next_object;
        inner.mounts.push(RawMount {
            object,
            root: PathBuf::from(root),
            label: label.to_string(),
            native: None,
            uuid: None,
            can_unmount: true,
            can_eject: true,
        });
        object
    }

    pub(crate) fn remove_plain_mount(&self, root: &str) {
        self.inner
            .borrow_mut()
            .mounts
            .retain(|mount| mount.root != Path::new(root));
    }

    pub(crate) fn fail_next_mount(&self, kind: gio::IOErrorEnum, message: &str) {
        self.inner.borrow_mut().fail_next_mount = Some((kind, message.to_string()));
    }

    pub(crate) fn set_capabilities(&self, object: u64, unmount: bool, eject: bool) {
        let mut inner = self.inner.borrow_mut();
        if let Some(volume) = inner
            .volumes
            .iter_mut()
            .find(|volume| volume.object == object)
            && let Some(mount) = volume.mount.as_mut()
        {
            mount.can_unmount = unmount;
            mount.can_eject = eject;
        }
    }

    pub(crate) fn order(&self) -> Rc<RefCell<Vec<String>>> {
        self.inner.borrow().order.clone()
    }

    pub(crate) fn removal_calls(&self) -> Vec<RemovalTarget> {
        self.inner.borrow().removal_calls.clone()
    }

    pub(crate) fn complete_removal(&self, id: &VolumeId, result: Result<(), glib::Error>) {
        let pending = {
            let mut inner = self.inner.borrow_mut();
            inner
                .pending_removals
                .iter()
                .position(|(candidate, _, _)| candidate == id)
                .map(|index| inner.pending_removals.remove(index))
        };
        if let Some((_, _, done)) = pending {
            done(result);
        }
    }

    pub(crate) fn removal_cancellable(&self, id: &VolumeId) -> Option<gio::Cancellable> {
        self.inner
            .borrow()
            .pending_removals
            .iter()
            .find(|(candidate, _, _)| candidate == id)
            .map(|(_, cancellable, _)| cancellable.clone())
    }

    pub(crate) fn snapshot_calls(&self) -> u64 {
        self.inner.borrow().snapshot_calls
    }

    pub(crate) fn set_mounted(&self, object: u64, root: &str) {
        let mut inner = self.inner.borrow_mut();
        inner.next_object += 1;
        let mount_object = inner.next_object;
        let volume = inner
            .volumes
            .iter_mut()
            .find(|volume| volume.object == object)
            .expect("fake volume");
        volume.mount = Some(RawMount {
            object: mount_object,
            root: PathBuf::from(root),
            label: volume.label.clone(),
            native: volume.native.clone(),
            uuid: volume.uuid.clone(),
            can_unmount: true,
            can_eject: true,
        });
    }

    pub(crate) fn set_unmounted(&self, object: u64) {
        if let Some(volume) = self
            .inner
            .borrow_mut()
            .volumes
            .iter_mut()
            .find(|volume| volume.object == object)
        {
            volume.mount = None;
        }
    }

    pub(crate) fn remove_volume(&self, object: u64) {
        self.inner
            .borrow_mut()
            .volumes
            .retain(|volume| volume.object != object);
    }

    pub(crate) fn block_usage(&self, blocked: bool) {
        self.inner.borrow_mut().block_usage = blocked;
    }

    pub(crate) fn mount_calls(&self) -> usize {
        self.inner.borrow().mount_calls.len()
    }

    pub(crate) fn usage_calls(&self) -> usize {
        self.inner.borrow().usage_calls.len()
    }

    fn usage_calls_for(&self, root: &Path) -> usize {
        self.inner
            .borrow()
            .usage_calls
            .iter()
            .filter(|candidate| candidate.as_path() == root)
            .count()
    }

    pub(crate) fn complete_mount(&self, id: &VolumeId, result: Result<(), glib::Error>) {
        let pending = {
            let mut inner = self.inner.borrow_mut();
            inner
                .pending_mounts
                .iter()
                .position(|(candidate, _, _)| candidate == id)
                .map(|index| inner.pending_mounts.remove(index))
        };
        if let Some((_, _, done)) = pending {
            done(result);
        }
    }

    pub(crate) fn mount_cancellable(&self, id: &VolumeId) -> Option<gio::Cancellable> {
        self.inner
            .borrow()
            .pending_mounts
            .iter()
            .find(|(candidate, _, _)| candidate == id)
            .map(|(_, cancellable, _)| cancellable.clone())
    }

    fn start_removal(
        &self,
        target: &RemovalTarget,
        kind: &str,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.removal_calls.push(target.clone());
        inner.order.borrow_mut().push(format!("backend:{kind}"));
        inner
            .pending_removals
            .push((target.id().clone(), cancellable.clone(), done));
        Ok(None)
    }

    fn complete_usage(&self, root: &Path, result: Result<(u64, u64), glib::Error>) {
        let pending = {
            let mut inner = self.inner.borrow_mut();
            inner
                .pending_usage
                .iter()
                .position(|(candidate, _, _)| candidate.as_path() == root)
                .map(|index| inner.pending_usage.remove(index))
        };
        if let Some((_, _, done)) = pending {
            done(result);
        }
    }

    fn pending_usage_roots(&self) -> Vec<PathBuf> {
        self.inner
            .borrow()
            .pending_usage
            .iter()
            .map(|(root, _, _)| root.clone())
            .collect()
    }

    fn complete_usage_nth(
        &self,
        root: &Path,
        occurrence: usize,
        result: Result<(u64, u64), glib::Error>,
    ) {
        let pending = {
            let mut inner = self.inner.borrow_mut();
            let index = inner
                .pending_usage
                .iter()
                .enumerate()
                .filter(|(_, (candidate, _, _))| candidate.as_path() == root)
                .nth(occurrence)
                .map(|(index, _)| index);
            index.map(|index| inner.pending_usage.remove(index))
        };
        if let Some((_, _, done)) = pending {
            done(result);
        }
    }
}

impl Backend for FakeBackend {
    fn snapshot(&self) -> RawSnapshot {
        let mut inner = self.inner.borrow_mut();
        inner.snapshot_calls += 1;
        RawSnapshot {
            volumes: inner.volumes.clone(),
            mounts: inner.mounts.clone(),
        }
    }

    fn mount(
        &self,
        target: &MountTarget,
        _parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        let mut inner = self.inner.borrow_mut();
        inner.mount_calls.push(target.clone());
        if let Some((kind, message)) = inner.fail_next_mount.take() {
            return Err(glib::Error::new(kind, &message));
        }
        inner
            .pending_mounts
            .push((target.id().clone(), cancellable.clone(), done));
        Ok(None)
    }

    fn unmount(
        &self,
        target: &RemovalTarget,
        _parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        self.start_removal(target, "unmount", cancellable, done)
    }

    fn eject(
        &self,
        target: &RemovalTarget,
        _parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        self.start_removal(target, "eject", cancellable, done)
    }

    fn usage(&self, root: &Path, cancellable: &gio::Cancellable, done: UsageDone) {
        let mut inner = self.inner.borrow_mut();
        inner.usage_calls.push(root.to_path_buf());
        if inner.block_usage {
            inner
                .pending_usage
                .push((root.to_path_buf(), cancellable.clone(), done));
            return;
        }
        let result = inner
            .usage_result
            .clone()
            .unwrap_or(Ok((1_000_000, 250_000)))
            .map_err(|message| glib::Error::new(gio::IOErrorEnum::Failed, &message));
        drop(inner);
        done(result);
    }

    fn note(&self) -> String {
        "injected fake volume backend".to_string()
    }
}

fn window(raw: u64) -> WindowId {
    WindowId::from_raw(raw)
}

fn entry_by_native(volumes: &Rc<Volumes>, native: &str) -> fm_core::volumes::DeviceEntry {
    volumes
        .entries()
        .into_iter()
        .find(|entry| entry.native() == Some(native))
        .expect("a device row for the native id")
}

fn error(kind: gio::IOErrorEnum, message: &str) -> glib::Error {
    glib::Error::new(kind, message)
}

#[test]
fn duplicate_requests_across_windows_call_the_backend_once() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let target = volumes.mount_target(entry.id().as_str()).unwrap();

    assert_eq!(
        volumes.mount(window(1), target.clone(), None),
        MountBegin::Pending
    );
    assert_eq!(
        volumes.mount(window(2), target, None),
        MountBegin::AlreadyPending
    );
    assert_eq!(backend.mount_calls(), 1);
    assert_eq!(volumes.pending_requests_for_tests(), 1);
}

#[test]
fn refusal_and_stale_ids_never_reach_the_backend() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    backend.add_unmountable_volume(Some("/dev/sdc1"), "Locked");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();

    assert_eq!(
        volumes.mount_target("volume-99999"),
        Err(MountRefusal::UnknownId)
    );

    let locked = entry_by_native(&volumes, "/dev/sdc1");
    let refusal = volumes.mount_target(locked.id().as_str()).unwrap_err();
    assert!(refusal.to_string().contains("does not offer"));
    assert_eq!(backend.mount_calls(), 0);
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn the_process_wide_cap_is_four_and_releases_only_at_a_terminal_result() {
    let backend = Rc::new(FakeBackend::default());
    let mut objects = Vec::new();
    for index in 0..5 {
        objects.push(backend.add_volume(&format!("/dev/sd{index}b1"), "Drive"));
    }
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let targets: Vec<MountTarget> = volumes
        .entries()
        .iter()
        .map(|entry| volumes.mount_target(entry.id().as_str()).unwrap())
        .collect();

    for target in targets.iter().take(4) {
        assert_eq!(
            volumes.mount(window(1), target.clone(), None),
            MountBegin::Pending
        );
    }
    assert_eq!(
        volumes.mount(window(1), targets[4].clone(), None),
        MountBegin::Busy
    );
    assert_eq!(backend.mount_calls(), 4);
    assert_eq!(volumes.active_mounts(), 4);

    // Monitor arrival completes visible ownership, but the backend call is
    // still outstanding and keeps its slot until its callback arrives.
    backend.set_mounted(objects[0], "/media/one");
    volumes.refresh_for_tests();
    assert_eq!(backend.mount_calls(), 4);
    assert_eq!(
        volumes.active_mounts(),
        4,
        "monitor completion cannot release a backend slot before the callback"
    );
    assert_eq!(
        volumes.mount(window(1), targets[4].clone(), None),
        MountBegin::Busy,
        "a fifth backend call is refused while four callbacks are outstanding"
    );
    assert_eq!(backend.mount_calls(), 4);

    backend.complete_mount(targets[0].id(), Ok(()));
    assert_eq!(volumes.active_mounts(), 3);
    assert_eq!(
        volumes.mount(window(1), targets[4].clone(), None),
        MountBegin::Pending
    );
    assert_eq!(backend.mount_calls(), 5);
}

#[test]
fn monitor_completion_then_unmount_and_same_volume_retry_have_independent_outcomes() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let id = entry.id().clone();
    let first = volumes.mount_target(id.as_str()).unwrap();
    assert_eq!(volumes.mount(window(1), first, None), MountBegin::Pending);

    // Attempt A is completed by the monitor before its callback.
    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    assert!(volumes.status_for(&id).unwrap().is_mounted());
    assert_eq!(volumes.pending_requests_for_tests(), 0);
    assert_eq!(
        volumes.active_mounts(),
        1,
        "attempt A still owns its backend slot"
    );

    // The volume unmounts and the same live volume starts attempt B.
    backend.set_unmounted(object);
    volumes.refresh_for_tests();
    assert!(volumes.status_for(&id).is_none());
    let second = volumes.mount_target(id.as_str()).unwrap();
    assert_eq!(volumes.mount(window(1), second, None), MountBegin::Pending);
    assert_eq!(volumes.active_mounts(), 2);

    // A's old callback is a permission failure. It may retire only its own
    // accounting; it must not turn attempt B into a failure.
    backend.complete_mount(
        &id,
        Err(error(gio::IOErrorEnum::PermissionDenied, "old attempt")),
    );
    assert_eq!(
        volumes.status_for(&id),
        Some(MountStatus::Pending),
        "the old callback cannot complete the retry"
    );
    assert_eq!(volumes.pending_requests_for_tests(), 1);
    assert_eq!(
        volumes.active_mounts(),
        1,
        "A released only its own backend call"
    );

    backend.complete_mount(
        &id,
        Err(error(
            gio::IOErrorEnum::PermissionDenied,
            "attempt B denied",
        )),
    );
    assert_eq!(
        volumes.status_for(&id),
        Some(MountStatus::failed(
            MountFailureKind::Denied,
            "attempt B denied"
        ))
    );
    assert_eq!(volumes.pending_requests_for_tests(), 0);
    assert_eq!(volumes.active_mounts(), 0);
}

#[test]
fn stale_mount_reconciliation_timer_cannot_refresh_a_retry() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let id = entry.id().clone();
    let first = volumes.mount_target(id.as_str()).unwrap();
    volumes.mount(window(1), first, None);

    // A callback without a visible root schedules A's reconciliation timer.
    backend.complete_mount(&id, Ok(()));
    let stale_token = volumes.attempt_token_for_tests(&id).unwrap();
    assert_eq!(volumes.pending_requests_for_tests(), 1);

    // The monitor completes A before the timer, the volume unmounts, and B
    // starts on the same id and incarnation.
    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    backend.set_unmounted(object);
    volumes.refresh_for_tests();
    let second = volumes.mount_target(id.as_str()).unwrap();
    volumes.mount(window(1), second, None);
    assert_ne!(volumes.attempt_token_for_tests(&id), Some(stale_token));

    let snapshots = backend.snapshot_calls();
    volumes.deliver_mount_retry_for_tests(&id, stale_token);
    assert_eq!(
        backend.snapshot_calls(),
        snapshots,
        "A's stale timer must not refresh or reconcile B"
    );
    assert_eq!(
        volumes.status_for(&id),
        Some(MountStatus::Pending),
        "B remains its own independent request"
    );

    backend.complete_mount(
        &id,
        Err(error(gio::IOErrorEnum::Cancelled, "retry cancelled")),
    );
    assert!(matches!(
        volumes.status_for(&id),
        Some(MountStatus::Failed {
            kind: MountFailureKind::Cancelled,
            ..
        })
    ));
}

#[test]
fn removal_and_reinsertion_churn_keeps_four_outstanding_calls() {
    let backend = Rc::new(FakeBackend::default());
    let mut objects = Vec::new();
    for index in 0..4 {
        objects.push(backend.add_volume(&format!("/dev/sd{index}b1"), "Drive"));
    }
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let first_targets: Vec<MountTarget> = volumes
        .entries()
        .iter()
        .map(|entry| volumes.mount_target(entry.id().as_str()).unwrap())
        .collect();
    let first_ids: Vec<VolumeId> = first_targets
        .iter()
        .map(|target| target.id().clone())
        .collect();
    for target in first_targets {
        assert_eq!(volumes.mount(window(1), target, None), MountBegin::Pending);
    }
    assert_eq!(volumes.active_mounts(), 4);

    // Removing every volume retires its visible request, not its outstanding
    // backend call.
    for object in &objects {
        backend.remove_volume(*object);
    }
    volumes.refresh_for_tests();
    assert_eq!(volumes.pending_requests_for_tests(), 0);
    assert_eq!(
        volumes.active_mounts(),
        4,
        "the four callbacks still own all four slots"
    );
    for id in &first_ids {
        assert!(matches!(
            volumes.status_for(id),
            Some(MountStatus::Failed {
                kind: MountFailureKind::DeviceGone,
                ..
            })
        ));
    }

    // Reinsertion produces new identities and a fifth call is still refused.
    for index in 0..4 {
        backend.add_volume(&format!("/dev/sd{index}b1"), "Drive");
    }
    volumes.refresh_for_tests();
    let replacements: Vec<MountTarget> = volumes
        .entries()
        .iter()
        .map(|entry| volumes.mount_target(entry.id().as_str()).unwrap())
        .collect();
    assert!(
        replacements
            .iter()
            .all(|target| !first_ids.contains(target.id()))
    );
    assert_eq!(
        volumes.mount(window(1), replacements[0].clone(), None),
        MountBegin::Busy
    );
    assert_eq!(backend.mount_calls(), 4);

    // Each old callback releases exactly its own call. Old success cannot
    // revive a removed id or mount a replacement.
    backend.complete_mount(&first_ids[0], Ok(()));
    assert_eq!(volumes.active_mounts(), 3);
    assert!(
        volumes
            .status_for(&first_ids[0])
            .is_none_or(|status| !status.is_mounted())
    );
    assert_eq!(
        volumes.mount(window(1), replacements[0].clone(), None),
        MountBegin::Pending
    );
    assert_eq!(backend.mount_calls(), 5);

    for id in first_ids.iter().skip(1) {
        backend.complete_mount(id, Ok(()));
    }
    assert_eq!(volumes.active_mounts(), 1);
    backend.complete_mount(replacements[0].id(), Ok(()));
}

#[test]
fn synchronous_launch_failure_releases_its_slot_once() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let target = volumes.mount_target(entry.id().as_str()).unwrap();

    backend.fail_next_mount(gio::IOErrorEnum::Failed, "the desktop refused to start");
    assert_eq!(
        volumes.mount(window(1), target.clone(), None),
        MountBegin::Pending
    );
    assert!(matches!(
        volumes.status_for(entry.id()),
        Some(MountStatus::Failed {
            kind: MountFailureKind::Failed,
            ..
        })
    ));
    assert_eq!(volumes.active_mounts(), 0);
    assert_eq!(volumes.pending_requests_for_tests(), 0);

    assert_eq!(volumes.mount(window(1), target, None), MountBegin::Pending);
    assert_eq!(volumes.active_mounts(), 1);
}

#[test]
fn cancellation_denial_and_failure_are_distinct_and_retry_is_explicit() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let target = volumes.mount_target(entry.id().as_str()).unwrap();

    volumes.mount(window(1), target.clone(), None);
    backend.complete_mount(
        entry.id(),
        Err(error(gio::IOErrorEnum::Cancelled, "cancelled by user")),
    );
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::failed(
            MountFailureKind::Cancelled,
            "cancelled by user"
        ))
    );

    volumes.mount(window(1), target.clone(), None);
    backend.complete_mount(
        entry.id(),
        Err(error(gio::IOErrorEnum::PermissionDenied, "not authorized")),
    );
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::failed(
            MountFailureKind::Denied,
            "not authorized"
        ))
    );

    volumes.mount(window(1), target.clone(), None);
    backend.complete_mount(
        entry.id(),
        Err(error(gio::IOErrorEnum::Failed, "the filesystem is damaged")),
    );
    let status = volumes.status_for(entry.id()).unwrap();
    assert!(matches!(
        status,
        MountStatus::Failed {
            kind: MountFailureKind::Failed,
            ..
        }
    ));
    assert_eq!(status.detail(), Some("the filesystem is damaged"));

    // A deliberate retry after a terminal failure is a new backend call.
    assert_eq!(volumes.mount(window(1), target, None), MountBegin::Pending);
    assert_eq!(backend.mount_calls(), 4);
}

#[test]
fn removal_revokes_pending_ownership_and_cancels_the_operation() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let target = volumes.mount_target(entry.id().as_str()).unwrap();
    volumes.mount(window(1), target, None);
    let cancellable = backend.mount_cancellable(entry.id()).unwrap();
    assert!(!cancellable.is_cancelled());

    backend.remove_volume(object);
    volumes.refresh_for_tests();
    assert!(cancellable.is_cancelled());
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::failed(
            MountFailureKind::DeviceGone,
            "the volume was removed before the mount completed"
        ))
    );
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn closing_one_window_cancels_only_its_own_request() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "One");
    backend.add_volume("/dev/sdc1", "Two");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let one = entry_by_native(&volumes, "/dev/sdb1");
    let two = entry_by_native(&volumes, "/dev/sdc1");
    volumes.mount(
        window(1),
        volumes.mount_target(one.id().as_str()).unwrap(),
        None,
    );
    volumes.mount(
        window(2),
        volumes.mount_target(two.id().as_str()).unwrap(),
        None,
    );
    let first = backend.mount_cancellable(one.id()).unwrap();
    let second = backend.mount_cancellable(two.id()).unwrap();

    volumes.cancel_window(window(1));
    assert!(first.is_cancelled());
    assert!(!second.is_cancelled());
    assert_eq!(
        volumes.pending_requests_for_tests(),
        1,
        "the closed window's visible ownership is retired"
    );
    assert_eq!(
        volumes.active_mounts(),
        2,
        "both backend calls remain counted until their callbacks"
    );
    assert!(!volumes.cancel_requested(one.id()));
    assert_eq!(
        volumes.status_for(one.id()),
        Some(MountStatus::failed(
            MountFailureKind::Cancelled,
            "the window requesting the mount was closed"
        ))
    );
    assert_eq!(volumes.status_for(two.id()), Some(MountStatus::Pending));
}

#[test]
fn late_success_cannot_revive_an_old_id_or_target_a_replacement() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let old = entry_by_native(&volumes, "/dev/sdb1");
    let old_id = old.id().clone();
    volumes.mount(
        window(1),
        volumes.mount_target(old_id.as_str()).unwrap(),
        None,
    );

    backend.remove_volume(object);
    volumes.refresh_for_tests();
    assert!(matches!(
        volumes.status_for(&old_id).unwrap(),
        MountStatus::Failed {
            kind: MountFailureKind::DeviceGone,
            ..
        }
    ));

    // The same native id, UUID and label is a different object after reinsert.
    backend.add_volume("/dev/sdb1", "USB");
    volumes.refresh_for_tests();
    let replacement = entry_by_native(&volumes, "/dev/sdb1");
    assert_ne!(replacement.id(), &old_id);
    assert_eq!(
        volumes.mount_target(old_id.as_str()),
        Err(MountRefusal::UnknownId)
    );

    // The old callback arrives after removal and reinsert.
    backend.complete_mount(&old_id, Ok(()));
    assert!(
        volumes
            .status_for(&old_id)
            .is_none_or(|status| !status.is_mounted()),
        "a late callback cannot revive the removed id"
    );
    assert!(
        volumes
            .status_for(replacement.id())
            .is_none_or(|status| !status.is_mounted())
    );
}

#[test]
fn callback_before_monitor_stays_pending_then_reconciles() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    volumes.mount(
        window(1),
        volumes.mount_target(entry.id().as_str()).unwrap(),
        None,
    );

    backend.complete_mount(entry.id(), Ok(()));
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::Pending),
        "a queued callback is not a mounted volume"
    );

    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    let mount_object = volumes
        .entry(entry.id().as_str())
        .unwrap()
        .mount_object()
        .unwrap();
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::mounted(
            PathBuf::from("/media/usb"),
            mount_object
        ))
    );
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn monitor_before_callback_completes_once_and_late_callback_is_idempotent() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    volumes.mount(
        window(1),
        volumes.mount_target(entry.id().as_str()).unwrap(),
        None,
    );

    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    assert!(volumes.status_for(entry.id()).unwrap().is_mounted());
    assert_eq!(volumes.pending_requests_for_tests(), 0);

    backend.complete_mount(entry.id(), Ok(()));
    assert!(volumes.status_for(entry.id()).unwrap().is_mounted());
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn cancelled_callback_racing_a_live_mount_reports_the_actual_mounted_state() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    volumes.mount(
        window(1),
        volumes.mount_target(entry.id().as_str()).unwrap(),
        None,
    );

    // The desktop mounted it and a callback reported cancellation. Inventory
    // is the truth, not the callback's final status.
    backend.set_mounted(object, "/media/usb");
    backend.complete_mount(
        entry.id(),
        Err(error(gio::IOErrorEnum::Cancelled, "cancelled late")),
    );
    let mount_object = volumes
        .entry(entry.id().as_str())
        .unwrap()
        .mount_object()
        .unwrap();
    assert_eq!(
        volumes.status_for(entry.id()),
        Some(MountStatus::mounted(
            PathBuf::from("/media/usb"),
            mount_object
        ))
    );
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn an_externally_mounted_volume_needs_no_request() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(entry.state().root(), Some(Path::new("/media/usb")));
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

#[test]
fn mounts_without_a_volume_stay_visible_and_do_not_duplicate_a_volume() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "Same");
    backend.set_mounted(object, "/media/same");
    backend.add_plain_mount("/media/navigation-only", "Navigation");
    backend.add_plain_mount("/", "Root");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();

    let entries = volumes.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.state().root() == Some(Path::new("/media/same")))
            .count(),
        1,
        "the mounted Volume and its Mount are one row"
    );
    assert!(entries.iter().any(|entry| {
        entry.state().root() == Some(Path::new("/media/navigation-only")) && !entry.is_volume()
    }));
    assert!(
        entries
            .iter()
            .all(|entry| entry.state().root() != Some(Path::new("/")))
    );
}

#[test]
fn usage_queries_are_shared_bounded_and_not_restarted_by_event_floods() {
    let backend = Rc::new(FakeBackend::default());
    for index in 0..10 {
        let object = backend.add_volume(&format!("/dev/sd{index}b1"), "Drive");
        backend.set_mounted(object, &format!("/media/drive-{index}"));
    }
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();

    assert_eq!(volumes.usage_counts(), (4, 7));
    assert_eq!(backend.usage_calls(), 4);

    for _ in 0..40 {
        volumes.refresh_for_tests();
    }
    assert_eq!(
        volumes.usage_counts(),
        (4, 7),
        "monitor floods and repeated windows reuse the bounded queries"
    );
    assert_eq!(
        backend.usage_calls(),
        4,
        "monitor floods and repeated windows reuse the bounded queries"
    );
}

#[test]
fn mount_change_refreshes_only_its_usage_and_coalesces_pending_queries() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    let root = Path::new("/media/usb");
    backend.set_mounted(object, root.to_str().unwrap());
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    assert_eq!(backend.usage_calls_for(root), 1);

    backend.block_usage(true);
    volumes.mark_usage_changed(root);
    volumes.mark_usage_changed(root);
    volumes.refresh_for_tests();
    for _ in 0..4 {
        volumes.mark_usage_changed(root);
        volumes.refresh_for_tests();
    }
    assert_eq!(backend.usage_calls_for(root), 2);
    assert_eq!(backend.usage_calls_for(Path::new("/")), 1);
    backend.complete_usage(root, Ok((1000, 200)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(volumes.usage_for_entry(&entry).ready(), Some((1000, 200)));
}

#[test]
fn main_usage_is_one_shared_query() {
    let backend = Rc::new(FakeBackend::default());
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();
    volumes.request_main_usage();
    volumes.request_main_usage();
    assert_eq!(backend.usage_calls_for(Path::new("/")), 1);
    assert_eq!(backend.usage_calls(), 1);
    assert_eq!(volumes.main_usage_for_tests(), UsageValue::Pending);
}

#[test]
fn usage_pending_requests_stop_at_thirty_two_and_rows_say_busy() {
    let backend = Rc::new(FakeBackend::default());
    for index in 0..40 {
        let object = backend.add_volume(&format!("/dev/sd{index}b1"), "Drive");
        backend.set_mounted(object, &format!("/media/drive-{index}"));
    }
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();

    assert_eq!(volumes.usage_counts(), (4, 32));
    assert_eq!(backend.usage_calls(), 4);
    let busy = volumes
        .entries()
        .iter()
        .filter(|entry| volumes.usage_state_for_tests(entry) == UsageValue::Busy)
        .count();
    assert!(busy >= 4, "excess rows stay honestly busy");
}

#[test]
fn stale_usage_results_and_replaced_roots_never_acquire_bars() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/media/old");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();
    assert!(
        backend
            .pending_usage_roots()
            .contains(&PathBuf::from("/media/old"))
    );

    backend.set_unmounted(object);
    backend.set_mounted(object, "/media/new");
    volumes.refresh_for_tests();
    assert!(
        backend
            .pending_usage_roots()
            .contains(&PathBuf::from("/media/new"))
    );

    backend.complete_usage(Path::new("/media/old"), Ok((9_000, 1)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Pending,
        "the old root's result cannot become the new root's bar"
    );

    backend.complete_usage(Path::new("/media/new"), Ok((1_000, 500)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Ready {
            total: 1_000,
            free: 500
        }
    );
}

#[test]
fn same_root_mount_replacement_retires_cached_usage_and_queries_only_the_new_mount() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/media/usb");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();

    // Mount A's capacity arrives and is cached.
    backend.complete_usage_nth(Path::new("/media/usb"), 0, Ok((1_000, 800)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Ready {
            total: 1_000,
            free: 800
        }
    );

    // The same volume is remounted at the exact same root. Mount B must not
    // inherit A's cached bar while its own query is outstanding.
    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Pending,
        "the cached result belongs to the retired mount object"
    );
    assert_eq!(backend.usage_calls_for(Path::new("/media/usb")), 2);

    // A second replacement makes B's delayed callback stale. Repeated monitor
    // refreshes (one shared process service, many windows) coalesce to one new
    // query for C.
    backend.set_mounted(object, "/media/usb");
    volumes.refresh_for_tests();
    for _ in 0..20 {
        volumes.refresh_for_tests();
    }
    assert_eq!(
        backend.usage_calls_for(Path::new("/media/usb")),
        3,
        "refresh floods add no duplicate query for the replacement"
    );

    // B's delayed result is ignored; C's own result fills the bar.
    backend.complete_usage_nth(Path::new("/media/usb"), 0, Ok((2_000, 1_000)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Pending,
        "the old mount's delayed callback cannot fill the new mount"
    );
    backend.complete_usage_nth(Path::new("/media/usb"), 0, Ok((3_000, 2_500)));
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Ready {
            total: 3_000,
            free: 2_500
        }
    );
}

#[test]
fn main_usage_follows_the_root_mount_object_across_refreshes() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_plain_mount("/", "Root");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();
    volumes.request_main_usage();
    volumes.request_main_usage();
    assert_eq!(
        backend.usage_calls_for(Path::new("/")),
        1,
        "every window shares the main drive query"
    );

    backend.complete_usage(Path::new("/"), Ok((10_000, 4_000)));
    assert_eq!(
        volumes.main_usage_for_tests(),
        UsageValue::Ready {
            total: 10_000,
            free: 4_000
        }
    );

    // A same-root Main Drive remount has a new backend mount serial. The old
    // cached capacity is retired and the new mount is queried once.
    backend.remove_plain_mount("/");
    backend.add_plain_mount("/", "Root");
    volumes.refresh_for_tests();
    for _ in 0..20 {
        volumes.refresh_for_tests();
        volumes.request_main_usage();
    }
    assert_eq!(
        volumes.main_usage_for_tests(),
        UsageValue::Pending,
        "the replacement does not inherit the old main bar"
    );
    assert_eq!(
        backend.usage_calls_for(Path::new("/")),
        2,
        "refresh and window requests deduplicate the replacement query"
    );

    backend.complete_usage(Path::new("/"), Ok((20_000, 5_000)));
    assert_eq!(
        volumes.main_usage_for_tests(),
        UsageValue::Ready {
            total: 20_000,
            free: 5_000
        }
    );
}

#[test]
fn failed_usage_is_unavailable_not_zero() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/media/usb");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();

    backend.complete_usage(
        Path::new("/media/usb"),
        Err(error(gio::IOErrorEnum::NotSupported, "no capacity here")),
    );
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    assert_eq!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Unavailable {
            detail: "no capacity here".to_string()
        }
    );
    assert_ne!(
        volumes.usage_state_for_tests(&entry),
        UsageValue::Ready { total: 0, free: 0 }
    );
}

#[test]
fn shutdown_cancels_requests_and_usage_without_waiting() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "Mounted");
    backend.set_mounted(object, "/media/usb");
    backend.add_volume("/dev/sdc1", "Pending");
    let volumes = Volumes::new(backend.clone());
    backend.block_usage(true);
    volumes.refresh_for_tests();
    let pending = entry_by_native(&volumes, "/dev/sdc1");
    volumes.mount(
        window(1),
        volumes.mount_target(pending.id().as_str()).unwrap(),
        None,
    );
    let mount_cancellable = backend.mount_cancellable(pending.id()).unwrap();
    let usage_cancellables: Vec<gio::Cancellable> = backend
        .inner
        .borrow()
        .pending_usage
        .iter()
        .map(|(_, cancellable, _)| cancellable.clone())
        .collect();

    volumes.shutdown();
    assert!(mount_cancellable.is_cancelled());
    assert!(
        usage_cancellables
            .iter()
            .all(gio::Cancellable::is_cancelled)
    );
    assert_eq!(volumes.pending_requests_for_tests(), 0);
}

// -------------------------------------------------------------------------
// Unmount and eject
// -------------------------------------------------------------------------

fn mounted_entry(volumes: &Rc<Volumes>, native: &str) -> fm_core::volumes::DeviceEntry {
    entry_by_native(volumes, native)
}

fn notice_log(volumes: &Rc<Volumes>) -> Rc<RefCell<Vec<RemovalNotice>>> {
    let notices = Rc::new(RefCell::new(Vec::new()));
    let captured = notices.clone();
    volumes.listen_removal(Rc::new(move |notice| {
        captured.borrow_mut().push(notice.clone());
        true
    }));
    notices
}

#[test]
fn unmount_success_returns_the_row_to_unmounted_and_drops_its_bar() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let notices = notice_log(&volumes);
    let entry = mounted_entry(&volumes, "/dev/sdb1");

    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Unmount)
        .expect("a mounted row with can_unmount");
    assert_eq!(
        volumes.remove(window(1), target, None),
        RemovalBegin::Pending
    );
    assert_eq!(
        volumes.removal_status_for(entry.id()),
        Some(RemovalStatus::pending(DriveAction::Unmount))
    );
    assert_eq!(volumes.pending_removals_for_tests(), 1);
    assert_eq!(backend.removal_calls().len(), 1);
    assert_eq!(backend.removal_calls()[0].action(), DriveAction::Unmount);

    backend.set_unmounted(object);
    backend.complete_removal(entry.id(), Ok(()));
    volumes.refresh_for_tests();

    let after = mounted_entry(&volumes, "/dev/sdb1");
    assert!(!after.state().is_mounted());
    assert_eq!(volumes.usage_for_entry(&after), UsageValue::Unknown);
    assert_eq!(volumes.pending_removals_for_tests(), 0);
    assert!(matches!(
        notices.borrow().last().map(|notice| &notice.status),
        Some(RemovalStatus::Unmounted)
    ));
}

#[test]
fn eject_success_removes_the_volume_row_and_reports_it_safely_removable() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let notices = notice_log(&volumes);
    let entry = mounted_entry(&volumes, "/dev/sdb1");

    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Eject)
        .expect("eject follows can_eject");
    assert_eq!(
        volumes.remove(window(7), target, None),
        RemovalBegin::Pending
    );
    assert_eq!(backend.removal_calls()[0].action(), DriveAction::Eject);

    backend.remove_volume(object);
    backend.complete_removal(entry.id(), Ok(()));
    volumes.refresh_for_tests();

    assert!(volumes.entry(entry.id().as_str()).is_none());
    assert_eq!(volumes.pending_removals_for_tests(), 0);
    let notices = notices.borrow();
    let notice = notices.last().expect("one terminal notice");
    assert_eq!(notice.status, RemovalStatus::Ejected);
    assert_eq!(notice.window, window(7));
    assert_eq!(notice.label, "USB");
}

#[test]
fn capabilities_and_pending_state_refuse_before_the_backend() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = mounted_entry(&volumes, "/dev/sdb1");

    backend.set_capabilities(object, false, true);
    volumes.refresh_for_tests();
    assert_eq!(
        volumes.removal_target(entry.id().as_str(), DriveAction::Unmount),
        Err(RemovalRefusal::NotSupported(DriveAction::Unmount))
    );
    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Eject)
        .unwrap();
    assert_eq!(
        volumes.remove(window(1), target.clone(), None),
        RemovalBegin::Pending
    );
    assert_eq!(
        volumes.removal_target(entry.id().as_str(), DriveAction::Eject),
        Err(RemovalRefusal::Pending)
    );
    assert_eq!(
        volumes.remove(window(2), target, None),
        RemovalBegin::AlreadyPending
    );
    assert_eq!(backend.removal_calls().len(), 1);
}

#[test]
fn busy_and_auth_cancelled_are_distinct_and_leave_the_drive_mounted() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let notices = notice_log(&volumes);
    let entry = mounted_entry(&volumes, "/dev/sdb1");

    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Unmount)
        .unwrap();
    volumes.remove(window(1), target, None);
    backend.complete_removal(
        entry.id(),
        Err(error(gio::IOErrorEnum::Busy, "another app holds files")),
    );
    assert_eq!(
        volumes.removal_status_for(entry.id()),
        Some(RemovalStatus::failed(
            DriveAction::Unmount,
            RemovalFailureKind::Busy,
            "another app holds files"
        ))
    );
    assert!(mounted_entry(&volumes, "/dev/sdb1").state().is_mounted());

    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Eject)
        .unwrap();
    volumes.remove(window(1), target, None);
    backend.complete_removal(
        entry.id(),
        Err(error(
            gio::IOErrorEnum::Cancelled,
            "authentication cancelled",
        )),
    );
    assert_eq!(
        volumes.removal_status_for(entry.id()),
        Some(RemovalStatus::failed(
            DriveAction::Eject,
            RemovalFailureKind::Cancelled,
            "authentication cancelled"
        ))
    );
    assert!(mounted_entry(&volumes, "/dev/sdb1").state().is_mounted());
    let kinds: Vec<_> = notices
        .borrow()
        .iter()
        .filter_map(|notice| notice.status.kind())
        .collect();
    assert_eq!(
        kinds,
        vec![RemovalFailureKind::Busy, RemovalFailureKind::Cancelled]
    );
}

#[test]
fn cancel_cancels_only_that_requests_own_operation() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Unmount)
        .unwrap();
    volumes.remove(window(1), target, None);
    let cancellable = backend.removal_cancellable(entry.id()).unwrap();
    assert!(!cancellable.is_cancelled());

    volumes.cancel(entry.id());
    assert!(cancellable.is_cancelled());
    assert!(volumes.cancel_requested(entry.id()));
    assert_eq!(volumes.pending_removals_for_tests(), 1);

    backend.complete_removal(
        entry.id(),
        Err(error(gio::IOErrorEnum::Cancelled, "cancelled by user")),
    );
    assert_eq!(
        volumes.removal_status_for(entry.id()),
        Some(RemovalStatus::failed(
            DriveAction::Unmount,
            RemovalFailureKind::Cancelled,
            "cancelled by user"
        ))
    );
    assert!(mounted_entry(&volumes, "/dev/sdb1").state().is_mounted());
}

#[test]
fn a_stale_removal_callback_cannot_finish_a_replacement_attempt() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let old = mounted_entry(&volumes, "/dev/sdb1");
    let old_id = old.id().clone();
    let first = volumes
        .removal_target(old_id.as_str(), DriveAction::Unmount)
        .unwrap();
    volumes.remove(window(1), first, None);

    // The stick disappears before its callback. Inventory is the truth.
    backend.remove_volume(object);
    volumes.refresh_for_tests();
    assert_eq!(volumes.pending_removals_for_tests(), 0);
    assert_eq!(
        volumes.active_mounts(),
        1,
        "the vanished attempt still owns its backend slot"
    );

    // Reinsertion makes a new id; a new unmount starts on it.
    let replacement_object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(replacement_object, "/run/media/kyle/USB");
    volumes.refresh_for_tests();
    let replacement = mounted_entry(&volumes, "/dev/sdb1");
    assert_ne!(replacement.id(), &old_id);
    let second = volumes
        .removal_target(replacement.id().as_str(), DriveAction::Unmount)
        .unwrap();
    volumes.remove(window(1), second, None);
    assert_eq!(volumes.active_mounts(), 2);

    // The old callback arrives late. It releases only its own slot.
    backend.complete_removal(
        &old_id,
        Err(error(gio::IOErrorEnum::PermissionDenied, "old attempt")),
    );
    assert_eq!(
        volumes.removal_status_for(replacement.id()),
        Some(RemovalStatus::pending(DriveAction::Unmount)),
        "the old callback cannot finish the replacement"
    );
    assert_eq!(volumes.pending_removals_for_tests(), 1);
    assert_eq!(volumes.active_mounts(), 1);
    backend.complete_removal(
        replacement.id(),
        Err(error(gio::IOErrorEnum::Busy, "new attempt busy")),
    );
}

#[test]
fn release_runs_before_the_backend_call() {
    struct RecordingGate {
        order: Rc<RefCell<Vec<String>>>,
    }
    impl DriveGate for RecordingGate {
        fn before_removal(
            &self,
            _window: WindowId,
            _root: &Path,
            _label: &str,
            _action: DriveAction,
        ) -> Result<(), String> {
            self.order.borrow_mut().push("release".to_string());
            Ok(())
        }
    }

    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let order = backend.order();
    volumes.set_gate(Rc::new(RecordingGate {
        order: order.clone(),
    }));
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Unmount)
        .unwrap();
    volumes.remove(window(1), target, None);
    assert_eq!(
        order.borrow().as_slice(),
        ["release", "backend:unmount"],
        "all Gimbal handles are released before the backend starts"
    );
}

#[test]
fn a_refused_gate_never_reaches_the_backend() {
    struct RefusingGate;
    impl DriveGate for RefusingGate {
        fn before_removal(
            &self,
            _window: WindowId,
            _root: &Path,
            label: &str,
            _action: DriveAction,
        ) -> Result<(), String> {
            Err(format!("{label} is in use by a file operation."))
        }
    }

    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    volumes.set_gate(Rc::new(RefusingGate));
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let target = volumes
        .removal_target(entry.id().as_str(), DriveAction::Unmount)
        .unwrap();
    assert_eq!(
        volumes.remove(window(1), target, None),
        RemovalBegin::Refused("USB is in use by a file operation.".to_string())
    );
    assert!(backend.removal_calls().is_empty());
    assert_eq!(volumes.pending_removals_for_tests(), 0);
}

// -------------------------------------------------------------------------
// Registry -> the real UiHandler -> Process -> controller -> injected backend
// -------------------------------------------------------------------------

struct NoClipboard;

impl crate::fileops::clipboard::Reader for NoClipboard {
    fn read(
        &self,
        _display: Option<&gtk::gdk::Display>,
        done: Box<dyn FnOnce(Result<crate::fileops::clipboard::ClipboardPayload, String>)>,
    ) {
        done(Err("the mount routing test has no clipboard".to_string()));
    }
}

fn scratch_process(tag: &str, volumes: Rc<Volumes>) -> (Rc<crate::app::Process>, PathBuf) {
    let root = PathBuf::from(format!(
        "/tmp/gimbal-p2p-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(root.join("home")).expect("scratch home");
    let process = crate::app::Process::headless(
        root.join("config.toml"),
        root.join("home"),
        Rc::new(NoClipboard),
    );
    process.install_volumes_for_tests(volumes);
    (process, root)
}

fn dispatch_mount(
    process: &Rc<crate::app::Process>,
    window: WindowId,
    device: &str,
) -> fm_core::command::Outcome {
    use fm_core::command::{Caller, Invocation, Registry};

    let mut handler = crate::commands::UiHandler {
        caller: Caller::Ui,
        window,
        process: Rc::downgrade(process),
    };
    Registry::new().dispatch(
        Caller::Ui,
        &Invocation::new(
            "drive.mount",
            vec![("device".to_string(), device.to_string())],
        ),
        &mut handler,
    )
}

fn dispatch_removal(
    process: &Rc<crate::app::Process>,
    window: WindowId,
    command: &str,
    device: &str,
) -> fm_core::command::Outcome {
    use fm_core::command::{Caller, Invocation, Registry};

    let mut handler = crate::commands::UiHandler {
        caller: Caller::Ui,
        window,
        process: Rc::downgrade(process),
    };
    Registry::new().dispatch(
        Caller::Ui,
        &Invocation::new(command, vec![("device".to_string(), device.to_string())]),
        &mut handler,
    )
}

#[test]
fn registry_unmount_and_eject_route_through_the_shared_controller() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let (process, root) = scratch_process("handler-unmount", volumes);
    let window = process.seed_window(root.join("home"));

    let outcome = dispatch_removal(&process, window, "drive.unmount", entry.id().as_str());
    assert_eq!(outcome, fm_core::command::Outcome::Done);
    assert_eq!(
        process.volumes().removal_status_for(entry.id()),
        Some(RemovalStatus::pending(DriveAction::Unmount))
    );
    assert_eq!(backend.removal_calls()[0].action(), DriveAction::Unmount);

    let duplicate = dispatch_removal(&process, window, "drive.eject", entry.id().as_str());
    assert!(matches!(duplicate, fm_core::command::Outcome::Failed(_)));
    assert_eq!(backend.removal_calls().len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_running_file_operation_refuses_unmount_before_the_backend() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let (process, root) = scratch_process("handler-fileop", volumes);
    let window = process.seed_window(root.join("home"));
    process
        .fileops()
        .expect("the headless process has a file-op worker")
        .submit(
            window,
            fm_core::fileops::Request::Copy {
                paths: vec![PathBuf::from("/run/media/kyle/USB/file.txt")],
                to: PathBuf::from("/run/media/kyle/USB/destination"),
            },
            false,
            None,
        );

    let outcome = dispatch_removal(&process, window, "drive.unmount", entry.id().as_str());
    match outcome {
        fm_core::command::Outcome::Failed(message) => assert_eq!(
            message,
            "USB is in use by a file operation. Wait for it to finish or cancel it."
        ),
        other => panic!("expected a file-operation refusal, got {other:?}"),
    }
    assert!(backend.removal_calls().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_dirty_editor_refuses_unmount_before_the_backend() {
    let backend = Rc::new(FakeBackend::default());
    let object = backend.add_volume("/dev/sdb1", "USB");
    backend.set_mounted(object, "/run/media/kyle/USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = mounted_entry(&volumes, "/dev/sdb1");
    let (process, root) = scratch_process("handler-dirty", volumes);
    let window = process.seed_window(root.join("home"));
    process.set_dirty_preview_for_tests(
        window,
        PathBuf::from("/run/media/kyle/USB/notes.txt"),
        "notes.txt",
    );

    let outcome = dispatch_removal(&process, window, "drive.unmount", entry.id().as_str());
    match outcome {
        fm_core::command::Outcome::Failed(message) => {
            assert_eq!(message, "Save or discard the changes to “notes.txt” first")
        }
        other => panic!("expected a dirty-editor refusal, got {other:?}"),
    }
    assert!(backend.removal_calls().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn registry_mount_refuses_unknown_ids_before_the_backend_and_never_touches_config() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let (process, root) = scratch_process("handler-refuse", volumes);
    let window = process.seed_window(root.join("home"));
    let config_before = process.config.borrow().config().clone();
    let windows_before = process.windows_snapshot();

    let outcome = dispatch_mount(&process, window, "volume-99999");
    assert!(matches!(outcome, fm_core::command::Outcome::Failed(_)));
    assert_eq!(backend.mount_calls(), 0);
    assert_eq!(process.config.borrow().config(), &config_before);
    assert_eq!(process.windows_snapshot(), windows_before);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn registry_mount_accepts_pending_and_duplicate_across_windows_calls_once() {
    let backend = Rc::new(FakeBackend::default());
    backend.add_volume("/dev/sdb1", "USB");
    let volumes = Volumes::new(backend.clone());
    volumes.refresh_for_tests();
    let entry = entry_by_native(&volumes, "/dev/sdb1");
    let (process, root) = scratch_process("handler-route", volumes);
    let first_window = process.seed_window(root.join("home"));
    let second_window = process.seed_window(root.join("home"));

    let outcome = dispatch_mount(&process, first_window, entry.id().as_str());
    assert_eq!(outcome, fm_core::command::Outcome::Done);
    assert_eq!(
        process.volumes().status_for(entry.id()),
        Some(MountStatus::Pending),
        "handler acceptance means pending, not mounted"
    );
    assert_eq!(backend.mount_calls(), 1);

    let duplicate = dispatch_mount(&process, second_window, entry.id().as_str());
    assert!(matches!(duplicate, fm_core::command::Outcome::Failed(_)));
    assert_eq!(backend.mount_calls(), 1);

    let _ = std::fs::remove_dir_all(root);
}
