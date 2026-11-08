// SPDX-License-Identifier: GPL-3.0-or-later
//! The right sidebar's preview: selection-driven text/image/media states,
//! line numbers, Alt-click editing, Save/Save As/Discard and the close guard.
//!
//! One panel owns one worker thread. File reads, image decode, destination
//! checks and saves happen there; the main thread only applies typed results
//! whose generation still matches. `PreviewSession` in `fm-core` owns the
//! selection/dirty/stale transitions.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

use adw::prelude::*;
use fm_core::command::{Caller, Invocation};
use fm_core::preview::{
    self, CloseRequest, Destination, FileRevision, LoadTicket, Loaded, PdfRenderRequest,
    PreviewDocument, PreviewSession, RenderSize, SaveAsRoute, SaveCompletion, SaveError,
    SaveRequest, SaveSuccess, SaveTarget, SaveTicket, Selection, ViewState,
};
use fm_core::workspace::WindowId;
use gtk::{gdk, gio, glib, pango};

use crate::app::Process;
use crate::image::{self, DecodedImage, ImageProblem};
use crate::look::icons::{self, MockupIcon};
use crate::look::tokens::*;

/// Base durations (before the motion setting) for Save As leaving and
/// returning in administrator edit mode. The fade matches `.gl-fade` in the
/// look module's stylesheet.
const SAVE_AS_FADE_MS: u32 = 120;
const SAVE_AS_SLIDE_MS: u32 = 160;
use crate::pdf::{self, PdfFailure, PdfOutcome};

const POLL_MS: u64 = 30;
const MONITOR_DEBOUNCE_MS: u64 = 250;
const STATUS_MS: u64 = 7000;
const RENDER_DEBOUNCE_MS: u64 = 120;
const PDF_RETRY_MS: u64 = 50;
static NEXT_PDF_OWNER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// How the visual preview maps source pixels onto logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ZoomMode {
    /// Contain the source in the panel (images).
    Fit,
    /// Fill the panel width and scroll vertically (PDF page 1).
    FitWidth,
    /// One source pixel per logical pixel.
    Actual,
    /// A manual factor of the source size; 1.0 is Actual.
    Manual(f64),
}

impl ZoomMode {
    fn label(self) -> String {
        match self {
            Self::Fit => "Fit".to_string(),
            Self::FitWidth => "Fit width".to_string(),
            Self::Actual => "100%".to_string(),
            Self::Manual(factor) => format!("{}%", (factor * 100.0).round() as i32),
        }
    }

    fn factor(self, source: (i32, i32), viewport: (i32, i32)) -> f64 {
        match self {
            Self::Fit => {
                if source.0 <= 0 || source.1 <= 0 || viewport.0 <= 0 || viewport.1 <= 0 {
                    1.0
                } else {
                    (f64::from(viewport.0) / f64::from(source.0))
                        .min(f64::from(viewport.1) / f64::from(source.1))
                        .min(1.0)
                }
            }
            Self::FitWidth => {
                if source.0 <= 0 || viewport.0 <= 0 {
                    1.0
                } else {
                    f64::from(viewport.0) / f64::from(source.0)
                }
            }
            Self::Actual => 1.0,
            Self::Manual(factor) => factor,
        }
    }
}

// ---------------------------------------------------------------------------
// Worker jobs and results
// ---------------------------------------------------------------------------

enum ImageJob {
    Load {
        generation: u64,
        path: PathBuf,
        target: RenderSize,
        cancel: Arc<AtomicBool>,
    },
    RenderImage {
        generation: u64,
        path: PathBuf,
        target: RenderSize,
        cancel: Arc<AtomicBool>,
    },
}

enum ControlJob {
    Save { id: u64, request: SaveRequest },
    Check { generation: u64, path: PathBuf },
    Destination { id: u64, path: PathBuf },
}

impl ImageJob {
    fn cancel(&self) -> &Arc<AtomicBool> {
        match self {
            Self::Load { cancel, .. } | Self::RenderImage { cancel, .. } => cancel,
        }
    }
}

/// One-slot, newest-wins queue for expensive preview reads/decodes. A new job
/// replaces (and cancels) a job that has not started, so a zoom/selection
/// burst cannot build an unbounded backlog. At most one job runs and one waits,
/// and the bounded event channel caps decoded result memory.
struct QueueState<T> {
    slot: Option<T>,
    closed: bool,
}

/// One-slot, newest-wins queue for expensive preview reads/decodes. A new job
/// replaces (and aborts) a job that has not started, so a zoom/selection burst
/// cannot build an unbounded backlog. At most one job runs and one waits, and
/// the bounded event channel caps decoded result memory.
struct LatestQueue<T> {
    state: std::sync::Mutex<QueueState<T>>,
    ready: std::sync::Condvar,
    outstanding: Arc<std::sync::atomic::AtomicUsize>,
}

impl<T> LatestQueue<T> {
    fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(QueueState {
                slot: None,
                closed: false,
            }),
            ready: std::sync::Condvar::new(),
            outstanding: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }

    /// Replace the pending job. `Ok(Some(job))` is a never-started job the
    /// caller must cancel; `Err(job)` means the queue is closed.
    fn submit(&self, job: T) -> Result<Option<T>, T> {
        let previous = {
            let mut state = self.state.lock().expect("the preview queue lock");
            if state.closed {
                return Err(job);
            }
            let previous = state.slot.replace(job);
            if previous.is_some() {
                self.outstanding
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            self.outstanding
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            previous
        };
        self.ready.notify_one();
        Ok(previous)
    }

    fn take(&self) -> Option<T> {
        let mut state = self.state.lock().expect("the preview queue lock");
        loop {
            if let Some(job) = state.slot.take() {
                return Some(job);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .expect("the preview queue lock is not poisoned");
        }
    }

    /// Close the queue and return any never-started job so its owner can set
    /// the job's own cancellation flag.
    fn close(&self) -> Option<T> {
        let pending = {
            let mut state = self.state.lock().expect("the preview queue lock");
            state.closed = true;
            let pending = state.slot.take();
            if pending.is_some() {
                self.outstanding
                    .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            }
            pending
        };
        self.ready.notify_all();
        pending
    }

    fn outstanding(&self) -> usize {
        self.outstanding.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn complete(&self) {
        self.outstanding
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[allow(clippy::large_enum_variant)]
enum Event {
    Loaded {
        generation: u64,
        path: PathBuf,
        loaded: Loaded,
        image: Option<Result<DecodedImage, ImageProblem>>,
        fallback: Option<Result<DecodedImage, ImageProblem>>,
    },
    Rendered {
        generation: u64,
        path: PathBuf,
        result: Result<DecodedImage, ImageProblem>,
    },
    Saved {
        id: u64,
        result: Result<SaveSuccess, SaveError>,
    },
    Checked {
        generation: u64,
        path: PathBuf,
        result: Result<FileRevision, SaveError>,
    },
    Destination {
        id: u64,
        path: PathBuf,
        result: Result<Destination, SaveError>,
    },
}

fn buffer_text(buffer: &gtk::TextBuffer) -> String {
    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.text(&start, &end, true).to_string()
}

/// The owner recorded when a document is loaded. A text preview is always a
/// regular file, so following the path's final metadata is enough.
fn owner_of_path(path: &Path) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|metadata| metadata.uid())
}

struct PreviewWorkers {
    image: Arc<LatestQueue<ImageJob>>,
    image_events: Receiver<Event>,
    control: Sender<ControlJob>,
    control_events: Receiver<Event>,
    control_outstanding: Arc<std::sync::atomic::AtomicUsize>,
}

fn spawn_workers() -> PreviewWorkers {
    let image = Arc::new(LatestQueue::new());
    let (image_events_tx, image_events) = std::sync::mpsc::sync_channel::<Event>(1);
    {
        let queue = image.clone();
        std::thread::Builder::new()
            .name("gimbal-preview-image".to_string())
            .spawn(move || image_worker(queue, image_events_tx))
            .expect("the preview image worker starts");
    }

    let (control, control_jobs) = std::sync::mpsc::channel::<ControlJob>();
    let (control_events_tx, control_events) = std::sync::mpsc::sync_channel::<Event>(8);
    let control_outstanding = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    {
        let outstanding = control_outstanding.clone();
        std::thread::Builder::new()
            .name("gimbal-preview-save".to_string())
            .spawn(move || control_worker(control_jobs, control_events_tx, outstanding))
            .expect("the preview save worker starts");
    }

    PreviewWorkers {
        image,
        image_events,
        control,
        control_events,
        control_outstanding,
    }
}

fn image_worker(queue: Arc<LatestQueue<ImageJob>>, events: std::sync::mpsc::SyncSender<Event>) {
    crate::log!("preview", "image worker started");
    while let Some(job) = queue.take() {
        let cancel = job.cancel().clone();
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            queue.complete();
            continue;
        }
        let event = match job {
            ImageJob::Load {
                generation,
                path,
                target,
                ..
            } => {
                let loaded = preview::load(&path);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    queue.complete();
                    continue;
                }
                let mut image = None;
                let mut fallback = None;
                if let Loaded::Document(document) = &loaded {
                    match &document.content {
                        fm_core::preview::PreviewContent::Image(_) => {
                            image = Some(image::decode_image_file(&path, target, true));
                        }
                        fm_core::preview::PreviewContent::Media(media) => {
                            image = decode_candidates(&media.thumbnails, target);
                        }
                        fm_core::preview::PreviewContent::Pdf(pdf) => {
                            fallback = decode_candidates(&pdf.thumbnails, target);
                        }
                        fm_core::preview::PreviewContent::Text(_) => {}
                    }
                }
                Event::Loaded {
                    generation,
                    path,
                    loaded,
                    image,
                    fallback,
                }
            }
            ImageJob::RenderImage {
                generation,
                path,
                target,
                ..
            } => {
                let result = image::decode_image_file(&path, target, true);
                if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    queue.complete();
                    continue;
                }
                Event::Rendered {
                    generation,
                    path,
                    result,
                }
            }
        };
        let sent = events.send(event).is_ok();
        queue.complete();
        if !sent {
            return;
        }
    }
}

fn control_worker(
    jobs: Receiver<ControlJob>,
    events: std::sync::mpsc::SyncSender<Event>,
    outstanding: Arc<std::sync::atomic::AtomicUsize>,
) {
    crate::log!("preview", "save worker started");
    while let Ok(job) = jobs.recv() {
        let event = match job {
            ControlJob::Save { id, request } => {
                let result = preview::save(&request);
                Event::Saved { id, result }
            }
            ControlJob::Check { generation, path } => {
                let result = preview::inspect_revision(&path);
                Event::Checked {
                    generation,
                    path,
                    result,
                }
            }
            ControlJob::Destination { id, path } => {
                let result = preview::inspect_destination(&path);
                Event::Destination { id, path, result }
            }
        };
        let sent = events.send(event).is_ok();
        outstanding.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        if !sent {
            return;
        }
    }
}

/// Try every valid cache size, largest first. A corrupt or missing candidate
/// falls through; the first one with enough decoded detail wins.
fn decode_candidates(
    candidates: &[preview::ThumbnailSource],
    target: RenderSize,
) -> Option<Result<DecodedImage, ImageProblem>> {
    let mut best: Option<Result<DecodedImage, ImageProblem>> = None;
    for candidate in candidates {
        match image::decode_image_file(&candidate.path, target, false) {
            Ok(image) => {
                let enough = image.source_width.max(image.source_height)
                    >= target.width.min(target.height).max(64);
                if enough {
                    return Some(Ok(image));
                }
                if best.as_ref().is_none_or(|current| match current {
                    Ok(current) => {
                        current.source_width * current.source_height
                            < image.source_width * image.source_height
                    }
                    Err(_) => true,
                }) {
                    best = Some(Ok(image));
                }
            }
            Err(problem) => {
                if best.is_none() {
                    best = Some(Err(problem));
                }
            }
        }
    }
    best
}

// ---------------------------------------------------------------------------
// Panel
// ---------------------------------------------------------------------------

struct Monitors {
    _file: gio::FileMonitor,
    _parent: gio::FileMonitor,
}

/// One debounce source per panel. A one-shot source must not be removed after
/// it fired (`glib-0.22.10:src/source.rs:29-35` unwraps a failed removal): the
/// callback takes its own stored handle first. Generation checks keep an
/// obsolete callback from clearing a newer timer.
struct MonitorTimer {
    source: RefCell<Option<glib::SourceId>>,
    generation: Cell<u64>,
}

impl MonitorTimer {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            source: RefCell::new(None),
            generation: Cell::new(0),
        })
    }

    fn generation(&self) -> u64 {
        self.generation.get()
    }

    fn schedule(self: &Rc<Self>, delay: Duration, fire: impl FnOnce(u64) + 'static) {
        self.cancel();
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(delay, move || {
            let Some(timer) = weak.upgrade() else {
                return;
            };
            if timer.generation.get() != generation {
                // This one-shot is obsolete; the stored handle belongs to a
                // newer source and must stay untouched.
                return;
            }
            // Take the expired handle. Dropping it is correct; calling
            // SourceId::remove here would panic.
            let _ = timer.source.borrow_mut().take();
            fire(generation);
        });
        *self.source.borrow_mut() = Some(id);
    }

    fn cancel(&self) {
        self.generation.set(self.generation.get().wrapping_add(1));
        if let Some(id) = self.source.borrow_mut().take() {
            id.remove();
        }
    }
}

#[derive(Clone)]
struct PendingSave {
    ticket: SaveTicket,
    request: SaveRequest,
}

/// One close/quit continuation. The document/revision lives in the session's
/// `CloseRequest`; this pair runs only after that request is satisfied.
struct CloseWait {
    proceed: Rc<dyn Fn()>,
    cancel: Rc<dyn Fn()>,
}

/// What one close request needs from the preview panel. `Busy` means an
/// ordinary close already owns the panel; a Quit caller must cancel its own
/// attempt without touching that dialog or continuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CloseGuard {
    Free,
    Waiting,
    Busy,
}

/// The idle decision after a save succeeded. Only a close request that names
/// this exact save job may proceed or be aborted; a newer request is left
/// alone.
#[derive(Clone, Debug, PartialEq, Eq)]
enum SavedClose {
    Proceed(CloseRequest),
    Abort,
    None,
}

/// The production guard decision, separated from widget/dialog presentation so
/// it can be driven with a `RefCell<PreviewSession>` in tests.
pub(crate) fn close_guard_decision(
    session: &RefCell<PreviewSession>,
    wait_present: bool,
) -> CloseGuard {
    {
        let session = session.borrow();
        if session.has_close() || wait_present {
            return CloseGuard::Busy;
        }
        if !session.is_dirty() && !session.saving() {
            return CloseGuard::Free;
        }
    }
    session.borrow_mut().invalidate_actions();
    if session.borrow_mut().arm_close().is_ok() {
        CloseGuard::Waiting
    } else {
        CloseGuard::Free
    }
}

/// Confirm a validated AwaitingReplace request, then start it. Each session
/// borrow ends before the next call, so a `RefCell` re-entry cannot panic.
fn confirm_and_start_save_as(
    session: &RefCell<PreviewSession>,
    id: u64,
) -> Result<SaveTicket, String> {
    let confirmed = session.borrow_mut().confirm_save_as(id);
    match confirmed {
        Ok(_) => session.borrow_mut().start_save_as(id),
        Err(message) => Err(message),
    }
}

enum DestinationAction {
    StartSave(SaveTicket),
    Replace(FileRevision),
    Cancel { message: String, warning: bool },
    Ignore(String),
}

/// Apply one destination result. A missing destination is confirmed without a
/// Replace dialog, then started. A failure cancels that exact request so it
/// cannot keep blocking later Save/Save As calls.
fn apply_destination_reply(
    session: &RefCell<PreviewSession>,
    id: u64,
    path: &Path,
    result: Result<Destination, SaveError>,
) -> DestinationAction {
    let revision = match &result {
        Ok(Destination::Existing { revision, .. }) => Some(revision.clone()),
        _ => None,
    };
    if let Err(message) = session.borrow_mut().save_as_destination(id, path, revision) {
        return DestinationAction::Ignore(message);
    }
    match result {
        Ok(Destination::Missing) => match confirm_and_start_save_as(session, id) {
            Ok(ticket) => DestinationAction::StartSave(ticket),
            Err(message) => {
                session.borrow_mut().cancel_save_as(id);
                DestinationAction::Cancel {
                    message,
                    warning: true,
                }
            }
        },
        Ok(Destination::Existing {
            revision,
            writable,
            reason,
        }) => {
            if writable {
                DestinationAction::Replace(revision)
            } else {
                session.borrow_mut().cancel_save_as(id);
                DestinationAction::Cancel {
                    message: reason
                        .map(|reason| reason.message().to_string())
                        .unwrap_or_else(|| "The destination is not writable.".to_string()),
                    warning: true,
                }
            }
        }
        Err(error) => {
            session.borrow_mut().cancel_save_as(id);
            DestinationAction::Cancel {
                message: error.message(),
                warning: true,
            }
        }
    }
}

