//! The daemon process: a unix socket, a thread per connection, one shared query engine.
//!
//! The whole reason this exists (design.md §1) is that Oxc, the built-in definitions and the
//! runtime metadata should be paid for once per session rather than once per command. That makes
//! *process lifetime* the resource being protected here: no single client may be able to wedge
//! the daemon, because doing so throws away everything warm.

use kiln_db::{MemoEngine, QueryEngine};
use kiln_module_graph::ModuleGraph;
use kiln_proto::{Request, ResolutionMap, Response, SourceDigest, SourceView, Stats, SyncId};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::{self, BufReader, ErrorKind};
use std::net::Shutdown as SocketShutdown;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use crate::dispatch::{
    Next, dispatch, dispatch_source, execute_prepared_run, native_build_graph, native_build_output,
    native_build_program, native_build_shared_graph, native_build_shared_program,
    native_graph_with_cache, native_program, native_run_graph, native_run_program,
    prepare_run_with_cache,
};
use crate::framing::{MAX_MESSAGE_BYTES, read_frame, write_frame};
use crate::graph_cache::GraphCache;

#[derive(Debug)]
pub enum DaemonError {
    /// Another daemon is already listening on this socket. Not an error to paper over: two
    /// daemons on one project would each hold a divergent view of the same files.
    AlreadyRunning { socket: PathBuf },
    /// The socket could not be made ready to accept on - preparing its directory, clearing a
    /// stale path, or the `bind` itself.
    Listen { socket: PathBuf, source: io::Error },
}

/// The authoritative source set and its coherence identity move together. Every request that
/// consumes source bytes checks `source_view` while holding this same mutex through preparation;
/// this prevents a query from observing a source set that changes between its check and parse.
pub(crate) struct DaemonState<E> {
    pub(crate) engine: E,
    pub(crate) source_view: SourceView,
    pub(crate) resolutions: Option<ResolutionMap>,
    pub(crate) graph_cache: GraphCache,
}

/// Connection-local synchronization state. Staged text is private until CommitSync obtains the
/// daemon-state lock and publishes the complete manifest in one critical section.
struct PendingSync {
    id: SyncId,
    base_view: SourceView,
    manifest: BTreeMap<String, SourceDigest>,
    needed: BTreeSet<String>,
    staged: BTreeMap<String, String>,
    resolutions: ResolutionMap,
}

impl fmt::Display for DaemonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning { socket } => {
                write!(
                    f,
                    "a kiln daemon is already listening on {}",
                    socket.display()
                )
            }
            Self::Listen { socket, source } => {
                write!(f, "cannot listen on {}: {source}", socket.display())
            }
        }
    }
}

impl std::error::Error for DaemonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Listen { source, .. } => Some(source),
            Self::AlreadyRunning { .. } => None,
        }
    }
}

/// State shared between the accept loop, every connection thread and every shutdown handle.
struct Shared {
    stop: AtomicBool,
    socket: PathBuf,
    /// A clone of every live client socket, so shutdown can unblock a thread parked in `read`.
    /// Without this a client that connects and says nothing keeps `serve` from ever returning.
    live: Mutex<HashMap<u64, UnixStream>>,
    next_connection: AtomicU64,
}

impl Shared {
    fn lock_live(&self) -> std::sync::MutexGuard<'_, HashMap<u64, UnixStream>> {
        // A panic while registering a connection would be a bug, but wedging every future
        // connection because of it would be worse. See `lock_engine` for the same argument.
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn register(&self, stream: UnixStream) -> u64 {
        let id = self.next_connection.fetch_add(1, Ordering::Relaxed);
        self.lock_live().insert(id, stream);
        id
    }

    fn deregister(&self, id: u64) {
        self.lock_live().remove(&id);
    }

    fn hang_up_everyone(&self) {
        for stream in self.lock_live().values() {
            // Errors are expected here - the peer may already be gone - and are not actionable.
            let _ = stream.shutdown(SocketShutdown::Both);
        }
    }
}

/// Asks a running [`Daemon`] to stop. Cheap to clone and safe to call from any thread, including
/// from inside a request handler.
#[derive(Clone)]
pub struct ShutdownHandle {
    shared: Arc<Shared>,
}

