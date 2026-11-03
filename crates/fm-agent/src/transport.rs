// SPDX-License-Identifier: GPL-3.0-or-later
//! The private Unix endpoint and its worker threads.
//!
//! The listener lives at `$XDG_RUNTIME_DIR/gimbal-files/agent.sock`. The app
//! creates a 0700 subdirectory and a 0600 socket, refuses symlinks and foreign
//! owners, checks every peer's kernel credentials against the server uid, and
//! removes only the socket identity it created. Reads and writes are bounded,
//! parse/encode happen on the worker, and the GTK bridge only ever sees typed
//! messages.

use std::collections::VecDeque;
use std::fmt;
use std::fs;
use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gio::prelude::*;

use crate::log;
use crate::protect::{self, ResolvedRoots};
use crate::protocol::{self, MAX_FRAME_BYTES, Reply, Request};

/// How many transport client workers may exist across the whole process.
/// This is a process-lifetime budget, not a fresh allocation per enable cycle:
/// a worker still blocked in a filesystem resolver after Off keeps its slot, so
/// repeated On/Off cycles cannot multiply blocked calls.
pub const MAX_CLIENTS: usize = 8;
/// How many parsed requests may wait for the GTK bridge. This is real queue
/// capacity, not just a per-tick drain budget: admission is nonblocking and a
/// full queue gets a typed `busy` reply without reaching a worker or the UI.
pub const MAX_PENDING_REQUESTS: usize = 4;
/// How many requests one explicit connection may make. The budget is fixed so
/// replay detection never has to forget an id; a client that exhausts it must
/// open a fresh connection.
pub const MAX_SESSION_REQUESTS: usize = 64;
/// How long a peer may stay silent between frames.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(15);
/// How long a request may wait for the GTK bridge.
pub const BRIDGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Read and write calls wake this often so stop is timely.
const WAKE: Duration = Duration::from_millis(250);
/// The control subdirectory and socket names.
pub const CONTROL_DIR: &str = "gimbal-files";
pub const SOCKET_NAME: &str = "agent.sock";

type Resolver = dyn Fn(&Request, &ResolvedRoots) -> Result<(), String> + Send + Sync;

/// The one process-lifetime worker owner. Every enable cycle draws from this
/// fixed budget and registers its handles here. A stopped server detaches its
/// blocked workers without giving a later server a second allocation.
struct WorkerOwner {
    active: AtomicUsize,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl WorkerOwner {
    fn try_acquire(&self) -> bool {
        let mut current = self.active.load(Ordering::SeqCst);
        loop {
            if current >= MAX_CLIENTS {
                return false;
            }
            match self.active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(observed) => current = observed,
            }
        }
    }

    fn release(&self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }

    fn track(&self, handle: JoinHandle<()>) {
        if let Ok(mut threads) = self.threads.lock() {
            threads.push(handle);
        }
    }

    fn reap(&self) {
        if let Ok(mut threads) = self.threads.lock() {
            threads.retain(|handle| !handle.is_finished());
        }
    }

    fn active(&self) -> usize {
        self.active.load(Ordering::SeqCst)
    }

    fn wait_idle(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.active() == 0 {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn workers() -> &'static WorkerOwner {
    static WORKERS: OnceLock<WorkerOwner> = OnceLock::new();
    WORKERS.get_or_init(|| WorkerOwner {
        active: AtomicUsize::new(0),
        threads: Mutex::new(Vec::new()),
    })
}

/// A test seam for the filesystem resolver. Production leaves this empty and
/// uses [`protect::check_request`] on the same transport worker.
pub struct ResolverHook(pub Arc<Resolver>);

impl std::fmt::Debug for ResolverHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ResolverHook(..)")
    }
}

impl Clone for ResolverHook {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

/// What the listener needs from the app.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// `$XDG_RUNTIME_DIR`; must be absolute, owned by `uid` and a real folder.
    pub runtime_dir: PathBuf,
    /// The uid that owns the runtime directory, the socket and every peer.
    pub uid: u32,
    /// This enable cycle's opaque instance id.
    pub instance: String,
    /// This enable cycle's generation.
    pub generation: u64,
    /// Test seam: refuse every peer as if its credentials did not match.
    pub reject_peer: bool,
    /// Resolved Gimbal control paths. The transport refuses a file request
    /// that would touch them before the UI bridge sees it; the bridge repeats
    /// the check for plan-bound work.
    pub protected: Option<ResolvedRoots>,
    /// Test seam for a blocked filesystem resolver. Production is `None`.
    pub resolve_hook: Option<ResolverHook>,
    /// How long one request may wait for the bridge. Tests shorten it; the
    /// production value is [`BRIDGE_TIMEOUT`].
    pub bridge_timeout: Duration,
}

