// SPDX-License-Identifier: GPL-3.0-or-later
//! Production-path tests for the search pool, session and pure routing.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fm_core::folder_size::SystemClock;
use fm_core::search::PartialReason;
use fm_core::workspace::{TabId, WindowId};
use gtk::gdk;

use super::test_support::{Gate, TestSource};
use super::*;

fn wait_until(timeout_ms: u64, mut ready: impl FnMut() -> bool) -> bool {
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(timeout_ms) {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    ready()
}

fn service(source: TestSource) -> Arc<SearchService> {
    Arc::new(SearchService::with_parts(
        Arc::new(source),
        Arc::new(SystemClock),
    ))
}

fn view(root: &str, rows: usize, phase: SearchPhase) -> SearchView {
    SearchView {
        generation: 1,
        root: PathBuf::from(root),
        walked_root: None,
        query: "q".to_string(),
        rows,
        total_matches: rows as u64,
        truncated: false,
        visited: 0,
        skipped: 0,
        mounts_skipped: 0,
        examples: Vec::new(),
        phase,
        notice: None,
    }
}

struct Job {
    token: u64,
    cancel: Arc<AtomicBool>,
    events: Arc<BatchQueue>,
}

fn start_job(service: &SearchService, root: &str) -> Job {
    let cancel = Arc::new(AtomicBool::new(false));
    let events = Arc::new(BatchQueue::new());
    let progress = Arc::new(ProgressCell::new());
    let token = match service.start(
        fm_core::search::SearchRequest::new(PathBuf::from(root), "x"),
        cancel.clone(),
        events.clone(),
        progress,
    ) {
        StartOutcome::Started(token) => token,
        other => panic!("expected a started job, got {other:?}"),
    };
    Job {
        token,
        cancel,
        events,
    }
}

#[test]
fn status_never_says_no_matches_for_partial_or_unavailable() {
    let complete = view("/r", 0, SearchPhase::Complete);
    assert_eq!(status_text(&complete), "No matches");

    let searching = view("/r", 3, SearchPhase::Searching { queued: false });
    assert!(status_text(&searching).starts_with("Searching… 3 matches"));

    let partial = view(
        "/r",
        3,
        SearchPhase::Partial(vec![PartialReason::EntryLimit]),
    );
    assert!(status_text(&partial).contains("Partial results"));
    assert!(!status_text(&partial).contains("No matches"));

    let mut truncated = view(
        "/r",
        2,
        SearchPhase::Partial(vec![PartialReason::ResultLimit]),
    );
    truncated.total_matches = 21;
    truncated.truncated = true;
    let text = status_text(&truncated);
    assert!(text.contains("2 best of 21 matches"), "{text}");
    assert!(text.contains("result limit"), "{text}");
    assert!(!text.contains("No matches"));

    let unavailable = view("/r", 0, SearchPhase::Unavailable(RootFailure::Missing));
    assert!(status_text(&unavailable).contains("Search unavailable"));
    assert!(!status_text(&unavailable).contains("No matches"));

    let cancelled = view("/r", 2, SearchPhase::Cancelled);
    assert!(status_text(&cancelled).contains("Search cancelled"));
}

#[test]
fn scope_names_the_device_and_mount_rule() {
    let mut search = view("/r", 0, SearchPhase::Searching { queued: false });
    search.mounts_skipped = 2;
    let scope = scope_text(&search);
    assert!(scope.contains("Same filesystem only"));
    assert!(scope.contains("hidden entries included"));
    assert!(scope.contains("2 mount point(s) listed, not entered"));
}

#[test]
fn file_actions_are_refused_but_navigation_and_refresh_are_allowed() {
    for command in [
        "file.trash",
        "file.copy",
        "file.move",
        "file.rename",
        "file.new-folder",
        "file.new-file",
        "file.undo",
        "file.cut",
        "file.copy-to-clipboard",
        "file.paste",
        "file.open",
        "preview.edit",
        "selection.all",
        "selection.none",
    ] {
        assert!(
            refusal_for(command).is_some(),
            "{command} must be refused while results are shown"
        );
    }
    for command in ["search.open", "nav.go", "nav.refresh", "view.set-sort"] {
        assert!(
            refusal_for(command).is_none(),
            "{command} stays available while results are shown"
        );
    }
}

#[test]
fn search_owns_its_keys_without_modifiers() {
    let empty = gdk::ModifierType::empty();
    assert!(search_entry_owns_key(gdk::Key::Return, empty));
    assert!(search_entry_owns_key(gdk::Key::Home, empty));
    assert!(search_entry_owns_key(gdk::Key::Down, empty));
    assert!(search_entry_owns_key(gdk::Key::Escape, empty));
    assert!(!search_entry_owns_key(
        gdk::Key::Return,
        gdk::ModifierType::CONTROL_MASK
    ));
    assert!(search_results_own_key(gdk::Key::Up, empty));
    assert!(search_results_own_key(gdk::Key::Page_Down, empty));
    assert!(search_results_own_key(gdk::Key::KP_Enter, empty));
    assert!(search_results_own_key(gdk::Key::Escape, empty));
    assert!(!search_results_own_key(
        gdk::Key::Down,
        gdk::ModifierType::ALT_MASK
    ));
}

#[test]
fn at_most_eight_window_sessions_are_admitted() {
    let source = TestSource::new()
        .dir("/r", &["/r/a.txt"])
        .file("/r/a.txt", 1);
    let runtime = SearchRuntime::with_service(service(source));
    for index in 0..MAX_SESSIONS {
        runtime
            .submit(
                WindowId::from_raw(index as u64 + 1),
                TabId::from_raw(1),
                PathBuf::from("/r"),
                "a".to_string(),
            )
            .expect("an admitted session");
    }
    let error = runtime
        .submit(
            WindowId::from_raw(99),
            TabId::from_raw(1),
            PathBuf::from("/r"),
            "a".to_string(),
        )
        .expect_err("the ninth window is busy");
    assert!(error.to_lowercase().contains("busy"));
    assert_eq!(runtime.session_count(), MAX_SESSIONS);
}

#[test]
fn a_new_query_replaces_the_old_one_and_old_batches_never_apply() {
    let gate = Arc::new(Gate::default());
    let source = TestSource::new()
        .with_gate(gate.clone())
        .dir("/slow", &["/slow/old.txt"])
        .file("/slow/old.txt", 1)
        .block("/slow")
        .dir("/fast", &["/fast/new.txt"])
        .file("/fast/new.txt", 1);
    let runtime = SearchRuntime::with_service(service(source));
    let window = WindowId::from_raw(7);
    let tab = TabId::from_raw(9);
    let first = runtime
        .submit(window, tab, PathBuf::from("/slow"), "old".to_string())
        .expect("first query");
    assert!(wait_until(1_000, || gate.entered() == 1));

    let second = runtime
        .submit(window, tab, PathBuf::from("/fast"), "new".to_string())
        .expect("replacement query");
    assert_ne!(first.generation, second.generation);
    assert!(wait_until(1_000, || {
        let _ = runtime.poll(window);
        runtime
            .view(window)
            .is_some_and(|view| view.phase == SearchPhase::Complete && view.rows == 1)
    }));

    gate.release();
    let _ = wait_until(1_000, || runtime.poll(window).changes.is_empty());
    let current = runtime.view(window).expect("current view");
    assert_eq!(current.root, PathBuf::from("/fast"));
    assert_eq!(current.rows, 1);
    let hit = runtime
        .result(window, current.generation, 0)
        .expect("the exact result");
    assert_eq!(hit.path, PathBuf::from("/fast/new.txt"));
}

#[test]
fn clearing_and_closing_remove_the_session_and_ignore_late_results() {
    let gate = Arc::new(Gate::default());
    let source = TestSource::new()
        .with_gate(gate.clone())
        .dir("/slow", &["/slow/old.txt"])
        .file("/slow/old.txt", 1)
        .block("/slow");
    let runtime = SearchRuntime::with_service(service(source));
    let window = WindowId::from_raw(3);
    let tab = TabId::from_raw(4);

    runtime
        .submit(window, tab, PathBuf::from("/slow"), "old".to_string())
        .expect("query");
    assert!(wait_until(1_000, || gate.entered() == 1));
    runtime.close(window);
    assert_eq!(runtime.session_count(), 0);
    gate.release();
    assert!(wait_until(1_000, || gate.entered() == 0));
    let tick = runtime.poll(window);
    assert!(!tick.alive);
    assert!(tick.changes.is_empty());
    assert!(runtime.view(window).is_none());

    gate.reset();
    runtime
        .submit(window, tab, PathBuf::from("/slow"), "old".to_string())
        .expect("query again");
    assert!(wait_until(1_000, || gate.entered() == 1));
    runtime.close(window);
    gate.release();
    assert!(wait_until(1_000, || gate.entered() == 0));
    assert_eq!(runtime.session_count(), 0);
}

#[test]
fn repeated_blocked_scans_keep_two_workers_for_the_process_lifetime() {
    let gate = Arc::new(Gate::default());
    let source = Arc::new(
        TestSource::new()
            .with_gate(gate.clone())
            .dir("/a", &[])
            .dir("/b", &[])
            .dir("/c", &[])
            .block("/a")
            .block("/b")
            .block("/c"),
    );
    let service = Arc::new(SearchService::with_parts(
        source.clone(),
        Arc::new(SystemClock),
    ));
    assert_eq!(service.worker_count(), MAX_WORKERS);

    for round in 0..3 {
        gate.reset();
        let first = start_job(&service, "/a");
        let second = start_job(&service, "/b");
        let third = start_job(&service, "/c");
        assert!(wait_until(1_000, || gate.entered() == 2));
        assert_eq!(service.running_count(), 2);
        assert_eq!(service.pending_len(), 1);
        assert_eq!(gate.peak(), 2, "round {round}: a third worker started");
        for job in [first, second, third] {
            job.cancel.store(true, Ordering::SeqCst);
            service.cancel(job.token);
            job.events.abandon();
        }
        gate.release();
        assert!(wait_until(2_000, || service.running_count() == 0
            && service.pending_len() == 0));
    }
    assert_eq!(service.worker_count(), MAX_WORKERS);
    assert!(
        source.thread_count() <= MAX_WORKERS,
        "repeated rounds used {} worker threads",
        source.thread_count()
    );
}

#[test]
fn a_full_queue_wakes_the_worker_when_the_session_is_cancelled() {
    let children: Vec<String> = (0..600)
        .map(|index| format!("/q/match-{index}.txt"))
        .collect();
    let refs: Vec<&str> = children.iter().map(String::as_str).collect();
    let mut source = TestSource::new().dir("/q", &refs);
    for path in &children {
        source = source.file(path, 1);
    }
    let service = service(source);
    let job = start_job(&service, "/q");
    assert!(wait_until(2_000, || job.events.len() >= QUEUE_EVENTS));
    let started = Instant::now();
    job.cancel.store(true, Ordering::SeqCst);
    service.cancel(job.token);
    job.events.abandon();
    assert!(
        wait_until(2_000, || service.running_count() == 0),
        "a cancelled worker stayed blocked on a full queue"
    );
    assert!(started.elapsed() < Duration::from_secs(2));
}

#[test]
fn pending_activation_uses_the_native_path_not_the_reordered_index() {
    use fm_core::search::SearchLimits;
    use fm_core::workspace::Workspace;
    use std::fs;

    let root = PathBuf::from(format!(
        "/tmp/gimbal-search-activation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|time| time.as_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).expect("scratch root");
    fs::write(root.join("a-report.txt"), b"a").expect("first file");
    fs::write(root.join("b-report.txt"), b"b").expect("second file");

    let service = Arc::new(SearchService::new());
    let gate = Arc::new(Gate::default());
    let activation_gate = gate.clone();
    let evidence = Arc::new(EvidencePool::with_runners(
        Arc::new(move |path: &std::path::Path| {
            activation_gate.block();
            resolve_native_navigation(path)
        }),
        Arc::new(check_root_lifetime),
    ));
    let runtime = SearchRuntime::with_evidence(service, evidence);
    runtime.set_limits_for_tests(SearchLimits::default());

    let mut workspace = Workspace::with_home(root.clone());
    let events = workspace
        .window_new(Some(root.clone()))
        .expect("model window");
    let window = events
        .iter()
        .find_map(|event| match event {
            fm_core::workspace::Event::WindowOpened(id) => Some(*id),
            _ => None,
        })
        .expect("window id");
    let model = workspace
        .windows
        .iter()
        .find(|candidate| candidate.id == window)
        .cloned()
        .expect("window snapshot");
    let tab = model.focused_tab;

    let view = runtime
        .submit(window, tab, root.clone(), "report".to_string())
        .expect("search");
    assert!(wait_until(2_000, || {
        let _ = runtime.poll(window);
        runtime
            .view(window)
            .is_some_and(|view| view.phase == SearchPhase::Complete && view.rows == 2)
    }));
    let first = runtime
        .result(window, view.generation, 0)
        .expect("first row")
        .path;
    let second = runtime
        .result(window, view.generation, 1)
        .expect("second row")
        .path;
    assert_ne!(first, second);

    assert_eq!(
        runtime.begin_activation(window, view.generation, second.clone(), &model),
        ActivationStart::Started
    );
    assert!(wait_until(1_000, || gate.entered() == 1));

    // The list reorders while the worker is blocked. The pending lookup must
    // still carry the exact path it started with.
    {
        let mut state = runtime.state.borrow_mut();
        let session = state.sessions.get_mut(&window).expect("session");
        session.rows.swap(0, 1);
        session.total_matches = session.rows.len() as u64;
    }
    gate.release();
    let mut ready = None;
    assert!(wait_until(2_000, || {
        let tick = runtime.poll(window);
        if let Some(activation) = tick.activation {
            ready = Some(activation);
            true
        } else {
            false
        }
    }));
    let ready = ready.expect("activation evidence");
    assert_eq!(ready.path, second);
    assert!(
        runtime.activation_valid(window, view.generation, &second, &model),
        "the original path remains the target after the reorder"
    );
    assert!(matches!(
        ready.target,
        Ok(NativeNavTarget { select: Some(ref name), .. })
            if name.as_encoded_bytes() == b"b-report.txt"
    ));

    // If ranking evicts that path instead, the late completion is explicitly
    // invalid rather than pointing at the new occupant of its old index.
    {
        let mut state = runtime.state.borrow_mut();
        let session = state.sessions.get_mut(&window).expect("session");
        session.rows.retain(|row| row.path != second);
    }
    assert!(!runtime.activation_valid(window, view.generation, &second, &model));

    runtime.close(window);
    gate.release();
    let _ = fs::remove_dir_all(&root);
}