impl ShutdownHandle {
    /// Signal shutdown and return immediately; `Daemon::serve` returns once its threads have
    /// finished. Idempotent.
    pub fn shutdown(&self) {
        // Ordering matters: the flag must be visible before either wake-up, so that anything
        // waking sees it.
        self.shared.stop.store(true, Ordering::SeqCst);
        // `accept` is blocking, and there is no portable way to interrupt it, so we become a
        // client for a moment. The connection is refused by the loop, which re-checks `stop`
        // before touching it. This relies on the socket file still existing, which it does for
        // as long as the `Daemon` is alive.
        let _ = UnixStream::connect(&self.shared.socket);
        self.shared.hang_up_everyone();
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shared.stop.load(Ordering::SeqCst)
    }
}

/// Unlinks the socket when the daemon goes away. A path left behind is recoverable (see
/// [`bind_listener`]) but it is rude to rely on the next process to clean up after this one.
struct SocketGuard(PathBuf);

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A bound, not-yet-serving daemon.
///
/// Split from `serve` so that a caller - the CLI spawning a daemon, or the timing harness
/// measuring cold start - knows the socket is accepting connections before it does anything
/// else. Otherwise every caller has to poll for the socket to appear.
pub struct Daemon<E: QueryEngine = MemoEngine> {
    listener: UnixListener,
    shared: Arc<Shared>,
    state: Arc<Mutex<DaemonState<E>>>,
    _socket: SocketGuard,
}

impl Daemon<MemoEngine> {
    /// Bind `socket`, serving a fresh [`MemoEngine`] backed by the Oxc frontend.
    pub fn bind(socket: &Path) -> Result<Self, DaemonError> {
        Self::bind_with(socket, MemoEngine::new(frontend_oxc::parse))
    }
}

impl<E: QueryEngine + Send + 'static> Daemon<E> {
    /// Bind `socket`, serving `engine`.
    ///
    /// Generic over the engine so that tests and experiments can drive the server against
    /// something other than the production database - the server half only ever depends on
    /// [`QueryEngine`], never on a concrete backend (docs/adr/0001).
    pub fn bind_with(socket: &Path, engine: E) -> Result<Self, DaemonError> {
        if let Some(parent) = socket.parent() {
            // 0700: on a shared /tmp this stops another user from connecting to, or replacing,
            // this project's socket. If the directory already exists and is someone else's, the
            // bind below fails with a permission error rather than silently sharing.
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)
                .map_err(|source| DaemonError::Listen {
                    socket: socket.to_path_buf(),
                    source,
                })?;
        }
        let listener = bind_listener(socket)?;

        Ok(Self {
            listener,
            shared: Arc::new(Shared {
                stop: AtomicBool::new(false),
                socket: socket.to_path_buf(),
                live: Mutex::new(HashMap::new()),
                next_connection: AtomicU64::new(0),
            }),
            state: Arc::new(Mutex::new(DaemonState {
                engine,
                source_view: SourceView(0),
                resolutions: None,
                graph_cache: GraphCache::default(),
            })),
            _socket: SocketGuard(socket.to_path_buf()),
        })
    }

    pub fn socket_path(&self) -> &Path {
        &self.shared.socket
    }

    pub fn shutdown_handle(&self) -> ShutdownHandle {
        ShutdownHandle {
            shared: Arc::clone(&self.shared),
        }
    }

    /// Accept connections until shutdown, then wait for the in-flight ones to finish.
    ///
    /// Consumes the daemon so the socket file is unlinked on return.
    pub fn serve(self) {
        let mut workers: Vec<JoinHandle<()>> = Vec::new();

        for incoming in self.listener.incoming() {
            if self.shared.stop.load(Ordering::SeqCst) {
                break;
            }
            match incoming {
                Ok(stream) => {
                    workers.push(self.spawn_connection(stream));
                    workers = reap(workers);
                }
                // A signal landing mid-accept is not a reason to stop serving.
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(error) => {
                    // One failed accept (a peer that vanished, a transient fd shortage) must not
                    // cost every warm client its daemon.
                    eprintln!("kiln-daemon: accept failed: {error}");
                }
            }
        }

        // A connection accepted in the window between `stop` being set and the loop noticing has
        // not been hung up yet; do it now so no worker is parked in `read` forever.
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.hang_up_everyone();
        for worker in workers {
            let _ = worker.join();
        }
    }

    fn spawn_connection(&self, stream: UnixStream) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        let shared = Arc::clone(&self.shared);

        // Built rather than `spawn`ed, for the stack: see `crate::SERVE_STACK_BYTES`. A default
        // 2 MiB thread overflows on an ordinary project, and a stack overflow here aborts the
        // whole daemon rather than failing one connection.
        thread::Builder::new()
            .stack_size(crate::SERVE_STACK_BYTES)
            .spawn(move || {
                // A clone of the socket, not the socket, so the thread keeps sole ownership of the
                // one it reads and writes.
                let handle = match stream.try_clone() {
                    Ok(handle) => handle,
                    Err(error) => {
                        eprintln!("kiln-daemon: cannot track connection: {error}");
                        return;
                    }
                };
                let id = shared.register(handle);

                // Backstop for a panic outside the per-request guard - in the framing layer, say.
                // It costs one connection; the alternative is a dead daemon.
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    serve_connection(stream, &state, &shared)
                }));
                shared.deregister(id);

                match outcome {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("kiln-daemon: connection closed: {error}"),
                    Err(payload) => {
                        eprintln!(
                            "kiln-daemon: connection panicked: {}",
                            panic_message(&*payload)
                        )
                    }
                }
            })
            // A thread that cannot be created is not a connection that can be served, and the
            // accept loop has no better answer than the one it already gives an unservable peer.
            .expect("spawn a connection thread")
    }
}

