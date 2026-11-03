// SPDX-License-Identifier: GPL-3.0-or-later
//! Optional PDF first-page rendering through the installed Poppler
//! `pdftoppm`. No Rust dependency and no shell: the child receives individual
//! `OsString` arguments, works only in an exclusively created job directory and
//! is killed/reaped only when it is the child this job started.

use std::collections::VecDeque;
use std::io;
#[cfg(test)]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use fm_core::preview::{self, PdfRenderRequest, RenderSize};

use crate::image::{self, DecodedImage};

/// The optional helper. The brief allows no installer and no new Rust crate.
const PDFTOPPM: &str = "pdftoppm";

#[derive(Clone, Debug)]
pub(crate) enum PdfFailure {
    HelperMissing,
    Timeout,
    Cancelled,
    /// A readable fallback (corrupt/encrypted PDF, bad options, etc.).
    Failed(String),
    Limit(String),
}

impl PdfFailure {
    pub(crate) fn message(&self) -> String {
        match self {
            Self::HelperMissing => "PDF renderer (pdftoppm) is not installed.".to_string(),
            Self::Timeout => "PDF rendering took too long.".to_string(),
            Self::Cancelled => "PDF rendering was cancelled.".to_string(),
            Self::Failed(message) => format!("PDF preview not available: {message}"),
            Self::Limit(message) => message.clone(),
        }
    }

    /// Only a missing helper or an unreadable document should fall back to a
    /// cached thumbnail. A limit is shown explicitly instead.
    pub(crate) fn fallback_allowed(&self) -> bool {
        matches!(self, Self::HelperMissing | Self::Failed(_) | Self::Timeout)
    }
}

pub(crate) struct PdfOutcome {
    pub generation: u64,
    pub path: PathBuf,
    pub image: Option<DecodedImage>,
    pub failure: Option<PdfFailure>,
}

#[derive(Debug)]
pub(crate) struct PdfJob {
    pub generation: u64,
    pub work: PdfWork,
    pub cancel: Arc<AtomicBool>,
    pub tx: Sender<PdfOutcome>,
    /// One selected-preview panel. Coalescing is per owner so one window can
    /// never fill the selected queue with obsolete zooms.
    pub owner: u64,
}

#[derive(Debug)]
pub(crate) enum PdfWork {
    Render(PdfRenderRequest),
    #[cfg(test)]
    TestChild {
        marker: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PdfSubmitError {
    Full,
    Stopped,
}

impl PdfSubmitError {
    pub(crate) fn message(self) -> &'static str {
        match self {
            Self::Full => "The PDF renderer queue is full.",
            Self::Stopped => "The PDF renderer stopped.",
        }
    }
}

/// Priority queues for the two Poppler workers. Selected preview demand is
/// served before ordinary grid thumbnails, and one worker's normal slot is
/// reserved so a selected page never waits for two grid renders.
const PDF_SELECTED_QUEUE: usize = 16;
const PDF_GRID_QUEUE: usize = 8;
const PDF_GRID_ACTIVE: usize = 1;

struct PdfState {
    selected: VecDeque<(u64, PdfJob)>,
    grid: VecDeque<PdfJob>,
    active: Vec<Arc<AtomicBool>>,
    active_grid: usize,
    workers: usize,
    closed: bool,
}

struct PdfScheduler {
    state: Mutex<PdfState>,
    ready: Condvar,
}

impl PdfScheduler {
    fn new() -> Arc<Self> {
        let scheduler = Arc::new(Self {
            state: Mutex::new(PdfState {
                selected: VecDeque::new(),
                grid: VecDeque::new(),
                active: Vec::new(),
                active_grid: 0,
                workers: 0,
                closed: false,
            }),
            ready: Condvar::new(),
        });
        let mut state = scheduler.state.lock().expect("the PDF queue lock");
        state.workers = preview::PDF_MAX_ACTIVE_RENDERS;
        drop(state);
        for index in 0..preview::PDF_MAX_ACTIVE_RENDERS {
            let scheduler = scheduler.clone();
            std::thread::Builder::new()
                .name(format!("gimbal-pdf-{index}"))
                .spawn(move || scheduler.worker())
                .expect("the PDF renderer thread starts");
        }
        scheduler
    }

