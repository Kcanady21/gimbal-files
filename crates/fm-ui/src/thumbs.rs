// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded grid thumbnails: a main-thread texture cache and a small worker
//! pool for image/video/PDF decoding. Cells only request when GTK binds them,
//! so a 100,000-file listing never starts 100,000 reads.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, TrySendError};
use std::sync::{Arc, OnceLock};

use fm_core::preview::{
    self, FileRevision, MAX_THUMBNAIL_CACHE_BYTES, RenderSize, ThumbnailSource,
};
use gtk::prelude::*;

use crate::image::{self, DecodedImage};
use crate::pdf;

const THUMB_WORKERS: usize = 2;
const THUMB_QUEUE: usize = 64;
/// Bounded, coalesced demand that waits for a free worker. Each bound cell
/// keeps at most one entry (a rebind cancels the old token first).
const THUMB_DEFERRED: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Scheduled {
    Queued,
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestError {
    QueueFull,
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ThumbKind {
    Image,
    Pdf,
    Video,
}

/// Everything that invalidates a cached thumbnail: the exact file/revision,
/// the requested device-pixel box and the render kind.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ThumbKey {
    pub path: PathBuf,
    pub revision: FileRevision,
    pub width: i32,
    pub height: i32,
    pub kind: ThumbKind,
}

pub(crate) struct ThumbEvent {
    pub token: u64,
    pub key: ThumbKey,
    pub image: Option<DecodedImage>,
    pub failure: Option<String>,
}

struct ThumbJob {
    token: u64,
    key: ThumbKey,
    source: PathBuf,
    candidates: Vec<ThumbnailSource>,
    cancel: Arc<AtomicBool>,
    tx: Sender<ThumbEvent>,
    #[cfg(test)]
    sent: Arc<std::sync::atomic::AtomicUsize>,
}

fn pool() -> &'static SyncSender<ThumbJob> {
    static POOL: OnceLock<SyncSender<ThumbJob>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<ThumbJob>(THUMB_QUEUE);
        let receiver = Arc::new(std::sync::Mutex::new(rx));
        for index in 0..THUMB_WORKERS {
            let receiver = receiver.clone();
            std::thread::Builder::new()
                .name(format!("gimbal-thumb-{index}"))
                .spawn(move || {
                    loop {
                        let job = {
                            let receiver = receiver.lock().expect("the thumbnail queue lock");
                            receiver.recv()
                        };
                        let Ok(job) = job else {
                            return;
                        };
                        #[cfg(test)]
                        QUEUED.fetch_sub(1, Ordering::SeqCst);
                        if job.cancel.load(Ordering::Relaxed) {
                            continue;
                        }
                        let (image, failure) = match decode(&job) {
                            Ok(image) => (Some(image), None),
                            Err(message) => (None, Some(message)),
                        };
                        if job.cancel.load(Ordering::Relaxed) {
                            continue;
                        }
                        #[cfg(test)]
                        wait_send_gate(&job.source);
                        let _ = job.tx.send(ThumbEvent {
                            token: job.token,
                            key: job.key,
                            image,
                            failure,
                        });
                        #[cfg(test)]
                        job.sent.fetch_add(1, Ordering::SeqCst);
                    }
                })
                .expect("the thumbnail worker starts");
        }
        tx
    })
}