/// Join the finished workers and keep the rest, so a long-lived daemon does not accumulate a
/// handle per connection it has ever served.
fn reap(workers: Vec<JoinHandle<()>>) -> Vec<JoinHandle<()>> {
    let (finished, running): (Vec<_>, Vec<_>) = workers.into_iter().partition(|w| w.is_finished());
    for worker in finished {
        let _ = worker.join();
    }
    running
}

/// Bind, clearing a socket file left behind by a daemon that died without cleaning up.
///
/// Unix rejects a bind to any existing path, so a crash would otherwise wedge the project
/// permanently. The only safe test for "still in use" is to try to be a client: a path nothing
/// answers on is dead and can be removed.
fn bind_listener(socket: &Path) -> Result<UnixListener, DaemonError> {
    let listening = |source: io::Error| DaemonError::Listen {
        socket: socket.to_path_buf(),
        source,
    };

    match UnixListener::bind(socket) {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == ErrorKind::AddrInUse => {
            if UnixStream::connect(socket).is_ok() {
                return Err(DaemonError::AlreadyRunning {
                    socket: socket.to_path_buf(),
                });
            }
            // Nothing is listening: a stale socket, or some other file in the way. Removing it
            // is safe precisely because no daemon can be using it. Exactly one retry - if the
            // bind still fails, something else is wrong and looping would only hide it.
            std::fs::remove_file(socket).map_err(listening)?;
            UnixListener::bind(socket).map_err(listening)
        }
        Err(error) => Err(listening(error)),
    }
}

/// Read requests from one client until it hangs up, the request stream breaks, or the daemon
/// stops. Socket I/O happens here, deliberately outside the engine lock.
fn serve_connection<E: QueryEngine>(
    stream: UnixStream,
    state: &Mutex<DaemonState<E>>,
    shared: &Arc<Shared>,
) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    let mut pending = None;
    let mut next_sync_id = 0u64;

    // Checked before every read as well as after every response: `shutdown` may have hung this
    // socket up before the thread ever reached its first read.
    while !shared.stop.load(Ordering::SeqCst) {
        let frame = match read_frame(&mut reader, MAX_MESSAGE_BYTES) {
            Ok(Some(frame)) => frame,
            Ok(None) => return Ok(()),
            // A frame we cannot read is a frame we cannot resynchronise from, so say why and
            // hang up on this client only.
            Err(error) => {
                let _ = respond(
                    &mut writer,
                    &Response::Error {
                        message: error.to_string(),
                    },
                );
                return Err(error);
            }
        };

        let (response, next) = match serde_json::from_str::<Request>(&frame) {
            Ok(request) => handle(state, &mut pending, &mut next_sync_id, request),
            // Line framing resynchronises on its own, so a garbled request costs one response,
            // not the connection.
            Err(error) => (
                Response::Error {
                    message: format!("malformed request: {error}"),
                },
                Next::Continue,
            ),
        };

        respond(&mut writer, &response)?;

        match next {
            Next::Continue => {}
            Next::CloseConnection => return Ok(()),
            // The response is already flushed, so the client sees `Ok` before we hang up.
            Next::StopDaemon => {
                ShutdownHandle {
                    shared: Arc::clone(shared),
                }
                .shutdown();
                return Ok(());
            }
        }
    }
    Ok(())
}