impl ServerConfig {
    pub fn new(
        runtime_dir: impl Into<PathBuf>,
        uid: u32,
        instance: impl Into<String>,
        generation: u64,
    ) -> Self {
        Self {
            runtime_dir: runtime_dir.into(),
            uid,
            instance: instance.into(),
            generation,
            reject_peer: false,
            protected: None,
            resolve_hook: None,
            bridge_timeout: BRIDGE_TIMEOUT,
        }
    }
}

/// A listening endpoint owned by this app instance.
pub struct Server {
    path: PathBuf,
    directory: PathBuf,
    identity: (u64, u64),
    inner: Arc<Inner>,
    accept: Option<JoinHandle<()>>,
}

struct Inner {
    stop: AtomicBool,
    config: ServerConfig,
    sender: SyncSender<Inbound>,
    /// Connected streams are retained so Off can shut them down even when the
    /// worker that owns one is blocked inside a filesystem resolver.
    clients: Mutex<Vec<Arc<UnixStream>>>,
}

/// One parsed request handed to the GTK bridge, with the channel that answers
/// it. `abandoned` is set when the connection stopped waiting before the
/// bridge could run it; the bridge must then drop it without any effect.
pub struct Inbound {
    pub generation: u64,
    pub instance: String,
    pub request: Request,
    reply: SyncSender<Reply>,
    abandoned: Arc<AtomicBool>,
}

impl Inbound {
    /// Fill the identity fields the server owns and hand the reply back.
    pub fn respond(self, mut reply: Reply) {
        reply.request_id = self.request.request_id.clone();
        reply.operation = self.request.operation.name().to_string();
        reply.instance = Some(self.instance.clone());
        reply.generation = Some(self.generation);
        let _ = self.reply.send(reply);
    }

    /// Whether the client stopped waiting for this request.
    pub fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::SeqCst)
    }

    /// Mark a queued request as no longer wanted. The bridge checks this
    /// immediately before any dispatch.
    pub fn abandon(&self) {
        self.abandoned.store(true, Ordering::SeqCst);
    }
}

#[derive(Debug)]
pub enum EndpointError {
    RuntimeDir(String),
    ControlDir(String),
    NotOwned(PathBuf),
    Symlink(PathBuf),
    NotSocket(PathBuf),
    LiveServer(PathBuf),
    Permissions(PathBuf, String),
    Busy(PathBuf),
    Io(String),
}