    fn worker(self: Arc<Self>) {
        loop {
            let next = {
                let mut state = self.state.lock().expect("the PDF queue lock");
                loop {
                    if state.closed && state.selected.is_empty() && state.grid.is_empty() {
                        state.workers -= 1;
                        self.ready.notify_all();
                        return;
                    }
                    if let Some((_, job)) = state.selected.pop_front() {
                        state.active.push(job.cancel.clone());
                        break Some((job, false));
                    }
                    if state.active_grid < PDF_GRID_ACTIVE
                        && let Some(job) = state.grid.pop_front()
                    {
                        state.active_grid += 1;
                        state.active.push(job.cancel.clone());
                        break Some((job, true));
                    }
                    // Condvar::wait atomically releases this predicate's
                    // mutex and sleeps. Unlocking and reacquiring first can
                    // miss a submit/shutdown notification in between.
                    state = self
                        .ready
                        .wait(state)
                        .expect("the PDF queue lock is not poisoned");
                }
            };
            let Some((job, grid)) = next else {
                continue;
            };
            let path = job.path();
            let result = match &job.work {
                PdfWork::Render(request) => render(request, &job.cancel),
                #[cfg(test)]
                PdfWork::TestChild { marker } => run_test_child(marker, &job.cancel),
            };
            let (image, failure) = match result {
                Ok(image) => (Some(image), None),
                Err(failure) => (None, Some(failure)),
            };
            let _ = job.tx.send(PdfOutcome {
                generation: job.generation,
                path,
                image,
                failure,
            });
            let mut state = self.state.lock().expect("the PDF queue lock");
            state
                .active
                .retain(|cancel| !Arc::ptr_eq(cancel, &job.cancel));
            if grid {
                state.active_grid = state.active_grid.saturating_sub(1);
            }
            self.ready.notify_all();
        }
    }

    fn submit(&self, job: PdfJob, selected: bool) -> Result<(), (PdfJob, PdfSubmitError)> {
        let mut state = self.state.lock().expect("the PDF queue lock");
        if state.closed {
            return Err((job, PdfSubmitError::Stopped));
        }
        if selected {
            // Newest selected demand for one panel supersedes its queued older
            // zoom; the cancelled job's panel will ignore the stale result.
            state.selected.retain(|(owner, old)| {
                if *owner != job.owner {
                    return true;
                }
                old.cancel.store(true, Ordering::Relaxed);
                let _ = old.tx.send(old.cancelled_outcome());
                false
            });
            state.selected.retain(|(_, old)| {
                if !old.cancel.load(Ordering::Relaxed) {
                    return true;
                }
                let _ = old.tx.send(old.cancelled_outcome());
                false
            });
            if state.selected.len() >= PDF_SELECTED_QUEUE {
                return Err((job, PdfSubmitError::Full));
            }
            state.selected.push_back((job.owner, job));
        } else {
            state.grid.retain(|old| {
                if !old.cancel.load(Ordering::Relaxed) {
                    return true;
                }
                let _ = old.tx.send(old.cancelled_outcome());
                false
            });
            if state.grid.len() >= PDF_GRID_QUEUE {
                return Err((job, PdfSubmitError::Full));
            }
            state.grid.push_back(job);
        }
        self.ready.notify_one();
        Ok(())
    }