fn respond(writer: &mut UnixStream, response: &Response) -> io::Result<()> {
    let json = serde_json::to_string(response)?;
    write_frame(writer, &json)
}

fn begin_sync<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    id: SyncId,
    files: Vec<kiln_proto::SourceHash>,
    resolutions: ResolutionMap,
) -> (Response, Option<PendingSync>) {
    let mut manifest = BTreeMap::new();
    let mut duplicates = BTreeSet::new();
    for file in files {
        if manifest.insert(file.path.clone(), file.hash).is_some() {
            duplicates.insert(file.path);
        }
    }
    if !duplicates.is_empty() {
        return (
            Response::Error {
                message: format!(
                    "Sync request contains duplicate paths: {}",
                    duplicates.into_iter().collect::<Vec<_>>().join(", ")
                ),
            },
            None,
        );
    }

    let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    let held = guard
        .engine
        .source_files()
        .into_iter()
        .filter_map(|(file, path)| {
            guard
                .engine
                .source(file)
                .map(|text| (path, kiln_proto::source_digest(text)))
        })
        .collect::<BTreeMap<_, _>>();
    let needed = manifest
        .iter()
        .filter_map(|(path, expected)| match held.get(path) {
            Some(actual) if *actual == *expected => None,
            _ => Some(path.clone()),
        })
        .collect::<BTreeSet<_>>();
    let response = Response::Needed {
        sync_id: id,
        paths: needed.iter().cloned().collect(),
    };
    (
        response,
        Some(PendingSync {
            id,
            base_view: guard.source_view,
            manifest,
            needed,
            staged: BTreeMap::new(),
            resolutions,
        }),
    )
}

fn stage_source(
    pending: Option<&mut PendingSync>,
    sync_id: SyncId,
    path: String,
    text: String,
) -> Response {
    let Some(pending) = pending else {
        return Response::Error {
            message: "StageSource requires an active Sync".into(),
        };
    };
    if pending.id != sync_id {
        return Response::Error {
            message: format!("unknown sync id {}", sync_id.0),
        };
    }
    if !pending.needed.contains(&path) {
        return Response::Error {
            message: format!("path is not needed by sync: {path}"),
        };
    }
    let expected = pending.manifest[&path];
    if kiln_proto::source_digest(&text) != expected {
        return Response::Error {
            message: format!("staged source digest does not match manifest for {path}"),
        };
    }
    pending.staged.insert(path, text);
    Response::Ok
}

fn commit_sync<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    pending: &mut Option<PendingSync>,
    sync_id: SyncId,
) -> Response {
    let Some(transaction) = pending.as_ref() else {
        return Response::Error {
            message: "CommitSync requires an active Sync".into(),
        };
    };
    if transaction.id != sync_id {
        return Response::Error {
            message: format!("unknown sync id {}", sync_id.0),
        };
    }
    let Some(missing) = transaction
        .needed
        .iter()
        .find(|path| !transaction.staged.contains_key(*path))
    else {
        // A stale transaction is consumed deliberately: the caller must begin a fresh snapshot
        // rather than retrying against a source view that has already lost its authority.
        let transaction = pending.take().expect("transaction checked above");
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.source_view != transaction.base_view {
            return Response::StaleSourceView {
                current: guard.source_view,
            };
        }
        let next_view = match next_source_view(guard.source_view) {
            Ok(view) => view,
            Err(message) => return Response::Error { message },
        };
        let PendingSync {
            manifest,
            staged,
            resolutions,
            ..
        } = transaction;
        let mutation = panic::catch_unwind(AssertUnwindSafe(|| {
            let active = guard.engine.source_files();
            for (file, path) in active {
                if !manifest.contains_key(&path) {
                    guard.engine.remove_source(file);
                }
            }
            for (path, text) in staged {
                let file = guard.engine.intern(&path);
                guard.engine.set_source(file, text);
            }
        }));
        if let Err(payload) = mutation {
            guard.graph_cache.invalidate();
            if let Ok(view) = next_source_view(guard.source_view) {
                guard.source_view = view;
            }
            return internal_error(&*payload);
        }
        guard.resolutions = Some(resolutions);
        guard.source_view = next_view;
        return Response::Synced {
            source_view: guard.source_view,
        };
    };
    Response::Error {
        message: format!("sync is missing staged source: {missing}"),
    }
}