impl fmt::Display for EndpointError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EndpointError::RuntimeDir(reason) => {
                write!(f, "the runtime directory is not usable: {reason}")
            }
            EndpointError::ControlDir(reason) => {
                write!(f, "the control directory is not usable: {reason}")
            }
            EndpointError::NotOwned(path) => {
                write!(f, "{} is not owned by this user", path.display())
            }
            EndpointError::Symlink(path) => {
                write!(f, "{} is a symbolic link", path.display())
            }
            EndpointError::NotSocket(path) => {
                write!(f, "{} is not a Unix socket", path.display())
            }
            EndpointError::LiveServer(path) => {
                write!(
                    f,
                    "another Gimbal instance is already listening at {}",
                    path.display()
                )
            }
            EndpointError::Permissions(path, reason) => {
                write!(f, "{} has unsafe permissions: {reason}", path.display())
            }
            EndpointError::Busy(path) => {
                write!(
                    f,
                    "the endpoint {} is held by an unknown peer",
                    path.display()
                )
            }
            EndpointError::Io(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for EndpointError {}

impl Server {
    /// Validate and create the endpoint, then start accepting.
    pub fn start(config: ServerConfig) -> Result<(Self, Receiver<Inbound>), EndpointError> {
        let (directory, path) = endpoint_paths(&config.runtime_dir)?;
        validate_runtime_dir(&config.runtime_dir, config.uid)?;
        prepare_control_dir(&directory, config.uid)?;
        remove_stale_socket(&path, config.uid)?;

        let listener = UnixListener::bind(&path).map_err(|error| {
            EndpointError::Io(format!("could not bind {}: {error}", path.display()))
        })?;
        listener.set_nonblocking(true).map_err(|error| {
            EndpointError::Io(format!("could not set the listener nonblocking: {error}"))
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).map_err(|error| {
            EndpointError::Permissions(path.clone(), format!("could not set 0600: {error}"))
        })?;

        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            EndpointError::Io(format!("could not inspect {}: {error}", path.display()))
        })?;
        if !metadata.file_type().is_socket() {
            let _ = fs::remove_file(&path);
            return Err(EndpointError::NotSocket(path));
        }
        if metadata.uid() != config.uid {
            let _ = fs::remove_file(&path);
            return Err(EndpointError::NotOwned(path));
        }
        if metadata.mode() & 0o077 != 0 {
            let _ = fs::remove_file(&path);
            return Err(EndpointError::Permissions(
                path,
                format!("mode {:o}, expected 0600", metadata.mode() & 0o777),
            ));
        }
        let identity = (metadata.dev(), metadata.ino());

        let (sender, requests) = std::sync::mpsc::sync_channel(MAX_PENDING_REQUESTS);
        workers().reap();
        let inner = Arc::new(Inner {
            stop: AtomicBool::new(false),
            config: config.clone(),
            sender,
            clients: Mutex::new(Vec::new()),
        });

        log::event(
            "agent",
            format!(
                "listener {} generation {} instance {}",
                path.display(),
                config.generation,
                config.instance
            ),
        );

        let accept_inner = Arc::clone(&inner);
        let accept = std::thread::Builder::new()
            .name("gimbal-agent-accept".to_string())
            .spawn(move || accept_loop(listener, accept_inner))
            .map_err(|error| {
                let _ = fs::remove_file(&path);
                EndpointError::Io(format!("could not start the accept thread: {error}"))
            })?;

        Ok((
            Self {
                path,
                directory,
                identity,
                inner,
                accept: Some(accept),
            },
            requests,
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn generation(&self) -> u64 {
        self.inner.config.generation
    }

    /// Revoke access and publish the endpoint removal without waiting for a
    /// client worker that may be blocked in a filesystem resolver. Every
    /// connected stream is shut down immediately, so peers see EOF even when
    /// their worker cannot yet reach the stop flag. The accept loop is
    /// nonblocking and joins promptly; client threads exit on their own.
    pub fn stop(&mut self) {
        if self.inner.stop.swap(true, Ordering::SeqCst) {
            return;
        }
        log::event(
            "agent",
            format!("stopping generation {}", self.inner.config.generation),
        );
        // A connection attempt wakes a listener blocked in accept on some
        // kernels; the nonblocking loop also checks stop every 20 ms.
        let _ = UnixStream::connect(&self.path);

        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }

        // Publish revocation first: every read/write path checks `stop` before
        // producing output. Shutting down the stream wakes a peer immediately
        // and does not wait for the worker's resolver call.
        if let Ok(clients) = self.inner.clients.lock() {
            for client in clients.iter() {
                let _ = client.shutdown(Shutdown::Both);
            }
        }

        if socket_identity_matches(&self.path, self.identity, self.inner.config.uid) {
            if let Err(error) = fs::remove_file(&self.path) {
                log::event(
                    "agent",
                    format!("could not remove {}: {error}", self.path.display()),
                );
            } else {
                log::event("agent", format!("removed {}", self.path.display()));
            }
        } else {
            log::event(
                "agent",
                format!("not removing {}: its identity changed", self.path.display()),
            );
        }
        workers().reap();
        log::event("agent", "stopped");
    }

    /// Live transport workers, process-wide. Finished handles are dropped
    /// instead of accumulating across enable cycles.
    pub fn retained_workers(&self) -> usize {
        workers().reap();
        workers().active()
    }

    /// Test/lifecycle helper: wait for every detached client worker to exit.
    pub fn wait_for_workers(&self, timeout: Duration) -> bool {
        workers().wait_idle(timeout)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

impl fmt::Debug for Server {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Server")
            .field("path", &self.path)
            .field("generation", &self.inner.config.generation)
            .finish_non_exhaustive()
    }
}

fn protected_refusal(
    request: &Request,
    roots: &ResolvedRoots,
    hook: Option<&ResolverHook>,
) -> Option<String> {
    let protocol::Operation::Run {
        command, arguments, ..
    } = &request.operation
    else {
        return None;
    };
    let invocation = fm_core::command::Invocation::new(command.clone(), arguments.clone());
    let checked = fm_core::command::Registry::new()
        .validate(fm_core::command::Caller::Agent, &invocation)
        .ok()?;
    let file_request = fm_core::fileops::request_for(&checked).ok().flatten()?;
    if let Some(hook) = hook {
        return (hook.0)(request, roots).err();
    }
    protect::check_request(&file_request, roots)
        .err()
        .map(|error| error.to_string())
}

fn server_error(
    inner: &Inner,
    request_id: &str,
    operation: &str,
    code: &'static str,
    message: impl Into<String>,
) -> Reply {
    let mut reply = Reply::error(request_id, operation, code, message);
    reply.instance = Some(inner.config.instance.clone());
    reply.generation = Some(inner.config.generation);
    reply
}

fn accept_loop(listener: UnixListener, inner: Arc<Inner>) {
    while !inner.stop.load(Ordering::SeqCst) {
        // Reap completed clients even on an idle server, so many short
        // sessions cannot retain an unbounded handle history.
        workers().reap();
        match listener.accept() {
            Ok((stream, _)) => {
                if inner.stop.load(Ordering::SeqCst) {
                    let _ = stream.shutdown(Shutdown::Both);
                    break;
                }
                if !workers().try_acquire() {
                    log::event(
                        "agent",
                        "refusing a client: the process-wide transport worker budget is full",
                    );
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    let reply = server_error(
                        &inner,
                        "",
                        "",
                        "busy",
                        "too many local transport workers are busy; try again later",
                    );
                    if let Ok(payload) = protocol::encode_reply(&reply)
                        && let Ok(framed) = protocol::frame_bytes(&payload)
                    {
                        let _ = stream.write_all(&framed);
                    }
                    continue;
                }

                let stream = Arc::new(stream);
                if let Ok(mut clients) = inner.clients.lock() {
                    clients.push(Arc::clone(&stream));
                }
                let connection = Arc::clone(&inner);
                let worker_stream = Arc::clone(&stream);
                let spawned = std::thread::Builder::new()
                    .name("gimbal-agent-client".to_string())
                    .spawn(move || {
                        serve_client(&worker_stream, &connection);
                        if let Ok(mut clients) = connection.clients.lock() {
                            clients.retain(|client| !Arc::ptr_eq(client, &worker_stream));
                        }
                        workers().release();
                    });
                match spawned {
                    Ok(handle) => workers().track(handle),
                    Err(error) => {
                        if let Ok(mut clients) = inner.clients.lock() {
                            clients.retain(|client| !Arc::ptr_eq(client, &stream));
                        }
                        let _ = stream.shutdown(Shutdown::Both);
                        workers().release();
                        log::event("agent", format!("could not start a client thread: {error}"));
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                if !inner.stop.load(Ordering::SeqCst) {
                    log::event("agent", format!("accept failed: {error}"));
                }
                break;
            }
        }
    }
}

fn serve_client(stream: &Arc<UnixStream>, inner: &Arc<Inner>) {
    if let Err(error) = stream.set_read_timeout(Some(WAKE)) {
        log::event("agent", format!("could not set the read timeout: {error}"));
        return;
    }
    if let Err(error) = stream.set_write_timeout(Some(WAKE)) {
        log::event("agent", format!("could not set the write timeout: {error}"));
        return;
    }

    if inner.config.reject_peer {
        log::event("agent", "refused a client: injected credential failure");
        return;
    }
    match peer_is_same_user(stream, inner.config.uid) {
        Ok(true) => {}
        Ok(false) => {
            log::event(
                "agent",
                "refused a client: its uid does not match the server",
            );
            return;
        }
        Err(error) => {
            log::event(
                "agent",
                format!("refused a client: credentials failed: {error}"),
            );
            return;
        }
    }

    let mut seen: VecDeque<String> = VecDeque::new();
    let mut accepted = 0usize;

    loop {
        if inner.stop.load(Ordering::SeqCst) {
            return;
        }
        let frame = match read_frame_interruptible(stream, &inner.stop) {
            Ok(Some(frame)) => frame,
            Ok(None) => return,
            Err(FrameRead::Stopped) => return,
            Err(FrameRead::Timeout) => {
                log::event("agent", "closing an idle client");
                return;
            }
            Err(FrameRead::Error(error)) => {
                log::event("agent", format!("closing a client: {error}"));
                return;
            }
        };

        let request = match protocol::parse_frame(&frame) {
            Ok(request) => request,
            Err(error) => {
                log::event("agent", format!("protocol error: {error}"));
                let reply = server_error(inner, "", "", error.code, error.message);
                let _ = write_reply(stream, &reply, &inner.stop);
                return;
            }
        };

        if accepted >= MAX_SESSION_REQUESTS {
            let reply = server_error(
                inner,
                &request.request_id,
                request.operation.name(),
                "session-exhausted",
                format!(
                    "this connection has used its {MAX_SESSION_REQUESTS} request ids; \
                     open a fresh connection"
                ),
            );
            let _ = write_reply(stream, &reply, &inner.stop);
            log::event(
                "agent",
                format!("closing a session after {MAX_SESSION_REQUESTS} requests"),
            );
            return;
        }
        accepted += 1;
        if seen.contains(&request.request_id) {
            let reply = server_error(
                inner,
                &request.request_id,
                request.operation.name(),
                "duplicate-request",
                "this request_id was already used on this connection",
            );
            let _ = write_reply(stream, &reply, &inner.stop);
            continue;
        }
        // The session budget bounds this queue, so an id is never silently
        // forgotten while its connection is still open.
        seen.push_back(request.request_id.clone());

        // Bind every request to this enable cycle before it reaches the UI.
        if request
            .instance
            .as_deref()
            .is_some_and(|instance| instance != inner.config.instance)
            || request
                .generation
                .is_some_and(|generation| generation != inner.config.generation)
        {
            let reply = server_error(
                inner,
                &request.request_id,
                request.operation.name(),
                "stale",
                "this request names an earlier application instance or enable cycle",
            );
            let _ = write_reply(stream, &reply, &inner.stop);
            continue;
        }

        // The control-path check needs filesystem resolution, so it runs on
        // this worker, not the GTK thread. A refusal never opens a job.
        if let Some(roots) = &inner.config.protected
            && let Some(reason) =
                protected_refusal(&request, roots, inner.config.resolve_hook.as_ref())
        {
            log::event(
                "agent",
                format!(
                    "refused {} {}: {reason}",
                    request.request_id,
                    request.operation.name()
                ),
            );
            let reply = server_error(
                inner,
                &request.request_id,
                request.operation.name(),
                "protected-path",
                reason,
            );
            let _ = write_reply(stream, &reply, &inner.stop);
            continue;
        }

        log::event(
            "agent",
            format!(
                "request {} {} generation {}",
                request.request_id,
                request.operation.name(),
                inner.config.generation
            ),
        );

        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        let abandoned = Arc::new(AtomicBool::new(false));
        let inbound = Inbound {
            generation: inner.config.generation,
            instance: inner.config.instance.clone(),
            request: request.clone(),
            reply: reply_tx,
            abandoned: Arc::clone(&abandoned),
        };
        match inner.sender.try_send(inbound) {
            Ok(()) => {}
            Err(std::sync::mpsc::TrySendError::Full(inbound)) => {
                inbound.abandon();
                let reply = server_error(
                    inner,
                    &request.request_id,
                    request.operation.name(),
                    "busy",
                    "the application already has the maximum pending requests; try again",
                );
                let _ = write_reply(stream, &reply, &inner.stop);
                continue;
            }
            Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                log::event("agent", "the UI bridge is gone");
                return;
            }
        }

        match wait_for_bridge(&reply_rx, &inner.stop, inner.config.bridge_timeout) {
            BridgeWait::Reply(reply) => {
                log::event(
                    "agent",
                    format!(
                        "reply {} {} {}",
                        reply.request_id,
                        reply.operation,
                        reply.status.as_str()
                    ),
                );
                if write_reply(stream, &reply, &inner.stop).is_err() {
                    abandoned.store(true, Ordering::SeqCst);
                    return;
                }
            }
            BridgeWait::Stopped => {
                abandoned.store(true, Ordering::SeqCst);
                return;
            }
            BridgeWait::Timeout => {
                // The bridge may still pop this message later. Mark it so it
                // is discarded without any command or file effect.
                abandoned.store(true, Ordering::SeqCst);
                let reply = server_error(
                    inner,
                    &request.request_id,
                    request.operation.name(),
                    "timeout",
                    "the application did not answer in time",
                );
                let _ = write_reply(stream, &reply, &inner.stop);
                return;
            }
        }
    }
}

enum BridgeWait {
    Reply(Box<Reply>),
    Stopped,
    Timeout,
}

fn wait_for_bridge(
    receiver: &Receiver<Reply>,
    stop: &AtomicBool,
    bridge_timeout: Duration,
) -> BridgeWait {
    let deadline = Instant::now() + bridge_timeout;
    loop {
        if stop.load(Ordering::SeqCst) {
            return BridgeWait::Stopped;
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(reply) => return BridgeWait::Reply(Box::new(reply)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if Instant::now() >= deadline {
                    return BridgeWait::Timeout;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return BridgeWait::Stopped;
            }
        }
    }
}

enum FrameRead {
    Stopped,
    Timeout,
    Error(String),
}

fn read_frame_interruptible(
    stream: &UnixStream,
    stop: &AtomicBool,
) -> Result<Option<Vec<u8>>, FrameRead> {
    let mut stream = stream;
    let mut length = [0_u8; 4];
    let mut read = 0;
    let idle_deadline = Instant::now() + IDLE_TIMEOUT;
    while read < length.len() {
        if stop.load(Ordering::SeqCst) {
            return Err(FrameRead::Stopped);
        }
        match stream.read(&mut length[read..]) {
            Ok(0) if read == 0 => return Ok(None),
            Ok(0) => return Err(FrameRead::Error("a partial length".to_string())),
            Ok(count) => read += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= idle_deadline {
                    return Err(FrameRead::Timeout);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameRead::Error(error.to_string())),
        }
    }

    let length = u32::from_be_bytes(length) as usize;
    if length == 0 {
        return Err(FrameRead::Error("an empty frame".to_string()));
    }
    if length > MAX_FRAME_BYTES {
        return Err(FrameRead::Error(format!(
            "a {length}-byte frame exceeds the limit"
        )));
    }
    let mut payload = vec![0_u8; length];
    let mut read = 0;
    while read < payload.len() {
        if stop.load(Ordering::SeqCst) {
            return Err(FrameRead::Stopped);
        }
        match stream.read(&mut payload[read..]) {
            Ok(0) => return Err(FrameRead::Error("a truncated frame".to_string())),
            Ok(count) => read += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if Instant::now() >= idle_deadline {
                    return Err(FrameRead::Timeout);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameRead::Error(error.to_string())),
        }
    }
    Ok(Some(payload))
}

fn write_reply(stream: &UnixStream, reply: &Reply, stop: &AtomicBool) -> io::Result<()> {
    let mut stream = stream;
    let payload =
        protocol::encode_reply(reply).map_err(|error| io::Error::other(error.to_string()))?;
    let framed =
        protocol::frame_bytes(&payload).map_err(|error| io::Error::other(error.to_string()))?;
    let mut written = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    while written < framed.len() {
        if stop.load(Ordering::SeqCst) {
            return Err(io::Error::other("stopped"));
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other("the client did not read its reply"));
        }
        match stream.write(&framed[written..]) {
            Ok(0) => return Err(io::Error::other("the client closed the connection")),
            Ok(count) => written += count,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn peer_is_same_user(stream: &UnixStream, uid: u32) -> Result<bool, String> {
    let clone = stream
        .try_clone()
        .map_err(|error| format!("could not clone the stream: {error}"))?;
    let fd: std::os::unix::io::OwnedFd = clone.into();
    let socket =
        gio::Socket::from_fd(fd).map_err(|error| format!("could not wrap the stream: {error}"))?;
    let credentials = socket
        .credentials()
        .map_err(|error| format!("no peer credentials: {error}"))?;
    let peer = credentials
        .unix_user()
        .map_err(|error| format!("no peer uid: {error}"))?;
    Ok(peer == uid)
}

fn endpoint_paths(runtime_dir: &Path) -> Result<(PathBuf, PathBuf), EndpointError> {
    if !runtime_dir.is_absolute() {
        return Err(EndpointError::RuntimeDir(format!(
            "{} is not absolute",
            runtime_dir.display()
        )));
    }
    let directory = runtime_dir.join(CONTROL_DIR);
    let path = directory.join(SOCKET_NAME);
    Ok((directory, path))
}

fn validate_runtime_dir(runtime_dir: &Path, uid: u32) -> Result<(), EndpointError> {
    let mut prefix = PathBuf::from("/");
    for component in runtime_dir.components() {
        prefix.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&prefix)
            .map_err(|error| EndpointError::RuntimeDir(format!("{}: {error}", prefix.display())))?;
        if metadata.file_type().is_symlink() {
            return Err(EndpointError::Symlink(prefix));
        }
    }
    let metadata = fs::symlink_metadata(runtime_dir).map_err(|error| {
        EndpointError::RuntimeDir(format!("{}: {error}", runtime_dir.display()))
    })?;
    if !metadata.is_dir() {
        return Err(EndpointError::RuntimeDir(format!(
            "{} is not a folder",
            runtime_dir.display()
        )));
    }
    if metadata.uid() != uid {
        return Err(EndpointError::NotOwned(runtime_dir.to_path_buf()));
    }
    Ok(())
}

fn prepare_control_dir(directory: &Path, uid: u32) -> Result<(), EndpointError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(EndpointError::Symlink(directory.to_path_buf()));
            }
            if !metadata.is_dir() {
                return Err(EndpointError::ControlDir(format!(
                    "{} is not a folder",
                    directory.display()
                )));
            }
            if metadata.uid() != uid {
                return Err(EndpointError::NotOwned(directory.to_path_buf()));
            }
            if metadata.mode() & 0o077 != 0 {
                return Err(EndpointError::Permissions(
                    directory.to_path_buf(),
                    format!("mode {:o}, expected 0700", metadata.mode() & 0o777),
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(directory)
                .map_err(|error| {
                    EndpointError::ControlDir(format!(
                        "could not create {}: {error}",
                        directory.display()
                    ))
                })?;
            let metadata = fs::symlink_metadata(directory).map_err(|error| {
                EndpointError::ControlDir(format!(
                    "could not inspect {}: {error}",
                    directory.display()
                ))
            })?;
            if metadata.uid() != uid {
                return Err(EndpointError::NotOwned(directory.to_path_buf()));
            }
        }
        Err(error) => {
            return Err(EndpointError::ControlDir(format!(
                "could not inspect {}: {error}",
                directory.display()
            )));
        }
    }
    Ok(())
}

/// Remove a socket left by a dead server, and only that: a symlink, a regular
/// file, a foreign owner, a live server or an unknown connect error all refuse.
fn remove_stale_socket(path: &Path, uid: u32) -> Result<(), EndpointError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(EndpointError::Io(format!(
                "could not inspect {}: {error}",
                path.display()
            )));
        }
    };
    if metadata.file_type().is_symlink() {
        return Err(EndpointError::Symlink(path.to_path_buf()));
    }
    if !metadata.file_type().is_socket() {
        return Err(EndpointError::NotSocket(path.to_path_buf()));
    }
    if metadata.uid() != uid {
        return Err(EndpointError::NotOwned(path.to_path_buf()));
    }

    match UnixStream::connect(path) {
        Ok(_) => return Err(EndpointError::LiveServer(path.to_path_buf())),
        Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
            fs::remove_file(path).map_err(|error| {
                EndpointError::Io(format!(
                    "could not remove the stale socket {}: {error}",
                    path.display()
                ))
            })?;
            log::event(
                "agent",
                format!("removed stale endpoint {}", path.display()),
            );
            Ok(())
        }
        Err(_error) => Err(EndpointError::Busy(path.to_path_buf())),
    }
    .map_err(|error| match error {
        EndpointError::Busy(path) => EndpointError::Io(format!(
            "{} is a socket but liveness could not be established",
            path.display()
        )),
        other => other,
    })
}

fn socket_identity_matches(path: &Path, identity: (u64, u64), uid: u32) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_socket()
        && metadata.uid() == uid
        && (metadata.dev(), metadata.ino()) == identity
}