#[cfg(test)]
static SEND_GATE: std::sync::Mutex<Option<Arc<image::TestGate>>> = std::sync::Mutex::new(None);
#[cfg(test)]
static SCHED_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
#[cfg(test)]
static QUEUED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
fn set_send_gate(prefix: &Path) -> Arc<image::TestGate> {
    let gate = Arc::new(image::TestGate::new(prefix));
    *SEND_GATE.lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn clear_send_gate() {
    if let Some(gate) = SEND_GATE.lock().unwrap().take() {
        gate.release();
    }
}

#[cfg(test)]
fn wait_send_gate(path: &Path) {
    let gate = SEND_GATE.lock().unwrap().clone();
    if let Some(gate) = gate {
        gate.wait(path);
    }
}

fn decode(job: &ThumbJob) -> Result<DecodedImage, String> {
    let target = RenderSize {
        width: job.key.width,
        height: job.key.height,
    };
    match job.key.kind {
        ThumbKind::Image => image::decode_image_file(&job.source, target, true)
            .map_err(|error| format!("{error:?}")),
        ThumbKind::Video => decode_cached(&job.candidates, target),
        ThumbKind::Pdf => render_pdf(&job.source, target, &job.cancel),
    }
}

fn render_pdf(
    path: &Path,
    target: RenderSize,
    cancel: &Arc<AtomicBool>,
) -> Result<DecodedImage, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let input = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    pdf::submit_grid(pdf::PdfJob {
        generation: 0,
        work: pdf::PdfWork::Render(fm_core::preview::PdfRenderRequest {
            input,
            output_dir: PathBuf::from("/tmp"),
            target,
        }),
        cancel: cancel.clone(),
        tx,
        owner: 0,
    })
    .map_err(|error| error.message().to_string())?;
    let outcome = rx
        .recv_timeout(std::time::Duration::from_millis(
            preview::PDF_RENDER_TIMEOUT_MS + 2_000,
        ))
        .map_err(|_| "PDF renderer did not answer.".to_string())?;
    match (outcome.image, outcome.failure) {
        (Some(image), _) => Ok(image),
        (None, Some(failure)) => Err(failure.message()),
        (None, None) => Err("PDF renderer returned nothing.".to_string()),
    }
}

fn decode_cached(
    candidates: &[ThumbnailSource],
    target: RenderSize,
) -> Result<DecodedImage, String> {
    let mut best: Option<DecodedImage> = None;
    for candidate in candidates {
        let Ok(image) = image::decode_image_file(&candidate.path, target, false) else {
            continue;
        };
        let enough =
            image.source_width.max(image.source_height) >= target.width.min(target.height).max(64);
        if best.as_ref().is_none_or(|best| {
            image.source_width * image.source_height > best.source_width * best.source_height
        }) {
            best = Some(image);
        }
        if enough {
            break;
        }
    }
    best.ok_or_else(|| "no readable cached thumbnail".to_string())
}

// ---------------------------------------------------------------------------
// Main-thread texture cache
// ---------------------------------------------------------------------------

struct CacheEntry {
    texture: gtk::gdk::MemoryTexture,
    bytes: u64,
}

#[derive(Default)]
struct Cache {
    entries: HashMap<ThumbKey, CacheEntry>,
    order: Vec<ThumbKey>,
    bytes: u64,
}

thread_local! {
    static CACHE: RefCell<Cache> = RefCell::new(Cache::default());
}

fn texture_bytes(texture: &gtk::gdk::MemoryTexture) -> u64 {
    u64::from(texture.width() as u32) * u64::from(texture.height() as u32) * 4
}

pub(crate) fn cached(key: &ThumbKey) -> Option<gtk::gdk::MemoryTexture> {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        let texture = cache.entries.get(key).map(|entry| entry.texture.clone())?;
        cache.order.retain(|candidate| candidate != key);
        cache.order.push(key.clone());
        Some(texture)
    })
}

pub(crate) fn insert(key: ThumbKey, image: &DecodedImage) {
    let texture = image::texture(image);
    let bytes = texture_bytes(&texture);
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some(old) = cache.entries.remove(&key) {
            cache.bytes = cache.bytes.saturating_sub(old.bytes);
            cache.order.retain(|candidate| candidate != &key);
        }
        cache.bytes = cache.bytes.saturating_add(bytes);
        cache
            .entries
            .insert(key.clone(), CacheEntry { texture, bytes });
        cache.order.push(key);
        while cache.bytes > MAX_THUMBNAIL_CACHE_BYTES {
            let Some(oldest) = cache.order.first().cloned() else {
                break;
            };
            cache.order.remove(0);
            if let Some(entry) = cache.entries.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(entry.bytes);
            }
        }
    });
}

#[cfg(test)]
pub(crate) fn cache_bytes() -> u64 {
    CACHE.with(|cache| cache.borrow().bytes)
}

#[cfg(test)]
pub(crate) fn cache_len() -> usize {
    CACHE.with(|cache| cache.borrow().entries.len())
}

#[cfg(test)]
pub(crate) fn clear_cache() {
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        cache.entries.clear();
        cache.order.clear();
        cache.bytes = 0;
    });
}

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