fn set_source<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    path: String,
    text: String,
    source_view: SourceView,
) -> Response {
    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.source_view != source_view {
        return Response::StaleSourceView {
            current: guard.source_view,
        };
    }
    let next_view = match next_source_view(guard.source_view) {
        Ok(view) => view,
        Err(message) => return Response::Error { message },
    };
    let mutation = panic::catch_unwind(AssertUnwindSafe(|| {
        let file = guard.engine.intern(&path);
        guard.engine.set_source(file, text);
    }));
    if let Err(payload) = mutation {
        guard.graph_cache.invalidate();
        guard.source_view = next_view;
        return internal_error(&*payload);
    }
    // Direct updates are authority events even when bytes are identical, matching CommitSync's
    // warm no-op behavior. Graph entries validate only the reachable source revisions, so both a
    // byte-identical update and an edit outside a graph retain that graph safely.
    guard.source_view = next_view;
    Response::SourceUpdated {
        source_view: guard.source_view,
    }
}

fn handle_source_request<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    source_view: SourceView,
    operation: impl FnOnce(&mut E, &mut GraphCache, Option<&ResolutionMap>) -> Response,
) -> (Response, Next) {
    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.source_view != source_view {
        return (
            Response::StaleSourceView {
                current: guard.source_view,
            },
            Next::Continue,
        );
    }
    let revision = guard.engine.revision().0;
    let resolutions = guard.resolutions.clone();
    match panic::catch_unwind(AssertUnwindSafe(|| {
        let DaemonState {
            engine,
            graph_cache,
            ..
        } = &mut *guard;
        operation(engine, graph_cache, resolutions.as_ref())
    })) {
        Ok(_response) if guard.engine.revision().0 != revision => {
            match next_source_view(guard.source_view) {
                Ok(view) => {
                    guard.source_view = view;
                    (Response::StaleSourceView { current: view }, Next::Continue)
                }
                Err(message) => (Response::Error { message }, Next::Continue),
            }
        }
        Ok(response) => (response, Next::Continue),
        Err(payload) => {
            if guard.engine.revision().0 != revision {
                guard.graph_cache.invalidate();
                match next_source_view(guard.source_view) {
                    Ok(view) => guard.source_view = view,
                    Err(message) => return (Response::Error { message }, Next::Continue),
                }
            }
            (internal_error(&*payload), Next::Continue)
        }
    }
}

fn next_source_view(current: SourceView) -> Result<SourceView, String> {
    current
        .0
        .checked_add(1)
        .map(SourceView)
        .ok_or_else(|| "source view exhausted".to_owned())
}

