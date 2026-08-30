//! The daemon process: a unix socket, a thread per connection, one shared query engine.
//!
//! The whole reason this exists (design.md §1) is that Oxc, the built-in definitions and the
//! runtime metadata should be paid for once per session rather than once per command. That makes
//! *process lifetime* the resource being protected here: no single client may be able to wedge
//! the daemon, because doing so throws away everything warm.

use kiln_db::{MemoEngine, QueryEngine};
use kiln_module_graph::ModuleGraph;
use kiln_proto::{Request, Response};
use std::collections::HashMap;
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
    Next, dispatch, execute_prepared_run, native_build_graph, native_build_output,
    native_build_program, native_build_shared_graph, native_build_shared_program, native_graph,
    native_program, native_run_graph, native_run_program, prepare_run,
};
use crate::framing::{MAX_MESSAGE_BYTES, read_frame, write_frame};

#[derive(Debug)]
pub enum DaemonError {
    /// Another daemon is already listening on this socket. Not an error to paper over: two
    /// daemons on one project would each hold a divergent view of the same files.
    AlreadyRunning { socket: PathBuf },
    /// The socket could not be made ready to accept on - preparing its directory, clearing a
    /// stale path, or the `bind` itself.
    Listen { socket: PathBuf, source: io::Error },
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
    engine: Arc<Mutex<E>>,
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
            engine: Arc::new(Mutex::new(engine)),
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
        let engine = Arc::clone(&self.engine);
        let shared = Arc::clone(&self.shared);

        thread::spawn(move || {
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
                serve_connection(stream, &engine, &shared)
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
    engine: &Mutex<E>,
    shared: &Arc<Shared>,
) -> io::Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);

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
            Ok(request) => handle(engine, request),
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
fn handle<E: QueryEngine>(engine: &Mutex<E>, request: Request) -> (Response, Next) {
    match request {
        Request::Run { path, project_root } => handle_run(engine, path, project_root),
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
        Request::NativeBuildAtRevision {
            path,
            project_root,
            output,
            profile,
            expected_revision,
        } => handle_native_build_at_revision(
            engine,
            path,
            project_root,
            output,
            profile,
            expected_revision,
        ),
        Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
        } => {
            let graph = match prepare_native_graph(engine, &path, &project_root) {
                Ok(graph) => graph,
                Err(message) => return (Response::Error { message }, Next::Continue),
            };
            match graph {
                Some(graph) => run_without_engine_lock(|| {
                    native_build_graph(graph, path, project_root, output, profile)
                }),
                None => {
                    let program = match prepare_native(engine, &path) {
                        Ok(program) => program,
                        Err(message) => return (Response::Error { message }, Next::Continue),
                    };
                    run_without_engine_lock(|| {
                        native_build_program(program, path, project_root, output, profile)
                    })
                }
            }
        }
        Request::NativeSharedBuild {
            path,
            project_root,
            output,
            profile,
        } => {
            let graph = match prepare_native_graph(engine, &path, &project_root) {
                Ok(graph) => graph,
                Err(message) => return (Response::Error { message }, Next::Continue),
            };
            match graph {
                Some(graph) if graph.module_count() > 1 => run_without_engine_lock(|| {
                    native_build_shared_graph(graph, path, project_root, output, profile)
                }),
                Some(_) | None => {
                    let program = match prepare_native(engine, &path) {
                        Ok(program) => program,
                        Err(message) => return (Response::Error { message }, Next::Continue),
                    };
                    run_without_engine_lock(|| {
                        native_build_shared_program(program, path, project_root, output, profile)
                    })
                }
            }
        }
        Request::NativeRun {
            path,
            project_root,
            profile,
        } => {
            let graph = match prepare_native_graph(engine, &path, &project_root) {
                Ok(graph) => graph,
                Err(message) => return (Response::Error { message }, Next::Continue),
            };
            match graph {
                Some(graph) => {
                    run_without_engine_lock(|| native_run_graph(graph, path, project_root, profile))
                }
                None => {
                    let program = match prepare_native(engine, &path) {
                        Ok(program) => program,
                        Err(message) => return (Response::Error { message }, Next::Continue),
                    };
                    run_without_engine_lock(|| {
                        native_run_program(program, path, project_root, profile)
                    })
                }
            }
        }
        request => handle_engine_request(engine, request),
    }
}

