// SPDX-License-Identifier: GPL-3.0-or-later
//! Authorization, bounded work and job-history tests for the bridge.

use super::*;
use crate::fileops::clipboard::{ClipboardMode, ClipboardPayload, Reader};
use fm_agent::client::{self, ClientError};
use fm_agent::protocol::{Operation, Request as WireRequest};
use fm_core::fileops::Request as FileRequest;
use gtk::gio::prelude::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

type ClipboardDone = Box<dyn FnOnce(Result<ClipboardPayload, String>)>;

#[derive(Clone, Default)]
struct PausedReader {
    calls: Rc<RefCell<Vec<ClipboardDone>>>,
}

impl Reader for PausedReader {
    fn read(&self, _display: Option<&gtk::gdk::Display>, done: ClipboardDone) {
        self.calls.borrow_mut().push(done);
    }
}

#[derive(Clone)]
struct QueueReader {
    results: Rc<RefCell<VecDeque<Result<ClipboardPayload, String>>>>,
}

impl QueueReader {
    fn new(results: Vec<Result<ClipboardPayload, String>>) -> Self {
        Self {
            results: Rc::new(RefCell::new(results.into())),
        }
    }
}

impl Reader for QueueReader {
    fn read(&self, _display: Option<&gtk::gdk::Display>, done: ClipboardDone) {
        let result = self
            .results
            .borrow_mut()
            .pop_front()
            .unwrap_or_else(|| Err("the injected clipboard is empty".to_string()));
        done(result);
    }
}