/// Whether a close request or wait belongs to this exact save job. Used by a
/// late save result so it can only retire its own continuation.
fn close_owned_by_save(session: &PreviewSession, wait_present: bool, ticket: &SaveTicket) -> bool {
    match session.close_request() {
        Some(close) => close.save == Some(ticket.id) && close.token == ticket.token,
        None => wait_present,
    }
}

/// Decide whether a completed save may still run its close continuation. A
/// different close request (for example one opened after this save started) is
/// not ours and must be left untouched.
fn saved_close_decision(
    session: &PreviewSession,
    wait_present: bool,
    ticket: &SaveTicket,
) -> SavedClose {
    let Some(close) = session.close_request() else {
        return if wait_present {
            SavedClose::Abort
        } else {
            SavedClose::None
        };
    };
    let belongs_to_ticket = close.save == Some(ticket.id) && close.token == ticket.token;
    if !belongs_to_ticket {
        return SavedClose::None;
    }
    if wait_present && session.close_satisfied_by(ticket) && session.close_ready(close) {
        SavedClose::Proceed(close.clone())
    } else {
        SavedClose::Abort
    }
}

/// The production idle step: revalidate the accepted request, then consume it
/// only when no fresh edit or newer save exists. `before_callback` dismisses a
/// waiting dialog after the session state is settled and before the caller's
/// continuation can destroy the window.
fn complete_saved_close(
    session: &RefCell<PreviewSession>,
    wait: &RefCell<Option<CloseWait>>,
    ticket: &SaveTicket,
    before_callback: impl FnOnce(),
) -> SavedClose {
    let decision = saved_close_decision(&session.borrow(), wait.borrow().is_some(), ticket);
    match &decision {
        SavedClose::Proceed(_) => {
            let wait = wait.borrow_mut().take();
            let taken = session.borrow_mut().take_close();
            before_callback();
            if taken.is_some()
                && let Some(wait) = wait
            {
                (wait.proceed)();
            }
        }
        SavedClose::Abort => {
            let wait = wait.borrow_mut().take();
            session.borrow_mut().invalidate_close();
            before_callback();
            if let Some(wait) = wait {
                (wait.cancel)();
            }
        }
        SavedClose::None => {}
    }
    decision
}

pub struct PreviewPanel {
    root: gtk::Box,
    title: gtk::Label,
    notice: gtk::Label,
    admin_banner: gtk::Box,
    admin_banner_label: gtk::Label,
    stack: gtk::Stack,
    message: gtk::Label,
    text_page: gtk::Box,
    text_scroll: gtk::ScrolledWindow,
    gutter: gtk::DrawingArea,
    text_view: gtk::TextView,
    buffer: gtk::TextBuffer,
    image_page: gtk::Box,
    image_scroll: gtk::ScrolledWindow,
    picture: gtk::Picture,
    zoom_label: gtk::Label,
    zoom_fit: gtk::Button,
    zoom_actual: gtk::Button,
    zoom_out: gtk::Button,
    zoom_in: gtk::Button,
    status: gtk::Label,
    actions: gtk::Box,
    discard: gtk::Button,
    save: gtk::Button,
    save_as: gtk::Button,
    /// Holds Save As so its slot can collapse: Save then slides into its place
    /// while editing as administrator (Kyle, Nov 6).
    save_as_reveal: gtk::Revealer,
    /// Whether Save As is (or is becoming) shown, and a generation that lets a
    /// newer fade/slide cancel an older one's pending second step.
    save_as_shown: Cell<bool>,
    save_as_generation: Cell<u64>,

    process: Weak<Process>,
    window: WindowId,
    self_weak: RefCell<Weak<Self>>,
    session: RefCell<PreviewSession>,
    workers: PreviewWorkers,
    poll: RefCell<Option<glib::SourceId>>,
    scale: Cell<f64>,
    buffer_revision: Cell<u64>,
    applying: Cell<bool>,
    chooser_reply: Cell<Option<u64>>,
    pending_save: RefCell<Option<PendingSave>>,
    close_wait: RefCell<Option<CloseWait>>,
    close_dialog: RefCell<Option<adw::AlertDialog>>,
    close_dialog_open: Cell<bool>,
    allow_close: Cell<bool>,
    change_warning: Cell<bool>,
    change_dialog_open: Cell<bool>,
    admin_warning_open: Cell<bool>,
    owner_uid: Cell<Option<u32>>,
    current_uid: Cell<Option<u32>>,
    status_generation: Cell<u64>,
    monitors: RefCell<Option<Monitors>>,
    monitor_timer: Rc<MonitorTimer>,

    compact_title: Cell<bool>,
    notice_animation: RefCell<Option<adw::Animation>>,
    notice_generation: Cell<u64>,
    zoom: Cell<ZoomMode>,
    source_size: Cell<Option<(i32, i32)>>,
    render_generation: Cell<u64>,
    /// Cancellation for the current or most recent image read/decode. A new
    /// load/render sets it before replacing the pending job, so the worker can
    /// skip the expensive part or the stale send as soon as it checks.
    image_cancel: RefCell<Option<Arc<AtomicBool>>>,
    /// The path whose load is in flight, for the unmount release step.
    loading_path: RefCell<Option<PathBuf>>,
    /// The honest longest-edge cap for the current document. PDF rasterization
    /// is bounded by a conservative square, so its display cap is lower than
    /// the image cap until the actual page geometry is known.
    render_edge_cap: Cell<i32>,
    render_timer: Rc<MonitorTimer>,
    last_render_viewport: Cell<(i32, i32)>,
    fallback: RefCell<Option<(u64, DecodedImage)>>,
    render_limit: RefCell<Option<String>>,
    pdf_tx: Sender<PdfOutcome>,
    pdf_events: Receiver<PdfOutcome>,
    pdf_cancel: RefCell<Option<Arc<AtomicBool>>>,
    pdf_pending: Cell<u32>,
    pdf_owner: u64,
    pdf_retry: Rc<MonitorTimer>,
}