fn handle_run<E: QueryEngine>(
    engine: &Mutex<E>,
    path: String,
    project_root: String,
) -> (Response, Next) {
    let prepared = {
        let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
        match panic::catch_unwind(AssertUnwindSafe(|| {
            prepare_run(&mut *guard, path, project_root)
        })) {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(message)) => return (Response::Error { message }, Next::Continue),
            Err(payload) => return (internal_error(&*payload), Next::Continue),
        }
    };
    run_without_engine_lock(|| execute_prepared_run(prepared))
}

/// Prepare a native build against one exact engine snapshot, compile without the lock, and
/// publish atomically only if no source edit occurred while the toolchain was running.
fn handle_native_build_at_revision<E: QueryEngine>(
    engine: &Mutex<E>,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: kiln_proto::NativeProfile,
    expected_revision: u64,
) -> (Response, Next) {
    let (revision, prepared) = {
        let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
        let revision = guard.revision().0;
        if revision != expected_revision {
            return (
                Response::Stale {
                    current_revision: revision,
                },
                Next::Continue,
            );
        }
        let prepared = match panic::catch_unwind(AssertUnwindSafe(|| {
            let graph = native_graph(&mut *guard, &path, &project_root)?;
            match graph {
                Some(graph) => Ok(PreparedNative::Graph(graph)),
                None => native_program(&mut *guard, &path).map(PreparedNative::Program),
            }
        })) {
            Ok(Ok(prepared)) => prepared,
            Ok(Err(message)) => return (Response::Error { message }, Next::Continue),
            Err(payload) => {
                return (internal_error(&*payload), Next::Continue);
            }
        };
        (revision, prepared)
    };

    let final_output = native_build_output(&project_root, &path, output);
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
            graph,
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

    let guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
    let current_revision = guard.revision().0;
    if current_revision != revision {
        let _ = std::fs::remove_file(&temp_output);
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
    Graph(ModuleGraph),
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

fn prepare_native_graph<E: QueryEngine>(
    engine: &Mutex<E>,
    path: &str,
    project_root: &str,
) -> Result<Option<ModuleGraph>, String> {
    let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
    match panic::catch_unwind(AssertUnwindSafe(|| {
        native_graph(&mut *guard, path, project_root)
    })) {
        Ok(result) => result,
        Err(payload) => Err(format!("internal error: {}", panic_message(&*payload))),
    }
}

fn prepare_native<E: QueryEngine>(
    engine: &Mutex<E>,
    path: &str,
) -> Result<kiln_ir::Program, String> {
    let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
    match panic::catch_unwind(AssertUnwindSafe(|| native_program(&mut *guard, path))) {
        Ok(Ok(program)) => Ok(program),
        Ok(Err(message)) => Err(message),
        Err(payload) => Err(format!("internal error: {}", panic_message(&*payload))),
    }
}

fn run_without_engine_lock(operation: impl FnOnce() -> Response) -> (Response, Next) {
    match panic::catch_unwind(AssertUnwindSafe(operation)) {
        Ok(response) => (response, Next::Continue),
        Err(payload) => (internal_error(&*payload), Next::Continue),
    }
}

fn handle_engine_request<E: QueryEngine>(engine: &Mutex<E>, request: Request) -> (Response, Next) {
    let mut guard = engine.lock().unwrap_or_else(PoisonError::into_inner);
    match panic::catch_unwind(AssertUnwindSafe(|| dispatch(&mut *guard, request))) {
        Ok(outcome) => outcome,
        Err(payload) => (internal_error(&*payload), Next::Continue),
    }
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