/// Test/lifecycle observation: live transport workers across every server in
/// this process, including detached generations.
#[doc(hidden)]
pub fn active_workers() -> usize {
    workers().reap();
    workers().active()
}

/// The socket path for a runtime directory, without touching the disk.
pub fn socket_path(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(CONTROL_DIR).join(SOCKET_NAME)
}

/// A test/integration helper: wait for one inbound request, or return `None`
/// when the server stopped.
pub fn next_request(receiver: &Receiver<Inbound>, timeout: Duration) -> Option<Inbound> {
    receiver.recv_timeout(timeout).ok()
}

/// Test seam: a nonblocking admission that fails when the bridge is full.
pub fn try_send(sender: &SyncSender<Inbound>, inbound: Inbound) -> Result<(), String> {
    sender.try_send(inbound).map_err(|error| match error {
        std::sync::mpsc::TrySendError::Full(inbound) => {
            inbound.abandon();
            "the pending request queue is full".to_string()
        }
        std::sync::mpsc::TrySendError::Disconnected(_) => "the transport stopped".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// The worker budget and the socket path namespace are process-wide, so
    /// these tests must not overlap each other.
    fn test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
        let deadline = Instant::now() + Duration::from_secs(3);
        while active_workers() != 0 {
            assert!(
                Instant::now() < deadline,
                "a previous test leaked transport workers"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        guard
    }

    struct Scratch {
        root: PathBuf,
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
            fs::set_permissions(&unique, fs::Permissions::from_mode(0o700))
                .expect("private scratch");
            Self {
                root: fs::canonicalize(unique).expect("canonical"),
            }
        }

        fn config(&self, generation: u64) -> ServerConfig {
            let uid = fs::metadata(&self.root).expect("metadata").uid();
            ServerConfig::new(&self.root, uid, format!("i{generation}"), generation)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn the_endpoint_is_private_and_removed_on_stop() {
        let _guard = test_lock();
        let scratch = Scratch::new("private");
        let (server, _requests) = Server::start(scratch.config(1)).expect("starts");
        let path = server.path().to_path_buf();
        let metadata = fs::symlink_metadata(&path).expect("socket exists");
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.mode() & 0o777, 0o600);
        let directory = fs::symlink_metadata(server.directory()).expect("directory");
        assert_eq!(directory.mode() & 0o777, 0o700);
        drop(server);
        assert!(!path.exists());
    }

    #[test]
    fn a_live_server_is_not_displaced() {
        let _guard = test_lock();
        let scratch = Scratch::new("live");
        let (server, _requests) = Server::start(scratch.config(1)).expect("starts");
        let error = Server::start(scratch.config(2)).expect_err("a live server wins");
        assert!(matches!(error, EndpointError::LiveServer(_)));
        drop(server);
    }

    #[test]
    fn a_stale_socket_is_removed_but_a_non_socket_is_never_touched() {
        let _guard = test_lock();
        let scratch = Scratch::new("stale");
        let config = scratch.config(1);
        let path = socket_path(&scratch.root);
        let directory = path.parent().expect("directory");
        fs::create_dir_all(directory).expect("dir");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).expect("dir mode");
        {
            let listener = UnixListener::bind(&path).expect("bind");
            drop(listener);
        }
        let (mut server, _requests) = Server::start(config).expect("stale removed");
        server.stop();
        assert!(!path.exists());

        fs::write(&path, "not a socket").expect("file");
        let error = Server::start(scratch.config(2)).expect_err("regular file refused");
        assert!(matches!(error, EndpointError::NotSocket(_)));
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).expect("kept"), "not a socket");
    }

    #[test]
    fn a_symlink_endpoint_is_refused_without_following_it() {
        let _guard = test_lock();
        let scratch = Scratch::new("symlink");
        let config = scratch.config(1);
        let path = socket_path(&scratch.root);
        let directory = path.parent().expect("directory");
        fs::create_dir_all(directory).expect("dir");
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).expect("dir mode");
        let target = scratch.root.join("target");
        fs::write(&target, "keep").expect("target");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        let error = Server::start(config).expect_err("symlink refused");
        assert!(matches!(error, EndpointError::Symlink(_)));
        assert_eq!(fs::read_to_string(&target).expect("kept"), "keep");
        assert!(
            fs::symlink_metadata(&path)
                .expect("still a link")
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn a_relative_or_foreign_runtime_directory_fails_closed() {
        let _guard = test_lock();
        let error = Server::start(ServerConfig::new("relative/runtime", 0, "i1", 1))
            .expect_err("relative refused");
        assert!(matches!(error, EndpointError::RuntimeDir(_)));

        let scratch = Scratch::new("foreign");
        let uid = fs::metadata(&scratch.root).expect("metadata").uid();
        let config = ServerConfig::new(&scratch.root, uid.wrapping_add(1), "i1", 1);
        let error = Server::start(config).expect_err("foreign uid refused");
        assert!(matches!(
            error,
            EndpointError::NotOwned(_) | EndpointError::RuntimeDir(_)
        ));
    }

    #[test]
    fn a_rejected_credential_failure_closes_the_client() {
        let _guard = test_lock();
        let scratch = Scratch::new("credentials");
        let mut config = scratch.config(1);
        config.reject_peer = true;
        let (server, requests) = Server::start(config).expect("starts");
        let mut client = UnixStream::connect(server.path()).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_millis(500)))
            .expect("timeout");
        protocol::write_frame(
            &mut client,
            b"version = 1\nrequest_id = \"r\"\noperation = \"state\"\n",
        )
        .expect("write");
        let mut buffer = [0_u8; 1];
        assert!(matches!(client.read(&mut buffer), Ok(0) | Err(_)));
        assert!(next_request(&requests, Duration::from_millis(200)).is_none());
        drop(server);
    }
}