impl PreviewPanel {
    pub fn new(
        process: &Rc<Process>,
        window: WindowId,
        scale: f64,
        title: &gtk::Label,
    ) -> Rc<Self> {
        title.set_wrap(true);
        title.set_wrap_mode(pango::WrapMode::WordChar);
        title.set_lines(0);
        title.set_ellipsize(pango::EllipsizeMode::None);
        title.set_xalign(0.0);
        // Width floor, not a text limit: the header gets the sidebar width
        // and wraps there instead of imposing a wider minimum.
        title.set_width_chars(1);
        title.set_max_width_chars(1);
        let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
        root.set_vexpand(true);
        root.set_hexpand(true);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.set_hexpand(true);
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_transition_duration(crate::look::motion::ms(180));

        let message = gtk::Label::builder()
            .label("Select a file to preview")
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .justify(gtk::Justification::Center)
            .wrap(true)
            .vexpand(true)
            .hexpand(true)
            .build();
        message.add_css_class("gl-preview-note");

        let gutter = gtk::DrawingArea::new();
        gutter.add_css_class("gl-num");
        gutter.add_css_class("gl-mono");
        gutter.set_width_request(scaled(PREVIEW_GUTTER, scale));
        gutter.set_hexpand(false);
        gutter.set_vexpand(true);
        gutter.set_valign(gtk::Align::Fill);

        let buffer = gtk::TextBuffer::new(None);
        let text_view = gtk::TextView::with_buffer(&buffer);
        text_view.add_css_class("gl-text");
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_editable(false);
        text_view.set_cursor_visible(false);
        text_view.set_left_margin(scaled(GAP_S, scale));
        text_view.set_right_margin(scaled(GAP_S, scale));
        text_view.set_top_margin(scaled(GAP_S, scale));
        text_view.set_bottom_margin(scaled(GAP_S, scale));
        text_view.set_pixels_below_lines(scaled(2.0, scale));
        text_view.set_pixels_inside_wrap(scaled(2.0, scale));

        let text_scroll = gtk::ScrolledWindow::new();
        text_scroll.set_policy(gtk::PolicyType::External, gtk::PolicyType::Automatic);
        text_scroll.set_propagate_natural_width(false);
        text_scroll.set_vexpand(true);
        text_scroll.set_hexpand(true);
        text_scroll.set_child(Some(&text_view));

        let text_page = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale));
        text_page.set_vexpand(true);
        text_page.set_hexpand(true);
        text_page.append(&gutter);
        text_page.append(&text_scroll);

        let picture = gtk::Picture::new();
        picture.set_can_shrink(true);
        picture.set_content_fit(gtk::ContentFit::Contain);
        picture.set_hexpand(false);
        picture.set_vexpand(false);
        picture.set_halign(gtk::Align::Center);
        picture.set_valign(gtk::Align::Center);
        // Never let a decoded image's natural width impose a sidebar minimum.
        picture.set_size_request(1, 1);

        let image_scroll = gtk::ScrolledWindow::new();
        image_scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
        image_scroll.set_vexpand(true);
        image_scroll.set_hexpand(true);
        image_scroll.set_propagate_natural_width(false);
        image_scroll.set_propagate_natural_height(false);
        image_scroll.set_child(Some(&picture));

        let zoom_label = gtk::Label::new(Some("Fit"));
        zoom_label.add_css_class("gl-preview-note");
        zoom_label.set_hexpand(true);
        zoom_label.set_xalign(0.0);
        zoom_label.set_ellipsize(pango::EllipsizeMode::End);
        let zoom_fit = gtk::Button::with_label("Fit");
        let zoom_actual = gtk::Button::with_label("100%");
        let zoom_out = gtk::Button::with_label("−");
        let zoom_in = gtk::Button::with_label("+");
        for button in [&zoom_fit, &zoom_actual, &zoom_out, &zoom_in] {
            button.add_css_class("gl-menu-item");
            button.set_valign(gtk::Align::Center);
        }
        zoom_fit.set_tooltip_text(Some("Fit the whole image; PDFs fit the page width"));
        zoom_actual.set_tooltip_text(Some("Show at 100% (one source pixel per logical pixel)"));
        zoom_out.set_tooltip_text(Some("Zoom out"));
        zoom_in.set_tooltip_text(Some("Zoom in"));
        let zoom_row = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale));
        zoom_row.append(&zoom_label);
        zoom_row.append(&zoom_out);
        zoom_row.append(&zoom_fit);
        zoom_row.append(&zoom_actual);
        zoom_row.append(&zoom_in);

        let image_page = gtk::Box::new(gtk::Orientation::Vertical, scaled(GAP_S, scale));
        image_page.set_vexpand(true);
        image_page.set_hexpand(true);
        image_page.append(&image_scroll);
        image_page.append(&zoom_row);

        stack.add_named(&message, Some("message"));
        stack.add_named(&text_page, Some("text"));
        stack.add_named(&image_page, Some("image"));
        stack.set_visible_child_name("message");

        let notice = gtk::Label::new(None);
        notice.add_css_class("gl-preview-notice");
        notice.set_xalign(0.0);
        notice.set_wrap(true);
        notice.set_wrap_mode(pango::WrapMode::WordChar);
        notice.set_hexpand(true);
        notice.set_width_chars(1);
        notice.set_max_width_chars(1);
        notice.set_visible(false);

        // The administrator mode banner. It reuses the theme's error colours
        // from P2z's administrator look; no fixed colour lives here.
        let admin_banner = gtk::Box::new(gtk::Orientation::Horizontal, scaled(GAP_S, scale));
        admin_banner.add_css_class("gl-preview-admin-banner");
        admin_banner.set_visible(false);
        let admin_lock = gtk::Image::from_icon_name(icons::ADMIN_LOCK_ICON);
        admin_lock.set_valign(gtk::Align::Center);
        let admin_banner_label = gtk::Label::new(None);
        admin_banner_label.set_xalign(0.0);
        admin_banner_label.set_wrap(true);
        admin_banner_label.set_wrap_mode(pango::WrapMode::WordChar);
        admin_banner_label.set_hexpand(true);
        admin_banner_label.set_width_chars(1);
        admin_banner_label.set_max_width_chars(1);
        admin_banner.append(&admin_lock);
        admin_banner.append(&admin_banner_label);

        let status = gtk::Label::new(None);
        status.add_css_class("gl-preview-note");
        status.set_xalign(0.0);
        status.set_wrap(true);
        status.set_visible(false);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        actions.add_css_class("gl-action-row");
        let discard = icons::button(MockupIcon::Discard);
        let save = icons::button(MockupIcon::Save);
        let save_as = icons::button(MockupIcon::SaveAs);
        for (button, label) in [
            (&discard, "Discard changes"),
            (&save, "Save changes"),
            (&save_as, "Save As"),
        ] {
            button.set_tooltip_text(Some(label));
            button.update_property(&[gtk::accessible::Property::Label(label)]);
        }
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        actions.append(&discard);
        actions.append(&spacer);
        actions.append(&save);
        save_as.add_css_class("gl-fade");
        let save_as_reveal = gtk::Revealer::new();
        save_as_reveal.set_transition_type(gtk::RevealerTransitionType::SlideLeft);
        save_as_reveal.set_transition_duration(crate::look::motion::ms(SAVE_AS_SLIDE_MS));
        save_as_reveal.set_child(Some(&save_as));
        save_as_reveal.set_reveal_child(true);
        actions.append(&save_as_reveal);

        root.append(&admin_banner);
        root.append(&stack);
        root.append(&notice);
        root.append(&status);
        let rule = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rule.add_css_class("gl-rule");
        root.append(&rule);
        root.append(&actions);

        let workers = spawn_workers();
        let (pdf_tx, pdf_events) = std::sync::mpsc::channel();
        let panel = Rc::new(Self {
            root,
            title: title.clone(),
            notice,
            admin_banner,
            admin_banner_label,
            stack,
            message,
            text_page,
            text_scroll,
            gutter,
            text_view,
            buffer,
            image_page,
            image_scroll,
            picture,
            zoom_label,
            zoom_fit,
            zoom_actual,
            zoom_out,
            zoom_in,
            status,
            actions,
            discard,
            save,
            save_as,
            save_as_reveal,
            save_as_shown: Cell::new(true),
            save_as_generation: Cell::new(0),
            process: Rc::downgrade(process),
            window,
            self_weak: RefCell::new(Weak::new()),
            session: RefCell::new(PreviewSession::new()),
            workers,
            poll: RefCell::new(None),
            scale: Cell::new(scale),
            buffer_revision: Cell::new(0),
            applying: Cell::new(false),
            chooser_reply: Cell::new(None),
            pending_save: RefCell::new(None),
            close_wait: RefCell::new(None),
            close_dialog: RefCell::new(None),
            close_dialog_open: Cell::new(false),
            allow_close: Cell::new(false),
            change_warning: Cell::new(false),
            change_dialog_open: Cell::new(false),
            admin_warning_open: Cell::new(false),
            owner_uid: Cell::new(None),
            current_uid: Cell::new(fm_core::admin::current_uid().ok()),
            status_generation: Cell::new(0),
            monitors: RefCell::new(None),
            monitor_timer: MonitorTimer::new(),
            compact_title: Cell::new(false),
            notice_animation: RefCell::new(None),
            notice_generation: Cell::new(0),
            zoom: Cell::new(ZoomMode::Fit),
            source_size: Cell::new(None),
            render_generation: Cell::new(0),
            image_cancel: RefCell::new(None),
            loading_path: RefCell::new(None),
            render_edge_cap: Cell::new(preview::MAX_RENDER_EDGE),
            render_timer: MonitorTimer::new(),
            last_render_viewport: Cell::new((0, 0)),
            fallback: RefCell::new(None),
            render_limit: RefCell::new(None),
            pdf_tx,
            pdf_events,
            pdf_cancel: RefCell::new(None),
            pdf_pending: Cell::new(0),
            pdf_owner: NEXT_PDF_OWNER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            pdf_retry: MonitorTimer::new(),
        });
        *panel.self_weak.borrow_mut() = Rc::downgrade(&panel);
        panel.wire();
        panel.show_message("Select a file to preview");
        panel.update_title();
        panel.update_actions();
        panel
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.root
    }

    pub fn text_focused(&self) -> bool {
        self.text_view.has_focus()
    }

    pub fn activity(&self) -> Option<fm_core::preview::EditorActivity> {
        self.session.borrow().activity()
    }

    /// The filename of a dirty editor whose file is under one mount root.
    pub fn dirty_file_name_under(&self, root: &Path) -> Option<String> {
        let session = self.session.borrow();
        if !session.is_dirty() {
            return None;
        }
        let document = session.current_document()?;
        document.path.starts_with(root).then(|| {
            document
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| document.path.display().to_string())
        })
    }

    /// Cancel preview image/PDF work and external monitors for a document
    /// under one mount root. The visible buffer is left alone; the unmount
    /// path has already refused a dirty editor.
    pub fn release_root(&self, root: &Path) -> bool {
        let current = self
            .session
            .borrow()
            .current_document()
            .map(|document| document.path.clone());
        let loading = self.loading_path.borrow().clone();
        let under_root = |path: &PathBuf| path.starts_with(root);
        if !current.as_ref().is_some_and(under_root) && !loading.as_ref().is_some_and(under_root) {
            return false;
        }
        if let Some(cancel) = self.image_cancel.borrow_mut().take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.loading_path.borrow_mut().take();
        self.cancel_pdf_render();
        self.render_timer.cancel();
        self.monitor_timer.cancel();
        self.clear_monitors();
        true
    }

    pub fn allow_close(&self) {
        self.allow_close.set(true);
    }

    pub fn close_allowed(&self) -> bool {
        self.allow_close.get()
    }

    pub fn apply_scale(self: &Rc<Self>, scale: f64) {
        self.scale.set(scale);
        self.gutter.set_width_request(scaled(PREVIEW_GUTTER, scale));
        self.text_view.set_left_margin(scaled(GAP_S, scale));
        self.text_view.set_right_margin(scaled(GAP_S, scale));
        self.text_view.set_top_margin(scaled(GAP_S, scale));
        self.text_view.set_bottom_margin(scaled(GAP_S, scale));
        self.text_page.set_spacing(scaled(GAP_S, scale));
        self.image_page.set_spacing(scaled(GAP_S, scale));
        self.actions.set_spacing(scaled(GAP_S, scale));
        self.gutter.queue_draw();
        self.picture.set_size_request(
            1.max(self.displayed_size().0),
            1.max(self.displayed_size().1),
        );
        self.schedule_render();
    }

    // -----------------------------------------------------------------------
    // Wiring
    // -----------------------------------------------------------------------

    fn wire(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.gutter.set_draw_func(move |area, cr, width, height| {
            if let Some(panel) = weak.upgrade() {
                panel.draw_gutter(area, cr, width, height);
            }
        });

        let weak = Rc::downgrade(self);
        self.buffer.connect_changed(move |_| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            if panel.applying.get() {
                return;
            }
            let revision = panel.buffer_revision.get().wrapping_add(1);
            panel.buffer_revision.set(revision);
            panel.session.borrow_mut().mark_dirty(revision);
            panel.gutter.queue_draw();
            panel.update_title();
            panel.update_actions();
        });

        let weak = Rc::downgrade(self);
        self.text_scroll
            .vadjustment()
            .connect_value_changed(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.gutter.queue_draw();
                }
            });

        // A capture-phase legacy controller observes Alt+click without
        // claiming the sequence, so ordinary clicks still move the caret and
        // select text in the view.
        let alt_click = gtk::EventControllerLegacy::new();
        alt_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        alt_click.connect_event(move |_, event| {
            let Some(panel) = weak.upgrade() else {
                return gtk::glib::Propagation::Proceed;
            };
            if event.event_type() == gdk::EventType::ButtonPress
                && event.modifier_state().contains(gdk::ModifierType::ALT_MASK)
                && event
                    .downcast_ref::<gdk::ButtonEvent>()
                    .is_some_and(|button| button.button() == 1)
                && panel.session.borrow().current_document().is_some()
            {
                panel.dispatch("preview.edit", Vec::new());
            }
            gtk::glib::Propagation::Proceed
        });
        self.text_view.add_controller(alt_click);

        let weak = Rc::downgrade(self);
        self.discard.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.dispatch("preview.discard", Vec::new());
            }
        });
        let weak = Rc::downgrade(self);
        self.save.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.dispatch("preview.save", Vec::new());
            }
        });
        let weak = Rc::downgrade(self);
        self.save_as.connect_clicked(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.choose_save_as();
            }
        });

        for (button, mode) in [
            (&self.zoom_fit, ZoomMode::Fit),
            (&self.zoom_actual, ZoomMode::Actual),
        ] {
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.set_zoom(mode);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.zoom_in.connect_clicked(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.zoom_step(1.25);
                }
            });
            let weak = Rc::downgrade(self);
            self.zoom_out.connect_clicked(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.zoom_step(0.8);
                }
            });
        }
        {
            let weak = Rc::downgrade(self);
            self.image_scroll
                .hadjustment()
                .connect_notify_local(Some("page-size"), move |_, _| {
                    if let Some(panel) = weak.upgrade() {
                        panel.resize_render();
                    }
                });
            let weak = Rc::downgrade(self);
            self.image_scroll
                .vadjustment()
                .connect_notify_local(Some("page-size"), move |_, _| {
                    if let Some(panel) = weak.upgrade() {
                        panel.resize_render();
                    }
                });
        }
        {
            // Only the filename label handles the compact-title gesture. The
            // sidebar collapse button is a separate widget.
            let click = gtk::GestureClick::new();
            click.set_button(1);
            let weak = Rc::downgrade(self);
            click.connect_released(move |_, n_press, _, _| {
                if n_press >= 2
                    && let Some(panel) = weak.upgrade()
                {
                    panel.toggle_compact_title();
                }
            });
            self.title.add_controller(click);
        }
    }

    /// Double-click on the filename toggles the per-window compact header.
    pub(crate) fn toggle_compact_title(&self) {
        self.compact_title.set(!self.compact_title.get());
        self.update_title();
    }

    pub(crate) fn set_zoom(self: &Rc<Self>, mode: ZoomMode) {
        let mode = if mode == ZoomMode::Fit
            && self
                .session
                .borrow()
                .current_document()
                .is_some_and(|document| document.pdf().is_some())
        {
            ZoomMode::FitWidth
        } else {
            mode
        };
        self.zoom.set(mode);
        self.update_zoom_label();
        self.schedule_render();
    }

    fn zoom_step(self: &Rc<Self>, factor: f64) {
        let Some((width, height)) = self.source_size.get() else {
            return;
        };
        let viewport = self.image_viewport();
        let current = self.zoom.get().factor((width, height), viewport).max(0.01);
        let next = (current * factor).clamp(0.01, 8.0);
        self.zoom.set(ZoomMode::Manual(next));
        self.update_zoom_label();
        self.schedule_render();
    }

    fn dispatch(&self, name: &str, arguments: Vec<(String, String)>) {
        if let Some(process) = self.process.upgrade() {
            process.dispatch_for_window(self.window, Caller::Ui, Invocation::new(name, arguments));
        }
    }

    // -----------------------------------------------------------------------
    // Selection and loading
    // -----------------------------------------------------------------------

    /// A passive model refresh (listing, monitor, status). A successful Save
    /// As keeps its new path preview while the old browser selection is merely
    /// reapplied; a different selection is a real navigation.
    pub fn apply_model_selection(self: &Rc<Self>, selection: Selection) {
        crate::log_verbose!("preview", "model selection {:?}", selection);
        let was_dirty = self.session.borrow().is_dirty();
        let ticket = self.session.borrow_mut().apply_model_selection(selection);
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        } else if was_dirty {
            self.update_title();
            self.update_actions();
            self.show_pending_note();
        } else {
            let state = self.session.borrow().state().clone();
            if matches!(
                state,
                ViewState::Empty
                    | ViewState::Multiple { .. }
                    | ViewState::Trash
                    | ViewState::Folder(_)
                    | ViewState::Unsupported { .. }
                    | ViewState::Failed { .. }
            ) {
                // A non-document state has nothing to monitor. A dirty pin
                // takes the earlier branch and keeps its monitors.
                self.clear_monitors();
            }
            self.show_current_state();
        }
    }

    /// An explicit pointer/keyboard selection. It releases a Save As preview
    /// override even when the chosen name was already the browser's selection.
    pub fn set_selection_user(self: &Rc<Self>, selection: Selection) {
        crate::log_verbose!("preview", "user selection {:?}", selection);
        let was_dirty = self.session.borrow().is_dirty();
        let ticket = self.session.borrow_mut().select_user(selection);
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        } else if was_dirty {
            self.update_title();
            self.update_actions();
            self.show_pending_note();
            self.pulse_dirty_notice();
        } else {
            let state = self.session.borrow().state().clone();
            if matches!(
                state,
                ViewState::Empty
                    | ViewState::Multiple { .. }
                    | ViewState::Trash
                    | ViewState::Folder(_)
                    | ViewState::Unsupported { .. }
                    | ViewState::Failed { .. }
            ) {
                self.clear_monitors();
            }
            self.show_current_state();
        }
    }

    /// A tab/folder focus change is a genuine navigation: a successful Save As
    /// preview yields to the actual selected candidate.
    pub fn release_saved_override(self: &Rc<Self>) {
        let ticket = self.session.borrow_mut().release_saved_override();
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        }
    }

    fn begin_load_display(self: &Rc<Self>, ticket: &LoadTicket) {
        crate::log_verbose!(
            "preview",
            "load generation {} for {}",
            ticket.generation,
            ticket.path.display()
        );
        self.clear_monitors();
        self.change_warning.set(false);
        self.clear_status();
        self.cancel_pdf_render();
        self.render_timer.cancel();
        self.render_generation
            .set(self.render_generation.get().wrapping_add(1));
        self.picture.set_paintable(None::<&gdk::Paintable>);
        self.clear_buffer();
        self.source_size.set(None);
        self.render_edge_cap.set(preview::MAX_RENDER_EDGE);
        self.last_render_viewport.set((0, 0));
        self.fallback.borrow_mut().take();
        self.render_limit.borrow_mut().take();
        self.zoom_label.set_text("Loading…");
        self.message.set_text("Loading…");
        self.stack.set_visible_child_name("message");
        self.update_title();
        self.update_actions();
        *self.loading_path.borrow_mut() = Some(ticket.path.clone());
        self.submit_image(ImageJob::Load {
            generation: ticket.generation,
            path: ticket.path.clone(),
            target: self.load_target(),
            cancel: Arc::new(AtomicBool::new(false)),
        });
    }

    fn image_viewport(&self) -> (i32, i32) {
        (
            self.image_scroll.hadjustment().page_size().round() as i32,
            self.image_scroll.vadjustment().page_size().round() as i32,
        )
    }

    fn device_scale(&self) -> f64 {
        f64::from(self.picture.scale_factor().max(1))
    }

    fn load_target(&self) -> RenderSize {
        let (width, height) = self.image_viewport();
        let (width, height) = if width > 16 && height > 16 {
            (width, height)
        } else {
            (1024, 1024)
        };
        preview::render_plan(width, height, self.device_scale()).size
    }

    fn submit_image(self: &Rc<Self>, job: ImageJob) -> bool {
        // Stop the previous in-flight read/render from doing more expensive
        // work or sending a result once the newest job exists.
        if let Some(cancel) = self.image_cancel.borrow_mut().take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let cancel = job.cancel().clone();
        match self.workers.image.submit(job) {
            Ok(Some(replaced)) => {
                // A queued read/decode that has not started is obsolete now.
                replaced
                    .cancel()
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(None) => {}
            Err(rejected) => {
                rejected
                    .cancel()
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                self.set_status("The preview worker is not available.", true);
                return false;
            }
        }
        *self.image_cancel.borrow_mut() = Some(cancel);
        self.ensure_poll();
        true
    }

    fn submit_control(self: &Rc<Self>, job: ControlJob) -> bool {
        self.workers
            .control_outstanding
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.workers.control.send(job).is_err() {
            self.workers
                .control_outstanding
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            self.set_status("The preview worker is not available.", true);
            return false;
        }
        self.ensure_poll();
        true
    }

    fn ensure_poll(self: &Rc<Self>) {
        if self.poll.borrow().is_some() {
            return;
        }
        let weak = self.self_weak.borrow().clone();
        let source = glib::timeout_add_local(Duration::from_millis(POLL_MS), move || {
            let Some(panel) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            panel.poll_once()
        });
        *self.poll.borrow_mut() = Some(source);
    }

    fn poll_once(self: &Rc<Self>) -> glib::ControlFlow {
        loop {
            match self.workers.image_events.try_recv() {
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        loop {
            match self.workers.control_events.try_recv() {
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        loop {
            match self.pdf_events.try_recv() {
                Ok(outcome) => {
                    self.pdf_pending
                        .set(self.pdf_pending.get().saturating_sub(1));
                    self.apply_pdf(outcome);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        // The worker counters own completion, including replaced/cancelled
        // image jobs that will never send an event.
        if self.workers.image.outstanding() == 0
            && self
                .workers
                .control_outstanding
                .load(std::sync::atomic::Ordering::SeqCst)
                == 0
            && self.pdf_pending.get() == 0
        {
            *self.poll.borrow_mut() = None;
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    }

    fn handle_event(self: &Rc<Self>, event: Event) {
        match event {
            Event::Loaded {
                generation,
                path,
                loaded,
                image,
                fallback,
            } => self.apply_loaded(generation, path, loaded, image, fallback),
            Event::Rendered {
                generation,
                path,
                result,
            } => self.apply_rendered(generation, path, result),
            Event::Saved { id, result } => self.apply_saved(id, result),
            Event::Checked {
                generation,
                path,
                result,
            } => self.apply_checked(generation, path, result),
            Event::Destination { id, path, result } => self.apply_destination(id, path, result),
        }
    }

    fn apply_loaded(
        self: &Rc<Self>,
        generation: u64,
        path: PathBuf,
        loaded: Loaded,
        image: Option<Result<DecodedImage, ImageProblem>>,
        fallback: Option<Result<DecodedImage, ImageProblem>>,
    ) {
        let ticket = LoadTicket {
            generation,
            path: path.clone(),
        };
        let accepted = self
            .session
            .borrow_mut()
            .load_finished(&ticket, loaded.clone());
        if !accepted {
            crate::log!("preview", "load result was stale");
            return;
        }
        *self.loading_path.borrow_mut() = None;
        self.fallback.borrow_mut().take();
        self.source_size.set(None);
        self.render_edge_cap.set(preview::MAX_RENDER_EDGE);
        self.render_limit.borrow_mut().take();
        match loaded {
            Loaded::Document(document) => {
                self.install_monitors(&document.path);
                match &document.content {
                    fm_core::preview::PreviewContent::Text(_) => {
                        self.show_text_document(&document);
                    }
                    fm_core::preview::PreviewContent::Image(_) => {
                        self.zoom.set(ZoomMode::Fit);
                        match image {
                            Some(Ok(image)) => {
                                self.show_image_document(&document, Some(image));
                                self.schedule_render();
                            }
                            Some(Err(problem)) => self.show_image_problem(&document, &problem),
                            None => self.show_document_message(&document, "Preview not available."),
                        }
                    }
                    fm_core::preview::PreviewContent::Media(_) => {
                        let media = document.media().expect("media");
                        self.zoom.set(ZoomMode::Fit);
                        match image {
                            Some(Ok(image)) => {
                                let (source_width, source_height) =
                                    (image.source_width, image.source_height);
                                self.show_image_document(&document, Some(image));
                                if let Some(thumbnail) = &media.thumbnail {
                                    self.set_status(
                                        &format!(
                                            "Cached thumbnail ({}, {} px). Video previews are cache-only.",
                                            preview::format_size(thumbnail.size),
                                            source_width.max(source_height)
                                        ),
                                        false,
                                    );
                                }
                            }
                            _ => self.show_document_message(&document, "Preview not available."),
                        }
                    }
                    fm_core::preview::PreviewContent::Pdf(_) => {
                        self.zoom.set(ZoomMode::FitWidth);
                        self.render_edge_cap.set(preview::PDF_MAX_RASTER_EDGE);
                        if let Some(Ok(fallback)) = fallback {
                            *self.fallback.borrow_mut() = Some((generation, fallback));
                        }
                        self.set_status("Rendering page 1…", false);
                        self.schedule_pdf_render(generation);
                    }
                }
            }
            Loaded::Folder(fm_core::preview::FolderInfo::Folder { name, .. }) => {
                self.clear_monitors();
                self.show_message(&format!("Folder\n{name}"));
                self.set_status("A folder has no text or image preview.", false);
            }
            Loaded::Unsupported { reason, .. } => {
                self.clear_monitors();
                self.show_message(&reason.message());
            }
            Loaded::Failed { failure, .. } => {
                self.clear_monitors();
                self.show_message(&failure.message());
            }
        }
        self.update_title();
        self.update_actions();
    }

    fn show_image_problem(&self, document: &PreviewDocument, problem: &ImageProblem) {
        let message = match problem {
            ImageProblem::FileTooLarge => format!(
                "This image is too large to preview (limit {}).",
                preview::format_size(preview::MAX_IMAGE_BYTES)
            ),
            ImageProblem::TooLarge { width, height } => format!(
                "Image too large to preview ({width} × {height} pixels; limit {} megapixels).",
                preview::MAX_IMAGE_PIXELS / 1_000_000
            ),
            ImageProblem::Invalid(message) => format!("Preview not available: {message}"),
            ImageProblem::Limit(message) => message.clone(),
        };
        self.show_document_message(document, &message);
    }

    fn apply_rendered(
        self: &Rc<Self>,
        generation: u64,
        path: PathBuf,
        result: Result<DecodedImage, ImageProblem>,
    ) {
        if generation != self.render_generation.get() {
            crate::log!("preview", "a stale image render was ignored");
            return;
        }
        let current = self
            .session
            .borrow()
            .current_document()
            .map(|document| document.path.clone());
        if current.as_deref() != Some(path.as_path()) {
            return;
        }
        match result {
            Ok(image) => {
                self.source_size
                    .set(Some((image.source_width, image.source_height)));
                self.show_image_paintable(&image);
                self.update_zoom_label();
            }
            Err(problem) => {
                let document = self.session.borrow().current_document().cloned();
                if let Some(document) = document
                    && document.path == path
                {
                    self.show_image_problem(&document, &problem);
                }
            }
        }
    }

    fn show_text_document(&self, document: &PreviewDocument) {
        if let Some(text) = document.text()
            && !(self.stack.visible_child_name().as_deref() == Some("text")
                && buffer_text(&self.buffer) == text.text)
        {
            self.load_buffer(&text.text);
        }
        self.record_owner(document);
        self.text_view.set_editable(false);
        self.text_view.set_cursor_visible(false);
        self.stack.set_visible_child_name("text");
        self.clear_status();
        if self.admin_editable_for(document) {
            self.set_status("Alt-click to edit as administrator.", false);
        } else if let Some(reason) = document.edit_block_reason() {
            self.set_status(reason.message(), false);
        } else {
            self.set_status("Alt-click the text to edit.", false);
        }
    }

    fn load_buffer(&self, text: &str) {
        self.applying.set(true);
        self.buffer.set_text(text);
        self.buffer_revision.set(0);
        self.applying.set(false);
        self.text_scroll.vadjustment().set_value(0.0);
        self.gutter.queue_draw();
    }

    fn show_image_document(&self, document: &PreviewDocument, image: Option<DecodedImage>) {
        if let Some(image) = image {
            self.source_size
                .set(Some((image.source_width, image.source_height)));
            self.show_image_paintable(&image);
        } else {
            self.show_document_message(document, "Preview not available.");
        }
    }

    fn show_image_paintable(&self, image: &DecodedImage) {
        let texture = image::texture(image);
        self.picture.set_paintable(Some(&texture));
        self.stack.set_visible_child_name("image");
        let (display_width, display_height) = self.displayed_size();
        let scale = self.device_scale();
        let texture_width = ((f64::from(image.width) / scale).round() as i32).max(1);
        let texture_height = ((f64::from(image.height) / scale).round() as i32).max(1);
        // Never enlarge a texture beyond the pixels this render actually
        // produced; a lower-quality pass is corrected by the next render.
        self.picture.set_size_request(
            display_width.min(texture_width).max(1),
            display_height.min(texture_height).max(1),
        );
        self.update_zoom_label();
    }

    fn displayed_size(&self) -> (i32, i32) {
        let Some(source) = self.source_size.get() else {
            return (1, 1);
        };
        let viewport = self.image_viewport();
        let factor = self.zoom.get().factor(source, viewport);
        let requested = (
            ((f64::from(source.0) * factor).round() as i32).max(1),
            ((f64::from(source.1) * factor).round() as i32).max(1),
        );
        let plan = preview::render_plan_bounded(
            requested.0,
            requested.1,
            self.device_scale(),
            self.render_edge_cap.get(),
        );
        if let Some(limit) = plan.limit {
            *self.render_limit.borrow_mut() = Some(limit.message());
        } else {
            self.render_limit.borrow_mut().take();
        }
        preview::display_size_for_render(plan.size, self.device_scale())
    }

    fn render_target(&self) -> RenderSize {
        let (display_width, display_height) = self.displayed_size();
        preview::render_plan_bounded(
            display_width,
            display_height,
            self.device_scale(),
            self.render_edge_cap.get(),
        )
        .size
    }

    fn update_zoom_label(&self) {
        let limit = self.render_limit.borrow().clone();
        let text = match limit {
            Some(limit) => format!("{} — {limit}", self.zoom.get().label()),
            None => self.zoom.get().label(),
        };
        self.zoom_label.set_text(&text);
        self.zoom_label.set_tooltip_text(Some(&text));
    }

    fn resize_render(self: &Rc<Self>) {
        let viewport = self.image_viewport();
        if viewport != self.last_render_viewport.get() {
            self.last_render_viewport.set(viewport);
            self.schedule_render();
        }
    }

    fn schedule_render(self: &Rc<Self>) {
        // Invalidate any result from the previous viewport/zoom immediately;
        // the timer only decides when the next request starts.
        self.render_generation
            .set(self.render_generation.get().wrapping_add(1));
        let weak = Rc::downgrade(self);
        self.render_timer
            .schedule(Duration::from_millis(RENDER_DEBOUNCE_MS), move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.render_now();
                }
            });
    }

    fn render_now(self: &Rc<Self>) {
        let document = self.session.borrow().current_document().cloned();
        let Some(document) = document else {
            return;
        };
        match &document.content {
            fm_core::preview::PreviewContent::Image(_) => {
                let generation = self.next_render_generation();
                self.submit_image(ImageJob::RenderImage {
                    generation,
                    path: document.path.clone(),
                    target: self.render_target(),
                    cancel: Arc::new(AtomicBool::new(false)),
                });
            }
            fm_core::preview::PreviewContent::Pdf(_) => {
                self.cancel_pdf_render();
                let generation = self.next_render_generation();
                self.submit_pdf(&document.path, generation);
            }
            fm_core::preview::PreviewContent::Text(_)
            | fm_core::preview::PreviewContent::Media(_) => {}
        }
        self.update_zoom_label();
    }

    fn next_render_generation(&self) -> u64 {
        let generation = self.render_generation.get().wrapping_add(1);
        self.render_generation.set(generation);
        generation
    }

    fn pdf_target(&self) -> RenderSize {
        let viewport = self.image_viewport();
        let source = self.source_size.get();
        let (display_width, display_height) = match source {
            Some(_source) => self.displayed_size(),
            // Before the page geometry is known, use the visible box. Poppler's
            // square `-scale-to` bound keeps the first pass cheap; a second
            // pass at the now-known fit size follows.
            None => (viewport.0.max(256), viewport.1.max(256)),
        };
        let plan = preview::render_plan_bounded(
            display_width.max(1),
            display_height.max(1),
            self.device_scale(),
            self.render_edge_cap.get(),
        );
        if let Some(limit) = plan.limit {
            *self.render_limit.borrow_mut() = Some(limit.message());
        }
        plan.size
    }

    fn submit_pdf(self: &Rc<Self>, path: &Path, generation: u64) {
        let target = self.pdf_target();
        let input = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let cancel = Arc::new(AtomicBool::new(false));
        *self.pdf_cancel.borrow_mut() = Some(cancel.clone());
        let job = pdf::PdfJob {
            generation,
            work: pdf::PdfWork::Render(PdfRenderRequest {
                input,
                output_dir: PathBuf::new(),
                target,
            }),
            cancel,
            tx: self.pdf_tx.clone(),
            owner: self.pdf_owner,
        };
        match pdf::submit_selected(job) {
            Ok(()) => {
                self.pdf_pending
                    .set(self.pdf_pending.get().saturating_add(1));
                self.ensure_poll();
            }
            Err(pdf::PdfSubmitError::Full) => {
                // Capacity is transient. Keep the current selection and retry
                // instead of falling back to a cached thumbnail permanently.
                self.set_status("Waiting for the PDF renderer…", false);
                self.schedule_pdf_retry(path.to_path_buf());
            }
            Err(pdf::PdfSubmitError::Stopped) => {
                self.show_pdf_fallback(&PdfFailure::Failed(
                    pdf::PdfSubmitError::Stopped.message().to_string(),
                ));
            }
        }
    }

    fn schedule_pdf_retry(self: &Rc<Self>, path: PathBuf) {
        let weak = Rc::downgrade(self);
        self.pdf_retry
            .schedule(Duration::from_millis(PDF_RETRY_MS), move |_| {
                let Some(panel) = weak.upgrade() else {
                    return;
                };
                let current = panel
                    .session
                    .borrow()
                    .current_document()
                    .map(|document| document.path.clone());
                if current.as_deref() == Some(path.as_path()) {
                    panel.schedule_pdf_render(panel.session.borrow().generation());
                }
            });
    }

    fn schedule_pdf_render(self: &Rc<Self>, _load_generation: u64) {
        let document = self.session.borrow().current_document().cloned();
        let Some(document) = document else {
            return;
        };
        if document.pdf().is_none() {
            return;
        }
        self.cancel_pdf_render();
        let generation = self.next_render_generation();
        self.submit_pdf(&document.path, generation);
    }

    fn cancel_pdf_render(&self) {
        if let Some(cancel) = self.pdf_cancel.borrow_mut().take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.pdf_retry.cancel();
    }

    fn apply_pdf(self: &Rc<Self>, outcome: PdfOutcome) {
        if outcome.generation != self.render_generation.get() {
            crate::log!("preview", "a stale PDF render was ignored");
            return;
        }
        let current = self
            .session
            .borrow()
            .current_document()
            .map(|document| document.path.clone());
        if current.as_deref() != Some(outcome.path.as_path()) {
            self.cancel_pdf_render();
            return;
        }
        match (outcome.image, outcome.failure) {
            (Some(image), _) => {
                // The first fit-width render fixes the page's logical basis;
                // later zoom renders only raise the texture resolution, so
                // 100% stays stable instead of following the last render.
                let first_geometry = self.source_size.get().is_none();
                if first_geometry {
                    self.source_size
                        .set(Some((image.source_width, image.source_height)));
                }
                self.show_image_paintable(&image);
                self.set_status("Page 1", false);
                if first_geometry {
                    // Re-request at the now-known fit size. The conservative
                    // pre-render bound kept the first pass small; this pass is
                    // bounded by the actual page geometry.
                    self.schedule_render();
                }
            }
            (None, Some(failure)) => self.show_pdf_fallback(&failure),
            (None, None) => self.show_pdf_fallback(&PdfFailure::Failed(
                "the renderer returned nothing".to_string(),
            )),
        }
        self.update_title();
        self.update_actions();
    }

    fn show_pdf_fallback(&self, failure: &PdfFailure) {
        if failure.fallback_allowed()
            && let Some((_, image)) = self.fallback.borrow().clone()
        {
            self.source_size
                .set(Some((image.source_width, image.source_height)));
            self.show_image_paintable(&image);
            self.set_status(
                &format!(
                    "Cached thumbnail, not a fresh page render. {}",
                    failure.message()
                ),
                true,
            );
            return;
        }
        self.show_message(&failure.message());
        self.set_status(&failure.message(), true);
    }

    fn show_document_message(&self, document: &PreviewDocument, message: &str) {
        self.show_message(message);
        self.set_status(
            &format!(
                "{} ({})",
                document.name,
                preview::format_size(document.size)
            ),
            false,
        );
    }

    fn show_message(&self, message: &str) {
        self.message.set_text(message);
        self.stack.set_visible_child_name("message");
        self.text_view.set_editable(false);
        self.picture.set_paintable(None::<&gdk::Paintable>);
    }

    fn clear_buffer(&self) {
        self.applying.set(true);
        self.buffer.set_text("");
        self.buffer_revision.set(0);
        self.applying.set(false);
        self.gutter.queue_draw();
    }

    fn show_current_state(self: &Rc<Self>) {
        let pinned = self.session.borrow().pinned().cloned();
        if let Some(_pinned) = pinned {
            // The buffer already holds the user's edits; only chrome changes.
            self.update_title();
            self.update_actions();
            return;
        }
        let state = self.session.borrow().state().clone();
        match state {
            ViewState::Empty => {
                self.show_message("Select a file to preview");
                self.clear_status();
            }
            ViewState::Multiple { count } => {
                self.show_message(&format!("{count} items selected"));
                self.clear_status();
            }
            ViewState::Trash => {
                self.show_message("Trash");
                self.clear_status();
            }
            ViewState::Loading { path } => {
                self.show_message("Loading…");
                let _ = path;
            }
            ViewState::Document(document) => match &document.content {
                fm_core::preview::PreviewContent::Text(_) => self.show_text_document(&document),
                // Image and media presentation belongs to the load result:
                // keeping it here avoids replacing a decoded picture with a
                // loading placeholder during an unrelated refresh.
                fm_core::preview::PreviewContent::Image(_)
                | fm_core::preview::PreviewContent::Media(_)
                | fm_core::preview::PreviewContent::Pdf(_) => {}
            },
            ViewState::Folder(fm_core::preview::FolderInfo::Folder { name, .. }) => {
                self.show_message(&format!("Folder\n{name}"));
            }
            ViewState::Unsupported { reason, .. } => {
                self.show_message(&reason.message());
            }
            ViewState::Failed { failure, .. } => {
                self.show_message(&failure.message());
            }
        }
        self.update_title();
        self.update_actions();
    }

    fn show_pending_note(&self) {
        self.refresh_notice();
    }

    /// The dedicated, persistent dirty-pin notice. It never expires with the
    /// ordinary status line and never replaces external-change/save-error text.
    fn refresh_notice(&self) {
        let session = self.session.borrow();
        let Some(pinned) = session.pinned() else {
            self.notice.set_visible(false);
            self.notice.set_text("");
            return;
        };
        if !pinned.dirty && !pinned.saving {
            self.notice.set_visible(false);
            self.notice.set_text("");
            return;
        }
        let target = pinned.document.path.clone();
        let name = pinned.document.name.clone();
        let candidate = session.candidate();
        let candidate_text = match candidate {
            Selection::None => "no selection".to_string(),
            Selection::File { path } => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            Selection::Multiple { count } => format!("{count} selected items"),
            Selection::Trash => "the Trash page".to_string(),
        };
        let same = candidate.path() == Some(target.as_path());
        if same {
            self.notice.set_visible(false);
            self.notice.set_text("");
            return;
        }
        let text = dirty_notice_text(&name, &candidate_text);
        self.notice.set_text(&text);
        self.notice.set_visible(true);
        self.notice
            .update_property(&[gtk::accessible::Property::Label(&text)]);
    }

    /// Two brief emphasis pulses per fresh selection attempt. Rapid attempts
    /// restart the same animation instead of accumulating timers.
    fn pulse_dirty_notice(&self) {
        if !self.notice.is_visible() {
            return;
        }
        let animations =
            gtk::Settings::default().is_none_or(|settings| settings.is_gtk_enable_animations());
        if !animations {
            self.notice.set_opacity(1.0);
            return;
        }
        let generation = self.notice_generation.get().wrapping_add(1);
        self.notice_generation.set(generation);
        self.notice_animation.borrow_mut().take();
        let notice = self.notice.clone();
        let target = adw::CallbackAnimationTarget::new(move |value| {
            // Two full sine pulses, ending at full opacity.
            notice.set_opacity(notice_opacity(value));
        });
        let animation =
            adw::TimedAnimation::new(&self.notice, 0.0, 1.0, crate::look::motion::ms(360), target);
        animation.play();
        *self.notice_animation.borrow_mut() = Some(animation.upcast());
    }

    // -----------------------------------------------------------------------
    // Editing, discard, save
    // -----------------------------------------------------------------------

    pub fn begin_edit(self: &Rc<Self>, caller: Caller) {
        if self.session.borrow().saving() {
            self.set_status("Wait for the current save to finish.", false);
            return;
        }
        let revision = self.buffer_revision.get();
        let document = self.session.borrow().current_document().cloned();
        let Some(document) = document else {
            self.set_status("Only a text preview can be edited.", false);
            return;
        };
        let owner = self
            .owner_uid
            .get()
            .or_else(|| owner_of_path(&document.path));
        let current = self.current_uid.get();
        let admin_editable = match (owner, current) {
            (Some(owner), Some(current)) => document.admin_editable(owner, current),
            _ => false,
        };
        let result = self.session.borrow_mut().begin_edit_for(
            caller,
            revision,
            owner.unwrap_or(0),
            current.unwrap_or(0),
        );
        match result {
            Ok(()) => {
                self.text_view.set_editable(true);
                self.text_view.set_cursor_visible(true);
                self.text_view.grab_focus();
                self.change_warning.set(false);
                if admin_editable {
                    crate::log!(
                        "admin",
                        "entered administrator edit mode for {} (owner {})",
                        document.path.display(),
                        owner.map(preview::owner_name).unwrap_or_default()
                    );
                    self.set_status(
                        "Editing as administrator — saving asks before it changes the file.",
                        false,
                    );
                } else {
                    self.set_status(
                        "Editing preview — changes are kept until Save or Discard.",
                        false,
                    );
                }
                self.update_title();
                self.update_actions();
            }
            Err(message) => {
                self.set_status(&message, true);
            }
        }
    }

    pub fn discard(self: &Rc<Self>) {
        if self.session.borrow().saving() {
            self.set_status("Wait for the current save to finish.", false);
            return;
        }
        let editing = self.session.borrow().is_editing();
        if !editing {
            self.set_status("There are no preview edits to discard.", false);
            return;
        }
        self.chooser_reply.set(None);
        self.change_warning.set(false);
        self.change_dialog_open.set(false);
        let ticket = self.session.borrow_mut().discard();
        self.clear_buffer();
        self.text_view.set_editable(false);
        self.text_view.set_cursor_visible(false);
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        } else {
            self.show_current_state();
        }
        self.set_status("Discarded preview edits.", false);
    }

    pub fn save_plan(&self, path: Option<&Path>) -> Vec<String> {
        let session = self.session.borrow();
        let Some(pinned) = session.pinned() else {
            return vec!["No unsaved preview edits.".to_string()];
        };
        let Some(text) = pinned.document.text() else {
            return vec!["Only editable text can be saved.".to_string()];
        };
        if path.is_some() && session.is_admin_editing() {
            return vec![
                "Refuse: Save As isn't available while editing as administrator.".to_string(),
            ];
        }
        let body = buffer_text(&self.buffer);
        let target = match path {
            Some(_) => SaveTarget::NewPath { replace: None },
            None if session.is_admin_editing() => SaveTarget::Admin {
                expected: preview::SaveExpectation {
                    revision: pinned.document.revision.clone(),
                },
            },
            None => SaveTarget::Original {
                expected: preview::SaveExpectation {
                    revision: pinned.document.revision.clone(),
                },
            },
        };
        preview::save_plan(&SaveRequest {
            path: path
                .map(Path::to_path_buf)
                .unwrap_or_else(|| pinned.document.path.clone()),
            text: body,
            encoding: text.encoding,
            target,
        })
    }

    pub fn save(self: &Rc<Self>, caller: Caller) {
        if self.change_warning.get() {
            self.show_change_dialog();
            return;
        }
        if self.session.borrow().is_admin_editing() {
            if caller != Caller::Ui {
                crate::log!(
                    "admin",
                    "refused administrator save: only the UI may save these edits"
                );
                self.set_status(preview::ADMIN_EDIT_UI_ONLY, true);
                return;
            }
            self.show_admin_save_warning();
            return;
        }
        let result = self.session.borrow_mut().begin_save_original_for(caller);
        self.start_save_ticket(result);
    }

    /// Every administrator Save warns first, and nothing is remembered.
    fn show_admin_save_warning(self: &Rc<Self>) {
        if self.admin_warning_open.get() {
            return;
        }
        let Some(pinned) = self.session.borrow().pinned().cloned() else {
            return;
        };
        if pinned.saving {
            self.set_status("Wait for the current save to finish.", false);
            return;
        }
        self.admin_warning_open.set(true);
        let path = pinned.document.path.clone();
        let owner = self
            .owner_uid
            .get()
            .or_else(|| owner_of_path(&path))
            .map(preview::owner_name)
            .unwrap_or_else(|| "another user".to_string());
        crate::log!(
            "admin",
            "save warning shown for {} (owner {owner})",
            path.display()
        );
        let dialog = adw::AlertDialog::new(
            Some("Save as Administrator?"),
            Some(&format!(
                "Save changes to {}?\nThis changes a system file owned by {owner}.",
                path.display()
            )),
        );
        dialog.add_css_class("gimbal-look");
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("save", "Save as Administrator");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            panel.admin_warning_open.set(false);
            let accepted = response == "save";
            crate::log!(
                "admin",
                "save warning answered {} for {}",
                if accepted { "save" } else { "cancel" },
                path.display()
            );
            if accepted {
                panel.start_admin_save_after_mount(path.clone());
            } else if panel.close_wait.borrow().is_some() {
                panel.abort_close();
            }
        });
        self.present_dialog(dialog);
    }

    /// The polkit prompt belongs to the GVfs administrator mount. It is only
    /// requested after the warning is accepted, so entering edit mode and
    /// cancelling a save are free.
    fn start_admin_save_after_mount(self: &Rc<Self>, path: PathBuf) {
        let Some(process) = self.process.upgrade() else {
            self.set_status("The window is gone.", true);
            self.abort_close();
            return;
        };
        let Some(ui) = self.ui() else {
            self.abort_close();
            return;
        };
        let window = ui.id;
        let weak = Rc::downgrade(self);
        let mount_path = path.clone();
        process.ensure_admin_mount(window, &path, move |result| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(()) => {
                    let ticket = panel
                        .session
                        .borrow_mut()
                        .begin_save_original_for(Caller::Ui);
                    panel.start_save_ticket(ticket);
                }
                Err(message) => {
                    crate::log!(
                        "admin",
                        "save refused for {}: {message}",
                        mount_path.display()
                    );
                    panel.set_status(&message, true);
                    panel.abort_close();
                }
            }
        });
    }

    pub fn save_as(self: &Rc<Self>, _caller: Caller, path: PathBuf) {
        let from_chooser = self.chooser_reply.take();
        if self.session.borrow().is_admin_editing() {
            if let Some(id) = from_chooser {
                self.cancel_save_as_request(id);
            }
            self.set_status(
                "Save As isn't available while editing as administrator.",
                true,
            );
            return;
        }
        if self.session.borrow().saving() {
            if let Some(id) = from_chooser {
                self.cancel_save_as_request(id);
                crate::log!("preview", "a stale Save As reply arrived during a save");
            } else {
                self.set_status("Wait for the current save to finish.", false);
            }
            return;
        }
        if !self.session.borrow().is_editing() {
            if let Some(id) = from_chooser {
                self.cancel_save_as_request(id);
                crate::log!(
                    "preview",
                    "a stale Save As reply arrived after the editor ended"
                );
            } else {
                self.set_status("Alt-click a writable text preview before Save As.", false);
            }
            return;
        }
        let request_id = match from_chooser {
            Some(id) => id,
            None => match self.session.borrow_mut().begin_save_as() {
                Ok(request) => request.id,
                Err(message) => {
                    self.set_status(&message, true);
                    return;
                }
            },
        };
        let route = self
            .session
            .borrow_mut()
            .choose_save_as_path(request_id, path.clone());
        match route {
            Ok(SaveAsRoute::Original) => {
                // Saving over the original path keeps Original's loaded-version
                // checks, even when it came from Save As.
                let result = self.session.borrow_mut().begin_save_original();
                self.start_save_ticket(result);
            }
            Ok(SaveAsRoute::NewPath) => {
                self.set_status("Checking the destination…", false);
                if !self.submit_control(ControlJob::Destination {
                    id: request_id,
                    path,
                }) {
                    self.session.borrow_mut().cancel_save_as(request_id);
                }
            }
            Err(message) => {
                if from_chooser.is_some() {
                    crate::log!("preview", "a stale Save As reply was ignored: {message}");
                } else {
                    self.set_status(&message, true);
                }
            }
        }
    }

    fn start_save_ticket(self: &Rc<Self>, result: Result<SaveTicket, String>) {
        let ticket = match result {
            Ok(ticket) => ticket,
            Err(message) => {
                self.set_status(&message, false);
                self.abort_close();
                return;
            }
        };
        let target = if ticket.admin {
            SaveTarget::Admin {
                expected: preview::SaveExpectation {
                    revision: ticket.token.editor.revision.clone(),
                },
            }
        } else if ticket.new_path {
            SaveTarget::NewPath {
                replace: ticket.replace.clone(),
            }
        } else {
            SaveTarget::Original {
                expected: preview::SaveExpectation {
                    revision: ticket.token.editor.revision.clone(),
                },
            }
        };
        let request = SaveRequest {
            path: ticket.path.clone(),
            text: buffer_text(&self.buffer),
            encoding: ticket.encoding,
            target,
        };
        *self.pending_save.borrow_mut() = Some(PendingSave {
            ticket: ticket.clone(),
            request: request.clone(),
        });
        self.session.borrow_mut().bind_close_to_save(&ticket);
        self.text_view.set_editable(false);
        self.text_view.set_cursor_visible(false);
        crate::log!("preview", "save started for {}", request.path.display());
        self.set_status("Saving…", false);
        self.update_title();
        self.update_actions();
        if !self.submit_control(ControlJob::Save {
            id: ticket.id,
            request,
        }) {
            *self.pending_save.borrow_mut() = None;
            self.session
                .borrow_mut()
                .save_failed(&ticket, "The preview worker is not available.");
            self.restore_editing();
            self.abort_close();
        }
    }

    /// Drop a close continuation that cannot be satisfied. The cancel closure
    /// aborts a pending quit but never closes the window. Session state is
    /// settled before the dialog is dismissed, so its scoped response ignores
    /// the forced close.
    fn abort_close(&self) {
        self.close_dialog_open.set(false);
        let wait = self.close_wait.borrow_mut().take();
        self.session.borrow_mut().invalidate_close();
        self.dismiss_close_dialog();
        if let Some(wait) = wait {
            (wait.cancel)();
        }
    }

    /// Abort only a continuation that belongs to this save job (or an orphaned
    /// wait). A newer close request is never disturbed by a late result.
    fn abort_close_for(&self, ticket: &SaveTicket) {
        let session = self.session.borrow();
        let abort = close_owned_by_save(&session, self.close_wait.borrow().is_some(), ticket);
        drop(session);
        if abort {
            self.abort_close();
        }
    }

    fn apply_saved(self: &Rc<Self>, id: u64, result: Result<SaveSuccess, SaveError>) {
        let pending = {
            let pending = self.pending_save.borrow();
            match pending.as_ref() {
                Some(pending) if pending.ticket.id == id => pending.clone(),
                _ => return,
            }
        };
        *self.pending_save.borrow_mut() = None;
        match result {
            Ok(success) => {
                let name = success
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| success.path.display().to_string());
                let document = PreviewDocument {
                    path: success.path.clone(),
                    name: name.clone(),
                    size: success.size,
                    revision: success.revision.clone(),
                    access: fm_core::preview::WriteAccess {
                        file_writable: !pending.ticket.admin,
                        parent_writable: !pending.ticket.admin,
                        reason: pending
                            .ticket
                            .admin
                            .then_some(fm_core::preview::ReadOnlyReason::NotWritable),
                    },
                    content: fm_core::preview::PreviewContent::Text(
                        fm_core::preview::TextDocument {
                            text: pending.request.text.clone(),
                            encoding: pending.request.encoding,
                        },
                    ),
                };
                let completion = self
                    .session
                    .borrow_mut()
                    .save_finished(&pending.ticket, document.clone());
                match completion {
                    SaveCompletion::Stale => {
                        // Never leave a close closure waiting for an event that
                        // cannot satisfy it. A newer close request is left for
                        // its own save/dialog to resolve.
                        self.abort_close_for(&pending.ticket);
                        crate::log!("preview", "a stale save result was ignored");
                        return;
                    }
                    SaveCompletion::EditorChanged => {
                        self.restore_editing();
                        self.abort_close();
                        self.set_status(
                            "The editor changed while saving; nothing was marked saved.",
                            true,
                        );
                        return;
                    }
                    SaveCompletion::Applied => {}
                }
                crate::log!(
                    "preview",
                    "save committed {} ({} bytes)",
                    success.path.display(),
                    success.size
                );
                if pending.ticket.admin {
                    crate::log!(
                        "admin",
                        "saved {} ({} bytes) as administrator",
                        success.path.display(),
                        success.size
                    );
                }
                self.buffer_revision
                    .set(pending.ticket.token.buffer_revision);
                self.text_view.set_editable(false);
                self.text_view.set_cursor_visible(false);
                self.change_warning.set(false);
                self.record_owner(&document);
                self.install_monitors(&success.path);
                let status = if pending.ticket.admin {
                    "Saved as administrator.".to_string()
                } else {
                    format!("Saved {} ({}).", name, preview::format_size(success.size))
                };
                self.set_status(&status, false);
                self.update_title();
                self.update_actions();
                self.finish_or_abort_close(&pending.ticket);
                if !pending.ticket.new_path {
                    self.reconcile_candidate();
                }
            }
            Err(error) => {
                crate::log!("preview", "save failed: {error:?}");
                if pending.ticket.admin {
                    crate::log!(
                        "admin",
                        "save failed for {}: {}",
                        pending.request.path.display(),
                        error.message()
                    );
                }
                if !self
                    .session
                    .borrow_mut()
                    .save_failed(&pending.ticket, error.message())
                {
                    self.abort_close_for(&pending.ticket);
                    return;
                }
                self.abort_close();
                self.restore_editing();
                self.set_status(&error.message(), true);
                if matches!(error, SaveError::ChangedOnDisk) {
                    self.change_warning.set(true);
                    self.show_change_dialog();
                }
            }
        }
    }

    /// Schedule the close continuation after a successful save. The accepted
    /// request stays in the session until the idle actually runs; the idle
    /// upgrades this same panel and revalidates the request against the
    /// completed save plus any fresh edit or newer save.
    fn finish_or_abort_close(self: &Rc<Self>, ticket: &SaveTicket) {
        let decision = saved_close_decision(
            &self.session.borrow(),
            self.close_wait.borrow().is_some(),
            ticket,
        );
        match decision {
            SavedClose::Proceed(_) => {
                let weak = Rc::downgrade(self);
                let ticket = ticket.clone();
                glib::idle_add_local_once(move || {
                    let Some(panel) = weak.upgrade() else {
                        return;
                    };
                    panel.apply_saved_close_idle(&ticket);
                });
            }
            SavedClose::Abort => self.abort_close(),
            SavedClose::None => {}
        }
    }

    fn apply_saved_close_idle(self: &Rc<Self>, ticket: &SaveTicket) {
        let decision = complete_saved_close(&self.session, &self.close_wait, ticket, || {
            self.dismiss_close_dialog();
        });
        match decision {
            SavedClose::Proceed(close) => {
                crate::log!(
                    "preview",
                    "save close continuation proceeded for editor {}",
                    close.token.editor.id
                );
            }
            SavedClose::Abort => {
                crate::log!("preview", "save close continuation was invalid at idle");
            }
            SavedClose::None => {}
        }
    }

    fn restore_editing(&self) {
        let editing = self.session.borrow().is_editing();
        if editing {
            self.text_view.set_editable(true);
            self.text_view.set_cursor_visible(true);
        }
        self.update_title();
        self.update_actions();
    }

    fn reconcile_candidate(self: &Rc<Self>) {
        let ticket = self.session.borrow_mut().reconcile();
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        } else {
            self.show_current_state();
        }
    }

    fn apply_destination(
        self: &Rc<Self>,
        id: u64,
        path: PathBuf,
        result: Result<Destination, SaveError>,
    ) {
        match apply_destination_reply(&self.session, id, &path, result) {
            DestinationAction::StartSave(ticket) => self.start_save_ticket(Ok(ticket)),
            DestinationAction::Replace(revision) => {
                self.show_replace_dialog(id, path, revision);
            }
            DestinationAction::Cancel { message, warning } => {
                self.set_status(&message, warning);
            }
            DestinationAction::Ignore(message) => {
                crate::log!(
                    "preview",
                    "a stale destination reply was ignored: {message}"
                );
            }
        }
    }

    fn choose_save_as(self: &Rc<Self>) {
        if self.session.borrow().saving() {
            self.set_status("Wait for the current save to finish.", false);
            return;
        }
        if !self.session.borrow().is_editing() {
            self.set_status("Alt-click a writable text preview before Save As.", false);
            return;
        }
        let request = match self.session.borrow_mut().begin_save_as() {
            Ok(request) => request,
            Err(message) => {
                self.set_status(&message, true);
                return;
            }
        };
        let Some(ui) = self.ui() else {
            self.cancel_save_as_request(request.id);
            return;
        };
        let dialog = gtk::FileDialog::new();
        dialog.set_title("Save As");
        dialog.set_modal(true);
        let (name, folder) = self
            .session
            .borrow()
            .pinned()
            .map(|pinned| {
                (
                    pinned.document.name.clone(),
                    pinned.document.path.parent().map(Path::to_path_buf),
                )
            })
            .unwrap_or_default();
        dialog.set_initial_name(Some(&name));
        if let Some(folder) = folder {
            dialog.set_initial_folder(Some(&gio::File::for_path(folder)));
        }
        // The chooser owns the window's dialog guard for its whole lifetime,
        // including cancellation and an error before any path arrives.
        ui.begin_dialog();
        let weak = Rc::downgrade(self);
        let weak_ui = Rc::downgrade(&ui);
        dialog.save(Some(ui.window()), None::<&gio::Cancellable>, move |result| {
            if let Some(ui) = weak_ui.upgrade() {
                ui.end_dialog();
            }
            let Some(panel) = weak.upgrade() else {
                return;
            };
            match result {
                Ok(file) => {
                    let Some(path) = file.path() else {
                        panel.cancel_save_as_request(request.id);
                        panel.set_status("That location has no local path.", true);
                        return;
                    };
                    let Some(path_text) = path.to_str() else {
                        panel.cancel_save_as_request(request.id);
                        panel.set_status(
                            "Save As needs a UTF-8 path for the command argument; choose another name.",
                            true,
                        );
                        return;
                    };
                    panel.chooser_reply.set(Some(request.id));
                    panel.dispatch(
                        "preview.save-as",
                        vec![("path".to_string(), path_text.to_string())],
                    );
                }
                Err(error) => {
                    panel.cancel_save_as_request(request.id);
                    if !error.matches(gio::IOErrorEnum::Cancelled) {
                        panel.set_status(&format!("Save As was not opened: {error}"), true);
                    }
                }
            }
        });
    }

    fn cancel_save_as_request(&self, id: u64) {
        self.session.borrow_mut().cancel_save_as(id);
        if self.chooser_reply.get() == Some(id) {
            self.chooser_reply.set(None);
        }
    }

    // -----------------------------------------------------------------------
    // Close and quit guards
    // -----------------------------------------------------------------------

    /// Returns `Free` when this panel needs no close guard, `Waiting` when it
    /// owns a newly armed continuation, and `Busy` when an ordinary close
    /// already owns one. Repeated window closes stay blocked by the one dialog;
    /// a Quit caller turns `Busy` into its own cancellation without disturbing
    /// that dialog or its continuation.
    pub fn guard_close(self: &Rc<Self>, proceed: Rc<dyn Fn()>, cancel: Rc<dyn Fn()>) -> CloseGuard {
        let decision = close_guard_decision(&self.session, self.close_wait.borrow().is_some());
        if decision != CloseGuard::Waiting {
            return decision;
        }
        self.chooser_reply.set(None);
        *self.close_wait.borrow_mut() = Some(CloseWait {
            proceed,
            cancel: cancel.clone(),
        });
        if self.close_dialog_open.get() {
            return CloseGuard::Waiting;
        }
        let Some(close) = self.session.borrow().close_request().cloned() else {
            self.abort_close();
            return CloseGuard::Free;
        };
        self.close_dialog_open.set(true);
        let saving = self.session.borrow().saving();
        let name = self
            .session
            .borrow()
            .pinned()
            .map(|pinned| pinned.document.name.clone())
            .unwrap_or_else(|| "this file".to_string());

        if saving {
            let dialog = adw::AlertDialog::new(
                Some("Saving…"),
                Some(&format!(
                    "“{name}” is still being saved. The window will close when the save finishes."
                )),
            );
            dialog.add_css_class("gimbal-look");
            dialog.add_response("cancel", "Cancel close");
            dialog.set_default_response(Some("cancel"));
            dialog.set_close_response("cancel");
            let dialog_close = close.clone();
            let weak = Rc::downgrade(self);
            dialog.connect_response(None, move |_, _| {
                let Some(panel) = weak.upgrade() else {
                    return;
                };
                if panel.session.borrow().close_request() != Some(&dialog_close) {
                    crate::log!("preview", "a stale saving-dialog reply was ignored");
                    return;
                }
                panel.take_close_dialog();
                panel.abort_close();
            });
            self.present_close_dialog(dialog);
            return CloseGuard::Waiting;
        }

        let dialog = adw::AlertDialog::new(
            Some("Save changes?"),
            Some(&format!(
                "“{name}” has unsaved preview edits. Save them before closing?"
            )),
        );
        dialog.add_css_class("gimbal-look");
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("discard", "Discard");
        dialog.add_response("save", "Save");
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let dialog_close = close.clone();
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            if panel.session.borrow().close_request() != Some(&dialog_close) {
                crate::log!("preview", "a stale close-dialog reply was ignored");
                return;
            }
            panel.take_close_dialog();
            match response {
                "save" => {
                    // `save` arms its own ticket and binds this close request
                    // to that exact job. A failure aborts both.
                    panel.save(Caller::Ui);
                }
                "discard" => {
                    let ticket = panel.session.borrow_mut().discard_preserving_close();
                    panel.chooser_reply.set(None);
                    panel.change_warning.set(false);
                    panel.change_dialog_open.set(false);
                    panel.clear_buffer();
                    panel.text_view.set_editable(false);
                    panel.text_view.set_cursor_visible(false);
                    if let Some(ticket) = ticket {
                        panel.begin_load_display(&ticket);
                    } else {
                        panel.show_current_state();
                    }
                    let weak = Rc::downgrade(&panel);
                    let close = dialog_close.clone();
                    glib::idle_add_local_once(move || {
                        let Some(panel) = weak.upgrade() else {
                            return;
                        };
                        let current = panel.session.borrow().close_request().cloned();
                        if current.as_ref() != Some(&close) {
                            // This reply belongs to an older dialog; a newer
                            // close request owns the panel now.
                            crate::log!("preview", "a stale close-discard idle was ignored");
                            return;
                        }
                        let ready = panel.session.borrow().close_ready(&close);
                        if ready {
                            let wait = panel.close_wait.borrow_mut().take();
                            let taken = panel.session.borrow_mut().take_close();
                            if taken.is_some()
                                && let Some(wait) = wait
                            {
                                (wait.proceed)();
                            }
                        } else {
                            panel.abort_close();
                        }
                    });
                }
                _ => panel.abort_close(),
            }
        });
        self.present_close_dialog(dialog);
        CloseGuard::Waiting
    }

    fn present_close_dialog(&self, dialog: adw::AlertDialog) {
        *self.close_dialog.borrow_mut() = Some(dialog.clone());
        self.present_dialog(dialog);
    }

    fn take_close_dialog(&self) -> Option<adw::AlertDialog> {
        self.close_dialog_open.set(false);
        self.close_dialog.borrow_mut().take()
    }

    fn dismiss_close_dialog(&self) {
        if let Some(dialog) = self.take_close_dialog() {
            // `force_close` emits the close response; callers invalidate or
            // take their request first so the scoped callback ignores it.
            dialog.force_close();
        }
    }

    fn present_dialog(&self, dialog: adw::AlertDialog) {
        if let Some(ui) = self.ui() {
            ui.begin_dialog();
            let weak_ui = Rc::downgrade(&ui);
            dialog.connect_closed(move |_| {
                if let Some(ui) = weak_ui.upgrade() {
                    ui.end_dialog();
                }
            });
            dialog.choose(Some(ui.window()), None::<&gio::Cancellable>, |_| {});
        }
    }

    fn show_replace_dialog(self: &Rc<Self>, id: u64, path: PathBuf, _revision: FileRevision) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let dialog = adw::AlertDialog::new(
            Some(&format!("Replace “{name}”?")),
            Some("That file already exists. Replace it with the editor's contents?"),
        );
        dialog.add_css_class("gimbal-look");
        dialog.add_response("cancel", "Cancel");
        dialog.add_response("replace", "Replace");
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            if response != "replace" {
                panel.cancel_save_as_request(id);
                return;
            }
            match confirm_and_start_save_as(&panel.session, id) {
                Ok(ticket) => panel.start_save_ticket(Ok(ticket)),
                Err(message) => {
                    panel.cancel_save_as_request(id);
                    crate::log!("preview", "a stale Replace reply was ignored: {message}");
                }
            }
        });
        self.present_dialog(dialog);
    }

    fn show_change_dialog(self: &Rc<Self>) {
        if self.change_dialog_open.get() {
            return;
        }
        self.change_dialog_open.set(true);
        let name = self
            .session
            .borrow()
            .pinned()
            .map(|pinned| pinned.document.name.clone())
            .unwrap_or_else(|| "this file".to_string());
        let admin = self.admin_editing();
        let body = if admin {
            format!(
                "“{name}” changed after it was loaded. Reload from disk (discarding edits) or cancel."
            )
        } else {
            format!(
                "“{name}” changed after it was loaded. Reload from disk (discarding edits), save a copy, or cancel."
            )
        };
        let dialog = adw::AlertDialog::new(Some("File changed on disk"), Some(&body));
        dialog.add_css_class("gimbal-look");
        dialog.add_response("cancel", "Cancel");
        if !admin {
            dialog.add_response("save-as", "Save As");
        }
        dialog.add_response("reload", "Reload");
        dialog.set_response_appearance("reload", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            let Some(panel) = weak.upgrade() else {
                return;
            };
            panel.change_dialog_open.set(false);
            match response {
                "reload" => panel.reload_pinned(),
                "save-as" => {
                    // Saving a copy does not satisfy a requested close.
                    panel.abort_close();
                    panel.choose_save_as();
                }
                _ => panel.abort_close(),
            }
        });
        let weak = Rc::downgrade(self);
        dialog.connect_closed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.change_dialog_open.set(false);
            }
        });
        self.present_dialog(dialog);
    }

    fn reload_pinned(self: &Rc<Self>) {
        self.chooser_reply.set(None);
        self.abort_close();
        let ticket = self.session.borrow_mut().reload_pinned();
        self.change_warning.set(false);
        self.text_view.set_editable(false);
        self.text_view.set_cursor_visible(false);
        self.clear_buffer();
        if let Some(ticket) = ticket {
            self.begin_load_display(&ticket);
        } else {
            self.show_current_state();
        }
    }

    // -----------------------------------------------------------------------
    // External monitors
    // -----------------------------------------------------------------------

    fn install_monitors(self: &Rc<Self>, path: &Path) {
        self.clear_monitors();
        let Some(parent) = path.parent() else {
            return;
        };
        let Ok(file_monitor) = gio::File::for_path(path)
            .monitor_file(gio::FileMonitorFlags::NONE, None::<&gio::Cancellable>)
        else {
            crate::log!("preview", "cannot monitor {}", path.display());
            return;
        };
        let Ok(parent_monitor) = gio::File::for_path(parent).monitor_directory(
            gio::FileMonitorFlags::WATCH_MOVES,
            None::<&gio::Cancellable>,
        ) else {
            crate::log!("preview", "cannot monitor {}", parent.display());
            return;
        };
        let watched = path.to_path_buf();
        let weak = Rc::downgrade(self);
        file_monitor.connect_changed(move |_, _, _, event| {
            if let Some(panel) = weak.upgrade() {
                crate::log_verbose!(
                    "preview",
                    "monitor event {event:?} on {}",
                    watched.display()
                );
                panel.schedule_monitor_check(watched.clone());
            }
        });
        let watched = path.to_path_buf();
        let weak = Rc::downgrade(self);
        parent_monitor.connect_changed(move |_, file, _, event| {
            if file.path().is_some_and(|changed| changed == watched)
                && let Some(panel) = weak.upgrade()
            {
                crate::log_verbose!(
                    "preview",
                    "parent monitor event {event:?} on {}",
                    watched.display()
                );
                panel.schedule_monitor_check(watched.clone());
            }
        });
        *self.monitors.borrow_mut() = Some(Monitors {
            _file: file_monitor,
            _parent: parent_monitor,
        });
    }

    fn clear_monitors(&self) {
        self.monitor_timer.cancel();
        self.monitors.borrow_mut().take();
    }

    fn schedule_monitor_check(self: &Rc<Self>, path: PathBuf) {
        if self.session.borrow().saving() {
            return;
        }
        if self
            .session
            .borrow()
            .current_document()
            .is_none_or(|document| document.path != path)
        {
            return;
        }
        let weak = Rc::downgrade(self);
        self.monitor_timer.schedule(
            Duration::from_millis(MONITOR_DEBOUNCE_MS),
            move |generation| {
                if let Some(panel) = weak.upgrade() {
                    panel.submit_control(ControlJob::Check { generation, path });
                }
            },
        );
    }

    fn apply_checked(
        self: &Rc<Self>,
        generation: u64,
        path: PathBuf,
        result: Result<FileRevision, SaveError>,
    ) {
        if generation != self.monitor_timer.generation() {
            return;
        }
        if self
            .session
            .borrow()
            .current_document()
            .is_none_or(|document| document.path != path)
        {
            return;
        }
        let current = self
            .session
            .borrow()
            .current_document()
            .map(|document| document.revision.clone());
        let Some(current) = current else {
            return;
        };
        let changed = match &result {
            Ok(revision) => !current.same_version(revision),
            Err(SaveError::Missing) => true,
            Err(_) => false,
        };
        if !changed {
            return;
        }
        let reload = self.session.borrow_mut().note_external_change();
        if reload {
            let ticket = self.session.borrow_mut().force_load(path);
            self.begin_load_display(&ticket);
        } else {
            self.change_warning.set(true);
            self.set_status("File changed on disk. Reload, Save As, or Cancel.", true);
            self.show_change_dialog();
        }
    }

    // -----------------------------------------------------------------------
    // Chrome
    // -----------------------------------------------------------------------

    fn update_title(&self) {
        let (mut text, tooltip) = Self::title_presentation(&self.session.borrow());
        text = text.replace('\n', " ");
        self.title.set_text(&text);
        let (wrap, lines, ellipsize) = title_layout(self.compact_title.get());
        self.title.set_wrap(wrap);
        self.title.set_lines(lines);
        self.title.set_ellipsize(ellipsize);
        self.title.set_xalign(0.0);
        self.title.set_tooltip_text(tooltip.as_deref());
        self.title
            .update_property(&[gtk::accessible::Property::Label(&text)]);
        self.refresh_notice();
    }

    fn title_presentation(session: &PreviewSession) -> (String, Option<String>) {
        let (text, tooltip) = if let Some(pinned) = session.pinned() {
            let document = &pinned.document;
            let suffix = if pinned.dirty {
                " — Unsaved changes"
            } else {
                ""
            };
            (
                format!(
                    "{}{}",
                    preview::document_title(&document.name, document.size),
                    suffix
                ),
                Some(document.path.display().to_string()),
            )
        } else {
            match session.state() {
                ViewState::Empty => ("File Name".to_string(), None),
                ViewState::Multiple { count } => (format!("{count} items selected"), None),
                ViewState::Trash => ("Trash".to_string(), None),
                ViewState::Loading { path } => (
                    path.file_name()
                        .map(|name| format!("{} — Loading…", name.to_string_lossy()))
                        .unwrap_or_else(|| "Loading…".to_string()),
                    Some(path.display().to_string()),
                ),
                ViewState::Document(document) => (
                    preview::document_title(&document.name, document.size),
                    Some(document.path.display().to_string()),
                ),
                ViewState::Folder(fm_core::preview::FolderInfo::Folder { name, path }) => {
                    (format!("{name} (folder)"), Some(path.display().to_string()))
                }
                ViewState::Unsupported {
                    name, reason, size, ..
                } => (
                    match size {
                        Some(size) => preview::document_title(name, *size),
                        None => name.clone(),
                    },
                    Some(reason.message()),
                ),
                ViewState::Failed { name, failure, .. } => (name.clone(), Some(failure.message())),
            }
        };
        (text.replace('\n', " "), tooltip)
    }

    /// Record the owner next to the loaded document, for the administrator
    /// status, banner and warning.
    fn record_owner(&self, document: &PreviewDocument) {
        self.owner_uid.set(owner_of_path(&document.path));
    }

    fn admin_editable_for(&self, document: &PreviewDocument) -> bool {
        let Some(owner) = self
            .owner_uid
            .get()
            .or_else(|| owner_of_path(&document.path))
        else {
            return false;
        };
        let Some(current) = self.current_uid.get() else {
            return false;
        };
        document.admin_editable(owner, current)
    }

    /// Whether the current clean document can enter administrator edit mode.
    pub fn admin_editable(&self) -> bool {
        self.session
            .borrow()
            .current_document()
            .is_some_and(|document| self.admin_editable_for(document))
    }

    /// Whether the pinned editor is in administrator mode.
    pub fn admin_editing(&self) -> bool {
        self.session.borrow().is_admin_editing()
    }

    /// P2z's administrator-tab dispatcher asks this before applying its
    /// read-only refusal to the preview commands.
    pub fn admin_allows(&self, command: &str) -> bool {
        match command {
            "preview.edit" => self.admin_editable(),
            "preview.save" | "preview.discard" | "preview.save-as" => self.admin_editing(),
            _ => false,
        }
    }

    fn refresh_admin_chrome(&self) {
        let editing = self.admin_editing();
        self.admin_banner.set_visible(editing);
        if editing {
            let owner = self
                .owner_uid
                .get()
                .or_else(|| {
                    self.session
                        .borrow()
                        .current_document()
                        .and_then(|document| owner_of_path(&document.path))
                })
                .map(preview::owner_name)
                .unwrap_or_else(|| "another user".to_string());
            let text = format!("Administrator: saving changes a file owned by {owner}.");
            self.admin_banner_label.set_text(&text);
            self.admin_banner_label
                .update_property(&[gtk::accessible::Property::Label(&text)]);
            self.root.add_css_class("gl-preview-admin");
        } else {
            self.admin_banner_label.set_text("");
            self.root.remove_css_class("gl-preview-admin");
        }
    }

    fn update_actions(&self) {
        let session = self.session.borrow();
        let editing = session.is_editing();
        let dirty = session.is_dirty();
        let saving = session.saving();
        let admin_editing = session.is_admin_editing();
        let text_document = session
            .current_document()
            .is_some_and(|document| document.text().is_some());
        let can_edit = session
            .current_document()
            .is_some_and(PreviewDocument::can_edit);
        let reason = session
            .current_document()
            .and_then(PreviewDocument::edit_block_reason);
        drop(session);
        let admin_editable = self.admin_editable();

        let discard_enabled = editing && !saving;
        let save_enabled = editing && dirty && !saving;
        let save_as_enabled = editing && !saving && !admin_editing;
        self.discard.set_sensitive(discard_enabled);
        self.save.set_sensitive(save_enabled);
        self.save_as.set_sensitive(save_as_enabled);
        // Save As isn't offered while editing as administrator, so it fades
        // out and its slot collapses; the spacer before Save then slides Save
        // into its place (Kyle, Nov 6). Leaving the mode reverses it.
        self.set_save_as_shown(!admin_editing);

        let tip = if !text_document {
            "Preview is read-only."
        } else if admin_editing {
            "Administrator edit mode."
        } else if !can_edit && admin_editable {
            "Alt-click to edit as administrator."
        } else if !can_edit {
            if let Some(reason) = &reason {
                reason.message()
            } else {
                "This text preview is read-only."
            }
        } else if editing {
            "Alt-click editing is active."
        } else {
            "Alt-click the text preview to edit."
        };
        self.discard.set_tooltip_text(Some(if discard_enabled {
            "Discard changes and reload from disk"
        } else {
            tip
        }));
        self.save
            .set_tooltip_text(Some(if save_enabled && admin_editing {
                "Save as administrator (asks first)"
            } else if save_enabled {
                "Save changes to this file"
            } else {
                tip
            }));
        self.save_as.set_tooltip_text(Some(if admin_editing {
            "Save As isn't available while editing as administrator."
        } else if save_as_enabled {
            "Save a copy to a new file"
        } else {
            tip
        }));
        self.refresh_admin_chrome();
    }

    /// Hide: fade Save As out, then collapse its slot so Save slides over.
    /// Show: open the slot (Save slides back), then fade Save As in. Both
    /// steps take their durations from the motion setting.
    fn set_save_as_shown(&self, shown: bool) {
        if self.save_as_shown.replace(shown) == shown {
            return;
        }
        let generation = self.save_as_generation.get() + 1;
        self.save_as_generation.set(generation);
        let weak = self.self_weak.borrow().clone();
        let (first_ms, second) = if shown {
            self.save_as_reveal.set_reveal_child(true);
            (SAVE_AS_SLIDE_MS, false)
        } else {
            self.save_as.add_css_class("gl-faded");
            (SAVE_AS_FADE_MS, true)
        };
        glib::timeout_add_local_once(
            Duration::from_millis(u64::from(crate::look::motion::ms(first_ms))),
            move || {
                let Some(panel) = weak.upgrade() else {
                    return;
                };
                if panel.save_as_generation.get() != generation {
                    return;
                }
                if second {
                    panel.save_as_reveal.set_reveal_child(false);
                } else {
                    panel.save_as.remove_css_class("gl-faded");
                }
            },
        );
    }

    fn set_status(&self, text: &str, warning: bool) {
        self.status.set_text(text);
        self.status.set_visible(!text.is_empty());
        if warning {
            self.status.add_css_class("gl-preview-warning");
        } else {
            self.status.remove_css_class("gl-preview-warning");
        }
        let generation = self.status_generation.get().wrapping_add(1);
        self.status_generation.set(generation);
        if warning || text.is_empty() {
            return;
        }
        let weak = self.self_weak.borrow().clone();
        glib::timeout_add_local_once(Duration::from_millis(STATUS_MS), move || {
            if let Some(panel) = weak.upgrade()
                && panel.status_generation.get() == generation
            {
                panel.clear_status();
            }
        });
    }

    fn clear_status(&self) {
        self.status_generation
            .set(self.status_generation.get().wrapping_add(1));
        self.status.set_text("");
        self.status.set_visible(false);
        self.status.remove_css_class("gl-preview-warning");
    }

    fn ui(&self) -> Option<Rc<crate::frame::WindowUi>> {
        self.process.upgrade()?.window(self.window)
    }

    // -----------------------------------------------------------------------
    // Gutter
    // -----------------------------------------------------------------------

    fn draw_gutter(
        &self,
        area: &gtk::DrawingArea,
        cr: &gtk::cairo::Context,
        width: i32,
        _height: i32,
    ) {
        if self.stack.visible_child_name().as_deref() != Some("text") {
            return;
        }
        let visible = self.text_view.visible_rect();
        if visible.height() <= 0 {
            return;
        }
        // These are buffer coordinates, as gtktextview.c:2531-2543 requires.
        let (first_iter, _) = self.text_view.line_at_y(visible.y());
        let (last_iter, _) = self
            .text_view
            .line_at_y(visible.y() + visible.height().saturating_sub(1));
        let first = first_iter.line();
        let last = last_iter.line().max(first);
        // The gutter starts slightly below/above the text view's allocation;
        // convert buffer line tops into the drawing area's coordinates.
        let view_origin = self
            .text_view
            .compute_bounds(&self.gutter)
            .map_or(0.0, |bounds| f64::from(bounds.y()));
        let lines = gutter_line_positions(
            visible.y(),
            visible.height(),
            first,
            last,
            view_origin,
            |line| {
                let iter = self.buffer.iter_at_line(line)?;
                Some(self.text_view.line_yrange(&iter))
            },
        );

        let color = area.color();
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            0.72,
        );
        let description = area.pango_context().font_description().unwrap_or_default();
        let family = description
            .family()
            .map(|family| family.to_string())
            .unwrap_or_else(|| "monospace".to_string());
        let size = f64::from(description.size()) / f64::from(pango::SCALE);
        cr.select_font_face(
            &family,
            gtk::cairo::FontSlant::Normal,
            gtk::cairo::FontWeight::Normal,
        );
        cr.set_font_size(size);
        let padding = scaled(GAP_S, self.scale.get()) as f64;
        for (line, y) in lines {
            let label = (line + 1).to_string();
            let Ok(extents) = cr.text_extents(&label) else {
                continue;
            };
            cr.move_to(
                f64::from(width) - extents.width() - padding,
                y - extents.y_bearing(),
            );
            let _ = cr.show_text(&label);
        }
    }
}