struct Scratch {
    root: PathBuf,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

struct Harness {
    process: Rc<Process>,
    runtime: PathBuf,
    config: PathBuf,
    scratch: Scratch,
    /// Transport workers share a process-wide budget; bridge tests also own
    /// real sockets, so they run one at a time.
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Harness {
    fn new(tag: &str) -> Self {
        Self::with_reader(tag, Rc::new(PausedReader::default()))
    }

    fn with_reader(tag: &str, reader: Rc<dyn Reader>) -> Self {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let deadline = Instant::now() + Duration::from_secs(5);
        while fm_agent::transport::active_workers() != 0 {
            assert!(
                Instant::now() < deadline,
                "a previous bridge test leaked transport workers"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let unique = format!(
            "/tmp/gimbal-agent-ui-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        );
        fs::create_dir_all(&unique).expect("scratch");
        fs::set_permissions(&unique, fs::Permissions::from_mode(0o700)).expect("private");
        let root = fs::canonicalize(unique).expect("canonical");
        let config_dir = root.join("config/gimbal-files");
        fs::create_dir_all(&config_dir).expect("config dir");
        let config = config_dir.join("config.toml");
        fs::write(&config, "[agent]\nenabled = false\n").expect("config");
        let runtime = root.join("run");
        fs::create_dir_all(&runtime).expect("runtime");
        let home = root.join("home");
        fs::create_dir_all(&home).expect("home");
        let process = Process::headless(config.clone(), home, reader);
        Self {
            process,
            runtime,
            config,
            scratch: Scratch { root },
            _guard: guard,
        }
    }

    fn enable(&self) {
        enable_at(&self.process, &self.runtime).expect("listener starts");
    }

    fn disable(&self) {
        disable(&self.process, None);
    }

    fn instance(&self) -> String {
        self.process
            .agent
            .borrow()
            .current_instance()
            .expect("instance")
            .to_string()
    }

    fn generation(&self) -> u64 {
        self.process.agent.borrow().generation
    }

    fn window(&self, id: WindowId) -> String {
        format!("{}/w{}", self.instance(), id.raw())
    }

    fn tab(&self, id: TabId) -> String {
        format!("{}/t{}", self.instance(), id.raw())
    }

    fn request(
        &self,
        request_id: &str,
        window: Option<WindowId>,
        tab: Option<TabId>,
        command: &str,
        arguments: &[(&str, &str)],
    ) -> WireRequest {
        WireRequest {
            request_id: request_id.to_string(),
            instance: Some(self.instance()),
            generation: Some(self.generation()),
            operation: Operation::Run {
                window: window.map(|id| self.window(id)),
                tab: tab.map(|id| self.tab(id)),
                command: command.to_string(),
                arguments: arguments
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
                wait: false,
            },
        }
    }

    fn spawn(&self, request: WireRequest) -> Receiver<Result<String, ClientError>> {
        let runtime = self.runtime.clone();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(client::request(&runtime, &request, Duration::from_secs(10)));
        });
        receiver
    }

    fn pump(&self) {
        poll(&self.process);
        if let Some(fileops) = self.process.fileops() {
            fileops.poll_for_tests();
        }
    }

    fn pump_until(&self, deadline: Instant, condition: impl Fn() -> bool) {
        while !condition() {
            self.pump();
            assert!(
                Instant::now() < deadline,
                "the bridge did not reach the state"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn reply(&self, receiver: &Receiver<Result<String, ClientError>>, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        loop {
            self.pump();
            match receiver.try_recv() {
                Ok(Ok(text)) => return text,
                Ok(Err(error)) => panic!("client error: {error}"),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => panic!("client thread ended"),
            }
            assert!(Instant::now() < deadline, "no bridge reply");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn job(&self, token: &str) -> fm_agent::protocol::JobInfo {
        self.process.agent.borrow_mut().job_by_token(token)
    }
}

fn reply_job(text: &str) -> String {
    text.lines()
        .find_map(|line| {
            line.strip_prefix("job = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or_else(|| panic!("no job in reply:\n{text}"))
        .to_string()
}

fn file_uri(path: &PathBuf) -> String {
    gtk::gio::File::for_path(path).uri().to_string()
}

fn payload(mode: ClipboardMode, paths: &[PathBuf]) -> Result<ClipboardPayload, String> {
    Ok(ClipboardPayload {
        mode,
        uris: paths.iter().map(file_uri).collect(),
    })
}

fn ticket(generation: u64) -> Arc<GenerationTicket> {
    GenerationTicket::new(generation)
}

fn authorization(generation: u64) -> JobAuthorization {
    JobAuthorization {
        ticket: ticket(generation),
        instance: format!("p1-g{generation}"),
        roots: ProtectedRoots::new(
            "/tmp/gimbal-agent-test/config.toml",
            "/tmp/gimbal-agent-test/config",
            "/tmp/gimbal-agent-test/run",
        )
        .resolved()
        .expect("test roots"),
    }
}

#[test]
fn binding_another_window_and_tab_releases_the_model_before_events() {
    let harness = Harness::new("binding-events");
    let a = harness
        .process
        .seed_window(harness.scratch.root.join("home"));
    let original_tab = harness.process.window_snapshot(a).expect("A").focused_tab;
    harness.process.dispatch_for_window(
        a,
        Caller::Ui,
        Invocation::new("tab.new", Vec::<(String, String)>::new()),
    );
    assert_ne!(
        harness.process.window_snapshot(a).expect("A").focused_tab,
        original_tab
    );
    let b = harness
        .process
        .seed_window(harness.scratch.root.join("home"));
    assert_eq!(harness.process.focused_window_id(), Some(b));
    assert!(harness.process.bind_execution_target(ExecutionTarget {
        window: a,
        tab: original_tab,
    }));
    assert_eq!(harness.process.focused_window_id(), Some(a));
    assert_eq!(
        harness.process.window_snapshot(a).expect("A").focused_tab,
        original_tab
    );
}

#[test]
fn window_targets_are_only_the_context_free_process_actions() {
    assert!(context_free("window.new"));
    assert!(context_free("app.quit"));
    assert!(!context_free("tab.new"));
    assert!(!context_free("file.copy"));
}

#[test]
fn selection_projection_is_bounded_and_keeps_non_utf8_bytes() {
    let names: Vec<std::ffi::OsString> = (0..SELECTION_NAMES + 5)
        .map(|index| std::ffi::OsString::from(format!("file-{index}")))
        .collect();
    let (total, page, truncated) = selection_page(names.iter().map(std::ffi::OsString::as_os_str));
    assert_eq!(total, SELECTION_NAMES + 5);
    assert_eq!(page.len(), SELECTION_NAMES);
    assert!(truncated);

    let odd = OsStr::from_bytes(b"caf\xe9");
    let (total, page, truncated) = selection_page([odd]);
    assert_eq!(total, 1);
    assert!(!truncated);
    assert_eq!(page[0].text, None);
    assert_eq!(page[0].hex, "636166e9");
}

#[test]
fn job_ids_are_scoped_to_one_enable_generation() {
    let mut runtime = AgentRuntime::new("p1".to_string());
    runtime.instance = Some("p1-g1".to_string());
    runtime.generation = 1;
    runtime.ticket = Some(ticket(1));
    let (_id, token) = runtime.reserve_job("copy".to_string()).expect("reserved");
    runtime.attach_fileops(&token, 7, true);
    assert_eq!(token, "p1-g1/j1");
    assert_eq!(runtime.job_by_token("p1-g1/j1").state, "running");

    // A new enable cycle gets a new instance. The old id is stale even in the
    // same process, and an unknown id is explicit.
    runtime.instance = Some("p1-g2".to_string());
    runtime.generation = 2;
    assert_eq!(runtime.job_by_token("p1-g1/j1").state, "unknown");
    assert_eq!(runtime.job_by_token("p1-g2/j99").state, "expired");
    assert_eq!(runtime.job_by_token("garbage").state, "unknown");
}

#[test]
fn terminal_history_is_bounded_and_never_claims_completion_early() {
    let mut runtime = AgentRuntime::new("p1".to_string());
    runtime.instance = Some("p1-g1".to_string());
    runtime.generation = 1;
    runtime.ticket = Some(ticket(1));
    let (_id, token) = runtime.reserve_job("copy".to_string()).expect("reserved");
    runtime.attach_fileops(&token, 1, false);
    assert_eq!(runtime.job_by_token("p1-g1/j1").state, "queued");
    runtime.job_by_fileops(1, |job| job.state = "waiting-for-user".to_string());
    assert_eq!(runtime.job_by_token("p1-g1/j1").state, "waiting-for-user");

    for id in 2..(MAX_TERMINAL_JOBS as u64 + 40) {
        let (_reserved, token) = runtime.reserve_job("copy".to_string()).expect("reserved");
        runtime.attach_fileops(&token, id, true);
        runtime.job_by_fileops(id, |job| job.state = "complete".to_string());
    }
    let terminal = runtime.jobs.iter().filter(|job| job.terminal).count();
    assert!(terminal <= MAX_TERMINAL_JOBS, "{terminal}");
    let newest = MAX_TERMINAL_JOBS as u64 + 39;
    assert_eq!(
        runtime.job_by_token(&format!("p1-g1/j{newest}")).state,
        "complete"
    );
    assert_eq!(runtime.job_by_token("p1-g1/j2").state, "expired");
}

#[test]
fn disabled_runtime_reports_off_and_no_instance() {
    let runtime = AgentRuntime::new("p1".to_string());
    let status = runtime.snapshot();
    assert_eq!(status.state, AgentState::Off);
    assert_eq!(status.instance, None);
    assert_eq!(status.generation, 0);
}

#[test]
fn one_generation_ticket_revokes_all_queued_and_pending_contexts() {
    let generation = ticket(4);
    let first = JobAuthorization {
        ticket: Arc::clone(&generation),
        instance: "p1-g4".to_string(),
        roots: authorization(4).roots,
    };
    let second = first.clone();
    assert!(first.is_active());
    generation.revoke();
    assert!(!first.is_active());
    assert!(!second.is_active());
    assert!(!second.same_cycle(&ticket(5)));
    assert!(ticket(5).is_active(), "a later On starts a fresh ticket");
}

#[test]
fn an_agent_job_keeps_its_original_policy_after_a_later_enable() {
    let old = authorization(3);
    let new = authorization(4);
    assert!(!old.same_cycle(&new.ticket));
    assert_eq!(old.generation(), 3);
    assert_eq!(new.generation(), 4);
}

#[test]
fn a_revoked_clipboard_callback_has_no_effect_and_no_new_job() {
    let mut runtime = AgentRuntime::new("p1".to_string());
    runtime.instance = Some("p1-g1".to_string());
    runtime.generation = 1;
    runtime.ticket = Some(ticket(1));
    let (_id, token) = runtime.reserve_job("paste".to_string()).expect("reserved");
    let auth = JobAuthorization {
        ticket: runtime.ticket.as_ref().expect("ticket").clone(),
        instance: "p1-g1".to_string(),
        roots: authorization(1).roots,
    };
    runtime.ticket.as_ref().expect("ticket").revoke();
    // The callback gate is the same authorization method production uses.
    assert!(!auth.is_active());
    runtime.fail_reserved(&token, "local automation was turned off".to_string());
    let job = runtime.job_by_token(&token);
    assert_eq!(job.state, "failed");
    assert!(job.errors[0].contains("turned off"));
}

#[test]
fn capability_projection_marks_system_and_ui_only_commands_unavailable() {
    let request = Request {
        request_id: "r".to_string(),
        instance: None,
        generation: None,
        operation: Operation::Commands {
            offset: 0,
            limit: 100,
        },
    };
    let reply = commands_reply(&request, 0, 100, "p1-g1", 1);
    let Data::Commands { commands, .. } = reply.data else {
        panic!("commands data");
    };
    let mount = commands
        .iter()
        .find(|command| command.name == "drive.mount")
        .expect("drive.mount listed");
    assert!(!mount.allowed);
    assert!(
        mount
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("system"))
    );
    let paste = commands
        .iter()
        .find(|command| command.name == "file.paste")
        .expect("file.paste listed");
    assert!(paste.allowed);
}

#[test]
fn a_paused_clipboard_callback_dies_at_off_and_is_not_revived_by_on() {
    let auth = Some(authorization(1));
    let gate = Arc::new(std::sync::Barrier::new(2));
    let callback_gate = Arc::clone(&gate);
    let callback_auth = auth.clone();
    let callback = std::thread::spawn(move || {
        // The real callback is paused here until Off has been published.
        callback_gate.wait();
        clipboard_callback_allowed(&callback_auth)
    });
    auth.as_ref().expect("auth").ticket.revoke();
    gate.wait();
    assert!(!callback.join().expect("callback"));
    // A later On creates a fresh ticket; it does not revive the old callback.
    assert!(clipboard_callback_allowed(&Some(authorization(2))));
    assert!(!clipboard_callback_allowed(&auth));
}

#[test]
fn a_dry_run_result_is_complete_but_writes_nothing() {
    let pool = work::WorkPool::start();
    pool.try_submit(
        1,
        9,
        work::Work::DryRun {
            request: FileRequest::Trash { paths: Vec::new() },
            undo_lines: None,
        },
    )
    .expect("queued");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let result = loop {
        if let Some(result) = pool.try_result() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline, "no work result");
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(result.generation, 9);
    let work::WorkOutcome::DryRun(lines) = result.outcome.expect("plan") else {
        panic!("dry run");
    };
    assert!(lines[0].starts_with("dry run:"), "{lines:?}");
}

#[test]
fn tab_and_window_commands_bind_the_requested_window_without_a_tab() {
    let harness = Harness::new("targets");
    let a_dir = harness.scratch.root.join("a");
    let b_dir = harness.scratch.root.join("b");
    let c_dir = harness.scratch.root.join("c");
    for folder in [&a_dir, &b_dir, &c_dir] {
        fs::create_dir_all(folder).expect("folder");
    }
    let a = harness.process.seed_window(a_dir.clone());
    let b = harness.process.seed_window(b_dir.clone());
    harness.process.focus_window_for_tests(b);
    harness.enable();

    // tab.new against A with no tab must not add a tab to focused B.
    let reply = harness.reply(
        &harness.spawn(harness.request(
            "new-a",
            Some(a),
            None,
            "tab.new",
            &[("path", c_dir.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"ok\""), "{reply}");
    assert_eq!(
        harness.process.window_snapshot(a).expect("A").tab_count(),
        2
    );
    assert_eq!(
        harness.process.window_snapshot(b).expect("B").tab_count(),
        1
    );

    // tab.close against A closes A's tab, not focused B's.
    harness.process.focus_window_for_tests(b);
    let reply = harness.reply(
        &harness.spawn(harness.request("close-a", Some(a), None, "tab.close", &[])),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"ok\""), "{reply}");
    assert_eq!(
        harness.process.window_snapshot(a).expect("A").tab_count(),
        1
    );
    assert_eq!(
        harness.process.window_snapshot(b).expect("B").tab_count(),
        1
    );

    // Deferred navigation affects the requested window's default tab.
    harness.process.focus_window_for_tests(b);
    let reply = harness.reply(
        &harness.spawn(harness.request(
            "nav-a",
            Some(a),
            None,
            "nav.go",
            &[("path", b_dir.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"ok\""), "{reply}");
    let a_window = harness.process.window_snapshot(a).expect("A");
    assert_eq!(
        a_window.tab(a_window.focused_tab).expect("tab").folder,
        b_dir
    );
    let b_window = harness.process.window_snapshot(b).expect("B");
    assert_eq!(
        b_window.tab(b_window.focused_tab).expect("tab").folder,
        b_dir
    );

    // window.close closes A, not focused B.
    harness.process.focus_window_for_tests(b);
    let reply = harness.reply(
        &harness.spawn(harness.request("close-window-a", Some(a), None, "window.close", &[])),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"ok\""), "{reply}");
    assert!(harness.process.window_snapshot(a).is_none());
    assert!(harness.process.window_snapshot(b).is_some());
}

#[test]
fn a_paused_lookup_whose_target_disappears_returns_invalid_target() {
    let harness = Harness::new("paused-nav");
    let a_dir = harness.scratch.root.join("a");
    let b_dir = harness.scratch.root.join("b");
    let c_dir = harness.scratch.root.join("c");
    for folder in [&a_dir, &b_dir, &c_dir] {
        fs::create_dir_all(folder).expect("folder");
    }
    let a = harness.process.seed_window(a_dir.clone());
    let b = harness.process.seed_window(b_dir.clone());
    harness.process.focus_window_for_tests(b);
    harness.enable();

    let entered = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    let folder = c_dir.clone();
    harness.process.agent.borrow_mut().work =
        work::WorkPool::with_runner(Arc::new(move |item| match item {
            work::Work::Nav { .. } => {
                *hook_entered.0.lock().expect("entered") = true;
                hook_entered.1.notify_all();
                let mut open = hook_release.0.lock().expect("release");
                while !*open {
                    open = hook_release.1.wait(open).expect("release");
                }
                Ok(work::WorkOutcome::Nav {
                    folder: folder.clone(),
                    select: None,
                })
            }
            other => Err(format!("unexpected work: {other:?}")),
        }));

    let receiver = harness.spawn(harness.request(
        "paused-nav",
        Some(a),
        None,
        "nav.go",
        &[("path", c_dir.to_str().expect("utf8"))],
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !*entered.0.lock().expect("entered") {
        harness.pump();
        assert!(Instant::now() < deadline, "the lookup never paused");
        std::thread::sleep(Duration::from_millis(2));
    }
    // The target window disappears while the evidence lookup is paused.
    harness.process.close_window_for_tests(a);
    harness.process.focus_window_for_tests(b);
    *release.0.lock().expect("release") = true;
    release.1.notify_all();

    let reply = harness.reply(&receiver, Duration::from_secs(5));
    assert!(reply.contains("invalid-target"), "{reply}");
    let b_window = harness.process.window_snapshot(b).expect("B");
    assert_eq!(
        b_window.tab(b_window.focused_tab).expect("tab").folder,
        b_dir
    );
    assert!(harness.process.window_snapshot(a).is_none());
}

#[test]
fn forbidden_malformed_and_invalid_targets_leave_the_other_window_alone() {
    let harness = Harness::new("purity");
    let a_dir = harness.scratch.root.join("a");
    let b_dir = harness.scratch.root.join("b");
    let src = harness.scratch.root.join("src.txt");
    let dest = harness.scratch.root.join("dest");
    for folder in [&a_dir, &b_dir, &dest] {
        fs::create_dir_all(folder).expect("folder");
    }
    fs::write(&src, "payload").expect("source");
    let a = harness.process.seed_window(a_dir.clone());
    let b = harness.process.seed_window(b_dir.clone());
    harness.process.focus_window_for_tests(b);
    harness.enable();
    let a_tab = harness.process.window_snapshot(a).expect("A").focused_tab;

    let reply = harness.reply(
        &harness.spawn(harness.request("forbidden", Some(a), Some(a_tab), "preview.edit", &[])),
        Duration::from_secs(5),
    );
    assert!(reply.contains("forbidden"), "{reply}");
    assert_eq!(harness.process.focused_window_id(), Some(b));
    assert_eq!(
        harness.process.window_snapshot(a).expect("A").focused_tab,
        a_tab
    );

    let reply = harness.reply(
        &harness.spawn(harness.request("malformed", Some(a), None, "tab.new", &[("bogus", "x")])),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"error\""), "{reply}");
    assert_eq!(harness.process.focused_window_id(), Some(b));
    assert_eq!(
        harness.process.window_snapshot(a).expect("A").tab_count(),
        1
    );

    // A real model failure is a typed error, never a successful completion.
    let reply = harness.reply(
        &harness.spawn(harness.request(
            "bad-index",
            Some(a),
            None,
            "tab.focus",
            &[("index", "99")],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"error\""), "{reply}");

    // A read-only dry run binds nothing, even with another tab focused.
    harness.process.focus_window_for_tests(b);
    let reply = harness.reply(
        &harness.spawn(harness.request(
            "dry-run",
            Some(a),
            None,
            "file.copy",
            &[
                ("paths", src.to_str().expect("utf8")),
                ("to", dest.to_str().expect("utf8")),
                ("dry-run", "true"),
            ],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("kind = \"dry-run\""), "{reply}");
    assert_eq!(harness.process.focused_window_id(), Some(b));
    let a_window = harness.process.window_snapshot(a).expect("A");
    assert_eq!(
        a_window.tab(a_window.focused_tab).expect("tab").folder,
        a_dir
    );
    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 0);
}

#[test]
fn contextual_refusal_and_config_save_failure_are_typed_errors() {
    let harness = Harness::new("truthful");
    let a_dir = harness.scratch.root.join("a");
    fs::create_dir_all(&a_dir).expect("a");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    harness.process.mark_trash_for_tests(a);
    let reply = harness.reply(
        &harness.spawn(harness.request("trash-up", Some(a), None, "nav.up", &[])),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"error\""), "{reply}");
    assert!(reply.contains("refused"), "{reply}");

    // Make the atomic config save fail deterministically: the rename target
    // is now a directory.
    fs::remove_file(&harness.config).expect("remove config");
    fs::create_dir(&harness.config).expect("config as directory");
    let reply = harness.reply(
        &harness.spawn(harness.request(
            "save-fail",
            Some(a),
            None,
            "view.set-opacity",
            &[("opacity", "0.9")],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"error\""), "{reply}");
    assert!(reply.contains("failed"), "{reply}");
}

#[test]
fn stale_paste_callbacks_cannot_change_a_new_generation_job() {
    let reader = Rc::new(PausedReader::default());
    let harness = Harness::with_reader("stale-paste", reader.clone());
    let a_dir = harness.scratch.root.join("a");
    let dest = harness.scratch.root.join("dest");
    fs::create_dir_all(&a_dir).expect("a");
    fs::create_dir_all(&dest).expect("dest");
    let source_one = harness.scratch.root.join("one.txt");
    let source_two = harness.scratch.root.join("two.txt");
    fs::write(&source_one, "one").expect("one");
    fs::write(&source_two, "two").expect("two");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let first = harness.reply(
        &harness.spawn(harness.request(
            "paste-one",
            Some(a),
            None,
            "file.paste",
            &[("in", dest.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    let first_token = reply_job(&first);
    let second = harness.reply(
        &harness.spawn(harness.request(
            "paste-two",
            Some(a),
            None,
            "file.paste",
            &[("in", dest.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    let second_token = reply_job(&second);
    assert_eq!(reader.calls.borrow().len(), 2);

    harness.disable();
    harness.enable();
    let (_id, new_token) = harness
        .process
        .agent
        .borrow_mut()
        .reserve_job("new generation".to_string())
        .expect("reserved");
    assert!(new_token.ends_with("/j1"), "{new_token}");
    assert_ne!(new_token, first_token);
    assert_eq!(harness.job(&new_token).state, "queued");

    // Old success: no file work and no change to the colliding new job.
    let succeed = reader.calls.borrow_mut().remove(0);
    succeed(payload(
        ClipboardMode::Copy,
        std::slice::from_ref(&source_one),
    ));
    assert_eq!(harness.job(&new_token).state, "queued");
    assert!(harness.job(&new_token).errors.is_empty());

    // Old failure: the same scoped rule.
    let fail = reader.calls.borrow_mut().remove(0);
    fail(Err("the old reader failed".to_string()));
    assert_eq!(harness.job(&new_token).state, "queued");
    assert!(harness.job(&new_token).errors.is_empty());

    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 0);
    assert!(source_one.exists());
    assert!(source_two.exists());
    assert_eq!(harness.job(&second_token).state, "unknown");
    assert_eq!(
        harness
            .process
            .fileops()
            .map(|fileops| fileops.waiting())
            .unwrap_or(0),
        0
    );
}

#[test]
fn same_cycle_paste_failures_update_only_their_own_job() {
    let reader = QueueReader::new(vec![
        Err("the clipboard could not be read".to_string()),
        Ok(ClipboardPayload {
            mode: ClipboardMode::Copy,
            uris: Vec::new(),
        }),
    ]);
    let harness = Harness::with_reader("paste-failures", Rc::new(reader));
    let a_dir = harness.scratch.root.join("a");
    let dest = harness.scratch.root.join("dest");
    fs::create_dir_all(&a_dir).expect("a");
    fs::create_dir_all(&dest).expect("dest");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let first = harness.reply(
        &harness.spawn(harness.request(
            "paste-read-failure",
            Some(a),
            None,
            "file.paste",
            &[("in", dest.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    let first_token = reply_job(&first);
    let second = harness.reply(
        &harness.spawn(harness.request(
            "paste-empty",
            Some(a),
            None,
            "file.paste",
            &[("in", dest.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    let second_token = reply_job(&second);
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        harness.job(&first_token).state == "failed" && harness.job(&second_token).state == "failed"
    });
    assert!(
        harness.job(&first_token).errors[0].contains("could not be read"),
        "{:?}",
        harness.job(&first_token).errors
    );
    assert!(
        harness.job(&second_token).errors[0].contains("no local files"),
        "{:?}",
        harness.job(&second_token).errors
    );
    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 0);
}

#[test]
fn paste_dry_run_binds_no_target_and_reports_real_plan_lines() {
    let reader = QueueReader::new(Vec::new());
    let harness = Harness::with_reader("paste-dry-run", Rc::new(reader.clone()));
    let a_dir = harness.scratch.root.join("a");
    let b_dir = harness.scratch.root.join("b");
    let dest = harness.scratch.root.join("dest");
    let source = harness.scratch.root.join("dry.txt");
    fs::create_dir_all(&a_dir).expect("a");
    fs::create_dir_all(&b_dir).expect("b");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(&source, "dry").expect("source");
    reader
        .results
        .borrow_mut()
        .push_back(payload(ClipboardMode::Copy, &[source]));
    let a = harness.process.seed_window(a_dir.clone());
    let b = harness.process.seed_window(b_dir.clone());
    harness.process.focus_window_for_tests(b);
    harness.enable();

    let reply = harness.reply(
        &harness.spawn(harness.request(
            "paste-dry-run",
            Some(a),
            None,
            "file.paste",
            &[("in", dest.to_str().expect("utf8")), ("dry-run", "true")],
        )),
        Duration::from_secs(5),
    );
    let token = reply_job(&reply);
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        harness.job(&token).state == "complete"
    });
    let job = harness.job(&token);
    assert!(
        job.lines
            .first()
            .is_some_and(|line| line.starts_with("dry run:")),
        "{:?}",
        job.lines
    );
    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 0);
    assert_eq!(harness.process.focused_window_id(), Some(b));
    let a_window = harness.process.window_snapshot(a).expect("A");
    assert_eq!(
        a_window.tab(a_window.focused_tab).expect("tab").folder,
        a_dir
    );
}

#[test]
fn a_paste_into_a_protected_destination_fails_without_a_write() {
    let reader = QueueReader::new(Vec::new());
    let harness = Harness::with_reader("paste-protected", Rc::new(reader.clone()));
    let a_dir = harness.scratch.root.join("a");
    let config_dir = harness.scratch.root.join("config/gimbal-files");
    let source = harness.scratch.root.join("source.txt");
    fs::create_dir_all(&a_dir).expect("a");
    fs::write(&source, "source").expect("source");
    reader
        .results
        .borrow_mut()
        .push_back(payload(ClipboardMode::Copy, &[source]));
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let reply = harness.reply(
        &harness.spawn(harness.request(
            "paste-protected",
            Some(a),
            None,
            "file.paste",
            &[("in", config_dir.to_str().expect("utf8"))],
        )),
        Duration::from_secs(5),
    );
    let token = reply_job(&reply);
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        harness.job(&token).state == "failed"
    });
    let errors = harness.job(&token).errors;
    assert!(
        errors.iter().any(|error| error.contains("protected")),
        "{errors:?}"
    );
    assert_eq!(
        fs::read_to_string(config_dir.join("config.toml")).expect("config"),
        "[agent]\nenabled = false\n"
    );
    assert_eq!(fs::read_dir(&config_dir).expect("config").count(), 1);
}

#[test]
fn a_queued_agent_job_behind_ui_work_is_dropped_by_off() {
    let harness = Harness::new("queued-off");
    let a_dir = harness.scratch.root.join("a");
    fs::create_dir_all(&a_dir).expect("a");
    let ui_src = harness.scratch.root.join("ui.txt");
    let ui_dest = harness.scratch.root.join("ui-dest");
    let agent_src = harness.scratch.root.join("agent.txt");
    let agent_dest = harness.scratch.root.join("agent-dest");
    fs::create_dir_all(&ui_dest).expect("ui dest");
    fs::create_dir_all(&agent_dest).expect("agent dest");
    fs::write(&ui_src, "ui").expect("ui source");
    fs::write(ui_dest.join("ui.txt"), "existing").expect("conflict");
    fs::write(&agent_src, "agent").expect("agent source");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let fileops = harness.process.fileops().expect("fileops");
    let ui_id = fileops.submit(
        a,
        FileRequest::Copy {
            paths: vec![ui_src],
            to: ui_dest.clone(),
        },
        false,
        None,
    );
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        fileops.current_answering()
    });

    let reply = harness.reply(
        &harness.spawn(harness.request(
            "agent-queued",
            Some(a),
            None,
            "file.copy",
            &[
                ("paths", agent_src.to_str().expect("utf8")),
                ("to", agent_dest.to_str().expect("utf8")),
            ],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"accepted\""), "{reply}");
    assert_eq!(fileops.waiting(), 1);

    harness.disable();
    assert_eq!(fileops.waiting(), 0);
    fileops.cancel_job(ui_id);
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        fileops.current_id().is_none()
    });
    assert_eq!(fs::read_dir(&agent_dest).expect("agent dest").count(), 0);
    assert_eq!(
        fs::read_to_string(ui_dest.join("ui.txt")).expect("ui conflict"),
        "existing"
    );
}

#[test]
fn a_plan_arriving_after_off_never_prompts_or_executes() {
    let harness = Harness::new("plan-after-off");
    let a_dir = harness.scratch.root.join("a");
    let src = harness.scratch.root.join("src.txt");
    let dest = harness.scratch.root.join("dest");
    fs::create_dir_all(&a_dir).expect("a");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(&src, "payload").expect("source");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let entered = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let hook_entered = Arc::clone(&entered);
    let hook_release = Arc::clone(&release);
    harness
        .process
        .install_fileops_with_pause(Arc::new(move |boundary, _id| {
            if boundary != "planned" {
                return;
            }
            *hook_entered.0.lock().expect("entered") = true;
            hook_entered.1.notify_all();
            let mut open = hook_release.0.lock().expect("release");
            while !*open {
                open = hook_release.1.wait(open).expect("release");
            }
        }));

    let reply = harness.reply(
        &harness.spawn(harness.request(
            "plan-after-off",
            Some(a),
            None,
            "file.copy",
            &[
                ("paths", src.to_str().expect("utf8")),
                ("to", dest.to_str().expect("utf8")),
            ],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"accepted\""), "{reply}");
    {
        let (lock, wake) = &*entered;
        let mut seen = lock.lock().expect("entered");
        while !*seen {
            seen = wake.wait(seen).expect("entered");
        }
    }
    harness.disable();
    *release.0.lock().expect("release") = true;
    release.1.notify_all();
    let fileops = harness.process.fileops().expect("fileops");
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        fileops.current_id().is_none()
    });
    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 0);
    assert!(src.exists());
    assert!(harness.process.agent.borrow().jobs.is_empty());
}

#[test]
fn a_job_waiting_for_a_conflict_is_cancelled_by_off_without_a_write() {
    let harness = Harness::new("conflict-off");
    let a_dir = harness.scratch.root.join("a");
    let src = harness.scratch.root.join("copy.txt");
    let dest = harness.scratch.root.join("dest");
    fs::create_dir_all(&a_dir).expect("a");
    fs::create_dir_all(&dest).expect("dest");
    fs::write(&src, "new").expect("source");
    fs::write(dest.join("copy.txt"), "existing").expect("conflict");
    let a = harness.process.seed_window(a_dir);
    harness.enable();

    let reply = harness.reply(
        &harness.spawn(harness.request(
            "conflict-off",
            Some(a),
            None,
            "file.copy",
            &[
                ("paths", src.to_str().expect("utf8")),
                ("to", dest.to_str().expect("utf8")),
            ],
        )),
        Duration::from_secs(5),
    );
    assert!(reply.contains("status = \"accepted\""), "{reply}");
    let fileops = harness.process.fileops().expect("fileops");
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        fileops.current_answering()
    });
    harness.disable();
    harness.pump_until(Instant::now() + Duration::from_secs(5), || {
        fileops.current_id().is_none()
    });
    assert_eq!(
        fs::read_to_string(dest.join("copy.txt")).expect("conflict"),
        "existing"
    );
    assert_eq!(fs::read_dir(&dest).expect("dest").count(), 1);
    assert!(harness.process.agent.borrow().jobs.is_empty());
}
