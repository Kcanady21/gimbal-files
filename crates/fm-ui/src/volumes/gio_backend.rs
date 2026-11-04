// SPDX-License-Identifier: GPL-3.0-or-later
//! The production GIO volume backend.
//!
//! It enumerates GIO's VolumeMonitor, which delegates to whatever desktop
//! volume service is installed (udisks2 on many desktops, another service
//! elsewhere). Nothing here talks to D-Bus directly, runs `/dev` lookups or
//! shells out. Only a live volume object with a `unix-device` identifier and
//! `can_mount` becomes a mount candidate.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use fm_core::volumes::{MountTarget, RawMount, RawSnapshot, RawVolume, RemovalTarget};

use super::{Backend, MountDone, UsageDone};

pub struct GioBackend {
    monitor: gio::VolumeMonitor,
    volume_serials: RefCell<HashMap<usize, (glib::WeakRef<gio::Volume>, u64)>>,
    mount_serials: RefCell<HashMap<usize, (glib::WeakRef<gio::Mount>, u64)>>,
    next_volume_serial: Cell<u64>,
    next_mount_serial: Cell<u64>,
}

impl GioBackend {
    pub fn new() -> Self {
        crate::log!("volumes", "GIO volume monitor acquired");
        Self {
            monitor: gio::VolumeMonitor::get(),
            volume_serials: RefCell::new(HashMap::new()),
            mount_serials: RefCell::new(HashMap::new()),
            next_volume_serial: Cell::new(0),
            next_mount_serial: Cell::new(1 << 62),
        }
    }

    fn volume_serial(&self, volume: &gio::Volume) -> u64 {
        let key = volume.as_ptr() as usize;
        let mut serials = self.volume_serials.borrow_mut();
        if let Some((weak, serial)) = serials.get(&key)
            && weak.upgrade().is_some_and(|live| live == *volume)
        {
            return *serial;
        }
        self.next_volume_serial
            .set(self.next_volume_serial.get() + 1);
        let serial = self.next_volume_serial.get();
        let weak = glib::WeakRef::new();
        weak.set(Some(volume));
        serials.insert(key, (weak, serial));
        serial
    }

    fn mount_serial(&self, mount: &gio::Mount) -> u64 {
        let key = mount.as_ptr() as usize;
        let mut serials = self.mount_serials.borrow_mut();
        if let Some((weak, serial)) = serials.get(&key)
            && weak.upgrade().is_some_and(|live| live == *mount)
        {
            return *serial;
        }
        self.next_mount_serial.set(self.next_mount_serial.get() + 1);
        let serial = self.next_mount_serial.get();
        let weak = glib::WeakRef::new();
        weak.set(Some(mount));
        serials.insert(key, (weak, serial));
        serial
    }

    fn existing_volume_serial(&self, volume: &gio::Volume) -> Option<u64> {
        let key = volume.as_ptr() as usize;
        let serials = self.volume_serials.borrow();
        serials.get(&key).and_then(|(weak, serial)| {
            weak.upgrade()
                .is_some_and(|live| live == *volume)
                .then_some(*serial)
        })
    }

    fn existing_mount_serial(&self, mount: &gio::Mount) -> Option<u64> {
        let key = mount.as_ptr() as usize;
        let serials = self.mount_serials.borrow();
        serials.get(&key).and_then(|(weak, serial)| {
            weak.upgrade()
                .is_some_and(|live| live == *mount)
                .then_some(*serial)
        })
    }

    fn find_mount(&self, serial: u64) -> Option<gio::Mount> {
        self.monitor
            .mounts()
            .into_iter()
            .find(|mount| self.existing_mount_serial(mount) == Some(serial))
    }

    fn raw_mount(&self, mount: &gio::Mount, volume: Option<&gio::Volume>) -> RawMount {
        let root = mount
            .root()
            .path()
            .unwrap_or_else(|| std::path::PathBuf::from("/"));
        let volume_label = volume.map(|volume| volume.name().to_string());
        let label = if mount.name().is_empty() {
            volume_label.unwrap_or_else(|| "Mount".to_string())
        } else {
            mount.name().to_string()
        };
        RawMount {
            object: self.mount_serial(mount),
            root,
            label,
            native: volume
                .and_then(|volume| volume.identifier("unix-device"))
                .map(|value| value.to_string()),
            uuid: mount.uuid().map(|value| value.to_string()).or_else(|| {
                volume
                    .and_then(|volume| volume.uuid())
                    .map(|value| value.to_string())
            }),
            can_unmount: mount.can_unmount(),
            can_eject: mount.can_eject(),
        }
    }
}