fn title_layout(compact: bool) -> (bool, i32, pango::EllipsizeMode) {
    if compact {
        (false, 1, pango::EllipsizeMode::End)
    } else {
        (true, 0, pango::EllipsizeMode::None)
    }
}

fn dirty_notice_text(name: &str, candidate: &str) -> String {
    format!(
        "Unsaved edits in “{name}”. The preview and Save still target {name}; Save or Discard before previewing {candidate}."
    )
}

fn notice_opacity(value: f64) -> f64 {
    let pulse = (value * std::f64::consts::PI * 4.0).sin().abs();
    1.0 - 0.55 * pulse
}

/// One number per logical line. `visible_top`/`visible_height` are GTK's
/// visible buffer rectangle; `view_origin` is the text view's top in gutter
/// coordinates. A partially visible line above the viewport keeps its (clipped)
/// number, and a wrapped continuation still appears only once.
fn gutter_line_positions(
    visible_top: i32,
    visible_height: i32,
    first: i32,
    last: i32,
    view_origin: f64,
    mut bounds: impl FnMut(i32) -> Option<(i32, i32)>,
) -> Vec<(i32, f64)> {
    if visible_height <= 0 || last < first {
        return Vec::new();
    }
    let bottom = i64::from(visible_top) + i64::from(visible_height);
    let mut lines = Vec::new();
    for line in first..=last {
        let Some((top, _line_height)) = bounds(line) else {
            continue;
        };
        if i64::from(top) >= bottom {
            continue;
        }
        lines.push((line, view_origin + f64::from(top - visible_top)));
    }
    lines
}