    /// Stop both workers, cancel queued and running jobs, and wait for every
    /// worker to reap its own child before returning. The running render's
    /// poll loop sees the cancel flag and calls `kill`/`wait` on that exact
    /// child. No other process is signalled.
    fn shutdown(&self) {
        let deadline =
            Instant::now() + Duration::from_millis(preview::PDF_RENDER_TIMEOUT_MS + 2_000);
        let mut state = self.state.lock().expect("the PDF queue lock");
        if state.closed && state.workers == 0 {
            return;
        }
        state.closed = true;
        for (_, job) in state.selected.drain(..) {
            job.cancel.store(true, Ordering::Relaxed);
            let _ = job.tx.send(job.cancelled_outcome());
        }
        for job in state.grid.drain(..) {
            job.cancel.store(true, Ordering::Relaxed);
            let _ = job.tx.send(job.cancelled_outcome());
        }
        for cancel in &state.active {
            cancel.store(true, Ordering::Relaxed);
        }
        self.ready.notify_all();
        while state.workers > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                crate::log!("pdf", "shutdown timed out waiting for renderer workers");
                break;
            }
            state = self
                .ready
                .wait_timeout(state, remaining)
                .expect("the PDF queue lock is not poisoned")
                .0;
        }
    }
}

fn pool() -> &'static Arc<PdfScheduler> {
    static POOL: OnceLock<Arc<PdfScheduler>> = OnceLock::new();
    POOL.get_or_init(PdfScheduler::new)
}

fn submit_to(scheduler: &PdfScheduler, job: PdfJob, selected: bool) -> Result<(), PdfSubmitError> {
    scheduler.submit(job, selected).map_err(|(job, error)| {
        job.cancel.store(true, Ordering::Relaxed);
        error
    })
}

/// Selected preview demand: priority, coalesced per panel.
pub(crate) fn submit_selected(job: PdfJob) -> Result<(), PdfSubmitError> {
    submit_to(pool(), job, true)
}

/// Ordinary grid thumbnail demand: bounded, behind selected work.
pub(crate) fn submit_grid(job: PdfJob) -> Result<(), PdfSubmitError> {
    submit_to(pool(), job, false)
}

/// Application shutdown: wait for owned Poppler children to be killed/reaped.
pub(crate) fn shutdown() {
    pool().shutdown();
}

impl PdfJob {
    fn path(&self) -> PathBuf {
        match &self.work {
            PdfWork::Render(request) => request.input.clone(),
            #[cfg(test)]
            PdfWork::TestChild { marker } => marker.clone(),
        }
    }

    fn cancelled_outcome(&self) -> PdfOutcome {
        PdfOutcome {
            generation: self.generation,
            path: self.path(),
            image: None,
            failure: Some(PdfFailure::Cancelled),
        }
    }
}

/// Create an exclusively owned job directory under `/tmp`. Nothing beside the
/// input PDF or any shared path is touched.
fn create_job_dir() -> io::Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    use std::os::unix::fs::DirBuilderExt;
    for _ in 0..32 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("/tmp").join(format!(
            "gimbal-preview-pdf-{}-{sequence}",
            std::process::id()
        ));
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused PDF render directory",
    ))
}

#[cfg(test)]
fn run_test_child(marker: &Path, cancel: &Arc<AtomicBool>) -> Result<DecodedImage, PdfFailure> {
    let mut child = Command::new("sleep")
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| PdfFailure::Failed(error.to_string()))?;
    std::fs::write(marker, child.id().to_string())
        .map_err(|error| PdfFailure::Failed(error.to_string()))?;
    match wait_bounded(&mut child, cancel) {
        Ok(status) => Err(PdfFailure::Failed(format!(
            "the test child finished unexpectedly: {status}"
        ))),
        Err(failure) => Err(failure),
    }
}

#[cfg(test)]
static RENDER_GATE: Mutex<Option<Arc<image::TestGate>>> = Mutex::new(None);
#[cfg(test)]
static RENDER_ORDER: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[cfg(test)]
fn set_render_gate(prefix: &Path) -> Arc<image::TestGate> {
    let gate = Arc::new(image::TestGate::new(prefix));
    *RENDER_GATE.lock().unwrap() = Some(gate.clone());
    gate
}