/// Run one request, holding the engine lock only while a query actually needs it.
///
/// The engine's methods take `&mut self`, so query preparation is serialized. Interpreter runs,
/// conformance batches, native compilation, linking, and process execution operate on owned input
/// after releasing the lock; expensive work must not block unrelated parse or check requests.
/// Socket I/O is likewise outside the lock, so a client that stops reading cannot hold the engine
/// hostage.
///
/// The guard is held *outside* `catch_unwind` and only reborrowed into it, which is what keeps
/// the mutex unpoisoned: nothing is dropped while the panic unwinds, and by the time the guard
/// itself is released the panic has already been caught. Poisoning here would be a bad trade -
/// one bad request would turn into a daemon that answers every later request with a lock error.
/// The engine can still be left mid-update by the panic, but its values are all derived and
/// recomputable, so the next query rebuilds them.
fn handle<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    pending: &mut Option<PendingSync>,
    next_sync_id: &mut u64,
    request: Request,
) -> (Response, Next) {
    match request {
        Request::Sync { files, resolutions } => {
            let id = SyncId(*next_sync_id);
            *next_sync_id = next_sync_id.wrapping_add(1);
            let (response, transaction) = begin_sync(state, id, files, resolutions);
            if let Some(transaction) = transaction {
                *pending = Some(transaction);
            }
            (response, Next::Continue)
        }
        Request::StageSource {
            sync_id,
            path,
            text,
        } => (
            stage_source(pending.as_mut(), sync_id, path, text),
            Next::Continue,
        ),
        Request::CommitSync { sync_id } => (commit_sync(state, pending, sync_id), Next::Continue),
        Request::SetSource {
            path,
            text,
            source_view,
        } => (set_source(state, path, text, source_view), Next::Continue),
        Request::Check {
            path,
            project_root,
            source_view,
        } => handle_source_request(state, source_view, |engine, graph_cache, resolutions| {
            crate::dispatch::check_with_cache(
                engine,
                Some(graph_cache),
                path,
                project_root,
                resolutions,
            )
        }),
        Request::Project { source_view } => {
            handle_source_request(state, source_view, |engine, _, resolutions| {
                dispatch_source(engine, resolutions, Request::Project { source_view }).0
            })
        }
        Request::Run {
            path,
            project_root,
            node,
            source_view,
        } => handle_run(state, path, project_root, node, source_view),
        Request::Test {
            root,
            mode,
            changed_paths,
            previously_failed,
        } => run_without_engine_lock(|| {
            #[cfg(test)]
            crate::dispatch::wait_for_test_gate(&root);
            match crate::testing::run(&root, mode, changed_paths, previously_failed) {
                Ok(summary) => Response::Tested(summary),
                Err(message) => Response::Error { message },
            }
        }),
        request @ Request::NativeBuildAtRevision { .. } => {
            handle_native_build_at_revision(state, request)
        }
        Request::NativeBuild {
            path,
            project_root,
            node,
            output,
            profile,
            source_view,
        } => {
            let prepared = match prepare_native_snapshot(
                state,
                &path,
                &project_root,
                source_view,
                false,
                node,
            ) {
                Ok(prepared) => prepared,
                Err(response) => return (response, Next::Continue),
            };
            match prepared {
                PreparedNative::Graph(graph) => run_without_engine_lock(|| {
                    native_build_graph(&graph, path, project_root, output, profile)
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_build_program(program, path, project_root, output, profile)
                }),
            }
        }
        Request::NativeSharedBuild {
            path,
            project_root,
            output,
            profile,
            source_view,
        } => {
            let prepared = match prepare_native_snapshot(
                state,
                &path,
                &project_root,
                source_view,
                true,
                false,
            ) {
                Ok(prepared) => prepared,
                Err(response) => return (response, Next::Continue),
            };
            match prepared {
                PreparedNative::Graph(graph) => run_without_engine_lock(|| {
                    native_build_shared_graph(&graph, path, project_root, output, profile)
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_build_shared_program(program, path, project_root, output, profile)
                }),
            }
        }
        Request::NativeRun {
            path,
            project_root,
            profile,
            node,
            source_view,
        } => {
            let prepared = match prepare_native_snapshot(
                state,
                &path,
                &project_root,
                source_view,
                false,
                node,
            ) {
                Ok(prepared) => prepared,
                Err(response) => return (response, Next::Continue),
            };
            match prepared {
                PreparedNative::Graph(graph) => run_without_engine_lock(|| {
                    native_run_graph(&graph, path, project_root, profile)
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_run_program(program, path, project_root, profile)
                }),
            }
        }
        Request::Stats => {
            let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            (Response::Stats(daemon_stats(&guard)), Next::Continue)
        }
        Request::ResetStats => {
            let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            guard.engine.reset_stats();
            guard.graph_cache.reset_stats();
            (Response::Ok, Next::Continue)
        }
        // The artifact cache is content addressed and independent of the source set, so a sweep
        // needs neither the engine nor its lock - and a sweep of a large cache is a directory
        // walk that an unrelated `check` must not wait behind.
        Request::CacheGc {
            project_root,
            dry_run,
        } => run_without_engine_lock(|| crate::dispatch::cache_gc(&project_root, dry_run)),
        request => handle_engine_request(state, request),
    }
}

fn handle_run<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    path: String,
    project_root: String,
    node: bool,
    source_view: SourceView,
) -> (Response, Next) {
    let prepared = {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.source_view != source_view {
            return (
                Response::StaleSourceView {
                    current: guard.source_view,
                },
                Next::Continue,
            );
        }
        let revision = guard.engine.revision().0;
        let resolutions = guard.resolutions.clone();
        match panic::catch_unwind(AssertUnwindSafe(|| {
            let DaemonState {
                engine,
                graph_cache,
                ..
            } = &mut *guard;
            prepare_run_with_cache(
                engine,
                Some(graph_cache),
                path,
                project_root,
                resolutions.as_ref(),
                node,
            )
        })) {
            Ok(Ok(_prepared)) if guard.engine.revision().0 != revision => {
                match next_source_view(guard.source_view) {
                    Ok(view) => {
                        guard.source_view = view;
                        return (Response::StaleSourceView { current: view }, Next::Continue);
                    }
                    Err(message) => return (Response::Error { message }, Next::Continue),
                }
            }
            Ok(Ok(prepared)) => prepared,
            Ok(Err(message)) => return (Response::Error { message }, Next::Continue),
            Err(payload) => {
                if guard.engine.revision().0 != revision {
                    guard.graph_cache.invalidate();
                    match next_source_view(guard.source_view) {
                        Ok(view) => guard.source_view = view,
                        Err(message) => return (Response::Error { message }, Next::Continue),
                    }
                }
                return (internal_error(&*payload), Next::Continue);
            }
        }
    };
    run_without_engine_lock(|| execute_prepared_run(prepared, node))
}