impl Drop for PreviewPanel {
    fn drop(&mut self) {
        self.monitor_timer.cancel();
        self.render_timer.cancel();
        self.cancel_pdf_render();
        if let Some(cancel) = self.image_cancel.borrow_mut().take() {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(pending) = self.workers.image.close() {
            pending
                .cancel()
                .store(true, std::sync::atomic::Ordering::Relaxed);
        }
        self.notice_animation.borrow_mut().take();
        let _ = self.monitors.borrow_mut().take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static TIMER_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from("/tmp").join(format!(
                "gimbal-preview-r2-{tag}-{}-{nanos}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.path.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }

        fn path(&self, name: &str) -> PathBuf {
            self.path.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn test_revision() -> FileRevision {
        FileRevision {
            device: 7,
            inode: 11,
            size: 4,
            modified_sec: 1,
            modified_nsec: 0,
            changed_sec: 1,
            changed_nsec: 0,
            etag: Some("test-etag".to_string()),
        }
    }

    fn test_document(path: &str, text: &str) -> PreviewDocument {
        PreviewDocument {
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            size: text.len() as u64,
            revision: test_revision(),
            access: fm_core::preview::WriteAccess {
                file_writable: true,
                parent_writable: true,
                reason: None,
            },
            content: fm_core::preview::PreviewContent::Text(fm_core::preview::TextDocument {
                text: text.to_string(),
                encoding: fm_core::preview::TextEncoding {
                    bom: false,
                    line_ending: Some(fm_core::preview::LineEnding::Lf),
                },
            }),
        }
    }

    fn dirty_preview(path: &str) -> PreviewSession {
        let mut session = PreviewSession::new();
        let ticket = session.select(Selection::file(path)).unwrap();
        session.load_finished(&ticket, Loaded::Document(test_document(path, "A")));
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        session
    }

    fn counted_wait(
        proceeded: &Rc<Cell<u32>>,
        cancelled: &Rc<Cell<u32>>,
    ) -> RefCell<Option<CloseWait>> {
        let proceeded = Rc::clone(proceeded);
        let cancelled = Rc::clone(cancelled);
        RefCell::new(Some(CloseWait {
            proceed: Rc::new(move || proceeded.set(proceeded.get() + 1)),
            cancel: Rc::new(move || cancelled.set(cancelled.get() + 1)),
        }))
    }

    fn saved_close_fixture() -> (PreviewSession, SaveTicket) {
        let mut session = dirty_preview("/a.txt");
        session.arm_close().unwrap();
        let save = session.begin_save_original().unwrap();
        session.bind_close_to_save(&save);
        assert_eq!(
            session.save_finished(&save, test_document("/a.txt", "saved")),
            SaveCompletion::Applied
        );
        (session, save)
    }

    fn lock_timers() -> std::sync::MutexGuard<'static, ()> {
        TIMER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spin_until(context: &glib::MainContext, done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the timer did not fire"
            );
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn spin_for(context: &glib::MainContext, duration: Duration) {
        let deadline = std::time::Instant::now() + duration;
        while std::time::Instant::now() < deadline {
            context.iteration(false);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn bump(cell: &Rc<Cell<u32>>) -> impl FnOnce(u64) + 'static {
        let cell = Rc::clone(cell);
        move |_| cell.set(cell.get() + 1)
    }

    #[test]
    fn an_expired_monitor_timer_can_be_rescheduled_without_removing_it_again() {
        let _guard = lock_timers();
        let context = glib::MainContext::default();
        let fired = Rc::new(Cell::new(0u32));
        let timer = MonitorTimer::new();
        timer.schedule(Duration::from_millis(2), bump(&fired));
        spin_until(&context, || fired.get() == 1);

        // The one-shot already fired. Rescheduling calls cancel first; the
        // expired SourceId must not be removed and panic.
        timer.schedule(Duration::from_millis(2), bump(&fired));
        spin_until(&context, || fired.get() == 2);
        timer.cancel();
    }

    #[test]
    fn an_expired_monitor_timer_survives_a_document_change() {
        let _guard = lock_timers();
        let context = glib::MainContext::default();
        let fired = Rc::new(Cell::new(0u32));
        let timer = MonitorTimer::new();
        timer.schedule(Duration::from_millis(2), bump(&fired));
        spin_until(&context, || fired.get() == 1);

        // clear_monitors after a clean reload reaches this cancel.
        timer.cancel();
        assert_eq!(fired.get(), 1);

        // A new document schedules again; the old generation is obsolete.
        timer.schedule(Duration::from_millis(2), bump(&fired));
        spin_until(&context, || fired.get() == 2);
        timer.cancel();
    }

    #[test]
    fn a_monitor_timer_cancelled_before_expiry_never_fires() {
        let _guard = lock_timers();
        let context = glib::MainContext::default();
        let fired = Rc::new(Cell::new(0u32));
        let timer = MonitorTimer::new();
        timer.schedule(Duration::from_millis(100), bump(&fired));
        timer.cancel();
        spin_for(&context, Duration::from_millis(20));
        assert_eq!(fired.get(), 0);
    }

    #[test]
    fn a_rescheduled_timer_generation_rejects_the_obsolete_callback() {
        let _guard = lock_timers();
        let context = glib::MainContext::default();
        let first = Rc::new(Cell::new(0u32));
        let second = Rc::new(Cell::new(0u32));
        let timer = MonitorTimer::new();
        timer.schedule(Duration::from_millis(20), bump(&first));
        timer.schedule(Duration::from_millis(2), bump(&second));
        spin_until(&context, || second.get() == 1);
        // The first source was removed by the superseding schedule.
        assert_eq!(first.get(), 0);
        timer.cancel();
    }

    #[test]
    fn gutter_numbers_follow_a_nonzero_visible_origin_and_wrap_once() {
        // 100 logical lines, 20 px apart; line 25 is three wrapped rows tall.
        let lines = gutter_line_positions(400, 205, 19, 30, 2.5, |line| {
            if line == 25 {
                Some((500, 60))
            } else {
                Some((line * 20, 20))
            }
        });
        assert_eq!(lines.first(), Some(&(19, -17.5)));
        assert_eq!(
            lines.iter().filter(|(line, _)| *line == 25).count(),
            1,
            "a wrapped continuation is numbered once at its logical start"
        );
        assert!(
            lines
                .iter()
                .any(|(line, y)| *line == 25 && (*y - 102.5).abs() < f64::EPSILON)
        );
        assert_eq!(lines.last(), Some(&(30, 202.5)));

        // A logical line starting exactly at the bottom edge is not visible.
        let clipped = gutter_line_positions(400, 200, 30, 30, 0.0, |line| Some((line * 20, 20)));
        assert!(clipped.is_empty());
    }

    // -----------------------------------------------------------------------
    // Save As destination, Replace and saved-close regressions
    // -----------------------------------------------------------------------

    #[test]
    fn a_missing_save_as_destination_is_confirmed_started_and_written() {
        let scratch = Scratch::new("missing-destination");
        let source = scratch.write("source.txt", b"original\n");
        let target = scratch.path("copy.txt");
        let loaded = match preview::load(&source) {
            Loaded::Document(document) => document,
            other => panic!("expected a document, got {other:?}"),
        };

        let session = RefCell::new(PreviewSession::new());
        let load = session
            .borrow_mut()
            .select(Selection::file(&source))
            .expect("the source is selected");
        session
            .borrow_mut()
            .load_finished(&load, Loaded::Document(loaded));
        session.borrow_mut().begin_edit(0).unwrap();
        session.borrow_mut().mark_dirty(1);

        // The production flow: Choosing -> CheckingDestination, then the
        // worker's Destination result reaches the real adapter.
        let request = session.borrow_mut().begin_save_as().unwrap();
        assert_eq!(
            session
                .borrow_mut()
                .choose_save_as_path(request.id, target.clone())
                .unwrap(),
            SaveAsRoute::NewPath
        );
        let destination = preview::inspect_destination(&target).unwrap();
        assert_eq!(destination, Destination::Missing);
        let save = match apply_destination_reply(&session, request.id, &target, Ok(destination)) {
            DestinationAction::StartSave(ticket) => ticket,
            _ => panic!("a missing destination must advance straight to a save ticket"),
        };
        assert!(!session.borrow().save_as_pending());
        assert!(session.borrow().saving());
        assert_eq!(save.path, target);
        assert!(save.new_path);

        let request = SaveRequest {
            path: save.path.clone(),
            text: "copied\n".to_string(),
            encoding: save.encoding,
            target: SaveTarget::NewPath {
                replace: save.replace.clone(),
            },
        };
        let success = preview::save(&request).unwrap();
        let document = PreviewDocument {
            path: success.path.clone(),
            name: "copy.txt".to_string(),
            size: success.size,
            revision: success.revision.clone(),
            access: fm_core::preview::WriteAccess {
                file_writable: true,
                parent_writable: true,
                reason: None,
            },
            content: fm_core::preview::PreviewContent::Text(fm_core::preview::TextDocument {
                text: request.text,
                encoding: request.encoding,
            }),
        };
        assert_eq!(
            session.borrow_mut().save_finished(&save, document),
            SaveCompletion::Applied
        );
        assert_eq!(fs::read(&target).unwrap(), b"copied\n");
        assert_eq!(fs::read(&source).unwrap(), b"original\n");
        assert!(!session.borrow().save_as_pending());
        assert!(!session.borrow().has_active_save());
        assert!(!session.borrow().saving());
        assert_eq!(
            session
                .borrow()
                .current_document()
                .map(|document| document.path.clone()),
            Some(target.clone())
        );
        assert_eq!(session.borrow().candidate(), &Selection::file(&source));
    }

    #[test]
    fn a_valid_replace_reply_confirms_and_starts_without_reentering_the_borrow() {
        let session = RefCell::new(dirty_preview("/a.txt"));
        let request = session.borrow_mut().begin_save_as().unwrap();
        let path = PathBuf::from("/dest.txt");
        session
            .borrow_mut()
            .choose_save_as_path(request.id, path.clone())
            .unwrap();
        let revision = test_revision();
        let destination = Destination::Existing {
            revision: revision.clone(),
            writable: true,
            reason: None,
        };
        // This is the production destination adapter followed by the exact
        // production Replace confirmation helper. A direct borrow across the
        // match would panic with `RefCell already borrowed`.
        let confirmed_revision =
            match apply_destination_reply(&session, request.id, &path, Ok(destination)) {
                DestinationAction::Replace(revision) => revision,
                _ => panic!("a writable existing destination must ask Replace"),
            };
        assert_eq!(confirmed_revision, revision);

        let ticket = confirm_and_start_save_as(&session, request.id)
            .expect("a valid Replace reply must confirm and start");
        assert!(ticket.new_path);
        assert_eq!(ticket.path, path);
        assert_eq!(
            ticket.replace.as_ref().map(|replace| &replace.revision),
            Some(&revision),
            "the confirmed destination revision is carried into the ticket"
        );
        assert!(!session.borrow().save_as_pending());
        assert!(session.borrow().saving());
    }

    #[test]
    fn a_stale_replace_reply_performs_no_write_or_mutation_of_a_later_editor() {
        let session = RefCell::new(dirty_preview("/a.txt"));
        let request = session.borrow_mut().begin_save_as().unwrap();
        let path = PathBuf::from("/dest.txt");
        session
            .borrow_mut()
            .choose_save_as_path(request.id, path.clone())
            .unwrap();
        session
            .borrow_mut()
            .save_as_destination(request.id, &path, Some(test_revision()))
            .unwrap();

        // A's request is abandoned and B becomes the pinned editor.
        session.borrow_mut().discard();
        let load = session
            .borrow_mut()
            .select(Selection::file("/b.txt"))
            .unwrap();
        session
            .borrow_mut()
            .load_finished(&load, Loaded::Document(test_document("/b.txt", "B")));
        session.borrow_mut().begin_edit(0).unwrap();
        session.borrow_mut().mark_dirty(3);
        let b_token = session.borrow().pinned().unwrap().action_token();

        assert!(
            confirm_and_start_save_as(&session, request.id).is_err(),
            "A's stale Replace reply must be refused, not panic"
        );
        let session = session.borrow();
        assert_eq!(
            session.pinned().unwrap().action_token(),
            b_token,
            "B's editor identity and buffer are untouched"
        );
        assert_eq!(session.pinned().unwrap().buffer_revision, 3);
        assert!(!session.has_active_save());
        assert!(!session.saving());
        assert!(!session.save_as_pending());
    }

    #[test]
    fn a_saved_close_waits_for_its_idle_and_proceeds_exactly_once() {
        let (session, save) = saved_close_fixture();
        let session = RefCell::new(session);
        let proceeded = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(0u32));
        let wait = counted_wait(&proceeded, &cancelled);

        let decision = complete_saved_close(&session, &wait, &save, || {});
        assert!(matches!(decision, SavedClose::Proceed(_)));
        assert_eq!(proceeded.get(), 1);
        assert_eq!(cancelled.get(), 0);
        assert!(session.borrow().close_request().is_none());
        assert!(wait.borrow().is_none());

        let again = complete_saved_close(&session, &wait, &save, || {});
        assert_eq!(again, SavedClose::None);
        assert_eq!(proceeded.get(), 1, "the continuation runs exactly once");
    }

    #[test]
    fn a_fresh_edit_before_the_saved_close_idle_keeps_the_window_open() {
        let (mut session, save) = saved_close_fixture();
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        let session = RefCell::new(session);
        let proceeded = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(0u32));
        let wait = counted_wait(&proceeded, &cancelled);

        let decision = complete_saved_close(&session, &wait, &save, || {});
        assert_eq!(decision, SavedClose::Abort);
        assert_eq!(proceeded.get(), 0, "a fresh edit must not be closed");
        assert_eq!(cancelled.get(), 1);
        assert!(session.borrow().close_request().is_none());
        assert!(wait.borrow().is_none());
        assert!(session.borrow().is_dirty());
    }

    #[test]
    fn a_new_save_before_the_saved_close_idle_keeps_the_window_open() {
        let (mut session, save) = saved_close_fixture();
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        session.begin_save_original().unwrap();
        let session = RefCell::new(session);
        let proceeded = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(0u32));
        let wait = counted_wait(&proceeded, &cancelled);

        let decision = complete_saved_close(&session, &wait, &save, || {});
        assert_eq!(decision, SavedClose::Abort);
        assert_eq!(proceeded.get(), 0);
        assert_eq!(cancelled.get(), 1);
        assert!(session.borrow().has_active_save());
    }

    #[test]
    fn a_cancelled_saved_close_never_runs_its_continuation() {
        let (mut session, save) = saved_close_fixture();
        session.invalidate_close();
        let session = RefCell::new(session);
        let proceeded = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(0u32));
        let wait = counted_wait(&proceeded, &cancelled);

        let decision = complete_saved_close(&session, &wait, &save, || {});
        assert_eq!(decision, SavedClose::Abort);
        assert_eq!(proceeded.get(), 0);
        assert_eq!(cancelled.get(), 1);
        assert!(wait.borrow().is_none(), "no stale CloseWait is left behind");
    }

    #[test]
    fn a_superseded_close_is_not_aborted_by_a_late_save_idle() {
        let (mut session, old_save) = saved_close_fixture();
        session.invalidate_close();
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        let newer_close = session.arm_close().unwrap();
        let session = RefCell::new(session);
        let proceeded = Rc::new(Cell::new(0u32));
        let cancelled = Rc::new(Cell::new(0u32));
        let wait = counted_wait(&proceeded, &cancelled);

        let decision = complete_saved_close(&session, &wait, &old_save, || {});
        assert_eq!(decision, SavedClose::None);
        assert_eq!(proceeded.get(), 0);
        assert_eq!(cancelled.get(), 0, "a newer close must not be cancelled");
        assert_eq!(
            session.borrow().close_request(),
            Some(&newer_close),
            "the newer request stays available for its own dialog"
        );
        assert!(wait.borrow().is_some());
    }

    #[test]
    fn a_late_save_result_only_aborts_a_continuation_it_owns() {
        let (mut session, old_save) = saved_close_fixture();
        session.invalidate_close();
        session.begin_edit(0).unwrap();
        session.mark_dirty(1);
        let newer_close = session.arm_close().unwrap();
        assert!(
            !close_owned_by_save(&session, true, &old_save),
            "a newer close request is not the old save's to cancel"
        );
        assert_eq!(session.close_request(), Some(&newer_close));

        let mut own = dirty_preview("/c.txt");
        own.arm_close().unwrap();
        let save = own.begin_save_original().unwrap();
        own.bind_close_to_save(&save);
        assert!(close_owned_by_save(&own, true, &save));
        own.invalidate_close();
        assert!(
            close_owned_by_save(&own, true, &save),
            "an orphaned wait belonging to this save is retired"
        );
        assert!(!close_owned_by_save(&own, false, &save));
    }

    #[test]
    fn title_presentation_covers_empty_loading_folder_error_and_dirty_states() {
        let empty = PreviewSession::new();
        assert_eq!(
            PreviewPanel::title_presentation(&empty),
            ("File Name".to_string(), None)
        );

        let mut loading = PreviewSession::new();
        let ticket = loading.select(Selection::file("/a/b.txt")).unwrap();
        let (title, _) = PreviewPanel::title_presentation(&loading);
        assert_eq!(title, "b.txt — Loading…");

        let mut folder = PreviewSession::new();
        let ticket2 = folder.select(Selection::file("/a/folder")).unwrap();
        folder.load_finished(
            &ticket2,
            Loaded::Folder(fm_core::preview::FolderInfo::Folder {
                path: PathBuf::from("/a/folder"),
                name: "folder".to_string(),
            }),
        );
        assert_eq!(
            PreviewPanel::title_presentation(&folder).0,
            "folder (folder)"
        );

        let mut failed = PreviewSession::new();
        let ticket3 = failed.select(Selection::file("/a/gone.txt")).unwrap();
        failed.load_failed(&ticket3, fm_core::preview::LoadFailure::Missing);
        assert_eq!(PreviewPanel::title_presentation(&failed).0, "gone.txt");

        let dirty = dirty_preview("/a.txt");
        assert_eq!(
            PreviewPanel::title_presentation(&dirty).0,
            "a.txt (1 B) — Unsaved changes"
        );

        // Compact mode is per-window; both forms keep the same text and only
        // change wrapping/ellipsis.
        assert_eq!(title_layout(false), (true, 0, pango::EllipsizeMode::None));
        assert_eq!(title_layout(true), (false, 1, pango::EllipsizeMode::End));
        // Keep the load ticket alive for the loading assertion above.
        let _ = ticket;
    }

    #[test]
    fn dirty_notice_names_the_edited_file_and_settles_visible_after_two_pulses() {
        let text = dirty_notice_text("notes.txt", "other.txt");
        assert!(text.contains("notes.txt"));
        assert!(text.contains("other.txt"));
        assert!(text.contains("Save or Discard"));
        assert!(
            (notice_opacity(0.0) - 1.0).abs() < f64::EPSILON,
            "the notice starts fully visible"
        );
        assert!(notice_opacity(0.125) < 0.6);
        assert!(notice_opacity(0.375) < 0.6);
        assert!(
            (notice_opacity(1.0) - 1.0).abs() < 1e-9,
            "the effect settles visible"
        );
    }

    #[test]
    fn zoom_modes_and_render_limits_are_shared_between_images_and_pdfs() {
        let source = (4000, 2000);
        let viewport = (800, 800);
        assert_eq!(ZoomMode::Fit.factor(source, viewport), 0.2);
        assert_eq!(ZoomMode::FitWidth.factor(source, viewport), 0.2);
        assert_eq!(ZoomMode::Actual.factor(source, viewport), 1.0);
        assert_eq!(ZoomMode::Manual(2.0).label(), "200%");
        assert_eq!(ZoomMode::Fit.label(), "Fit");
        assert_eq!(ZoomMode::FitWidth.label(), "Fit width");
        let fit = preview::fit_within(source.0, source.1, viewport.0, viewport.1);
        assert_eq!(fit, (800, 400));
        let plan = preview::render_plan(fit.0, fit.1, 2.0);
        assert_eq!(
            plan.size,
            fm_core::preview::RenderSize {
                width: 1600,
                height: 800
            }
        );
        let limited = preview::render_plan(100_000, 1, 1.0);
        assert!(limited.limit.is_some());
        assert!(limited.size.width <= fm_core::preview::MAX_RENDER_EDGE);
    }
    // -----------------------------------------------------------------------
    // P2m-r4: bounded preview work, newest-wins and save isolation
    // -----------------------------------------------------------------------

    fn preview_image(path: &Path) {
        let pixbuf =
            gtk::gdk_pixbuf::Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, 64, 64)
                .unwrap();
        pixbuf.savev(path, "png", &[]).unwrap();
    }

    fn preview_wait_until(done: impl Fn() -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(
                std::time::Instant::now() < deadline,
                "the preview scheduler did not progress"
            );
            std::thread::yield_now();
        }
    }

