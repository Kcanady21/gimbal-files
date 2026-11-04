// SPDX-License-Identifier: GPL-3.0-or-later
//! The process-shared volume service: one GIO-backed inventory for every
//! window, one bounded set of mount requests, and one deduplicated set of
//! asynchronous usage queries.
//!
//! This module owns GTK/GIO work. It never borrows the workspace or config
//! while updating a widget: listeners are called after every internal borrow
//! has been released.

mod gio_backend;
#[cfg(test)]
pub(crate) mod tests;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fm_core::volumes::{
    DeviceEntry, DriveAction, Inventory, MountFailureKind, MountRefusal, MountStatus, MountTarget,
    RawSnapshot, ReconcileReport, RemovalFailureKind, RemovalRefusal, RemovalStatus, RemovalTarget,
    VolumeId,
};
use fm_core::workspace::WindowId;

use crate::log;

pub use gio_backend::GioBackend;

/// At most four mount operations may be active at once. A fifth request is
/// refused as busy; authentication prompts are never queued.
pub const MAX_ACTIVE_MOUNTS: usize = 4;
/// At most four filesystem usage queries run at once.
pub const MAX_ACTIVE_USAGE: usize = 4;
/// At most 32 usage queries may wait for one of the four slots.
pub const MAX_PENDING_USAGE: usize = 32;

const MOUNT_RECONCILE_ATTEMPTS: u32 = 20;
const MOUNT_RECONCILE_DELAY: Duration = Duration::from_millis(100);

pub type MountDone = Box<dyn FnOnce(Result<(), glib::Error>)>;
pub type UsageDone = Box<dyn FnOnce(Result<(u64, u64), glib::Error>)>;

/// A UI-owned preflight that runs synchronously before the backend sees an
/// unmount or eject. It may refuse with the exact user-facing message, or
/// release Gimbal's own handles under the root and return `Ok`.
pub trait DriveGate {
    fn before_removal(
        &self,
        window: WindowId,
        root: &Path,
        label: &str,
        action: DriveAction,
    ) -> Result<(), String>;
}

/// The injectable backend. Production uses [`GioBackend`]; tests replace it
/// with an in-memory volume/mount/usage source and never touch a real device.
pub trait Backend {
    fn snapshot(&self) -> RawSnapshot;
    /// Start a mount. The operation is returned so the controller can drop it
    /// when the request ends. A stale or non-mountable object returns an error
    /// without starting anything.
    fn mount(
        &self,
        target: &MountTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error>;
    /// Start an unmount with `Mount::unmount_with_operation`. The operation is
    /// returned so the controller can drop it with the request.
    fn unmount(
        &self,
        target: &RemovalTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error>;
    /// Start an eject with `Mount::eject_with_operation`. The backend unmounts
    /// every mount on the drive and powers it down where it can.
    fn eject(
        &self,
        target: &RemovalTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error>;
    fn usage(&self, root: &Path, cancellable: &gio::Cancellable, done: UsageDone);
    /// Which volume service is present. No desktop is assumed to have the
    /// same one.
    fn note(&self) -> String;
    fn monitor(&self) -> Option<gio::VolumeMonitor> {
        None
    }
}

/// A backend for headless tests and for a process that never starts the volume
/// service. It has no volumes, never mounts and reports usage as unavailable.
#[cfg(test)]
pub(crate) struct HeadlessBackend;

#[cfg(test)]
impl Backend for HeadlessBackend {
    fn snapshot(&self) -> RawSnapshot {
        RawSnapshot::default()
    }

    fn mount(
        &self,
        _target: &MountTarget,
        _parent: Option<&gtk::Window>,
        _cancellable: &gio::Cancellable,
        _done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        Err(glib::Error::new(
            gio::IOErrorEnum::NotSupported,
            "no volume backend is available",
        ))
    }

    fn unmount(
        &self,
        _target: &RemovalTarget,
        _parent: Option<&gtk::Window>,
        _cancellable: &gio::Cancellable,
        _done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        Err(glib::Error::new(
            gio::IOErrorEnum::NotSupported,
            "no volume backend is available",
        ))
    }

    fn eject(
        &self,
        _target: &RemovalTarget,
        _parent: Option<&gtk::Window>,
        _cancellable: &gio::Cancellable,
        _done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        Err(glib::Error::new(
            gio::IOErrorEnum::NotSupported,
            "no volume backend is available",
        ))
    }

    fn usage(&self, _root: &Path, _cancellable: &gio::Cancellable, done: UsageDone) {
        done(Err(glib::Error::new(
            gio::IOErrorEnum::NotSupported,
            "no volume backend is available",
        )));
    }

    fn note(&self) -> String {
        "no volume backend (headless)".to_string()
    }
}

/// The accepted result of a mount dispatch. `Pending` means the handler
/// accepted a request; it is not a mounted volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountBegin {
    Pending,
    AlreadyPending,
    AlreadyMounted,
    Busy,
}

/// The accepted result of an unmount/eject dispatch. `Pending` is not a
/// completed removal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemovalBegin {
    Pending,
    AlreadyPending,
    Busy,
    /// The UI gate refused before any backend call. The string is the exact
    /// user-facing message.
    Refused(String),
}

/// The state of an asynchronous usage query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UsageValue {
    Unknown,
    /// Queued or running; no numbers are known yet.
    Pending,
    Ready {
        total: u64,
        free: u64,
    },
    Busy,
    Unavailable {
        detail: String,
    },
}

impl UsageValue {
    pub fn ready(&self) -> Option<(u64, u64)> {
        match self {
            UsageValue::Ready { total, free } => Some((*total, *free)),
            _ => None,
        }
    }