/// Prepare a native build against one exact engine snapshot, compile without the lock, and
/// publish atomically only if no source edit occurred while the toolchain was running.
fn handle_native_build_at_revision<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    request: Request,
) -> (Response, Next) {
    let Request::NativeBuildAtRevision {
        path,
        project_root,
        node,
        output,
        profile,
        source_view,
        expected_revision,
    } = request
    else {
        unreachable!("native revision build handler received another request")
    };
    let (revision, prepared) = {
        let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.source_view != source_view {
            return (
                Response::StaleSourceView {
                    current: guard.source_view,
                },
                Next::Continue,
            );
        }
        let current_revision = guard.engine.revision().0;
        if current_revision != expected_revision {
            return (Response::Stale { current_revision }, Next::Continue);
        }
        let resolutions = guard.resolutions.clone();
        let prepared = match panic::catch_unwind(AssertUnwindSafe(|| {
            let DaemonState {
                engine,
                graph_cache,
                ..
            } = &mut *guard;
            let graph = native_graph_with_cache(
                engine,
                Some(graph_cache),
                resolutions.as_ref(),
                &path,
                &project_root,
                node,
            )?;
            match graph {
                Some(graph) => Ok(PreparedNative::Graph(graph)),
                None => native_program(&mut guard.engine, &path).map(PreparedNative::Program),
            }
        })) {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(message)) => return (Response::Error { message }, Next::Continue),
            Err(payload) => {
                if guard.engine.revision().0 != current_revision {
                    guard.graph_cache.invalidate();
                    match next_source_view(guard.source_view) {
                        Ok(view) => guard.source_view = view,
                        Err(message) => return (Response::Error { message }, Next::Continue),
                    }
                }
                return (internal_error(&*payload), Next::Continue);
            }
        };
        if guard.engine.revision().0 != current_revision {
            match next_source_view(guard.source_view) {
                Ok(view) => guard.source_view = view,
                Err(message) => return (Response::Error { message }, Next::Continue),
            }
            return (
                Response::StaleSourceView {
                    current: guard.source_view,
                },
                Next::Continue,
            );
        }
        // Canonicalizing a legacy relative overlay is itself an input reconciliation and may
        // advance the revision while the lock is held. Pin publication to the reconciled
        // snapshot, while the check above still rejects a genuinely stale caller revision.
        (guard.engine.revision().0, prepared)
    };

    let final_output = match native_build_output(&project_root, &path, output) {
        Ok(output) => output,
        Err(message) => return (Response::Error { message }, Next::Continue),
    };
    let source = PathBuf::from(&path);
    let project_source = if source.is_absolute() {
        source.clone()
    } else {
        PathBuf::from(&project_root).join(&source)
    };
    let canonical_source = std::fs::canonicalize(&project_source).ok();
    if final_output == source
        || final_output == project_source
        || std::fs::canonicalize(&final_output)
            .ok()
            .zip(canonical_source.as_ref())
            .is_some_and(|(output, source)| output == *source)
    {
        return (
            Response::Error {
                message: format!("native output would overwrite source file {path}"),
            },
            Next::Continue,
        );
    }
    let temp_output = snapshot_temp_output(&final_output);
    let response = run_without_engine_lock(|| match prepared {
        PreparedNative::Graph(graph) => native_build_graph(
            &graph,
            path.clone(),
            project_root.clone(),
            Some(temp_output.to_string_lossy().into_owned()),
            profile,
        ),
        PreparedNative::Program(program) => native_build_program(
            program,
            path.clone(),
            project_root.clone(),
            Some(temp_output.to_string_lossy().into_owned()),
            profile,
        ),
    })
    .0;

    let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    let current_revision = guard.engine.revision().0;
    if current_revision != revision || guard.source_view != source_view {
        let _ = std::fs::remove_file(&temp_output);
        if guard.source_view != source_view {
            return (
                Response::StaleSourceView {
                    current: guard.source_view,
                },
                Next::Continue,
            );
        }
        return (Response::Stale { current_revision }, Next::Continue);
    }
    let Response::NativeBuilt(mut summary) = response else {
        let _ = std::fs::remove_file(&temp_output);
        return (response, Next::Continue);
    };
    if let Err(error) = std::fs::rename(&temp_output, &final_output) {
        let _ = std::fs::remove_file(&temp_output);
        return (
            Response::Error {
                message: format!("cannot publish native output: {error}"),
            },
            Next::Continue,
        );
    }
    summary.output_path = final_output.to_string_lossy().into_owned();
    (Response::NativeBuilt(summary), Next::Continue)
}