    fn render_job(path: &Path, generation: u64) -> ImageJob {
        ImageJob::RenderImage {
            generation,
            path: path.to_path_buf(),
            target: fm_core::preview::RenderSize {
                width: 64,
                height: 64,
            },
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[test]
    fn a_render_burst_coalesces_and_a_text_save_does_not_wait_behind_it() {
        let scratch = Scratch::new("queue-burst");
        let first = scratch.path("a.png");
        let second = scratch.path("b.png");
        let third = scratch.path("c.png");
        let newest = scratch.path("d.png");
        for path in [&first, &second, &third, &newest] {
            preview_image(path);
        }
        let gate = image::set_decode_gate(&scratch.path);
        let workers = spawn_workers();

        assert!(workers.image.submit(render_job(&first, 1)).is_ok());
        preview_wait_until(|| gate.entered() >= 1);
        for (generation, path) in [(2, &second), (3, &third), (4, &newest)] {
            match workers.image.submit(render_job(path, generation)) {
                Ok(previous) => {
                    if let Some(previous) = previous {
                        previous
                            .cancel()
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Err(_) => panic!("the latest image job was refused"),
            }
            // One in-flight decode plus exactly one newest pending job.
            assert!(workers.image.outstanding() <= 2);
        }
        assert_eq!(workers.image.outstanding(), 2);

        // A real save goes through the separate serialized worker while the
        // image decode is still held.
        let save_path = scratch.path("saved.txt");
        let save = SaveRequest {
            path: save_path.clone(),
            text: "saved while decoding\n".to_string(),
            encoding: fm_core::preview::TextEncoding {
                bom: false,
                line_ending: Some(fm_core::preview::LineEnding::Lf),
            },
            target: SaveTarget::NewPath { replace: None },
        };
        workers
            .control_outstanding
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        assert!(
            workers
                .control
                .send(ControlJob::Save {
                    id: 7,
                    request: save
                })
                .is_ok()
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let saved = loop {
            if let Ok(event) = workers.control_events.try_recv() {
                break event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the save worker was blocked by image decode"
            );
            std::thread::yield_now();
        };
        match saved {
            Event::Saved { id: 7, result } => {
                result.expect("the text save committed");
            }
            _ => panic!("the save worker returned another event"),
        }
        assert_eq!(fs::read(&save_path).unwrap(), b"saved while decoding\n");
        assert_eq!(workers.image.outstanding(), 2);

        gate.release();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        while !(seen.contains(&first) && seen.contains(&newest)) {
            if let Ok(Event::Rendered { path, .. } | Event::Loaded { path, .. }) =
                workers.image_events.try_recv()
            {
                seen.push(path);
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the in-flight and newest renders finish"
            );
            std::thread::yield_now();
        }
        assert_eq!(
            seen.len(),
            2,
            "only the in-flight and newest jobs ran: {seen:?}"
        );
        assert!(!seen.contains(&second));
        assert!(!seen.contains(&third));
        preview_wait_until(|| workers.image.outstanding() == 0);
    }

    #[test]
    fn closing_the_image_queue_drops_its_pending_job_and_finishes_accounting() {
        let scratch = Scratch::new("queue-close");
        let running = scratch.path("running.png");
        let pending = scratch.path("pending.png");
        preview_image(&running);
        preview_image(&pending);
        let gate = image::set_decode_gate(&scratch.path);
        let workers = spawn_workers();
        assert!(workers.image.submit(render_job(&running, 1)).is_ok());
        preview_wait_until(|| gate.entered() >= 1);
        assert!(workers.image.submit(render_job(&pending, 2)).is_ok());
        assert_eq!(workers.image.outstanding(), 2);

        let dropped = workers.image.close().expect("the pending job is returned");
        dropped
            .cancel()
            .store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(workers.image.outstanding(), 1);
        gate.release();
        preview_wait_until(|| workers.image.outstanding() == 0);
        let mut paths = Vec::new();
        while let Ok(Event::Rendered { path, .. } | Event::Loaded { path, .. }) =
            workers.image_events.try_recv()
        {
            paths.push(path);
        }
        assert!(paths.contains(&running));
        assert!(!paths.contains(&pending), "queued work is dropped on close");
    }
}