    pub fn short(&self) -> &'static str {
        match self {
            UsageValue::Unknown | UsageValue::Pending => "Usage…",
            UsageValue::Busy => "Usage busy",
            UsageValue::Unavailable { .. } => "Usage unavailable",
            UsageValue::Ready { .. } => "",
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match self {
            UsageValue::Unavailable { detail } => Some(detail),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct UsageKey {
    root: PathBuf,
    /// `Some(id, incarnation)` ties a volume's bar to the exact enumerated
    /// object; `None` is the main/root row.
    owner: Option<(VolumeId, u64)>,
    /// The exact live mount object. A same-root remount has a new serial and
    /// therefore a new key, never the previous mount's cached capacity.
    mount_object: Option<u64>,
}

impl UsageKey {
    fn main(mount_object: Option<u64>) -> Self {
        Self {
            root: PathBuf::from("/"),
            owner: None,
            mount_object,
        }
    }

    fn entry(entry: &DeviceEntry) -> Option<Self> {
        let root = entry.state().root()?.to_path_buf();
        Some(Self {
            root,
            owner: Some((entry.id().clone(), entry.incarnation())),
            mount_object: entry.mount_object(),
        })
    }
}

#[derive(Clone)]
struct UsageRecord {
    generation: u64,
    value: UsageValue,
    cancellable: Option<gio::Cancellable>,
}

#[derive(Clone)]
struct UsageJob {
    key: UsageKey,
    query: u64,
}

#[derive(Default)]
struct UsageState {
    records: HashMap<UsageKey, UsageRecord>,
    queue: VecDeque<UsageJob>,
    in_flight: HashMap<u64, UsageKey>,
    active: usize,
    next_query: u64,
    next_generation: u64,
}

struct MountRequest {
    /// Independent identity of this accepted attempt. It is never reused, so
    /// a callback or timer from an earlier attempt cannot act on a retry.
    token: u64,
    id: VolumeId,
    incarnation: u64,
    window: WindowId,
    cancellable: gio::Cancellable,
    operation: RefCell<Option<gio::MountOperation>>,
    /// Whether this attempt still owns the visible row/request state. Monitor
    /// completion, removal and window close retire it without releasing its
    /// backend call.
    visible: Cell<bool>,
    cancel_requested: Cell<bool>,
    callback_ok: Cell<bool>,
    attempts: Cell<u32>,
    retry_scheduled: Cell<bool>,
    /// The backend slot is released exactly once, by its callback or by a
    /// synchronous launch failure.
    slot_released: Cell<bool>,
}

impl MountRequest {
    fn id(&self) -> &VolumeId {
        &self.id
    }
}

/// One accepted unmount/eject attempt. It mirrors the mount attempt rules:
/// a process-monotonic token, visible ownership separate from backend slot
/// accounting, and a stale callback that can retire only its own call.
struct RemovalRequest {
    token: u64,
    id: VolumeId,
    incarnation: u64,
    mount_object: u64,
    root: PathBuf,
    label: String,
    action: DriveAction,
    window: WindowId,
    cancellable: gio::Cancellable,
    operation: RefCell<Option<gio::MountOperation>>,
    visible: Cell<bool>,
    cancel_requested: Cell<bool>,
    callback_ok: Cell<bool>,
    attempts: Cell<u32>,
    retry_scheduled: Cell<bool>,
    slot_released: Cell<bool>,
}

/// One terminal removal result, delivered to the process after every internal
/// borrow is released. It carries the initiating window for the eject summary
/// and the root for relocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalNotice {
    pub window: WindowId,
    pub id: VolumeId,
    pub label: String,
    pub root: PathBuf,
    pub action: DriveAction,
    pub status: RemovalStatus,
}

type Listener = Rc<dyn Fn() -> bool>;
type RemovalListener = Rc<dyn Fn(&RemovalNotice) -> bool>;

/// The one process-wide volume service.
pub struct Volumes {
    backend: Rc<dyn Backend>,
    inventory: RefCell<Inventory>,
    main_label: RefCell<String>,
    /// The current visible attempt for each volume. An attempt that has lost
    /// UI ownership is absent here but may still own a backend slot until its
    /// callback arrives.
    requests: RefCell<HashMap<VolumeId, Rc<MountRequest>>>,
    statuses: RefCell<HashMap<VolumeId, MountStatus>>,
    removals: RefCell<HashMap<VolumeId, Rc<RemovalRequest>>>,
    removal_statuses: RefCell<HashMap<VolumeId, RemovalStatus>>,
    /// Backend drive calls (mount, unmount or eject) that have not yet
    /// reported their callback. This is the actual four-request cap, separate
    /// from visible pending rows.
    outstanding_mounts: Cell<usize>,
    next_attempt: Cell<u64>,
    usage: RefCell<UsageState>,
    listeners: RefCell<Vec<Listener>>,
    removal_listeners: RefCell<Vec<RemovalListener>>,
    removal_gate: RefCell<Option<Rc<dyn DriveGate>>>,
    note: RefCell<String>,
    monitor: RefCell<Option<gio::VolumeMonitor>>,
    refresh_source: RefCell<Option<glib::SourceId>>,
    usage_refresh: RefCell<HashSet<UsageKey>>,
}

impl Volumes {
    pub fn new(backend: Rc<dyn Backend>) -> Rc<Self> {
        let note = backend.note();
        Rc::new(Self {
            backend,
            inventory: RefCell::new(Inventory::new()),
            main_label: RefCell::new("Main Drive".to_string()),
            requests: RefCell::new(HashMap::new()),
            statuses: RefCell::new(HashMap::new()),
            removals: RefCell::new(HashMap::new()),
            removal_statuses: RefCell::new(HashMap::new()),
            outstanding_mounts: Cell::new(0),
            next_attempt: Cell::new(0),
            usage: RefCell::new(UsageState::default()),
            listeners: RefCell::new(Vec::new()),
            removal_listeners: RefCell::new(Vec::new()),
            removal_gate: RefCell::new(None),
            note: RefCell::new(note),
            monitor: RefCell::new(None),
            refresh_source: RefCell::new(None),
            usage_refresh: RefCell::new(HashSet::new()),
        })
    }

    /// The production GIO service. The desktop's installed volume monitor is
    /// observed; no udisks2 or zbus crate is involved.
    pub fn production() -> Rc<Self> {
        let backend = Rc::new(GioBackend::new());
        let volumes = Self::new(backend);
        volumes.start_monitor();
        volumes.refresh();
        log!("volumes", "{}", volumes.backend_note());
        volumes
    }

    pub fn backend_note(&self) -> String {
        self.note.borrow().clone()
    }

    pub fn main_label(&self) -> String {
        self.main_label.borrow().clone()
    }

    /// A cloned row list. Widgets never hold an inventory borrow while they
    /// build or update GTK objects.
    pub fn entries(&self) -> Vec<DeviceEntry> {
        self.inventory.borrow().entries().to_vec()
    }

    pub fn entry(&self, device: &str) -> Option<DeviceEntry> {
        self.inventory.borrow().entry_for_device(device).cloned()
    }

    pub fn mount_target(&self, device: &str) -> Result<MountTarget, MountRefusal> {
        self.inventory.borrow().mount_target(device)
    }

    /// Resolve a mounted row's opaque id to the exact live mount. A pending
    /// mount or removal for the same id is refused as `Pending`.
    pub fn removal_target(
        &self,
        device: &str,
        action: DriveAction,
    ) -> Result<RemovalTarget, RemovalRefusal> {
        let busy = self
            .inventory
            .borrow()
            .entry_for_device(device)
            .is_some_and(|entry| self.is_pending(entry.id()));
        self.inventory.borrow().removal_target(device, action, busy)
    }

    pub fn status_for(&self, id: &VolumeId) -> Option<MountStatus> {
        if self.requests.borrow().contains_key(id) {
            return Some(MountStatus::pending());
        }
        self.statuses.borrow().get(id).cloned()
    }

    /// The shared pending/terminal state of an unmount or eject.
    pub fn removal_status_for(&self, id: &VolumeId) -> Option<RemovalStatus> {
        if let Some(request) = self.removals.borrow().get(id) {
            return Some(RemovalStatus::pending(request.action));
        }
        self.removal_statuses.borrow().get(id).cloned()
    }

    pub fn is_pending(&self, id: &VolumeId) -> bool {
        self.requests.borrow().contains_key(id) || self.removals.borrow().contains_key(id)
    }

    pub fn cancel_requested(&self, id: &VolumeId) -> bool {
        self.requests
            .borrow()
            .get(id)
            .is_some_and(|request| request.cancel_requested.get())
            || self
                .removals
                .borrow()
                .get(id)
                .is_some_and(|request| request.cancel_requested.get())
    }

    /// Install the synchronous UI preflight/release step. One process owns one
    /// gate; replacing it is only for tests.
    pub fn set_gate(&self, gate: Rc<dyn DriveGate>) {
        *self.removal_gate.borrow_mut() = Some(gate);
    }

    /// Receive terminal unmount/eject results after all internal borrows have
    /// been released. The listener returns false to be dropped.
    pub fn listen_removal(&self, listener: Rc<dyn Fn(&RemovalNotice) -> bool>) {
        self.removal_listeners.borrow_mut().push(listener);
    }

    #[cfg(test)]
    pub fn active_mounts(&self) -> usize {
        self.outstanding_mounts.get()
    }

    #[cfg(test)]
    pub fn usage_counts(&self) -> (usize, usize) {
        let usage = self.usage.borrow();
        (usage.active, usage.queue.len())
    }

    pub fn usage_for_entry(&self, entry: &DeviceEntry) -> UsageValue {
        let Some(key) = UsageKey::entry(entry) else {
            return UsageValue::Unknown;
        };
        self.usage_value(&key)
    }

    pub fn main_usage(&self) -> UsageValue {
        let key = self.main_usage_key();
        self.usage_value(&key)
    }

    fn main_usage_key(&self) -> UsageKey {
        UsageKey::main(self.inventory.borrow().root_mount_object())
    }

    fn usage_value(&self, key: &UsageKey) -> UsageValue {
        self.usage
            .borrow()
            .records
            .get(key)
            .map(|record| record.value.clone())
            .unwrap_or(UsageValue::Unknown)
    }

    /// Register a sidebar. The listener returns false once its widget is gone,
    /// so closed windows disappear without the service holding them alive.
    pub fn listen(&self, listener: Listener) {
        self.listeners.borrow_mut().push(listener);
    }

    /// Ask for the main row's usage once per process/window. Existing active
    /// or completed queries are shared, so a second window adds no query.
    pub fn request_main_usage(self: &Rc<Self>) {
        let key = self.main_usage_key();
        self.ensure_usage(key, false);
    }

    /// Refresh from the backend now. Monitor events coalesce into this call.
    pub fn refresh(self: &Rc<Self>) {
        let report = self.refresh_inventory();
        self.after_inventory(&report);
    }

    fn start_monitor(self: &Rc<Self>) {
        let Some(monitor) = self.backend.monitor() else {
            return;
        };
        for connect in [
            MonitorSignal::MountAdded,
            MonitorSignal::MountRemoved,
            MonitorSignal::VolumeAdded,
            MonitorSignal::VolumeRemoved,
            MonitorSignal::VolumeChanged,
        ] {
            let weak = Rc::downgrade(self);
            connect.connect(&monitor, move || {
                if let Some(volumes) = weak.upgrade() {
                    volumes.schedule_refresh();
                }
            });
        }
        let weak = Rc::downgrade(self);
        monitor.connect_mount_changed(move |_, mount| {
            if let Some(volumes) = weak.upgrade() {
                if let Some(root) = mount.root().path() {
                    volumes.mark_usage_changed(&root);
                }
                volumes.schedule_refresh();
            }
        });
        *self.monitor.borrow_mut() = Some(monitor);
    }

    fn mark_usage_changed(&self, root: &Path) {
        let usage = self.usage.borrow();
        self.usage_refresh.borrow_mut().extend(
            usage
                .records
                .iter()
                .filter(|(key, record)| key.root == root && record.value != UsageValue::Pending)
                .map(|(key, _)| key.clone()),
        );
    }

    fn schedule_refresh(self: &Rc<Self>) {
        if self.refresh_source.borrow().is_some() {
            return;
        }
        let weak = Rc::downgrade(self);
        let source = glib::idle_add_local_once(move || {
            let Some(volumes) = weak.upgrade() else {
                return;
            };
            *volumes.refresh_source.borrow_mut() = None;
            volumes.refresh();
        });
        *self.refresh_source.borrow_mut() = Some(source);
    }

    fn refresh_inventory(&self) -> ReconcileReport {
        let snapshot = self.backend.snapshot();
        let main = main_label(&snapshot);
        if *self.main_label.borrow() != main {
            *self.main_label.borrow_mut() = main;
        }
        self.inventory.borrow_mut().reconcile(&snapshot)
    }

    fn after_inventory(self: &Rc<Self>, report: &ReconcileReport) {
        self.prune_statuses();
        self.reconcile_mount_requests();
        self.reconcile_removals();
        self.reconcile_usage(report.changed);
        self.notify();
    }

    fn prune_statuses(&self) {
        let entries = self.inventory.borrow().entries().to_vec();
        let mut statuses = self.statuses.borrow_mut();
        statuses.retain(|id, status| {
            let Some(entry) = entries.iter().find(|entry| entry.id() == id) else {
                return false;
            };
            if entry.state().is_mounted() {
                status.is_mounted()
            } else {
                !status.is_mounted()
            }
        });

        let mut removal_statuses = self.removal_statuses.borrow_mut();
        removal_statuses.retain(|id, status| {
            let Some(entry) = entries.iter().find(|entry| entry.id() == id) else {
                // A successful eject removes the volume row entirely; a
                // failure for a removed id has already been reported.
                return false;
            };
            match status {
                RemovalStatus::Pending(_) => true,
                RemovalStatus::Failed { .. } => entry.state().is_mounted(),
                RemovalStatus::Unmounted | RemovalStatus::Ejected => false,
            }
        });
    }

    /// Start an explicit mount. The target was produced by the shared
    /// inventory; this method checks the live duplicate/cap rules before any
    /// backend call.
    pub fn mount(
        self: &Rc<Self>,
        window: WindowId,
        target: MountTarget,
        parent: Option<&gtk::Window>,
    ) -> MountBegin {
        if self
            .status_for(target.id())
            .is_some_and(|status| status.is_mounted())
        {
            return MountBegin::AlreadyMounted;
        }
        if self.requests.borrow().contains_key(target.id()) {
            return MountBegin::AlreadyPending;
        }
        if self.outstanding_mounts.get() >= MAX_ACTIVE_MOUNTS {
            return MountBegin::Busy;
        }

        self.next_attempt.set(self.next_attempt.get() + 1);
        let token = self.next_attempt.get();
        let request = Rc::new(MountRequest {
            token,
            id: target.id().clone(),
            incarnation: target.incarnation(),
            window,
            cancellable: gio::Cancellable::new(),
            operation: RefCell::new(None),
            visible: Cell::new(true),
            cancel_requested: Cell::new(false),
            callback_ok: Cell::new(false),
            attempts: Cell::new(0),
            retry_scheduled: Cell::new(false),
            slot_released: Cell::new(false),
        });
        self.requests
            .borrow_mut()
            .insert(target.id().clone(), request.clone());
        self.statuses.borrow_mut().remove(target.id());
        self.outstanding_mounts
            .set(self.outstanding_mounts.get() + 1);
        log!(
            "volumes",
            "mounting '{}' ({}) for window {window} as attempt {token}",
            target.label(),
            target.native()
        );
        self.notify();

        let weak = Rc::downgrade(self);
        let callback_request = request.clone();
        let cancellable = request.cancellable.clone();
        let done: MountDone = Box::new(move |result| {
            if let Some(volumes) = weak.upgrade() {
                volumes.finish_mount(&callback_request, result);
            }
        });

        match self.backend.mount(&target, parent, &cancellable, done) {
            Ok(operation) => {
                if request.visible.get() && self.is_current_attempt(&request) {
                    *request.operation.borrow_mut() = operation;
                }
            }
            Err(error) => self.finish_mount(&request, Err(error)),
        }
        MountBegin::Pending
    }

    /// Start an explicit unmount or eject. Duplicate/cap checks and the UI gate
    /// run before any backend call; the gate releases Gimbal's own handles.
    pub fn remove(
        self: &Rc<Self>,
        window: WindowId,
        target: RemovalTarget,
        parent: Option<&gtk::Window>,
    ) -> RemovalBegin {
        if self.removals.borrow().contains_key(target.id())
            || self.requests.borrow().contains_key(target.id())
        {
            return RemovalBegin::AlreadyPending;
        }
        if self.outstanding_mounts.get() >= MAX_ACTIVE_MOUNTS {
            return RemovalBegin::Busy;
        }

        let gate = self.removal_gate.borrow().clone();
        if let Some(gate) = gate
            && let Err(message) =
                gate.before_removal(window, target.root(), target.label(), target.action())
        {
            return RemovalBegin::Refused(message);
        }

        self.next_attempt.set(self.next_attempt.get() + 1);
        let token = self.next_attempt.get();
        let request = Rc::new(RemovalRequest {
            token,
            id: target.id().clone(),
            incarnation: target.incarnation(),
            mount_object: target.mount_object(),
            root: target.root().to_path_buf(),
            label: target.label().to_string(),
            action: target.action(),
            window,
            cancellable: gio::Cancellable::new(),
            operation: RefCell::new(None),
            visible: Cell::new(true),
            cancel_requested: Cell::new(false),
            callback_ok: Cell::new(false),
            attempts: Cell::new(0),
            retry_scheduled: Cell::new(false),
            slot_released: Cell::new(false),
        });
        self.removals
            .borrow_mut()
            .insert(target.id().clone(), request.clone());
        self.removal_statuses
            .borrow_mut()
            .insert(target.id().clone(), RemovalStatus::pending(target.action()));
        self.outstanding_mounts
            .set(self.outstanding_mounts.get() + 1);
        log!(
            "volumes",
            "{} '{}' at {} for window {window} as attempt {token}",
            target.action(),
            target.label(),
            target.root().display()
        );
        self.notify();

        let weak = Rc::downgrade(self);
        let callback_request = request.clone();
        let cancellable = request.cancellable.clone();
        let done: MountDone = Box::new(move |result| {
            if let Some(volumes) = weak.upgrade() {
                volumes.finish_removal(&callback_request, result);
            }
        });

        let result = match target.action() {
            DriveAction::Unmount => self.backend.unmount(&target, parent, &cancellable, done),
            DriveAction::Eject => self.backend.eject(&target, parent, &cancellable, done),
        };
        match result {
            Ok(operation) => {
                if request.visible.get() && self.is_current_removal(&request) {
                    *request.operation.borrow_mut() = operation;
                }
            }
            Err(error) => self.finish_removal(&request, Err(error)),
        }
        RemovalBegin::Pending
    }

    fn is_current_attempt(&self, request: &MountRequest) -> bool {
        self.requests
            .borrow()
            .get(request.id())
            .is_some_and(|current| current.token == request.token)
    }

    fn is_current_removal(&self, request: &RemovalRequest) -> bool {
        self.removals
            .borrow()
            .get(&request.id)
            .is_some_and(|current| current.token == request.token)
    }

    fn release_mount_slot(&self, request: &MountRequest) {
        if !request.slot_released.replace(true) {
            self.outstanding_mounts
                .set(self.outstanding_mounts.get().saturating_sub(1));
        }
    }

    fn release_removal_slot(&self, request: &RemovalRequest) {
        if !request.slot_released.replace(true) {
            self.outstanding_mounts
                .set(self.outstanding_mounts.get().saturating_sub(1));
        }
    }

    fn finish_mount(self: &Rc<Self>, request: &Rc<MountRequest>, result: Result<(), glib::Error>) {
        // The callback always retires this attempt's own backend accounting,
        // even when its visible row has already gone. It may do nothing else.
        let current = request.visible.get() && self.is_current_attempt(request);
        self.release_mount_slot(request);
        if !current {
            return;
        }

        let report = self.refresh_inventory();
        let mounted = self
            .inventory
            .borrow()
            .reconcile_mount(request.id(), request.incarnation);
        if let Some((root, mount_object)) = mounted {
            self.complete_attempt(request, MountStatus::mounted(root, mount_object));
            self.after_inventory(&report);
            return;
        }

        match result {
            // Success is not mounted until the live inventory reports the
            // actual root. The monitor or a short reconcile poll resolves it.
            Ok(()) => request.callback_ok.set(true),
            Err(error) => {
                let alive = self
                    .inventory
                    .borrow()
                    .entry(request.id())
                    .is_some_and(|entry| entry.incarnation() == request.incarnation);
                let status = if alive {
                    classify_mount_error(&error)
                } else {
                    MountStatus::failed(
                        MountFailureKind::DeviceGone,
                        "the volume was removed while it was mounting",
                    )
                };
                self.complete_attempt(request, status);
            }
        }
        self.after_inventory(&report);
    }

    fn finish_removal(
        self: &Rc<Self>,
        request: &Rc<RemovalRequest>,
        result: Result<(), glib::Error>,
    ) {
        let current = request.visible.get() && self.is_current_removal(request);
        self.release_removal_slot(request);
        if !current {
            return;
        }

        let report = self.refresh_inventory();
        let still_there = self
            .inventory
            .borrow()
            .entry(&request.id)
            .filter(|entry| entry.incarnation() == request.incarnation)
            .and_then(|entry| entry.mount_object())
            == Some(request.mount_object);
        if !still_there {
            self.complete_removal(request, removal_success(request.action));
            self.after_inventory(&report);
            return;
        }

        match result {
            // Success is not trusted until the live inventory stops reporting
            // this exact mount object. The monitor or a short poll resolves it.
            Ok(()) => request.callback_ok.set(true),
            Err(error) => {
                let status = classify_removal_error(&error, request.action);
                self.complete_removal(request, status);
            }
        }
        self.after_inventory(&report);
    }

    fn reconcile_removals(self: &Rc<Self>) {
        let requests: Vec<Rc<RemovalRequest>> = self.removals.borrow().values().cloned().collect();
        for request in requests {
            if !request.visible.get() || !self.is_current_removal(&request) {
                continue;
            }
            let still_there = self
                .inventory
                .borrow()
                .entry(&request.id)
                .filter(|entry| entry.incarnation() == request.incarnation)
                .and_then(|entry| entry.mount_object())
                == Some(request.mount_object);
            if !still_there {
                self.complete_removal(&request, removal_success(request.action));
                continue;
            }

            if request.callback_ok.get() && !request.retry_scheduled.get() {
                request.retry_scheduled.set(true);
                let attempts = request.attempts.get() + 1;
                request.attempts.set(attempts);
                if attempts > MOUNT_RECONCILE_ATTEMPTS {
                    self.complete_removal(
                        &request,
                        RemovalStatus::failed(
                            request.action,
                            RemovalFailureKind::Failed,
                            "the desktop reported success but the drive is still mounted",
                        ),
                    );
                } else {
                    self.schedule_removal_retry(request.id.clone(), request.token);
                }
            }
        }
    }

    fn schedule_removal_retry(self: &Rc<Self>, id: VolumeId, token: u64) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(MOUNT_RECONCILE_DELAY, move || {
            if let Some(volumes) = weak.upgrade() {
                volumes.deliver_removal_retry(&id, token);
            }
        });
    }

    fn deliver_removal_retry(self: &Rc<Self>, id: &VolumeId, token: u64) {
        let still_pending = {
            let removals = self.removals.borrow();
            match removals.get(id) {
                Some(request) if request.visible.get() && request.token == token => {
                    request.retry_scheduled.set(false);
                    true
                }
                _ => false,
            }
        };
        if still_pending {
            self.refresh();
        }
    }

    fn retire_removal(&self, request: &RemovalRequest, status: RemovalStatus) -> bool {
        if !request.visible.replace(false) {
            return false;
        }
        let was_current = {
            let mut removals = self.removals.borrow_mut();
            if removals
                .get(&request.id)
                .is_some_and(|current| current.token == request.token)
            {
                removals.remove(&request.id);
                true
            } else {
                false
            }
        };
        if !was_current {
            return false;
        }
        request.operation.borrow_mut().take();
        log!(
            "volumes",
            "{} of {} (attempt {}) ended: {:?}",
            request.action,
            request.id,
            request.token,
            status
        );
        self.removal_statuses
            .borrow_mut()
            .insert(request.id.clone(), status.clone());
        let notice = RemovalNotice {
            window: request.window,
            id: request.id.clone(),
            label: request.label.clone(),
            root: request.root.clone(),
            action: request.action,
            status,
        };
        self.notify_removal(&notice);
        self.notify();
        true
    }

    fn complete_removal(self: &Rc<Self>, request: &Rc<RemovalRequest>, status: RemovalStatus) {
        self.retire_removal(request, status);
    }

    fn notify_removal(&self, notice: &RemovalNotice) {
        let listeners = self.removal_listeners.borrow().clone();
        let mut live = Vec::with_capacity(listeners.len());
        for listener in listeners {
            if listener(notice) {
                live.push(listener);
            }
        }
        *self.removal_listeners.borrow_mut() = live;
    }

    fn reconcile_mount_requests(self: &Rc<Self>) {
        let requests: Vec<Rc<MountRequest>> = self.requests.borrow().values().cloned().collect();
        for request in requests {
            if !request.visible.get() || !self.is_current_attempt(&request) {
                continue;
            }
            let mounted = self
                .inventory
                .borrow()
                .reconcile_mount(request.id(), request.incarnation);
            if let Some((root, mount_object)) = mounted {
                self.complete_attempt(&request, MountStatus::mounted(root, mount_object));
                continue;
            }

            let entry = self.inventory.borrow().entry(request.id()).cloned();
            let live = entry
                .as_ref()
                .is_some_and(|entry| entry.incarnation() == request.incarnation);
            if !live {
                request.cancellable.cancel();
                self.retire_attempt(
                    &request,
                    Some(MountStatus::failed(
                        MountFailureKind::DeviceGone,
                        "the volume was removed before the mount completed",
                    )),
                );
                continue;
            }

            if request.callback_ok.get() && !request.retry_scheduled.get() {
                request.retry_scheduled.set(true);
                let attempts = request.attempts.get() + 1;
                request.attempts.set(attempts);
                if attempts > MOUNT_RECONCILE_ATTEMPTS {
                    self.complete_attempt(
                        &request,
                        MountStatus::failed(
                            MountFailureKind::Failed,
                            "the desktop reported a mount but no root became visible",
                        ),
                    );
                } else {
                    self.schedule_mount_retry(request.id().clone(), request.token);
                }
            }
        }
    }

    fn schedule_mount_retry(self: &Rc<Self>, id: VolumeId, token: u64) {
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(MOUNT_RECONCILE_DELAY, move || {
            if let Some(volumes) = weak.upgrade() {
                volumes.deliver_mount_retry(&id, token);
            }
        });
    }

    /// Apply one reconciliation timer only to the attempt that scheduled it.
    /// A timer delivered after a monitor completion, unmount or retry is
    /// stale and must not refresh or alter the replacement.
    fn deliver_mount_retry(self: &Rc<Self>, id: &VolumeId, token: u64) {
        let still_pending = {
            let requests = self.requests.borrow();
            match requests.get(id) {
                Some(request) if request.visible.get() && request.token == token => {
                    request.retry_scheduled.set(false);
                    true
                }
                _ => false,
            }
        };
        if still_pending {
            self.refresh();
        }
    }

    fn retire_attempt(&self, request: &MountRequest, status: Option<MountStatus>) -> bool {
        if !request.visible.replace(false) {
            return false;
        }
        let was_current = {
            let mut requests = self.requests.borrow_mut();
            if requests
                .get(request.id())
                .is_some_and(|current| current.token == request.token)
            {
                requests.remove(request.id());
                true
            } else {
                false
            }
        };
        if !was_current {
            return false;
        }
        request.operation.borrow_mut().take();
        if let Some(status) = status {
            log!(
                "volumes",
                "mount of {} (attempt {}) ended: {}",
                request.id(),
                request.token,
                status.short()
            );
            self.statuses
                .borrow_mut()
                .insert(request.id().clone(), status);
        }
        self.notify();
        true
    }

    fn complete_attempt(self: &Rc<Self>, request: &Rc<MountRequest>, status: MountStatus) {
        self.retire_attempt(request, Some(status));
    }

    /// Cancel one pending request through its own operation's cancellable.
    /// This is request lifecycle, not a registry command, and never unmounts a
    /// completed mount.
    pub fn cancel(&self, id: &VolumeId) {
        if let Some(request) = self.removals.borrow().get(id).cloned() {
            if !request.visible.get() || request.cancel_requested.replace(true) {
                return;
            }
            log!(
                "volumes",
                "cancelling the {} of {}",
                request.action,
                request.id
            );
            request.cancellable.cancel();
            request.operation.borrow_mut().take();
            self.notify();
            return;
        }
        let Some(request) = self.requests.borrow().get(id).cloned() else {
            return;
        };
        if !request.visible.get() || request.cancel_requested.replace(true) {
            return;
        }
        log!("volumes", "cancelling the mount of {}", request.id());
        request.cancellable.cancel();
        request.operation.borrow_mut().take();
        self.notify();
    }

    /// Closing a window cancels only the request that window started. Its
    /// visible ownership is retired; its backend call stays counted until the
    /// callback arrives.
    pub fn cancel_window(&self, window: WindowId) {
        let requests: Vec<Rc<MountRequest>> = self
            .requests
            .borrow()
            .values()
            .filter(|request| request.window == window)
            .cloned()
            .collect();
        for request in requests {
            request.cancel_requested.set(true);
            request.cancellable.cancel();
            request.operation.borrow_mut().take();
            self.retire_attempt(
                &request,
                Some(MountStatus::failed(
                    MountFailureKind::Cancelled,
                    "the window requesting the mount was closed",
                )),
            );
        }

        let removals: Vec<Rc<RemovalRequest>> = self
            .removals
            .borrow()
            .values()
            .filter(|request| request.window == window)
            .cloned()
            .collect();
        for request in removals {
            request.cancel_requested.set(true);
            request.cancellable.cancel();
            request.operation.borrow_mut().take();
            self.retire_removal(
                &request,
                RemovalStatus::failed(
                    request.action,
                    RemovalFailureKind::Cancelled,
                    "the window requesting the operation was closed",
                ),
            );
        }
    }

    /// The app is leaving. Cancel every request and usage query; never join a
    /// worker. A late callback finds no in-flight record and is ignored.
    pub fn shutdown(&self) {
        if let Some(source) = self.refresh_source.borrow_mut().take() {
            source.remove();
        }
        let requests: Vec<Rc<MountRequest>> = self.requests.borrow().values().cloned().collect();
        for request in requests {
            request.cancellable.cancel();
            request.operation.borrow_mut().take();
            request.visible.set(false);
            request.slot_released.set(true);
        }
        self.requests.borrow_mut().clear();
        let removals: Vec<Rc<RemovalRequest>> = self.removals.borrow().values().cloned().collect();
        for request in removals {
            request.cancellable.cancel();
            request.operation.borrow_mut().take();
            request.visible.set(false);
            request.slot_released.set(true);
        }
        self.removals.borrow_mut().clear();
        self.removal_statuses.borrow_mut().clear();
        self.outstanding_mounts.set(0);

        let mut usage = self.usage.borrow_mut();
        for record in usage.records.values() {
            if let Some(cancellable) = &record.cancellable {
                cancellable.cancel();
            }
        }
        usage.records.clear();
        usage.queue.clear();
        usage.in_flight.clear();
        usage.active = 0;
        self.usage_refresh.borrow_mut().clear();
    }

    // -----------------------------------------------------------------
    // Usage queries
    // -----------------------------------------------------------------

    fn reconcile_usage(self: &Rc<Self>, changed: bool) {
        let (entries, root_mount_object) = {
            let inventory = self.inventory.borrow();
            (inventory.entries().to_vec(), inventory.root_mount_object())
        };
        let mut live = HashSet::new();
        live.insert(UsageKey::main(root_mount_object));
        for entry in &entries {
            if let Some(key) = UsageKey::entry(entry) {
                live.insert(key);
            }
        }

        // Cancel queries for roots that no longer match a live entry/root.
        let dead: Vec<UsageKey> = self
            .usage
            .borrow()
            .records
            .keys()
            .filter(|key| !live.contains(*key))
            .cloned()
            .collect();
        if !dead.is_empty() {
            let mut usage = self.usage.borrow_mut();
            for key in dead {
                if let Some(record) = usage.records.remove(&key)
                    && let Some(cancellable) = record.cancellable
                {
                    cancellable.cancel();
                }
            }
            let live_keys: HashSet<UsageKey> = usage.records.keys().cloned().collect();
            usage.queue.retain(|job| live_keys.contains(&job.key));
        }

        let refresh = std::mem::take(&mut *self.usage_refresh.borrow_mut());
        self.usage
            .borrow_mut()
            .records
            .retain(|key, record| !refresh.contains(key) || record.value == UsageValue::Pending);

        // Only a mount-change notification invalidates a completed usage
        // result for a surviving key. Pending queries already cover that root.
        for key in live {
            self.ensure_usage(key, changed);
        }
    }

    fn ensure_usage(self: &Rc<Self>, key: UsageKey, retry: bool) {
        let action = {
            let mut usage = self.usage.borrow_mut();
            if let Some(value) = usage.records.get(&key).map(|record| record.value.clone()) {
                match value {
                    UsageValue::Ready { .. } | UsageValue::Pending => return,
                    UsageValue::Unknown | UsageValue::Busy | UsageValue::Unavailable { .. } => {
                        if !retry {
                            return;
                        }
                        usage.records.remove(&key);
                    }
                }
            }

            usage.next_generation += 1;
            let generation = usage.next_generation;
            if usage.active >= MAX_ACTIVE_USAGE {
                if usage.queue.len() >= MAX_PENDING_USAGE {
                    usage.records.insert(
                        key,
                        UsageRecord {
                            generation,
                            value: UsageValue::Busy,
                            cancellable: None,
                        },
                    );
                    None
                } else {
                    usage.next_query += 1;
                    let query = usage.next_query;
                    usage.records.insert(
                        key.clone(),
                        UsageRecord {
                            generation,
                            value: UsageValue::Pending,
                            cancellable: Some(gio::Cancellable::new()),
                        },
                    );
                    usage.queue.push_back(UsageJob { key, query });
                    None
                }
            } else {
                usage.next_query += 1;
                let query = usage.next_query;
                let cancellable = gio::Cancellable::new();
                usage.records.insert(
                    key.clone(),
                    UsageRecord {
                        generation,
                        value: UsageValue::Pending,
                        cancellable: Some(cancellable.clone()),
                    },
                );
                usage.in_flight.insert(query, key.clone());
                usage.active += 1;
                Some((key, query, generation, cancellable))
            }
        };

        if let Some((key, query, generation, cancellable)) = action {
            self.spawn_usage(key, query, generation, cancellable);
        }
        self.notify();
    }

    fn spawn_usage(
        self: &Rc<Self>,
        key: UsageKey,
        query: u64,
        generation: u64,
        cancellable: gio::Cancellable,
    ) {
        let weak = Rc::downgrade(self);
        let callback_key = key.clone();
        let done: UsageDone = Box::new(move |result| {
            if let Some(volumes) = weak.upgrade() {
                volumes.finish_usage(query, callback_key, generation, result);
            }
        });
        self.backend.usage(&key.root, &cancellable, done);
    }

    fn finish_usage(
        self: &Rc<Self>,
        query: u64,
        key: UsageKey,
        generation: u64,
        result: Result<(u64, u64), glib::Error>,
    ) {
        {
            let mut usage = self.usage.borrow_mut();
            let Some(in_flight_key) = usage.in_flight.remove(&query) else {
                return;
            };
            usage.active = usage.active.saturating_sub(1);
            if in_flight_key != key {
                // A result is never attributed to a different root.
                drop(usage);
                self.pump_usage();
                return;
            }
            let current = usage
                .records
                .get(&key)
                .is_some_and(|record| record.generation == generation);
            if current {
                let value = match result {
                    Ok((total, free)) if self.usage_key_live(&key) => {
                        UsageValue::Ready { total, free }
                    }
                    Ok(_) => UsageValue::Unknown,
                    Err(error) => UsageValue::Unavailable {
                        detail: usage_detail(&error),
                    },
                };
                if value == UsageValue::Unknown {
                    usage.records.remove(&key);
                } else if let Some(record) = usage.records.get_mut(&key) {
                    record.cancellable = None;
                    record.value = value;
                }
            }
        }
        self.pump_usage();
        self.notify();
    }

    fn pump_usage(self: &Rc<Self>) {
        loop {
            let action = {
                let mut usage = self.usage.borrow_mut();
                if usage.active >= MAX_ACTIVE_USAGE {
                    break;
                }
                let Some(job) = usage.queue.pop_front() else {
                    break;
                };
                let generation = match usage.records.get(&job.key) {
                    Some(record) if record.value == UsageValue::Pending => record.generation,
                    _ => continue,
                };
                let cancellable = usage
                    .records
                    .get(&job.key)
                    .and_then(|record| record.cancellable.clone())
                    .unwrap_or_default();
                usage.in_flight.insert(job.query, job.key.clone());
                usage.active += 1;
                Some((job.key, job.query, generation, cancellable))
            };
            if let Some((key, query, generation, cancellable)) = action {
                self.spawn_usage(key, query, generation, cancellable);
            }
        }
    }

    fn usage_key_live(&self, key: &UsageKey) -> bool {
        let inventory = self.inventory.borrow();
        match &key.owner {
            None => key.root == Path::new("/") && key.mount_object == inventory.root_mount_object(),
            Some((id, incarnation)) => inventory.entry(id).is_some_and(|entry| {
                entry.incarnation() == *incarnation
                    && entry.state().root() == Some(key.root.as_path())
                    && entry.mount_object() == key.mount_object
            }),
        }
    }

    fn notify(&self) {
        let listeners = self.listeners.borrow().clone();
        if listeners.is_empty() {
            return;
        }
        let mut live = Vec::with_capacity(listeners.len());
        for listener in listeners {
            if listener() {
                live.push(listener);
            }
        }
        *self.listeners.borrow_mut() = live;
    }
}

fn main_label(snapshot: &RawSnapshot) -> String {
    let from_volume = snapshot.volumes.iter().find_map(|volume| {
        volume
            .mount
            .as_ref()
            .filter(|mount| mount.root == Path::new("/"))
            .and_then(|mount| first_nonempty(&[&volume.label, &mount.label]))
    });
    let from_mount = snapshot
        .mounts
        .iter()
        .find(|mount| mount.root == Path::new("/"))
        .and_then(|mount| first_nonempty(&[&mount.label]));
    from_volume
        .or(from_mount)
        .unwrap_or_else(|| "Main Drive".to_string())
}

fn first_nonempty(values: &[&String]) -> Option<String> {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .map(str::to_string)
}

/// Map a GIO error to a truthful row state. Cancellation, permission denial,
/// removal and a missing backend stay distinct.
pub fn classify_mount_error(error: &glib::Error) -> MountStatus {
    let kind = match error_kind(error) {
        ErrorKind::Cancelled => MountFailureKind::Cancelled,
        ErrorKind::Denied => MountFailureKind::Denied,
        ErrorKind::DeviceGone => MountFailureKind::DeviceGone,
        ErrorKind::BackendUnavailable => MountFailureKind::BackendUnavailable,
        ErrorKind::Busy => MountFailureKind::Busy,
        ErrorKind::Handled => MountFailureKind::Handled,
        ErrorKind::Failed => MountFailureKind::Failed,
    };
    MountStatus::failed(kind, error.message())
}

/// The same honest mapping for an unmount or eject. An authentication prompt
/// the user cancelled is `Cancelled`, not a failure.
pub fn classify_removal_error(error: &glib::Error, action: DriveAction) -> RemovalStatus {
    let kind = match error_kind(error) {
        ErrorKind::Cancelled => RemovalFailureKind::Cancelled,
        ErrorKind::Denied => RemovalFailureKind::Denied,
        ErrorKind::DeviceGone => RemovalFailureKind::DeviceGone,
        ErrorKind::BackendUnavailable => RemovalFailureKind::BackendUnavailable,
        ErrorKind::Busy => RemovalFailureKind::Busy,
        ErrorKind::Handled => RemovalFailureKind::Handled,
        ErrorKind::Failed => RemovalFailureKind::Failed,
    };
    RemovalStatus::failed(action, kind, error.message())
}

enum ErrorKind {
    Cancelled,
    Denied,
    DeviceGone,
    BackendUnavailable,
    Busy,
    Handled,
    Failed,
}

fn error_kind(error: &glib::Error) -> ErrorKind {
    if error.matches(gio::IOErrorEnum::Cancelled) {
        ErrorKind::Cancelled
    } else if error.matches(gio::IOErrorEnum::PermissionDenied) {
        ErrorKind::Denied
    } else if error.matches(gio::IOErrorEnum::NotFound)
        || error.matches(gio::IOErrorEnum::NotMounted)
    {
        ErrorKind::DeviceGone
    } else if error.matches(gio::IOErrorEnum::NotSupported) {
        ErrorKind::BackendUnavailable
    } else if error.matches(gio::IOErrorEnum::Busy) || error.matches(gio::IOErrorEnum::WouldBlock) {
        ErrorKind::Busy
    } else if error.matches(gio::IOErrorEnum::FailedHandled) {
        ErrorKind::Handled
    } else {
        ErrorKind::Failed
    }
}

fn removal_success(action: DriveAction) -> RemovalStatus {
    match action {
        DriveAction::Unmount => RemovalStatus::Unmounted,
        DriveAction::Eject => RemovalStatus::Ejected,
    }
}

fn usage_detail(error: &glib::Error) -> String {
    let message = error.message().trim();
    if message.is_empty() {
        "filesystem capacity is not available".to_string()
    } else {
        message.to_string()
    }
}

enum MonitorSignal {
    MountAdded,
    MountRemoved,
    VolumeAdded,
    VolumeRemoved,
    VolumeChanged,
}

impl MonitorSignal {
    fn connect(self, monitor: &gio::VolumeMonitor, f: impl Fn() + 'static) {
        match self {
            MonitorSignal::MountAdded => {
                monitor.connect_mount_added(move |_, _| f());
            }
            MonitorSignal::MountRemoved => {
                monitor.connect_mount_removed(move |_, _| f());
            }
            MonitorSignal::VolumeAdded => {
                monitor.connect_volume_added(move |_, _| f());
            }
            MonitorSignal::VolumeRemoved => {
                monitor.connect_volume_removed(move |_, _| f());
            }
            MonitorSignal::VolumeChanged => {
                monitor.connect_volume_changed(move |_, _| f());
            }
        }
    }
}

#[cfg(test)]
impl Volumes {
    pub(crate) fn refresh_for_tests(self: &Rc<Self>) {
        self.refresh();
    }

    pub(crate) fn pending_requests_for_tests(&self) -> usize {
        self.requests.borrow().len()
    }

    pub(crate) fn pending_removals_for_tests(&self) -> usize {
        self.removals.borrow().len()
    }

    pub(crate) fn attempt_token_for_tests(&self, id: &VolumeId) -> Option<u64> {
        self.requests.borrow().get(id).map(|request| request.token)
    }

    pub(crate) fn deliver_mount_retry_for_tests(self: &Rc<Self>, id: &VolumeId, token: u64) {
        self.deliver_mount_retry(id, token);
    }

    pub(crate) fn usage_state_for_tests(&self, entry: &DeviceEntry) -> UsageValue {
        self.usage_for_entry(entry)
    }

    pub(crate) fn main_usage_for_tests(&self) -> UsageValue {
        self.main_usage()
    }
}