impl Backend for GioBackend {
    fn snapshot(&self) -> RawSnapshot {
        let volumes: Vec<RawVolume> = self
            .monitor
            .volumes()
            .iter()
            .map(|volume| RawVolume {
                object: self.volume_serial(volume),
                native: volume
                    .identifier("unix-device")
                    .map(|value| value.to_string()),
                uuid: volume.uuid().map(|value| value.to_string()),
                label: volume.name().to_string(),
                can_mount: volume.can_mount(),
                has_drive: volume.drive().is_some(),
                mount: volume
                    .get_mount()
                    .map(|mount| self.raw_mount(&mount, Some(volume))),
            })
            .collect();
        let mounts = self
            .monitor
            .mounts()
            .iter()
            .map(|mount| self.raw_mount(mount, mount.volume().as_ref()))
            .collect();
        RawSnapshot { volumes, mounts }
    }

    fn mount(
        &self,
        target: &MountTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        let volume = self.monitor.volumes().into_iter().find(|volume| {
            self.existing_volume_serial(volume) == Some(target.object())
                && volume.identifier("unix-device").as_deref() == Some(target.native())
        });
        let Some(volume) = volume else {
            return Err(glib::Error::new(
                gio::IOErrorEnum::NotFound,
                "the volume is no longer present",
            ));
        };
        if volume.get_mount().is_some() {
            return Err(glib::Error::new(
                gio::IOErrorEnum::AlreadyMounted,
                "the volume is already mounted",
            ));
        }
        if !volume.can_mount() {
            return Err(glib::Error::new(
                gio::IOErrorEnum::NotSupported,
                "the volume service does not offer to mount this volume",
            ));
        }
        let Some(parent) = parent else {
            return Err(glib::Error::new(
                gio::IOErrorEnum::Failed,
                "the window needed for authentication is gone",
            ));
        };

        // GtkMountOperation is the existing GIO operation with the desktop's
        // authentication prompt. No credential is read, stored or logged here.
        let operation = gtk::MountOperation::new(Some(parent));
        let keep = operation.clone();
        volume.mount(
            gio::MountMountFlags::NONE,
            Some(&operation),
            Some(cancellable),
            move |result| {
                drop(keep);
                done(result);
            },
        );
        Ok(Some(operation.upcast()))
    }

    fn unmount(
        &self,
        target: &RemovalTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        let mount = self.find_mount(target.mount_object()).ok_or_else(|| {
            glib::Error::new(gio::IOErrorEnum::NotFound, "the mount is no longer present")
        })?;
        if !mount.can_unmount() {
            return Err(glib::Error::new(
                gio::IOErrorEnum::NotSupported,
                "the volume service does not offer to unmount this mount",
            ));
        }
        let Some(parent) = parent else {
            return Err(glib::Error::new(
                gio::IOErrorEnum::Failed,
                "the window needed for authentication is gone",
            ));
        };
        let operation = gtk::MountOperation::new(Some(parent));
        let keep = operation.clone();
        mount.unmount_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&operation),
            Some(cancellable),
            move |result| {
                drop(keep);
                done(result);
            },
        );
        Ok(Some(operation.upcast()))
    }

    fn eject(
        &self,
        target: &RemovalTarget,
        parent: Option<&gtk::Window>,
        cancellable: &gio::Cancellable,
        done: MountDone,
    ) -> Result<Option<gio::MountOperation>, glib::Error> {
        let mount = self.find_mount(target.mount_object()).ok_or_else(|| {
            glib::Error::new(gio::IOErrorEnum::NotFound, "the mount is no longer present")
        })?;
        if !mount.can_eject() {
            return Err(glib::Error::new(
                gio::IOErrorEnum::NotSupported,
                "the volume service does not offer to eject this mount",
            ));
        }
        let Some(parent) = parent else {
            return Err(glib::Error::new(
                gio::IOErrorEnum::Failed,
                "the window needed for authentication is gone",
            ));
        };
        let operation = gtk::MountOperation::new(Some(parent));
        let keep = operation.clone();
        mount.eject_with_operation(
            gio::MountUnmountFlags::NONE,
            Some(&operation),
            Some(cancellable),
            move |result| {
                drop(keep);
                done(result);
            },
        );
        Ok(Some(operation.upcast()))
    }

    fn usage(&self, root: &Path, cancellable: &gio::Cancellable, done: UsageDone) {
        let file = gio::File::for_path(root);
        file.query_filesystem_info_async(
            "filesystem::size,filesystem::free",
            glib::Priority::DEFAULT_IDLE,
            Some(cancellable),
            move |result| {
                done(result.map(|info| {
                    (
                        info.attribute_uint64("filesystem::size"),
                        info.attribute_uint64("filesystem::free"),
                    )
                }));
            },
        );
    }

    fn note(&self) -> String {
        let backend = self.monitor.type_().name();
        format!(
            "GIO volume monitor {backend}; {} volume(s), {} mount(s)",
            self.monitor.volumes().len(),
            self.monitor.mounts().len()
        )
    }

    fn monitor(&self) -> Option<gio::VolumeMonitor> {
        Some(self.monitor.clone())
    }
}