pub(crate) struct Thumbnailer {
    events: Receiver<ThumbEvent>,
    tx: Sender<ThumbEvent>,
    next_token: u64,
    /// Tokens that were handed to the worker pool and have not produced a
    /// non-stale event yet. Only these count as pending work.
    live: RefCell<HashMap<u64, Arc<AtomicBool>>>,
    /// Bounded demand that met a full worker queue. Retried when a poll sees
    /// capacity; an unbind/cancel removes the exact token.
    deferred: RefCell<VecDeque<ThumbJob>>,
    #[cfg(test)]
    sent: Arc<std::sync::atomic::AtomicUsize>,
}

impl Thumbnailer {
    pub(crate) fn new() -> Self {
        let (tx, events) = std::sync::mpsc::channel();
        Self {
            events,
            tx,
            next_token: 0,
            live: RefCell::new(HashMap::new()),
            deferred: RefCell::new(VecDeque::new()),
            #[cfg(test)]
            sent: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    pub(crate) fn request(
        &mut self,
        key: ThumbKey,
        source: PathBuf,
        candidates: Vec<ThumbnailSource>,
    ) -> Result<(u64, Scheduled), RequestError> {
        self.next_token = self.next_token.wrapping_add(1);
        let token = self.next_token;
        let cancel = Arc::new(AtomicBool::new(false));
        let live_cancel = cancel.clone();
        let job = ThumbJob {
            token,
            key,
            source,
            candidates,
            cancel,
            tx: self.tx.clone(),
            #[cfg(test)]
            sent: self.sent.clone(),
        };
        match pool().try_send(job) {
            Ok(()) => {
                #[cfg(test)]
                QUEUED.fetch_add(1, Ordering::SeqCst);
                self.live.borrow_mut().insert(token, live_cancel);
                Ok((token, Scheduled::Queued))
            }
            Err(TrySendError::Full(job)) => {
                let mut deferred = self.deferred.borrow_mut();
                if deferred.len() >= THUMB_DEFERRED {
                    return Err(RequestError::QueueFull);
                }
                // A rebind calls cancel first, so one cell cannot accumulate
                // duplicate deferred work.
                deferred.push_back(job);
                Ok((token, Scheduled::Deferred))
            }
            Err(TrySendError::Disconnected(_)) => Err(RequestError::Stopped),
        }
    }

    pub(crate) fn cancel(&self, token: u64) {
        if let Some(cancel) = self.live.borrow_mut().remove(&token) {
            cancel.store(true, Ordering::Relaxed);
            return;
        }
        let position = self
            .deferred
            .borrow()
            .iter()
            .position(|job| job.token == token);
        if let Some(position) = position
            && let Some(job) = self.deferred.borrow_mut().remove(position)
        {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    pub(crate) fn try_next(&self) -> Option<ThumbEvent> {
        loop {
            let event = self.events.try_recv().ok()?;
            if self.live.borrow_mut().remove(&event.token).is_some() {
                return Some(event);
            }
            // A cancelled/unbound token's result consumes only its own work;
            // it can never decrement another request's accounting.
        }
    }

    /// Move deferred demand into the worker pool as capacity opens. Runs from
    /// the view's poll, so no separate timer can outlive the view.
    pub(crate) fn retry_deferred(&self) -> bool {
        let mut progressed = false;
        loop {
            let Some(job) = self.deferred.borrow_mut().pop_front() else {
                break;
            };
            let cancel = job.cancel.clone();
            let token = job.token;
            match pool().try_send(job) {
                Ok(()) => {
                    #[cfg(test)]
                    QUEUED.fetch_add(1, Ordering::SeqCst);
                    self.live.borrow_mut().insert(token, cancel);
                    progressed = true;
                }
                Err(TrySendError::Full(job)) => {
                    self.deferred.borrow_mut().push_front(job);
                    break;
                }
                Err(TrySendError::Disconnected(_)) => {
                    // Nothing can service this demand any more. Dropping the
                    // token lets the view stop polling instead of waiting
                    // forever.
                }
            }
        }
        progressed
    }

    pub(crate) fn has_work(&self) -> bool {
        !self.live.borrow().is_empty() || !self.deferred.borrow().is_empty()
    }

    #[cfg(test)]
    pub(crate) fn live_count(&self) -> usize {
        self.live.borrow().len()
    }

    #[cfg(test)]
    pub(crate) fn deferred_count(&self) -> usize {
        self.deferred.borrow().len()
    }

    #[cfg(test)]
    pub(crate) fn sent(&self) -> usize {
        self.sent.load(Ordering::SeqCst)
    }
}

impl Drop for Thumbnailer {
    fn drop(&mut self) {
        // Closing/replacing the view must cancel queued work too, not only the
        // token map: workers still hold these Arc flags in their channel jobs.
        for (_, cancel) in self.live.borrow_mut().drain() {
            cancel.store(true, Ordering::Relaxed);
        }
        for job in self.deferred.borrow_mut().drain(..) {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk_pixbuf;

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static SEQUENCE: AtomicU64 = AtomicU64::new(0);
            let path = PathBuf::from("/tmp").join(format!(
                "gimbal-thumbnails-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    fn thumbnail(path: &std::path::Path, width: i32, height: i32) -> ThumbnailSource {
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, width, height).unwrap();
        pixbuf.savev(path, "png", &[]).unwrap();
        ThumbnailSource {
            path: path.to_path_buf(),
            size: std::fs::metadata(path).unwrap().len(),
            kind: fm_core::preview::ThumbnailKind::Large,
        }
    }

    fn image_file(path: &std::path::Path, width: i32, height: i32) {
        let pixbuf =
            gdk_pixbuf::Pixbuf::new(gdk_pixbuf::Colorspace::Rgb, false, 8, width, height).unwrap();
        pixbuf.savev(path, "png", &[]).unwrap();
    }

    fn key_for(path: &std::path::Path, width: i32) -> ThumbKey {
        ThumbKey {
            path: path.to_path_buf(),
            revision: FileRevision {
                device: 1,
                inode: 2,
                size: 3,
                modified_sec: 0,
                modified_nsec: 0,
                changed_sec: 0,
                changed_nsec: 0,
                etag: None,
            },
            width,
            height: width,
            kind: ThumbKind::Image,
        }
    }

    fn wait_until(done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the scheduler did not progress"
            );
            std::thread::yield_now();
        }
    }

    fn wait_queue_idle() {
        wait_until(|| QUEUED.load(Ordering::SeqCst) == 0);
    }

    #[test]
    fn a_stale_cancelled_result_cannot_consume_the_next_requests_accounting() {
        let _serial = SCHED_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_cache();
        wait_queue_idle();
        let scratch = Scratch::new();
        let dir_a = scratch.path.join("a");
        let dir_b = scratch.path.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let first = dir_a.join("first.png");
        let second = dir_b.join("second.png");
        image_file(&first, 48, 48);
        image_file(&second, 48, 48);
        let send_gate = set_send_gate(&dir_a);

        let mut thumbnailer = Thumbnailer::new();
        let (first_token, first_state) = thumbnailer
            .request(key_for(&first, 48), first.clone(), Vec::new())
            .unwrap();
        assert_eq!(first_state, Scheduled::Queued);
        // The first worker has decoded A and is held just before sending.
        wait_until(|| send_gate.entered() >= 1);
        thumbnailer.cancel(first_token);
        assert_eq!(thumbnailer.live_count(), 0);

        let (second_token, second_state) = thumbnailer
            .request(key_for(&second, 48), second.clone(), Vec::new())
            .unwrap();
        assert_eq!(second_state, Scheduled::Queued);
        assert_eq!(thumbnailer.live_count(), 1);

        let before = thumbnailer.sent();
        send_gate.release();
        // A's stale event is now in the channel. It must not consume B's
        // pending token: try_next discards it and still yields B.
        wait_until(|| thumbnailer.sent() >= before + 2);
        let mut events = Vec::new();
        while let Some(event) = thumbnailer.try_next() {
            events.push(event);
        }
        assert_eq!(events.len(), 1, "only the live B result is returned");
        assert_eq!(events[0].token, second_token);
        assert!(events[0].image.is_some());
        assert_eq!(thumbnailer.live_count(), 0);
        assert!(!thumbnailer.has_work());
        clear_send_gate();
        wait_queue_idle();
        clear_cache();
    }

    #[test]
    fn saturated_queue_demand_is_deferred_and_retried_when_capacity_opens() {
        let _serial = SCHED_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        clear_cache();
        wait_queue_idle();
        let scratch = Scratch::new();
        let source = scratch.path.join("many.png");
        image_file(&source, 8, 8);
        let gate = image::set_decode_gate(&scratch.path);

        let mut thumbnailer = Thumbnailer::new();
        let mut tokens = Vec::new();
        // Both workers take one job and block in the controlled decode, so the
        // bounded queue then fills deterministically.
        for index in 0..THUMB_WORKERS {
            let (token, state) = thumbnailer
                .request(
                    key_for(&source, 8 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
            assert_eq!(state, Scheduled::Queued);
            tokens.push(token);
        }
        wait_until(|| gate.entered() >= THUMB_WORKERS);
        for index in 0..THUMB_QUEUE {
            let (token, state) = thumbnailer
                .request(
                    key_for(&source, 100 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
            assert_eq!(state, Scheduled::Queued);
            tokens.push(token);
        }
        let (deferred_token, deferred_state) = thumbnailer
            .request(key_for(&source, 900), source.clone(), Vec::new())
            .unwrap();
        assert_eq!(deferred_state, Scheduled::Deferred);
        assert_eq!(thumbnailer.deferred_count(), 1);

        gate.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut deferred_event = None;
        while deferred_event.is_none() {
            thumbnailer.retry_deferred();
            while let Some(event) = thumbnailer.try_next() {
                if event.token == deferred_token {
                    deferred_event = Some(event);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "visible deferred demand never started"
            );
            std::thread::yield_now();
        }
        let event = deferred_event.unwrap();
        assert!(event.image.is_some());
        assert_eq!(event.key.width, 900);
        assert_eq!(thumbnailer.deferred_count(), 0);
        gate.release();
        wait_queue_idle();
        clear_cache();
    }

    #[test]
    fn cancelling_deferred_demand_forgets_exactly_that_cell() {
        let _serial = SCHED_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wait_queue_idle();
        let scratch = Scratch::new();
        let source = scratch.path.join("queued.png");
        image_file(&source, 8, 8);
        let gate = image::set_decode_gate(&scratch.path);
        let mut thumbnailer = Thumbnailer::new();
        let mut tokens = Vec::new();
        for index in 0..THUMB_WORKERS {
            let (token, _) = thumbnailer
                .request(
                    key_for(&source, 40 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
            tokens.push(token);
        }
        wait_until(|| gate.entered() >= THUMB_WORKERS);
        for index in 0..THUMB_QUEUE {
            let (token, _) = thumbnailer
                .request(
                    key_for(&source, 100 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
            tokens.push(token);
        }
        let (deferred_token, state) = thumbnailer
            .request(key_for(&source, 800), source.clone(), Vec::new())
            .unwrap();
        assert_eq!(state, Scheduled::Deferred);
        assert_eq!(thumbnailer.deferred_count(), 1);
        thumbnailer.cancel(deferred_token);
        assert_eq!(thumbnailer.deferred_count(), 0);

        // Rebinding the same visible cell after the cancellation works.
        let (rebound, state) = thumbnailer
            .request(key_for(&source, 801), source.clone(), Vec::new())
            .unwrap();
        assert_eq!(state, Scheduled::Deferred);
        assert_ne!(rebound, deferred_token);

        gate.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        let mut rebound_event = None;
        while rebound_event.is_none() {
            thumbnailer.retry_deferred();
            while let Some(event) = thumbnailer.try_next() {
                assert_ne!(event.token, deferred_token);
                if event.token == rebound {
                    rebound_event = Some(event);
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "rebound cell never rendered"
            );
            std::thread::yield_now();
        }
        assert!(rebound_event.unwrap().image.is_some());
        gate.release();
        wait_queue_idle();
    }

    #[test]
    fn dropping_the_thumbnailer_cancels_queued_work_before_its_decode() {
        let _serial = SCHED_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        wait_queue_idle();
        let scratch = Scratch::new();
        let source = scratch.path.join("drop.png");
        image_file(&source, 8, 8);
        let gate = image::set_decode_gate(&scratch.path);
        let mut thumbnailer = Thumbnailer::new();
        for index in 0..THUMB_WORKERS {
            thumbnailer
                .request(
                    key_for(&source, 20 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
        }
        wait_until(|| gate.entered() >= THUMB_WORKERS);
        for index in 0..THUMB_QUEUE {
            thumbnailer
                .request(
                    key_for(&source, 200 + index as i32),
                    source.clone(),
                    Vec::new(),
                )
                .unwrap();
        }
        assert!(thumbnailer.has_work());
        drop(thumbnailer);
        gate.release();
        wait_queue_idle();
        assert_eq!(
            gate.entered(),
            THUMB_WORKERS,
            "queued work was cancelled before decode"
        );
    }

    #[test]
    fn a_decode_failure_is_reported_once_and_a_cache_hit_skips_the_worker() {
        let scratch = Scratch::new();
        let missing = scratch.path.join("does-not-exist.png");
        let mut thumbnailer = Thumbnailer::new();
        let (token, _) = thumbnailer
            .request(key_for(&missing, 64), missing.clone(), Vec::new())
            .unwrap();
        let event = loop {
            if let Some(event) = thumbnailer.try_next() {
                break event;
            }
            std::thread::yield_now();
        };
        assert_eq!(event.token, token);
        assert!(event.image.is_none());
        assert!(event.failure.is_some());
        assert!(!thumbnailer.has_work());

        // A cache hit is served without a request/binding.
        let key = key_for(&scratch.path.join("cached.png"), 64);
        let image = DecodedImage {
            width: 8,
            height: 8,
            stride: 32,
            bytes: vec![0; 8 * 8 * 4],
            source_width: 8,
            source_height: 8,
        };
        insert(key.clone(), &image);
        assert!(cached(&key).is_some());
        clear_cache();
    }

    #[test]
    fn the_cache_evicts_old_entries_at_its_byte_bound() {
        clear_cache();
        let image = DecodedImage {
            width: 2048,
            height: 2048,
            stride: 2048 * 4,
            bytes: vec![0; 2048 * 2048 * 4],
            source_width: 2048,
            source_height: 2048,
        };
        // 16 MiB each; four fills 64 MiB and a fifth evicts the first.
        for index in 0..5 {
            let key = ThumbKey {
                path: PathBuf::from(format!("/tmp/thumb-{index}.png")),
                revision: FileRevision {
                    device: 1,
                    inode: index,
                    size: 1,
                    modified_sec: 0,
                    modified_nsec: 0,
                    changed_sec: 0,
                    changed_nsec: 0,
                    etag: None,
                },
                width: 2048,
                height: 2048,
                kind: ThumbKind::Image,
            };
            insert(key, &image);
        }
        assert!(cache_bytes() <= MAX_THUMBNAIL_CACHE_BYTES);
        assert_eq!(cache_len(), 4);
        clear_cache();
    }

    #[test]
    fn a_corrupt_or_missing_cached_candidate_falls_through_to_a_readable_one() {
        let scratch = Scratch::new();
        let corrupt = scratch.path.join("corrupt.png");
        std::fs::write(&corrupt, b"not a png").unwrap();
        let good_path = scratch.path.join("good.png");
        let good = thumbnail(&good_path, 256, 256);
        let candidates = vec![
            ThumbnailSource {
                path: corrupt,
                size: 9,
                kind: fm_core::preview::ThumbnailKind::XXLarge,
            },
            ThumbnailSource {
                path: scratch.path.join("missing.png"),
                size: 0,
                kind: fm_core::preview::ThumbnailKind::XLarge,
            },
            good,
        ];
        let image = decode_cached(
            &candidates,
            RenderSize {
                width: 96,
                height: 96,
            },
        )
        .expect("the last readable candidate wins");
        assert_eq!(image.source_width.max(image.source_height), 256);
    }

    #[test]
    fn cache_keys_include_revision_and_kind() {
        let base = ThumbKey {
            path: PathBuf::from("/tmp/a.png"),
            revision: FileRevision {
                device: 1,
                inode: 2,
                size: 3,
                modified_sec: 4,
                modified_nsec: 0,
                changed_sec: 5,
                changed_nsec: 0,
                etag: Some("etag".to_string()),
            },
            width: 100,
            height: 100,
            kind: ThumbKind::Image,
        };
        let mut changed = base.clone();
        changed.revision.size = 4;
        assert_ne!(base, changed);
        let mut kind = base.clone();
        kind.kind = ThumbKind::Pdf;
        assert_ne!(base, kind);
        let mut dimensions = base.clone();
        dimensions.width = 101;
        assert_ne!(base, dimensions);
    }
}
