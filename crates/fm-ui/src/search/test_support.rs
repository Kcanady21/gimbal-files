// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only filesystem trees and blocking gates for the search workers.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::thread::ThreadId;

use fm_core::folder_size::{DirReader, EntryKind, EntryMeta};
use fm_core::search::SearchSource;

#[derive(Default)]
struct GateState {
    released: bool,
    entered: usize,
    peak: usize,
}

#[derive(Default)]
pub(crate) struct Gate {
    state: Mutex<GateState>,
    wake: Condvar,
}

impl Gate {
    pub(crate) fn block(&self) {
        let mut state = self.state.lock().unwrap();
        state.entered += 1;
        state.peak = state.peak.max(state.entered);
        while !state.released {
            state = self.wake.wait(state).unwrap();
        }
        state.entered = state.entered.saturating_sub(1);
    }

    pub(crate) fn release(&self) {
        self.state.lock().unwrap().released = true;
        self.wake.notify_all();
    }

    pub(crate) fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        state.released = false;
        state.entered = 0;
        state.peak = 0;
    }

    pub(crate) fn entered(&self) -> usize {
        self.state.lock().unwrap().entered
    }

    pub(crate) fn peak(&self) -> usize {
        self.state.lock().unwrap().peak
    }
}

struct Reader {
    entries: VecDeque<PathBuf>,
}

impl DirReader for Reader {
    fn next_entry(&mut self) -> Option<io::Result<PathBuf>> {
        self.entries.pop_front().map(Ok)
    }
}

#[derive(Default)]
pub(crate) struct TestSource {
    dirs: HashMap<PathBuf, Vec<PathBuf>>,
    metas: HashMap<PathBuf, EntryMeta>,
    block_roots: HashSet<PathBuf>,
    gate: Option<std::sync::Arc<Gate>>,
    threads: Mutex<HashSet<ThreadId>>,
}

impl TestSource {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_gate(mut self, gate: std::sync::Arc<Gate>) -> Self {
        self.gate = Some(gate);
        self
    }

    pub(crate) fn dir(mut self, path: &str, children: &[&str]) -> Self {
        self.dirs.insert(
            PathBuf::from(path),
            children.iter().map(PathBuf::from).collect(),
        );
        self.meta(path, EntryKind::Directory, 1, 0)
    }

    pub(crate) fn file(self, path: &str, device: u64) -> Self {
        self.meta(path, EntryKind::File, device, 12)
    }

    pub(crate) fn block(mut self, path: &str) -> Self {
        self.block_roots.insert(PathBuf::from(path));
        self
    }

    fn meta(mut self, path: &str, kind: EntryKind, device: u64, size: u64) -> Self {
        let inode = self.metas.len() as u64 + 1;
        self.metas.insert(
            PathBuf::from(path),
            EntryMeta {
                device,
                inode,
                size,
                blocks: 1,
                nlink: 1,
                kind,
                modified_sec: 1_700_000_000,
                modified_nsec: 0,
            },
        );
        self
    }

    pub(crate) fn thread_count(&self) -> usize {
        self.threads.lock().unwrap().len()
    }
}

impl SearchSource for TestSource {
    fn resolve_root(&self, root: &Path) -> io::Result<PathBuf> {
        if self.metas.contains_key(root) {
            Ok(root.to_path_buf())
        } else {
            Err(io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn lstat(&self, path: &Path) -> io::Result<EntryMeta> {
        self.threads
            .lock()
            .unwrap()
            .insert(std::thread::current().id());
        self.metas
            .get(path)
            .copied()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn read_dir(&self, path: &Path) -> io::Result<Box<dyn DirReader + '_>> {
        self.threads
            .lock()
            .unwrap()
            .insert(std::thread::current().id());
        if self.block_roots.contains(path)
            && let Some(gate) = &self.gate
        {
            gate.block();
        }
        let children = self
            .dirs
            .get(path)
            .cloned()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok(Box::new(Reader {
            entries: children.into(),
        }))
    }
}