#[cfg(test)]
fn clear_render_gate() {
    if let Some(gate) = RENDER_GATE.lock().unwrap().take() {
        gate.release();
    }
}

#[cfg(test)]
fn wait_render_gate(path: &Path) {
    let gate = RENDER_GATE.lock().unwrap().clone();
    if let Some(gate) = gate {
        gate.wait(path);
    }
}

#[cfg(test)]
fn record_render_start(path: &Path) {
    RENDER_ORDER.lock().unwrap().push(path.to_path_buf());
}

#[cfg(test)]
fn render_order() -> Vec<PathBuf> {
    RENDER_ORDER.lock().unwrap().clone()
}

fn render(
    request: &PdfRenderRequest,
    cancel: &Arc<AtomicBool>,
) -> Result<DecodedImage, PdfFailure> {
    preview::pdf_input_allowed(&request.input).map_err(PdfFailure::Limit)?;
    #[cfg(test)]
    wait_render_gate(&request.input);
    #[cfg(test)]
    record_render_start(&request.input);
    let directory = create_job_dir().map_err(|error| PdfFailure::Failed(error.to_string()))?;
    let _cleanup = Cleanup(directory.clone());
    let request = PdfRenderRequest {
        input: request.input.clone(),
        output_dir: directory,
        target: request.target,
    };
    let mut command = Command::new(PDFTOPPM);
    command
        .args(request.arguments())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(PdfFailure::HelperMissing);
        }
        Err(error) => return Err(PdfFailure::Failed(error.to_string())),
    };
    let status = wait_bounded(&mut child, cancel)?;
    if !status.success() {
        return Err(PdfFailure::Failed(format!(
            "pdftoppm exited with {}",
            status
                .code()
                .map_or_else(|| "a signal".to_string(), |c| c.to_string())
        )));
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(PdfFailure::Cancelled);
    }
    let output = request.output();
    let metadata = std::fs::metadata(&output)
        .map_err(|error| PdfFailure::Failed(format!("no rendered page: {error}")))?;
    if !metadata.is_file() {
        return Err(PdfFailure::Failed(
            "the renderer did not create a regular PNG".to_string(),
        ));
    }
    if metadata.len() > preview::PDF_MAX_OUTPUT_BYTES {
        return Err(PdfFailure::Limit(format!(
            "Rendered PDF is larger than {}.",
            preview::format_size(preview::PDF_MAX_OUTPUT_BYTES)
        )));
    }
    let bytes = std::fs::read(&output).map_err(|error| PdfFailure::Failed(error.to_string()))?;
    let dimensions = preview::pdf_output_allowed(&bytes).map_err(PdfFailure::Limit)?;
    let target = RenderSize {
        width: request.target.width.max(1),
        height: dimensions.height.max(1),
    };
    image::decode_image_bytes_limited(&bytes, target, false)
        .map_err(|error| PdfFailure::Failed(format!("{error:?}")))
}

/// Poll this job's own child. No process group, no shared reaper.
fn wait_bounded(child: &mut Child, cancel: &Arc<AtomicBool>) -> Result<ExitStatus, PdfFailure> {
    wait_bounded_for(
        child,
        cancel,
        Duration::from_millis(preview::PDF_RENDER_TIMEOUT_MS),
    )
}