enum PreparedNative {
    Graph(Arc<ModuleGraph>),
    Program(kiln_ir::Program),
}

fn snapshot_temp_output(final_output: &Path) -> PathBuf {
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
    let name = final_output
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("native-output");
    final_output.with_file_name(format!(".kiln-{name}-{}-{id}.tmp", std::process::id()))
}

#[allow(clippy::result_large_err)]
fn prepare_native_snapshot<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    path: &str,
    project_root: &str,
    source_view: SourceView,
    prefer_program_for_small_graph: bool,
    node: bool,
) -> Result<PreparedNative, Response> {
    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    if guard.source_view != source_view {
        return Err(Response::StaleSourceView {
            current: guard.source_view,
        });
    }
    let revision = guard.engine.revision().0;
    let resolutions = guard.resolutions.clone();
    match panic::catch_unwind(AssertUnwindSafe(|| {
        let DaemonState {
            engine,
            graph_cache,
            ..
        } = &mut *guard;
        let graph = native_graph_with_cache(
            engine,
            Some(graph_cache),
            resolutions.as_ref(),
            path,
            project_root,
            node,
        )?;
        match graph {
            Some(graph) if !prefer_program_for_small_graph || graph.module_count() > 1 => {
                Ok(PreparedNative::Graph(graph))
            }
            Some(graph) => {
                // Graph construction canonicalized this entry already. Reuse that spelling for
                // the scalar fallback; reusing the request's relative alias would reactivate an
                // alias that ensure_canonical_source just removed and make every retry stale.
                let canonical_path = graph.entry().to_string_lossy().into_owned();
                native_program(&mut guard.engine, &canonical_path).map(PreparedNative::Program)
            }
            None => native_program(&mut guard.engine, path).map(PreparedNative::Program),
        }
    })) {
        Ok(Ok(_prepared)) if guard.engine.revision().0 != revision => {
            match next_source_view(guard.source_view) {
                Ok(view) => guard.source_view = view,
                Err(message) => return Err(Response::Error { message }),
            }
            Err(Response::StaleSourceView {
                current: guard.source_view,
            })
        }
        Ok(Ok(prepared)) => Ok(prepared),
        Ok(Err(message)) => Err(Response::Error { message }),
        Err(payload) => {
            if guard.engine.revision().0 != revision {
                guard.graph_cache.invalidate();
                match next_source_view(guard.source_view) {
                    Ok(view) => guard.source_view = view,
                    Err(message) => return Err(Response::Error { message }),
                }
            }
            Err(internal_error(&*payload))
        }
    }
}

fn run_without_engine_lock(operation: impl FnOnce() -> Response) -> (Response, Next) {
    match panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(response) => (response, Next::Continue),
        Err(payload) => (internal_error(&*payload), Next::Continue),
    }
}

fn handle_engine_request<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    request: Request,
) -> (Response, Next) {
    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    match panic::catch_unwind(AssertUnwindSafe(|| dispatch(&mut guard.engine, request))) {
        Ok(outcome) => outcome,
        Err(payload) => (internal_error(&*payload), Next::Continue),
    }
}

fn daemon_stats<E: QueryEngine>(state: &DaemonState<E>) -> Stats {
    let mut stats = crate::dispatch::stats(&state.engine);
    let cache = state.graph_cache.stats();
    stats.graph_cache_hits = cache.hits;
    stats.graph_cache_partial_hits = cache.partial_hits;
    stats.graph_cache_misses = cache.misses;
    stats.graph_cache_evictions = cache.evictions;
    stats.graph_cache_invalidations = cache.invalidations;
    stats.graph_cache_entries = state.graph_cache.entries();
    stats.graph_cache_modules = state.graph_cache.cached_modules();
    stats
}

fn internal_error(payload: &(dyn std::any::Any + Send)) -> Response {
    Response::Error {
        message: format!("internal error: {}", panic_message(payload)),
    }
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&'static str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panicked".to_string()
    }
}
