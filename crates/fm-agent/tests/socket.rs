// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end framing and lifetime tests over real Unix sockets.

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use fm_agent::client;
use fm_agent::protocol::{self, Data, Operation, Request, Status};
use fm_agent::transport::{EndpointError, Inbound, Server, ServerConfig};

/// Transport workers share one process-wide fixed budget, so tests that open
/// servers and clients must not overlap each other.
fn test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
    // A previous test may have dropped its server just before its detached
    // workers finished. Wait for the process-wide budget to be empty so the
    // next test starts from a known state.
    let deadline = Instant::now() + Duration::from_secs(3);
    while fm_agent::transport::active_workers() != 0 {
        assert!(
            Instant::now() < deadline,
            "a previous test leaked transport workers"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    guard
}

struct Scratch {
    root: std::path::PathBuf,
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let unique = format!(
            "/tmp/gimbal-agent-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0)
        );
        fs::create_dir_all(&unique).expect("scratch");
        fs::set_permissions(&unique, fs::Permissions::from_mode(0o700)).expect("private");
        Self {
            root: fs::canonicalize(unique).expect("canonical"),
        }
    }

    fn config(&self, generation: u64) -> ServerConfig {
        let uid = fs::metadata(&self.root).expect("metadata").uid();
        ServerConfig::new(&self.root, uid, format!("test-g{generation}"), generation)
    }

    fn request(&self, id: &str) -> Request {
        Request {
            request_id: id.to_string(),
            instance: None,
            generation: None,
            operation: Operation::State {
                offset: 0,
                limit: 5,
            },
        }
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn state_reply(request: &Request) -> fm_agent::protocol::Reply {
    fm_agent::protocol::Reply {
        request_id: request.request_id.clone(),
        operation: request.operation.name().to_string(),
        status: Status::Ok,
        instance: Some("test-g1".to_string()),
        generation: Some(1),
        data: Data::State {
            offset: 0,
            total: 0,
            next: None,
            socket: fm_agent::protocol::SocketInfo {
                status: "on".to_string(),
                path: Some("/tmp/agent.sock".to_string()),
                detail: None,
                generation: 1,
            },
            windows: Vec::new(),
            jobs: Vec::new(),
        },
        error: None,
    }
}

fn answer_loop(
    receiver: std::sync::mpsc::Receiver<Inbound>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        while !stop.load(std::sync::atomic::Ordering::SeqCst) {
            match receiver.recv_timeout(Duration::from_millis(200)) {
                Ok(inbound) => {
                    let reply = state_reply(&inbound.request);
                    inbound.respond(reply);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    })
}

#[test]
fn a_real_framed_client_and_server_round_trip_state() {
    let _guard = test_lock();
    let scratch = Scratch::new("roundtrip");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = answer_loop(requests, stop.clone());

    let text = client::request(
        &scratch.root,
        &scratch.request("r1"),
        Duration::from_secs(2),
    )
    .expect("reply");
    assert!(text.contains("status = \"ok\""));
    assert!(text.contains("request_id = \"r1\""));
    assert!(text.contains("socket"));

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = worker.join();
    drop(server);
}

#[test]
fn partial_bad_and_oversized_frames_do_not_stop_the_next_client() {
    let _guard = test_lock();
    let scratch = Scratch::new("bad-frames");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = answer_loop(requests, stop.clone());

    // A declared length with a truncated body.
    let path = server.path().to_path_buf();
    let mut partial = UnixStream::connect(&path).expect("connect");
    partial.write_all(&(10_u32).to_be_bytes()).expect("length");
    partial.write_all(b"ver").expect("partial");
    drop(partial);

    // A frame far over the limit.
    let mut oversized = UnixStream::connect(&path).expect("connect");
    oversized
        .write_all(&(2_000_000_u32).to_be_bytes())
        .expect("length");
    drop(oversized);

    // A valid frame is still served afterwards.
    let text = client::request(
        &scratch.root,
        &scratch.request("r2"),
        Duration::from_secs(2),
    )
    .expect("reply");
    assert!(text.contains("request_id = \"r2\""));

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = worker.join();
    drop(server);
}

#[test]
fn a_silent_client_cannot_block_another_request() {
    let _guard = test_lock();
    let scratch = Scratch::new("slow");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = answer_loop(requests, stop.clone());
    let idle = UnixStream::connect(server.path()).expect("idle connect");

    let started = Instant::now();
    let text = client::request(
        &scratch.root,
        &scratch.request("r3"),
        Duration::from_secs(2),
    )
    .expect("reply");
    assert!(text.contains("request_id = \"r3\""));
    assert!(started.elapsed() < Duration::from_secs(2));

    drop(idle);
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = worker.join();
    drop(server);
}

#[test]
fn bounded_clients_are_refused_with_a_typed_reply() {
    let _guard = test_lock();
    let scratch = Scratch::new("bounded");
    let (server, _requests) = Server::start(scratch.config(1)).expect("starts");
    let mut idle = Vec::new();
    for _ in 0..fm_agent::transport::MAX_CLIENTS {
        idle.push(UnixStream::connect(server.path()).expect("connect"));
        // Give the accept loop a moment to register each connection.
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut extra = UnixStream::connect(server.path()).expect("connect");
    extra
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    protocol::write_frame(&mut extra, &scratch.request("r4").to_toml().into_bytes())
        .expect("write");
    let frame = read_one(&mut extra).expect("a busy reply");
    let text = String::from_utf8(frame).expect("utf8");
    assert!(text.contains("busy"), "{text}");

    drop(extra);
    drop(idle);
    drop(server);
}

fn read_one(stream: &mut UnixStream) -> Option<Vec<u8>> {
    let mut length = [0_u8; 4];
    stream.read_exact(&mut length).ok()?;
    let length = u32::from_be_bytes(length) as usize;
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).ok()?;
    Some(payload)
}

#[test]
fn stop_with_idle_and_partial_clients_returns_quickly_and_removes_only_its_socket() {
    let _guard = test_lock();
    let scratch = Scratch::new("stop");
    let (mut server, requests) = Server::start(scratch.config(1)).expect("starts");
    let receiver = requests;
    std::thread::spawn(move || {
        while let Ok(inbound) = receiver.recv() {
            let reply = state_reply(&inbound.request);
            inbound.respond(reply);
        }
    });
    let path = server.path().to_path_buf();

    let mut idle = UnixStream::connect(&path).expect("idle");
    let mut partial = UnixStream::connect(&path).expect("partial");
    partial.write_all(&(100_u32).to_be_bytes()).expect("length");
    idle.write_all(&(50_u32).to_be_bytes()).expect("length");

    let started = Instant::now();
    server.stop();
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(!path.exists());
}

#[test]
fn a_second_process_cannot_take_a_live_endpoint() {
    let _guard = test_lock();
    let scratch = Scratch::new("second");
    let (server, _requests) = Server::start(scratch.config(1)).expect("starts");
    let error = Server::start(scratch.config(2)).expect_err("live");
    assert!(matches!(error, EndpointError::LiveServer(_)));
    drop(server);
}

#[test]
fn an_old_generation_is_refused_before_the_bridge() {
    let _guard = test_lock();
    let scratch = Scratch::new("stale");
    let (server, requests) = Server::start(scratch.config(2)).expect("starts");
    let mut request = scratch.request("r5");
    request.generation = Some(1);
    let text = client::request(&scratch.root, &request, Duration::from_secs(2)).expect("a reply");
    assert!(text.contains("\"stale\""), "{text}");
    assert!(
        requests.recv_timeout(Duration::from_millis(200)).is_err(),
        "the stale request reached the bridge"
    );
    drop(server);
}

#[test]
fn a_protected_path_is_refused_on_the_transport_worker() {
    let _guard = test_lock();
    let scratch = Scratch::new("protected");
    let config_dir = scratch.root.join("config/gimbal-files");
    fs::create_dir_all(&config_dir).expect("config dir");
    let config_file = config_dir.join("config.toml");
    fs::write(&config_file, "[agent]\n").expect("config");
    let mut config = scratch.config(1);
    config.protected = Some(
        fm_agent::ProtectedRoots::new(&config_file, &config_dir, scratch.root.join("run"))
            .resolved()
            .expect("roots"),
    );
    let (server, requests) = Server::start(config).expect("starts");

    let request = Request {
        request_id: "r6".to_string(),
        instance: Some("test-g1".to_string()),
        generation: Some(1),
        operation: Operation::Run {
            window: None,
            tab: None,
            command: "file.copy".to_string(),
            arguments: vec![
                ("paths".to_string(), config_file.display().to_string()),
                (
                    "to".to_string(),
                    scratch.root.join("dest").display().to_string(),
                ),
            ],
            wait: false,
        },
    };
    let text = client::request(&scratch.root, &request, Duration::from_secs(2)).expect("a reply");
    assert!(text.contains("protected-path"), "{text}");
    assert!(
        requests.recv_timeout(Duration::from_millis(200)).is_err(),
        "the protected request reached the bridge"
    );
    drop(server);
}

#[test]
fn a_full_bridge_queue_returns_typed_busy_without_reaching_the_ui() {
    let _guard = test_lock();
    let scratch = Scratch::new("queue-full");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let mut waiting = Vec::new();
    for index in 0..fm_agent::transport::MAX_PENDING_REQUESTS {
        let mut stream = UnixStream::connect(server.path()).expect("connect");
        stream
            .set_write_timeout(Some(Duration::from_secs(1)))
            .expect("write timeout");
        protocol::write_frame(
            &mut stream,
            &scratch
                .request(&format!("wait-{index}"))
                .to_toml()
                .into_bytes(),
        )
        .expect("write");
        waiting.push(stream);
    }
    std::thread::sleep(Duration::from_millis(200));

    for index in 0..fm_agent::transport::MAX_PENDING_REQUESTS {
        let mut extra = UnixStream::connect(server.path()).expect("connect");
        extra
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        protocol::write_frame(
            &mut extra,
            &scratch
                .request(&format!("busy-{index}"))
                .to_toml()
                .into_bytes(),
        )
        .expect("write");
        let frame = read_one(&mut extra).expect("busy reply");
        let text = String::from_utf8(frame).expect("utf8");
        assert!(text.contains("busy"), "{text}");
    }
    drop(waiting);
    drop(requests);
    drop(server);
}

#[test]
fn a_timed_out_delivery_is_abandoned_and_never_dispatched() {
    let _guard = test_lock();
    let scratch = Scratch::new("expired");
    let mut config = scratch.config(1);
    config.bridge_timeout = Duration::from_millis(150);
    let (server, requests) = Server::start(config).expect("starts");
    let mut client = UnixStream::connect(server.path()).expect("connect");
    client
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    protocol::write_frame(
        &mut client,
        &scratch.request("expired-1").to_toml().into_bytes(),
    )
    .expect("write");
    let frame = read_one(&mut client).expect("timeout reply");
    let text = String::from_utf8(frame).expect("utf8");
    assert!(text.contains("timeout"), "{text}");

    // The bridge may pop the message later. It is marked abandoned so the
    // production bridge drops it before any command dispatch.
    let inbound = fm_agent::transport::next_request(&requests, Duration::from_secs(2))
        .expect("the request is still in the bounded queue");
    assert!(inbound.is_abandoned());
    assert_eq!(inbound.request.request_id, "expired-1");
    drop(inbound);
    drop(server);
}

#[test]
fn completed_sessions_do_not_accumulate_unbounded_thread_handles() {
    let _guard = test_lock();
    let scratch = Scratch::new("many-sessions");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = answer_loop(requests, stop.clone());
    for index in 0..80 {
        let text = client::request(
            &scratch.root,
            &scratch.request(&format!("session-{index}")),
            Duration::from_secs(2),
        )
        .expect("reply");
        assert!(text.contains(&format!("session-{index}")));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        server.retained_workers() <= fm_agent::transport::MAX_CLIENTS,
        "{} retained worker handles",
        server.retained_workers()
    );
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = worker.join();
    drop(server);
}

#[test]
fn duplicate_ids_and_the_session_request_budget_are_explicit() {
    let _guard = test_lock();
    let scratch = Scratch::new("session-budget");
    let (server, requests) = Server::start(scratch.config(1)).expect("starts");
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let worker = answer_loop(requests, stop.clone());
    let mut stream = UnixStream::connect(server.path()).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");

    let send = |stream: &mut UnixStream, id: &str| {
        protocol::write_frame(stream, &scratch.request(id).to_toml().into_bytes()).expect("write");
        String::from_utf8(read_one(stream).expect("reply")).expect("utf8")
    };
    let duplicate = send(&mut stream, "same-id");
    assert!(duplicate.contains("status = \"ok\""), "{duplicate}");
    let duplicate = send(&mut stream, "same-id");
    assert!(duplicate.contains("duplicate-request"), "{duplicate}");

    // Two ids are used. Fill the remaining budget exactly, then the next
    // request must be refused and the connection closed rather than silently
    // forgetting an older id.
    for index in 2..fm_agent::transport::MAX_SESSION_REQUESTS {
        let text = send(&mut stream, &format!("budget-{index}"));
        assert!(text.contains("status = \"ok\""), "{text}");
    }
    protocol::write_frame(
        &mut stream,
        &scratch.request("over-budget").to_toml().into_bytes(),
    )
    .expect("write");
    let text = String::from_utf8(read_one(&mut stream).expect("refusal")).expect("utf8");
    assert!(text.contains("session-exhausted"), "{text}");

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = worker.join();
    drop(server);
}

#[test]
fn run_requires_a_matching_instance_and_generation() {
    let _guard = test_lock();
    let scratch = Scratch::new("bound-run");
    let (server, _requests) = Server::start(scratch.config(1)).expect("starts");

    let mut unbound = UnixStream::connect(server.path()).expect("connect");
    unbound
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    protocol::write_frame(
        &mut unbound,
        b"version = 1\nrequest_id = \"u1\"\noperation = \"run\"\ncommand = \"window.new\"\n",
    )
    .expect("write");
    let text = String::from_utf8(read_one(&mut unbound).expect("refusal")).expect("utf8");
    assert!(text.contains("missing-field"), "{text}");

    let mut stale = UnixStream::connect(server.path()).expect("connect");
    stale
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    protocol::write_frame(
        &mut stale,
        b"version = 1\nrequest_id = \"u2\"\ninstance = \"old\"\ngeneration = 1\noperation = \"run\"\ncommand = \"window.new\"\n",
    )
    .expect("write");
    let text = String::from_utf8(read_one(&mut stale).expect("refusal")).expect("utf8");
    assert!(text.contains("stale"), "{text}");
    drop(server);
}

#[test]
fn stop_is_not_blocked_by_a_blocked_filesystem_resolver() {
    let _guard = test_lock();
    let scratch = Scratch::new("blocked-resolver");
    let config_dir = scratch.root.join("config/gimbal-files");
    fs::create_dir_all(&config_dir).expect("config dir");
    let config_file = config_dir.join("config.toml");
    fs::write(&config_file, "[agent]\n").expect("config");
    // Separate entered/release gates: the resolver stays provably blocked
    // until after Stop has returned and the peer has seen EOF.
    let entered = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let release = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let hook_entered = std::sync::Arc::clone(&entered);
    let hook_release = std::sync::Arc::clone(&release);
    let mut config = scratch.config(1);
    config.protected = Some(
        fm_agent::ProtectedRoots::new(&config_file, &config_dir, scratch.root.join("run"))
            .resolved()
            .expect("roots"),
    );
    config.resolve_hook = Some(fm_agent::transport::ResolverHook(std::sync::Arc::new(
        move |_, _| {
            *hook_entered.0.lock().expect("entered") = true;
            hook_entered.1.notify_all();
            let mut open = hook_release.0.lock().expect("release");
            while !*open {
                open = hook_release.1.wait(open).expect("release");
            }
            Ok(())
        },
    )));
    let (mut server, requests) = Server::start(config).expect("starts");

    let path = server.path().to_path_buf();
    let request = Request {
        request_id: "blocked-1".to_string(),
        instance: Some("test-g1".to_string()),
        generation: Some(1),
        operation: Operation::Run {
            window: None,
            tab: None,
            command: "file.copy".to_string(),
            arguments: vec![
                ("paths".to_string(), config_file.display().to_string()),
                (
                    "to".to_string(),
                    scratch.root.join("dest").display().to_string(),
                ),
            ],
            wait: false,
        },
    };
    let client = std::thread::spawn(move || {
        let mut stream = UnixStream::connect(path).expect("connect");
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = protocol::write_frame(&mut stream, &request.to_toml().into_bytes());
        let mut byte = [0_u8; 1];
        stream.read(&mut byte).ok()
    });
    {
        let (lock, wake) = &*entered;
        let mut seen = lock.lock().expect("entered");
        while !*seen {
            seen = wake.wait(seen).expect("entered");
        }
    }
    let started = Instant::now();
    server.stop();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "stop waited for the blocked resolver"
    );
    // The peer is disconnected promptly even though the resolver is still
    // inside the hook.
    assert_eq!(client.join().expect("client returns"), Some(0));
    assert_eq!(fm_agent::transport::active_workers(), 1);
    assert!(fm_agent::transport::next_request(&requests, Duration::from_millis(100)).is_none());

    *release.0.lock().expect("release") = true;
    release.1.notify_all();
    assert!(
        server.wait_for_workers(Duration::from_secs(2)),
        "client worker did not exit after the resolver released"
    );
}

#[test]
fn stop_wakes_a_client_that_is_waiting_for_the_bridge() {
    let _guard = test_lock();
    let scratch = Scratch::new("wait-stop");
    let (mut server, requests) = Server::start(scratch.config(1)).expect("starts");
    // Keep the receiver alive but never answer: the connection is parked in
    // the bridge wait when stop is called.
    let waiting = std::thread::spawn(move || {
        let request = Request {
            request_id: "r7".to_string(),
            instance: None,
            generation: None,
            operation: Operation::State {
                offset: 0,
                limit: 1,
            },
        };
        let _ = client::request(&scratch.root, &request, Duration::from_secs(10));
        drop(requests);
    });
    std::thread::sleep(Duration::from_millis(150));
    let started = Instant::now();
    server.stop();
    waiting.join().expect("the client thread returns");
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn repeated_enable_cycles_share_one_fixed_worker_budget() {
    let _guard = test_lock();
    let scratch = Scratch::new("worker-budget");
    let config_dir = scratch.root.join("config/gimbal-files");
    fs::create_dir_all(&config_dir).expect("config dir");
    let config_file = config_dir.join("config.toml");
    fs::write(&config_file, "[agent]\n").expect("config");
    let entered = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let release = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let hook_entered = std::sync::Arc::clone(&entered);
    let hook_release = std::sync::Arc::clone(&release);

    let mut all_clients = Vec::new();
    for (generation, count) in [(1_u64, 5_usize), (2, 3)] {
        let mut config = scratch.config(generation);
        config.protected = Some(
            fm_agent::ProtectedRoots::new(&config_file, &config_dir, scratch.root.join("run"))
                .resolved()
                .expect("roots"),
        );
        let entered_hook = std::sync::Arc::clone(&hook_entered);
        let release_hook = std::sync::Arc::clone(&hook_release);
        config.resolve_hook = Some(fm_agent::transport::ResolverHook(std::sync::Arc::new(
            move |_, _| {
                entered_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut open = release_hook.0.lock().expect("release");
                while !*open {
                    open = release_hook.1.wait(open).expect("release");
                }
                Ok(())
            },
        )));
        let (mut server, requests) = Server::start(config).expect("starts");
        let target = entered.load(std::sync::atomic::Ordering::SeqCst);
        let mut clients = Vec::new();
        for index in 0..count {
            let mut stream = UnixStream::connect(server.path()).expect("connect");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("timeout");
            let request = Request {
                request_id: format!("g{generation}-{index}"),
                instance: Some(format!("test-g{generation}")),
                generation: Some(generation),
                operation: Operation::Run {
                    window: None,
                    tab: None,
                    command: "file.copy".to_string(),
                    arguments: vec![
                        ("paths".to_string(), config_file.display().to_string()),
                        (
                            "to".to_string(),
                            scratch.root.join("dest").display().to_string(),
                        ),
                    ],
                    wait: false,
                },
            };
            protocol::write_frame(&mut stream, &request.to_toml().into_bytes()).expect("write");
            clients.push(stream);
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while entered.load(std::sync::atomic::Ordering::SeqCst) < target + count {
            assert!(
                Instant::now() < deadline,
                "the resolver did not block enough workers"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            fm_agent::transport::active_workers() <= fm_agent::transport::MAX_CLIENTS,
            "the process-wide worker budget was exceeded"
        );
        // Nothing reached the bridge while the resolvers were blocked.
        assert!(fm_agent::transport::next_request(&requests, Duration::from_millis(50)).is_none());

        server.stop();
        drop(requests);
        for mut client in clients {
            let mut byte = [0_u8; 1];
            assert_eq!(
                client.read(&mut byte).ok(),
                Some(0),
                "the peer was not disconnected by Off"
            );
            all_clients.push(client);
        }
        drop(server);
    }

    assert_eq!(entered.load(std::sync::atomic::Ordering::SeqCst), 8);
    assert_eq!(
        fm_agent::transport::active_workers(),
        fm_agent::transport::MAX_CLIENTS
    );

    // The next server is at capacity: a fresh client gets a typed refusal and
    // never gets a worker slot.
    let (mut third, _requests) = Server::start(scratch.config(3)).expect("starts");
    let mut extra = UnixStream::connect(third.path()).expect("connect");
    extra
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("timeout");
    protocol::write_frame(
        &mut extra,
        &scratch.request("capacity").to_toml().into_bytes(),
    )
    .expect("write");
    let text = String::from_utf8(read_one(&mut extra).expect("busy reply")).expect("utf8");
    assert!(text.contains("busy"), "{text}");
    assert_eq!(entered.load(std::sync::atomic::Ordering::SeqCst), 8);
    third.stop();
    drop(third);

    *release.0.lock().expect("release") = true;
    release.1.notify_all();
    let deadline = Instant::now() + Duration::from_secs(3);
    while fm_agent::transport::active_workers() != 0 {
        assert!(Instant::now() < deadline, "workers did not exit");
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(all_clients);
}