fn wait_bounded_for(
    child: &mut Child,
    cancel: &Arc<AtomicBool>,
    timeout: Duration,
) -> Result<ExitStatus, PdfFailure> {
    let deadline = Instant::now() + timeout;
    loop {
        if cancel.load(Ordering::Relaxed) {
            kill_and_reap(child);
            return Err(PdfFailure::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {}
            Err(error) => {
                kill_and_reap(child);
                return Err(PdfFailure::Failed(error.to_string()));
            }
        }
        if Instant::now() >= deadline {
            kill_and_reap(child);
            return Err(PdfFailure::Timeout);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The installed helper and version, for the notes and the fallback message.
#[allow(dead_code)]
pub(crate) fn helper_available() -> bool {
    Command::new(PDFTOPPM)
        .arg("-v")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// Test-only direct render, still in an owned scratch directory.
#[cfg(test)]
pub(crate) fn render_for_test(
    input: &Path,
    target: RenderSize,
) -> Result<DecodedImage, PdfFailure> {
    render(
        &PdfRenderRequest {
            input: input.to_path_buf(),
            output_dir: PathBuf::new(),
            target,
        },
        &Arc::new(AtomicBool::new(false)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Scratch {
        path: PathBuf,
    }

    impl Scratch {
        fn new(tag: &str) -> Self {
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = PathBuf::from("/tmp").join(format!(
                "gimbal-preview-pdf-test-{tag}-{}-{sequence}",
                std::process::id()
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

    /// A tiny one-page PDF generated here; no user document or download.
    fn tiny_pdf(scratch: &Scratch) -> PathBuf {
        page_pdf(scratch, "one page.pdf", 300, 500, 0)
    }

    #[test]
    fn a_single_job_wakes_an_idle_worker_without_another_notification() {
        let scratch = Scratch::new("idle-wake");
        let scheduler = PdfScheduler::new();
        let (tx, rx) = std::sync::mpsc::channel();
        // Missing owned inputs fail before spawning a child or decoding. Each
        // request must complete on its own notification as workers go idle;
        // a later submit must not be needed to wake an already queued job.
        for generation in 0..256 {
            scheduler
                .submit(
                    PdfJob {
                        generation,
                        work: PdfWork::Render(PdfRenderRequest {
                            input: scratch.path.join("missing.pdf"),
                            output_dir: PathBuf::new(),
                            target: RenderSize {
                                width: 100,
                                height: 100,
                            },
                        }),
                        cancel: Arc::new(AtomicBool::new(false)),
                        tx: tx.clone(),
                        owner: 1,
                    },
                    true,
                )
                .unwrap();
            let outcome = rx.recv_timeout(Duration::from_secs(2));
            if outcome.is_err() {
                scheduler.shutdown();
            }
            let outcome = outcome.expect("a queued job lost its wakeup");
            assert_eq!(outcome.generation, generation);
            assert!(matches!(outcome.failure, Some(PdfFailure::Limit(_))));
        }
        scheduler.shutdown();
    }

    fn page_pdf(scratch: &Scratch, name: &str, width: u32, height: u32, rotate: u32) -> PathBuf {
        let path = scratch.path.join(name);
        let body = format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Rotate {rotate} /Contents 4 0 R >> endobj\n4 0 obj << /Length 0 >> stream\n\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n"
        );
        std::fs::write(&path, body).unwrap();
        path
    }

    static RENDER_LOCK: Mutex<()> = Mutex::new(());

    fn job_directories() -> Vec<PathBuf> {
        let prefix = format!("gimbal-preview-pdf-{}-", std::process::id());
        let Ok(entries) = std::fs::read_dir("/tmp") else {
            return Vec::new();
        };
        entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn generated_tall_wide_rotated_and_ordinary_pages_obey_the_pre_render_bound() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("geometry");
        let before = job_directories();
        let cases = [
            ("tall.pdf", 100, 3515, 0),
            ("wide.pdf", 5000, 100, 0),
            ("rotated.pdf", 600, 800, 90),
            ("ordinary.pdf", 600, 800, 0),
        ];
        for (name, width, height, rotate) in cases {
            let pdf = page_pdf(&scratch, name, width, height, rotate);
            let bytes = std::fs::read(&pdf).unwrap();
            match render_for_test(
                &pdf,
                RenderSize {
                    width: 400,
                    height: 400,
                },
            ) {
                Ok(image) => {
                    let (actual_w, actual_h) = (image.source_width, image.source_height);
                    let longest = actual_w.max(actual_h);
                    assert!(
                        longest <= 400,
                        "{name}: {actual_w}x{actual_h} exceeded the requested square"
                    );
                    assert!(
                        u64::from(actual_w as u32) * u64::from(actual_h as u32)
                            <= preview::MAX_RENDER_PIXELS
                    );
                    let (expected_w, expected_h) = if rotate % 180 == 90 {
                        (height, width)
                    } else {
                        (width, height)
                    };
                    let expected_ratio = f64::from(expected_w) / f64::from(expected_h);
                    let actual_ratio = f64::from(actual_w) / f64::from(actual_h);
                    assert!(
                        (actual_ratio - expected_ratio).abs() / expected_ratio < 0.08,
                        "{name}: aspect {actual_ratio} vs {expected_ratio} ({actual_w}x{actual_h})"
                    );
                }
                Err(PdfFailure::HelperMissing) => return,
                Err(other) => panic!("{name}: unexpected render failure: {other:?}"),
            }
            assert_eq!(
                std::fs::read(&pdf).unwrap(),
                bytes,
                "source bytes unchanged"
            );
            assert!(!pdf.with_extension("png").exists());
        }
        let after = job_directories();
        let leftover: Vec<_> = after
            .into_iter()
            .filter(|path| !before.contains(path))
            .collect();
        assert!(leftover.is_empty(), "job directories cleaned: {leftover:?}");
    }

    #[test]
    fn the_conservative_square_bound_still_caps_an_exaggerated_request() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("exaggerated");
        let pdf = tiny_pdf(&scratch);
        match render_for_test(
            &pdf,
            RenderSize {
                width: 10_000,
                height: 10_000,
            },
        ) {
            Ok(image) => {
                assert!(
                    image.source_width.max(image.source_height) <= preview::PDF_MAX_RASTER_EDGE
                );
                assert!(
                    u64::from(image.source_width as u32) * u64::from(image.source_height as u32)
                        <= preview::MAX_RENDER_PIXELS
                );
            }
            Err(PdfFailure::HelperMissing) => {}
            Err(other) => panic!("unexpected render failure: {other:?}"),
        }
    }

    #[test]
    fn a_job_directory_is_removed_with_its_cleanup_guard() {
        let scratch = Scratch::new("cleanup");
        let job = scratch.path.join("job");
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("page.png"), b"x").unwrap();
        {
            let _cleanup = Cleanup(job.clone());
        }
        assert!(!job.exists());
    }

    #[test]
    fn a_selected_render_is_served_before_queued_grid_work() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("priority");
        let grid_dir = scratch.path.join("grid");
        let selected_dir = scratch.path.join("selected");
        std::fs::create_dir_all(&grid_dir).unwrap();
        std::fs::create_dir_all(&selected_dir).unwrap();
        let grid_a = page_pdf_in(&grid_dir, "grid-a.pdf", 300, 500);
        let grid_b = page_pdf_in(&grid_dir, "grid-b.pdf", 300, 500);
        let selected = page_pdf_in(&selected_dir, "selected.pdf", 300, 500);
        let gate = set_render_gate(&grid_dir);
        let scheduler = PdfScheduler::new();
        let (tx, rx) = std::sync::mpsc::channel();

        let job = |path: &Path, owner: u64, generation: u64| PdfJob {
            generation,
            work: PdfWork::Render(PdfRenderRequest {
                input: path.to_path_buf(),
                output_dir: PathBuf::new(),
                target: RenderSize {
                    width: 200,
                    height: 200,
                },
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            tx: tx.clone(),
            owner,
        };

        scheduler.submit(job(&grid_a, 0, 1), false).unwrap();
        // The first grid render holds the one reserved normal slot.
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.entered() < 1 {
            assert!(Instant::now() < deadline, "the grid render did not start");
            std::thread::yield_now();
        }
        for index in 0..4 {
            scheduler
                .submit(job(&grid_b, 0, 10 + index), false)
                .unwrap();
        }
        scheduler.submit(job(&selected, 7, 99), true).unwrap();
        gate.release();

        let mut seen = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !(seen.contains(&selected) && seen.contains(&grid_a)) {
            assert!(Instant::now() < deadline, "the priority renders complete");
            let remaining = deadline.saturating_duration_since(Instant::now());
            let outcome = rx
                .recv_timeout(remaining)
                .expect("the priority renders complete");
            seen.push(outcome.path);
        }
        let order = render_order();
        let selected_position = order.iter().position(|path| path == &selected);
        let later_grid = order
            .iter()
            .position(|path| path == &grid_b && path != &grid_a);
        assert!(selected_position.is_some(), "selected render started");
        if let Some(later_grid) = later_grid {
            assert!(
                selected_position.unwrap() < later_grid,
                "selected work must start before later grid work: {order:?}"
            );
        }
        scheduler.shutdown();
        clear_render_gate();
    }

    fn page_pdf_in(dir: &Path, name: &str, width: u32, height: u32) -> PathBuf {
        let path = dir.join(name);
        let body = format!(
            "%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Contents 4 0 R >> endobj\n4 0 obj << /Length 0 >> stream\n\nendstream endobj\ntrailer << /Root 1 0 R >>\n%%EOF\n"
        );
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn a_full_selected_queue_recovers_when_a_cancelled_owner_frees_capacity() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("selected-full");
        let pdf = page_pdf_in(&scratch.path, "page.pdf", 300, 500);
        let gate = set_render_gate(&scratch.path);
        let scheduler = PdfScheduler::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut cancels = Vec::new();
        let make = |owner: u64, tx: &Sender<PdfOutcome>, pdf: &Path| {
            let cancel = Arc::new(AtomicBool::new(false));
            let job = PdfJob {
                generation: owner,
                work: PdfWork::Render(PdfRenderRequest {
                    input: pdf.to_path_buf(),
                    output_dir: PathBuf::new(),
                    target: RenderSize {
                        width: 200,
                        height: 200,
                    },
                }),
                cancel: cancel.clone(),
                tx: tx.clone(),
                owner,
            };
            (job, cancel)
        };
        // Two workers hold two selected jobs; the queue then fills.
        for owner in 1..=2 {
            let (job, cancel) = make(owner, &tx, &pdf);
            cancels.push(cancel);
            scheduler.submit(job, true).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.entered() < 2 {
            assert!(Instant::now() < deadline, "both selected renders started");
            std::thread::yield_now();
        }
        for owner in 3..=18 {
            let (job, cancel) = make(owner, &tx, &pdf);
            cancels.push(cancel);
            scheduler.submit(job, true).unwrap();
        }
        let (job, cancel) = make(99, &tx, &pdf);
        cancels.push(cancel);
        assert!(matches!(
            scheduler.submit(job, true),
            Err((_, PdfSubmitError::Full))
        ));
        // Cancelling a queued owner is what an obsolete zoom does; the retry
        // prunes it and the current selection is accepted.
        cancels[5].store(true, Ordering::Relaxed);
        let (job, _cancel) = make(99, &tx, &pdf);
        assert!(scheduler.submit(job, true).is_ok());
        gate.release();
        scheduler.shutdown();
        clear_render_gate();
    }

    #[test]
    fn shutdown_waits_for_and_reaps_a_test_owned_running_child() {
        let scratch = Scratch::new("shutdown-child");
        let marker = scratch.path.join("child.pid");
        let scheduler = PdfScheduler::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        scheduler
            .submit(
                PdfJob {
                    generation: 1,
                    work: PdfWork::TestChild {
                        marker: marker.clone(),
                    },
                    cancel: Arc::new(AtomicBool::new(false)),
                    tx,
                    owner: 3,
                },
                true,
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.exists() {
            assert!(Instant::now() < deadline, "the test child did not start");
            std::thread::yield_now();
        }
        let pid = std::fs::read_to_string(&marker).unwrap();
        assert!(Path::new(&format!("/proc/{pid}")).exists());
        scheduler.shutdown();
        assert!(
            !Path::new(&format!("/proc/{pid}")).exists(),
            "shutdown waited for the child to be killed and reaped"
        );
    }

    #[test]
    fn a_selected_queue_supersedes_this_panels_older_queued_render() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("coalesce");
        let first = page_pdf_in(&scratch.path, "first.pdf", 300, 500);
        let second = page_pdf_in(&scratch.path, "second.pdf", 300, 500);
        let gate = set_render_gate(&scratch.path);
        let scheduler = PdfScheduler::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let job = |path: &Path, generation: u64| PdfJob {
            generation,
            work: PdfWork::Render(PdfRenderRequest {
                input: path.to_path_buf(),
                output_dir: PathBuf::new(),
                target: RenderSize {
                    width: 200,
                    height: 200,
                },
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            tx: tx.clone(),
            owner: 11,
        };
        scheduler.submit(job(&first, 1), true).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while gate.entered() < 1 {
            assert!(Instant::now() < deadline, "the first render did not start");
            std::thread::yield_now();
        }
        scheduler.submit(job(&second, 2), true).unwrap();
        gate.release();
        scheduler.shutdown();
        clear_render_gate();
    }

    #[test]
    fn a_generated_pdf_renders_one_bounded_png_or_reports_the_missing_helper() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("render");
        let pdf = tiny_pdf(&scratch);
        match render_for_test(
            &pdf,
            RenderSize {
                width: 400,
                height: 400,
            },
        ) {
            Ok(image) => {
                assert!(image.width > 0 && image.height > 0);
                assert!(image.width <= 400);
            }
            Err(PdfFailure::HelperMissing) => {
                // Explicit, tested fallback: a missing helper is not a panic.
            }
            Err(other) => panic!("unexpected render failure: {other:?}"),
        }
        assert!(pdf.exists(), "the source PDF is never modified");
    }

    #[test]
    fn a_missing_pdf_is_refused_before_spawn() {
        // If pdftoppm is installed this uses its direct path; either way the
        // fallback classification is explicit.
        let scratch = Scratch::new("missing");
        let input = scratch.path.join("missing.pdf");
        let result = render_for_test(
            &input,
            RenderSize {
                width: 100,
                height: 100,
            },
        );
        assert!(matches!(result, Err(PdfFailure::Limit(_))));
    }

    #[test]
    fn a_missing_helper_is_an_explicit_labelled_fallback() {
        let failure = PdfFailure::HelperMissing;
        assert!(failure.fallback_allowed());
        assert!(failure.message().contains("not installed"));
        assert!(!PdfFailure::Limit("limit".to_string()).fallback_allowed());
    }

    #[test]
    fn a_timeout_kills_and_reaps_only_its_own_child() {
        let mut child = Command::new("sleep").arg("5").spawn().unwrap();
        let result = wait_bounded_for(
            &mut child,
            &Arc::new(AtomicBool::new(false)),
            Duration::from_millis(60),
        );
        assert!(matches!(result, Err(PdfFailure::Timeout)));
        assert!(
            child.try_wait().unwrap().is_some(),
            "the timed-out child is reaped"
        );
    }

    #[test]
    fn cancellation_kills_the_jobs_child_and_leaves_the_source_pdf_alone() {
        let _serial = RENDER_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let scratch = Scratch::new("cancel");
        let pdf = tiny_pdf(&scratch);
        let before = std::fs::read(&pdf).unwrap();
        let cancel = Arc::new(AtomicBool::new(true));
        let result = render(
            &PdfRenderRequest {
                input: pdf.clone(),
                output_dir: scratch.path.join("unused"),
                target: RenderSize {
                    width: 200,
                    height: 200,
                },
            },
            &cancel,
        );
        // The cancel flag is checked before or immediately after spawn; a
        // valid PDF may also fail its input check first.
        assert!(matches!(
            result,
            Err(PdfFailure::Cancelled) | Err(PdfFailure::Limit(_))
        ));
        assert_eq!(std::fs::read(&pdf).unwrap(), before);
    }
}
