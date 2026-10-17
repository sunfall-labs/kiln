//! The daemon process: a unix socket, a thread per connection, one shared query engine.
//!
//! The whole reason this exists (design.md §1) is that Oxc, the built-in definitions and the
//! runtime metadata should be paid for once per session rather than once per command. That makes
//! *process lifetime* the resource being protected here: no single client may be able to wedge
//! the daemon, because doing so throws away everything warm.

use kiln_db::{FileId, MemoEngine, QueryEngine, Revision};
use kiln_module_graph::ModuleGraph;
use kiln_proto::{
    NativeSemantics, Request, ResolutionMap, Response, SourceDigest, SourceView, Stats, StepBudget,
    SyncId,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::io::{self, BufReader, ErrorKind};
use std::net::Shutdown as SocketShutdown;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::dispatch::{
    GraphRunPlan, Next, PreparedRun, build_graph_from_snapshot, dispatch, dispatch_source,
    execute_prepared_run, graph_run_plan, graph_snapshot_is_current, graph_sources_from_snapshot,
    native_build_graph, native_build_output, native_build_program, native_build_shared_graph,
    native_build_shared_program, native_explain_parallel_graph, native_explain_parallel_program,
    native_graph_with_cache, native_program, native_run_graph, native_run_program,
    patch_graph_from_snapshot, prepare_run_with_cache,
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

/// The authoritative source set and its coherence identity move together. Requests validate the
/// view and copy source bytes while holding this mutex, then validate again before publishing any
/// graph prepared from that owned snapshot.
pub(crate) struct DaemonState<E> {
    pub(crate) engine: E,
    pub(crate) source_view: SourceView,
    pub(crate) resolutions: Option<ResolutionMap>,
    pub(crate) graph_cache: GraphCache,
    /// Each held source's digest at the revision it was computed for, so a sync compares an
    /// unchanged project without hashing every source again.
    pub(crate) source_digests: HashMap<FileId, (Revision, SourceDigest)>,
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
    activity: Activity,
    tests: crate::testing::ConformanceCoordinator,
}

/// What the idle watcher needs to know: how many client connections are open, and since when
/// there have been none. A connection is open from accept until its thread finishes, so a request
/// in flight (a long build, a streaming run) and a `watch` session parked between saves both
/// count; only a daemon with no clients at all can go idle.
struct Activity {
    state: Mutex<ActivityState>,
    changed: std::sync::Condvar,
}

struct ActivityState {
    open: usize,
    /// When `open` last became zero (or the daemon started).
    idle_since: Instant,
    /// The watcher has decided to exit: nothing may be admitted any more.
    retired: bool,
}

impl Activity {
    fn new() -> Self {
        Self {
            state: Mutex::new(ActivityState {
                open: 0,
                idle_since: Instant::now(),
                retired: false,
            }),
            changed: std::sync::Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ActivityState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Admit a connection, unless the daemon has already decided to exit. Deciding and admitting
    /// happen under one lock, so a client is either served or refused, never half-accepted.
    fn admit(&self) -> bool {
        let mut state = self.lock();
        if state.retired {
            return false;
        }
        state.open += 1;
        true
    }

    fn release(&self) {
        let mut state = self.lock();
        state.open = state.open.saturating_sub(1);
        if state.open == 0 {
            state.idle_since = Instant::now();
        }
        self.changed.notify_all();
    }

    fn stop_watching(&self) {
        // Under the lock, so the wake-up cannot land between the watcher's check of `stop` and
        // its wait.
        let _state = self.lock();
        self.changed.notify_all();
    }
}

/// Decrements the open-connection count when a connection thread ends, however it ends.
struct OpenConnection(Arc<Shared>);

impl Drop for OpenConnection {
    fn drop(&mut self) {
        self.0.activity.release();
    }
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
    idle_timeout: Option<Duration>,
    _socket: SocketGuard,
}

impl Daemon<MemoEngine> {
    /// Bind `socket`, serving a fresh [`MemoEngine`] backed by the Oxc frontend.
    pub fn bind(socket: &Path) -> Result<Self, DaemonError> {
        Self::bind_with(socket, production_engine())
    }
}

fn production_engine() -> MemoEngine {
    let mut engine =
        MemoEngine::with_summary_query(parse_source, kiln_backend_c::summarize_program)
            .with_source_check_query(check_source_types);
    if TYPE_FACTS_UNUSED.load(std::sync::atomic::Ordering::Relaxed) {
        kiln_db::QueryEngine::set_compile_facts(&mut engine, false);
    }
    engine
}

/// Whether this process serves requests one after another, so that a result kept from one is
/// worth what keeping it costs. A one-shot command answers once and keeps nothing.
static SERVING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn serving() -> bool {
    SERVING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Set by [`without_type_facts`]: nothing this process compiles outlives it.
static TYPE_FACTS_UNUSED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// For a process whose compiled programs are discarded, as a one-shot `kiln check`'s are: source
/// checks report diagnostics without type facts, and compiling does not wait for them, so the
/// check and the module graph run at once. A daemon keeps both, since later builds read them.
pub fn without_type_facts() {
    TYPE_FACTS_UNUSED.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The production frontend. A declaration file has no executable code, so it is parsed as the
/// source checker reads it, without the transform, code generation and second parse that
/// [`kiln_frontend_oxc::parse`] spends on executable TypeScript; on a project that synchronizes
/// its packages' declarations that was most of the parse time of a check.
fn parse_source(path: &str, source: &str) -> kiln_hir::Module {
    if [".d.ts", ".d.mts", ".d.cts"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
    {
        kiln_frontend_oxc::parse_authored(path, source)
    } else {
        kiln_frontend_oxc::parse(path, source)
    }
}

/// The declaration libraries of every source check in this process. A check parses only the
/// libraries its configuration loads, each once per process, and reads a library's authored
/// module alone, never its transformed executable form.
fn source_check_libraries() -> &'static std::sync::Arc<kiln_type_checker::Libraries> {
    static LIBRARIES: std::sync::OnceLock<std::sync::Arc<kiln_type_checker::Libraries>> =
        std::sync::OnceLock::new();
    LIBRARIES.get_or_init(|| {
        std::sync::Arc::new(kiln_type_checker::Libraries::standard(
            kiln_frontend_oxc::parse_authored,
        ))
    })
}

/// Start parsing the `esnext` declaration libraries in the background, so a TypeScript check
/// that follows finds them parsed: the CLI spends its first milliseconds reading and resolving
/// the project on one thread. A library the check needs before its parse finishes is waited for,
/// not parsed twice; one outside `esnext` is parsed when the check loads it.
pub fn prewarm_source_checker() {
    let _ = std::thread::Builder::new()
        .name("kiln-library-prewarm".into())
        .stack_size(crate::SERVE_STACK_BYTES)
        .spawn(|| source_check_libraries().prepare(&["esnext"], 4));
}

/// The composition root supplies immutable parsed library inputs; neither database nor checker reads disk.
pub(crate) fn check_source_types(
    entry: &str,
    modules: &[(String, std::sync::Arc<kiln_hir::Module>)],
    auxiliary: &[(String, std::sync::Arc<[u8]>)],
) -> kiln_hir::source_types::CheckResult {
    use std::sync::Arc;
    let libraries = source_check_libraries();
    let mut snapshot = kiln_type_checker::SourceSnapshot {
        libraries: Some(libraries.clone()),
        ..Default::default()
    };
    snapshot
        .modules
        .extend(modules.iter().map(|(p, m)| (p.clone(), Arc::clone(m))));
    snapshot.auxiliary_files.extend(auxiliary.iter().cloned());
    for (path, bytes) in auxiliary {
        if path.ends_with(kiln_hir::source_types::CHECKER_LINKS_FILE)
            && let Ok(links) =
                serde_json::from_slice::<std::collections::BTreeMap<String, String>>(bytes)
        {
            snapshot.links.extend(links);
        }
    }
    // Ambient declarations belong to the entry package snapshot even when no runtime import
    // reaches them. A workspace snapshot can contain sibling packages; their declarations are
    // not ambient in this package and should enter only through normal module resolution.
    let mut roots = vec![entry.to_owned()];
    let package_manifests = auxiliary
        .iter()
        .filter_map(|(path, _)| {
            std::path::Path::new(path)
                .file_name()
                .is_some_and(|name| name == "package.json")
                .then_some(path.as_str())
        })
        .collect::<std::collections::HashSet<_>>();
    let entry_package = uploaded_package_root(entry, &package_manifests);
    roots.extend(modules.iter().filter_map(|(path, _)| {
        let file = std::path::Path::new(path);
        let declaration =
            path.ends_with(".d.ts") || path.ends_with(".d.mts") || path.ends_with(".d.cts");
        if path == entry
            || !declaration
            || file
                .components()
                .any(|part| part.as_os_str() == "node_modules")
        {
            return None;
        }
        let package = uploaded_package_root(path, &package_manifests);
        match entry_package.as_deref() {
            Some(entry_package) if package.as_deref() == Some(entry_package) => Some(path.clone()),
            None if package.is_none() => Some(path.clone()),
            _ => None,
        }
    }));
    // The closest uploaded project configuration owns checker options. Reading only
    // the immutable input set keeps editor revisions and compiler facts coherent.
    let config_path = std::path::Path::new(entry)
        .parent()
        .into_iter()
        .flat_map(|parent| parent.ancestors())
        .find_map(|parent| {
            ["tsconfig.json", "tsconfig.jsonc"]
                .into_iter()
                .map(|name| parent.join(name).to_string_lossy().into_owned())
                .find(|path| snapshot.auxiliary_files.contains_key(path))
        });
    roots.extend(builtin_declaration_roots(
        modules,
        &snapshot,
        config_path.is_none(),
        entry,
    ));
    let current_directory = config_path
        .as_deref()
        .and_then(|path| std::path::Path::new(path).parent())
        .filter(|parent| parent.is_absolute())
        .map(|parent| parent.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    let options = kiln_type_checker::CheckOptions {
        config_path,
        current_directory,
        libraries: vec!["esnext".into()],
        facts: if TYPE_FACTS_UNUSED.load(std::sync::atomic::Ordering::Relaxed) {
            kiln_type_checker::FactSelection::None
        } else {
            kiln_type_checker::FactSelection::FunctionContracts
        },
        ..Default::default()
    };
    kiln_type_checker::check(&snapshot, &roots, &options)
}

/// kiln's built-in declarations, as extra roots of the check: `@kiln/types` (the `kiln:` modules,
/// `Kiln`/`Bun`, the parallel methods) and `@types/node`. A kiln program has `process`, `Buffer`,
/// `console`, timers and `fetch` as globals whether or not the Node adapter is on (only `require`
/// and the `node:` modules need it), and `@types/node` declares exactly that surface, so it is
/// always present; the browser's `dom` library, which declares `document`, would be wrong.
/// Each is left out when the project brings its own copy under `node_modules`, which would
/// otherwise declare everything twice. A project's tsconfig is still respected: these add declarations and change
/// no option.
fn builtin_declaration_roots(
    modules: &[(String, std::sync::Arc<kiln_hir::Module>)],
    snapshot: &kiln_type_checker::SourceSnapshot,
    automatic_types: bool,
    entry: &str,
) -> Vec<String> {
    let provided = |package: &str| {
        let marker = format!("/node_modules/{package}/");
        modules.iter().any(|(path, _)| path.contains(&marker))
            || snapshot
                .auxiliary_files
                .keys()
                .any(|path| path.contains(&marker))
    };
    let mut roots = Vec::new();
    // `tsc` loads every `node_modules/@types/*` visible from the project when no configuration
    // narrows `types`; without a configuration there is nothing to narrow it, so do the same.
    if automatic_types {
        const MARKER: &str = "/node_modules/@types/";
        roots.extend(modules.iter().filter_map(|(path, _)| {
            let at = path.find(MARKER)?;
            let (project, package) = (&path[..at], &path[at + MARKER.len()..]);
            let (name, file) = package.split_once('/')?;
            (file == "index.d.ts"
                && !name.is_empty()
                && entry.starts_with(project)
                && entry[project.len()..].starts_with('/'))
            .then(|| path.clone())
        }));
    }
    if !provided("@kiln/types") {
        roots.push(kiln_type_checker::Libraries::path(
            kiln_type_checker::KILN_DECLARATIONS_ROOT,
        ));
    }
    if !provided("@types/node") {
        roots.push(kiln_type_checker::Libraries::path(
            kiln_type_checker::NODE_DECLARATIONS_ROOT,
        ));
    }
    roots
}

fn uploaded_package_root(
    source: &str,
    manifests: &std::collections::HashSet<&str>,
) -> Option<String> {
    std::path::Path::new(source)
        .parent()?
        .ancestors()
        .find_map(|directory| {
            if directory
                .components()
                .any(|part| part.as_os_str() == "node_modules")
            {
                return None;
            }
            let manifest = directory.join("package.json");
            manifests
                .contains(manifest.to_str()?)
                .then(|| directory.to_string_lossy().into_owned())
        })
}

/// A private request channel to the same handler used by socket clients. A one-shot CLI owns
/// one of these instead of binding a Unix socket: it needs no cross-process address, and peers
/// still share the source view, graph cache and conformance catalog. Each channel has its own
/// staging transaction and a full-size request stack, just like a daemon connection.
pub struct LocalClient {
    sender: Option<mpsc::Sender<(Request, mpsc::SyncSender<Response>, bool)>>,
    serving: Option<JoinHandle<()>>,
    state: Arc<Mutex<DaemonState<MemoEngine>>>,
    tests: Arc<crate::testing::ConformanceCoordinator>,
}

impl LocalClient {
    pub fn new() -> io::Result<Self> {
        Self::spawn(
            Arc::new(Mutex::new(DaemonState {
                engine: production_engine(),
                source_view: SourceView(0),
                resolutions: None,
                graph_cache: GraphCache::default(),
                source_digests: HashMap::new(),
            })),
            Arc::new(crate::testing::ConformanceCoordinator::default()),
        )
    }

    pub fn peer(&self) -> io::Result<Self> {
        Self::spawn(Arc::clone(&self.state), Arc::clone(&self.tests))
    }

    fn spawn(
        state: Arc<Mutex<DaemonState<MemoEngine>>>,
        tests: Arc<crate::testing::ConformanceCoordinator>,
    ) -> io::Result<Self> {
        let (sender, receiver) = mpsc::channel::<(Request, mpsc::SyncSender<Response>, bool)>();
        let lane_state = Arc::clone(&state);
        let lane_tests = Arc::clone(&tests);
        let serving = thread::Builder::new()
            .stack_size(crate::SERVE_STACK_BYTES)
            .spawn(move || {
                let mut pending = None;
                let mut next_sync_id = 0;
                while let Ok((request, reply, inherit_native)) = receiver.recv() {
                    let _inherit_native =
                        inherit_native.then(crate::stream::install_inherit_native_output);
                    let streaming = crate::stream::wants_stream(&request).then(|| {
                        let frames = reply.clone();
                        crate::stream::install(crate::stream::OutputSink::new(Box::new(
                            move |stream, text| {
                                let _ = frames.send(Response::Output { stream, text });
                            },
                        )))
                    });
                    let (response, next) = handle(
                        &lane_state,
                        &lane_tests,
                        &mut pending,
                        &mut next_sync_id,
                        request,
                    );
                    drop(streaming);
                    let _ = reply.send(response);
                    if next != Next::Continue {
                        break;
                    }
                }
            })?;
        Ok(Self {
            sender: Some(sender),
            serving: Some(serving),
            state,
            tests,
        })
    }

    pub fn request(&mut self, request: Request) -> io::Result<Response> {
        self.request_streaming(request, &mut |_, _| {})
    }

    /// As [`Self::request`], passing the program's output to `on_output` as it arrives when the
    /// request sets `stream_output`; the answer then carries empty `stdout`/`stderr`.
    pub fn request_streaming(
        &mut self,
        request: Request,
        on_output: &mut dyn FnMut(kiln_proto::OutputStream, &str),
    ) -> io::Result<Response> {
        self.request_with_output_mode(request, on_output, false)
    }

    /// As [`Self::request_streaming`], but asks native execution to inherit this process's
    /// stdout/stderr descriptors instead of transporting chunks. Used only by `--one-shot`.
    pub fn request_native_inheriting(&mut self, request: Request) -> io::Result<Response> {
        self.request_with_output_mode(request, &mut |_, _| {}, true)
    }

    fn request_with_output_mode(
        &mut self,
        request: Request,
        on_output: &mut dyn FnMut(kiln_proto::OutputStream, &str),
        inherit_native: bool,
    ) -> io::Result<Response> {
        let (reply, answer) = mpsc::sync_channel(1);
        self.sender
            .as_ref()
            .ok_or_else(|| io::Error::new(ErrorKind::BrokenPipe, "local engine is closed"))?
            .send((request, reply, inherit_native))
            .map_err(|_| io::Error::new(ErrorKind::BrokenPipe, "local engine stopped"))?;
        loop {
            match answer.recv() {
                Ok(Response::Output { stream, text }) => on_output(stream, &text),
                Ok(response) => return Ok(response),
                Err(_) => {
                    return Err(io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "local engine dropped its answer",
                    ));
                }
            }
        }
    }
}

impl Drop for LocalClient {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
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
                activity: Activity::new(),
                tests: crate::testing::ConformanceCoordinator::default(),
            }),
            state: Arc::new(Mutex::new(DaemonState {
                engine,
                source_view: SourceView(0),
                resolutions: None,
                graph_cache: GraphCache::default(),
                source_digests: HashMap::new(),
            })),
            idle_timeout: None,
            _socket: SocketGuard(socket.to_path_buf()),
        })
    }

    /// Exit once no client has been connected for `idle`. `None` (the default, and what
    /// in-process users get) serves until told to stop.
    ///
    /// "Connected" is the whole definition of activity: a connection that is open keeps the daemon
    /// alive, whether it is mid-request or parked between requests like a `watch` session's, so
    /// the daemon never exits under a client. The exit is the same one `Request::Shutdown` takes,
    /// so the socket is unlinked on the way out.
    pub fn with_idle_timeout(mut self, idle: Option<Duration>) -> Self {
        self.idle_timeout = idle.filter(|idle| !idle.is_zero());
        self
    }

    /// Run every program this daemon hosts as a process of its own: a virtual `process.pid`,
    /// and the signals it sends itself delivered in its realm, never to the daemon (realms
    /// issue 26; `kiln_runtime::signals`). The resident daemon calls this; a `--one-shot`
    /// engine does not, because its program is the process.
    pub fn isolate_hosted_programs(self) -> Self {
        kiln_runtime::signals::isolate_hosted_programs();
        self
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
        // Before the first connection, and before any rebuild can replace the file underneath us:
        // this daemon's answers belong to the build it started from, whatever is at that path now.
        let _ = crate::build_identity();
        SERVING.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut workers: Vec<JoinHandle<()>> = Vec::new();
        let watcher = self.idle_timeout.map(|idle| {
            let shared = Arc::clone(&self.shared);
            thread::spawn(move || watch_idle(&shared, idle))
        });

        for incoming in self.listener.incoming() {
            if self.shared.stop.load(Ordering::SeqCst) {
                break;
            }
            match incoming {
                Ok(stream) => {
                    if !self.shared.activity.admit() {
                        // The idle watcher retired the daemon between accept and here.
                        break;
                    }
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
        if let Some(watcher) = watcher {
            self.shared.activity.stop_watching();
            let _ = watcher.join();
        }
    }

    fn spawn_connection(&self, stream: UnixStream) -> JoinHandle<()> {
        let state = Arc::clone(&self.state);
        let shared = Arc::clone(&self.shared);
        // Admitted by the accept loop; released when this thread ends, wherever it ends.
        let open = OpenConnection(Arc::clone(&shared));

        // Built rather than `spawn`ed, for the stack: see `crate::SERVE_STACK_BYTES`. A default
        // 2 MiB thread overflows on an ordinary project, and a stack overflow here aborts the
        // whole daemon rather than failing one connection.
        thread::Builder::new()
            .stack_size(crate::SERVE_STACK_BYTES)
            .spawn(move || {
                let _open = open;
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

/// Shut the daemon down once it has had no open connection for `idle`.
fn watch_idle(shared: &Arc<Shared>, idle: Duration) {
    let activity = &shared.activity;
    let mut state = activity.lock();
    loop {
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        let wait = if state.open > 0 {
            // Woken by `release`; the timeout is only a backstop.
            idle
        } else {
            let elapsed = state.idle_since.elapsed();
            if elapsed >= idle {
                state.retired = true;
                drop(state);
                eprintln!(
                    "kiln-daemon: no client for {}s; exiting",
                    idle.as_secs_f64()
                );
                ShutdownHandle {
                    shared: Arc::clone(shared),
                }
                .shutdown();
                return;
            }
            idle - elapsed
        };
        state = activity
            .changed
            .wait_timeout(state, wait)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
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
            Ok(request) => {
                // Held across the request: output frames go out on this socket while the
                // program runs, ahead of the one answer written below.
                let _streaming = crate::stream::wants_stream(&request)
                    .then(|| writer.try_clone().ok())
                    .flatten()
                    .map(|mut frames| {
                        crate::stream::install(crate::stream::OutputSink::new(Box::new(
                            move |stream, text| {
                                let _ = respond(&mut frames, &Response::Output { stream, text });
                            },
                        )))
                    });
                handle(
                    state,
                    &shared.tests,
                    &mut pending,
                    &mut next_sync_id,
                    request,
                )
            }
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

    let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
    let DaemonState {
        engine,
        source_digests,
        ..
    } = &mut *guard;
    let held = engine
        .source_files()
        .into_iter()
        .filter_map(|(file, path)| {
            let text = engine.source(file)?;
            let revision = engine.source_revision(file)?;
            let digest = match source_digests.get(&file) {
                Some((at, digest)) if *at == revision => *digest,
                _ => {
                    let digest = kiln_proto::source_digest(text);
                    source_digests.insert(file, (revision, digest));
                    digest
                }
            };
            Some((path, digest))
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
    tests: &crate::testing::ConformanceCoordinator,
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
            args,
            node,
            wall_clock,
            steps,
            source_view,
            stream_output: _,
        } => handle_run(
            state,
            path,
            project_root,
            args,
            node,
            wall_clock,
            steps,
            source_view,
        ),
        Request::NodeRun {
            path,
            project_root,
            args,
            preloads,
            exec_argv,
            disallow_string_code_generation,
            wall_clock,
            steps,
            source_view,
            stream_output: _,
        } => handle_node_run(
            state,
            path,
            project_root,
            args,
            preloads,
            exec_argv,
            disallow_string_code_generation,
            wall_clock,
            steps,
            source_view,
        ),
        Request::TestInventory { root } => {
            run_without_engine_lock(|| match tests.inventory(&root) {
                Ok(tests) => Response::TestInventory(tests),
                Err(message) => Response::Error { message },
            })
        }
        Request::TestBatch { root, mode, ids } => {
            run_without_engine_lock(|| match tests.batch(&root, mode, ids) {
                Ok(summary) => Response::Tested(summary),
                Err(message) => Response::Error { message },
            })
        }
        Request::Test {
            root,
            mode,
            changed_root,
            changed_paths,
            previously_failed,
        } => run_without_engine_lock(|| {
            #[cfg(test)]
            crate::dispatch::wait_for_test_gate(&root);
            match tests.run(&root, mode, changed_root, changed_paths, previously_failed) {
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
            output,
            profile,
            node,
            explain,
            no_kiln_extensions,
            workers,
            source_view,
            static_link,
            pgo_generate,
            pgo_use,
            semantics,
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
            let pgo_gen = pgo_generate.map(PathBuf::from);
            let pgo_u = pgo_use.map(PathBuf::from);
            match prepared {
                PreparedNative::Graph(graph) => run_without_engine_lock(|| {
                    native_build_graph(
                        &graph,
                        path,
                        project_root,
                        output,
                        profile,
                        node,
                        explain,
                        no_kiln_extensions,
                        workers,
                        static_link,
                        pgo_gen,
                        pgo_u,
                        semantics,
                    )
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_build_program(
                        program,
                        path,
                        project_root,
                        output,
                        profile,
                        explain,
                        no_kiln_extensions,
                        workers,
                        static_link,
                        pgo_gen,
                        pgo_u,
                        semantics,
                    )
                }),
            }
        }
        Request::ExplainParallel {
            path,
            project_root,
            node,
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
                    native_explain_parallel_graph(&graph, &path, &project_root, profile, node)
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_explain_parallel_program(&program, &path, &project_root, profile)
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
            args: _,
            profile,
            node,
            wall_clock,
            source_view,
            static_link,
            pgo_generate,
            pgo_use,
            stream_output: _,
            semantics,
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
            let pgo_gen = pgo_generate.map(PathBuf::from);
            let pgo_u = pgo_use.map(PathBuf::from);
            match prepared {
                PreparedNative::Graph(graph) => run_without_engine_lock(|| {
                    native_run_graph(
                        &graph,
                        path,
                        project_root,
                        profile,
                        node,
                        wall_clock,
                        static_link,
                        pgo_gen,
                        pgo_u,
                        semantics,
                    )
                }),
                PreparedNative::Program(program) => run_without_engine_lock(|| {
                    native_run_program(
                        program,
                        path,
                        project_root,
                        profile,
                        wall_clock,
                        static_link,
                        pgo_gen,
                        pgo_u,
                        semantics,
                    )
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
    args: Vec<String>,
    node: bool,
    wall_clock: bool,
    steps: StepBudget,
    source_view: SourceView,
) -> (Response, Next) {
    enum RunPlan {
        Prepared(crate::dispatch::PreparedRun),
        Graph(GraphRunPlan),
    }

    let plan = {
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
            let entry = crate::dispatch::resolve_source_path(&path, &project_root);
            match entry {
                Some(entry) => {
                    let DaemonState {
                        engine,
                        graph_cache,
                        ..
                    } = &mut *guard;
                    let file = crate::dispatch::ensure_canonical_source(
                        engine,
                        &path,
                        &entry,
                        &project_root,
                    )?;
                    graph_run_plan(
                        &*engine,
                        graph_cache,
                        source_view,
                        path,
                        args,
                        file,
                        project_root,
                        entry,
                        resolutions.as_ref(),
                        node,
                    )
                    .map(RunPlan::Graph)
                }
                None => prepare_run_with_cache(
                    &mut guard.engine,
                    None,
                    path,
                    args,
                    project_root,
                    resolutions.as_ref(),
                    node,
                )
                .map(RunPlan::Prepared),
            }
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
            Ok(Ok(plan)) => plan,
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
    let prepared = match plan {
        RunPlan::Prepared(prepared) => prepared,
        RunPlan::Graph(GraphRunPlan::Cached {
            graph,
            path,
            args,
            file,
            revision,
        }) => {
            let overlay = {
                let guard = state.lock().unwrap_or_else(PoisonError::into_inner);
                crate::dispatch::build_vfs_overlay(&guard.engine)
            };
            PreparedRun::Graph {
                graph: Ok(graph),
                path,
                args,
                exec_argv: Vec::new(),
                disallow_string_code_generation: false,
                has_preloads: false,
                file,
                revision,
                overlay,
            }
        }
        RunPlan::Graph(GraphRunPlan::Build {
            snapshot,
            key,
            stale,
            path,
            args,
            file,
            revision,
        }) => {
            let graph = match panic::catch_unwind(AssertUnwindSafe(|| {
                if let Some((old, changed)) = stale.as_ref()
                    && let Some(graph) = patch_graph_from_snapshot(&snapshot, old, *changed)?
                {
                    return Ok((graph, true));
                }
                build_graph_from_snapshot(&snapshot).map(|graph| (graph, false))
            })) {
                Ok(graph) => graph,
                Err(payload) => return (internal_error(&*payload), Next::Continue),
            };
            let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            if !graph_snapshot_is_current(&snapshot, &guard.engine, guard.source_view) {
                return (
                    Response::StaleSourceView {
                        current: guard.source_view,
                    },
                    Next::Continue,
                );
            }
            match graph {
                Ok((graph, partial)) => {
                    let graph = match graph_sources_from_snapshot(&snapshot, &graph) {
                        Some(sources) => {
                            if stale.is_some() && partial {
                                guard.graph_cache.record_partial_hit();
                            } else if stale.is_some() {
                                guard.graph_cache.record_miss();
                            }
                            guard.graph_cache.insert(key, sources, graph)
                        }
                        None => Arc::new(graph),
                    };
                    let overlay = crate::dispatch::build_vfs_overlay(&guard.engine);
                    PreparedRun::Graph {
                        graph: Ok(graph),
                        path,
                        args,
                        exec_argv: Vec::new(),
                        disallow_string_code_generation: false,
                        has_preloads: false,
                        file,
                        revision,
                        overlay,
                    }
                }
                Err(error) => {
                    if stale.is_some() {
                        guard.graph_cache.record_miss();
                    }
                    PreparedRun::Graph {
                        graph: Err(error),
                        path,
                        args,
                        exec_argv: Vec::new(),
                        disallow_string_code_generation: false,
                        has_preloads: false,
                        file,
                        revision,
                        overlay: std::collections::HashMap::new(),
                    }
                }
            }
        }
    };
    run_without_engine_lock(|| execute_prepared_run(prepared, node, wall_clock, steps))
}

#[allow(clippy::too_many_arguments)]
fn handle_node_run<E: QueryEngine>(
    state: &Mutex<DaemonState<E>>,
    path: String,
    project_root: String,
    args: Vec<String>,
    preloads: Vec<String>,
    exec_argv: Vec<String>,
    disallow_string_code_generation: bool,
    wall_clock: bool,
    steps: StepBudget,
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
            crate::dispatch::prepare_run_with_launch_options(
                engine,
                Some(graph_cache),
                path,
                args,
                project_root,
                resolutions.as_ref(),
                true,
                preloads,
                exec_argv,
                disallow_string_code_generation,
            )
        })) {
            Ok(Ok(prepared)) if guard.engine.revision().0 == revision => prepared,
            Ok(Ok(_)) => {
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
            Ok(Err(message)) => return (Response::Error { message }, Next::Continue),
            Err(payload) => return (internal_error(&*payload), Next::Continue),
        }
    };
    run_without_engine_lock(|| execute_prepared_run(prepared, true, wall_clock, steps))
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
        explain,
        static_link,
        pgo_generate,
        pgo_use,
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
    let pgo_gen = pgo_generate.map(PathBuf::from);
    let pgo_u = pgo_use.map(PathBuf::from);
    let response = run_without_engine_lock(|| match prepared {
        PreparedNative::Graph(graph) => native_build_graph(
            &graph,
            path.clone(),
            project_root.clone(),
            Some(temp_output.to_string_lossy().into_owned()),
            profile,
            node,
            explain,
            false,
            None,
            static_link,
            pgo_gen.clone(),
            pgo_u.clone(),
            NativeSemantics::Js,
        ),
        PreparedNative::Program(program) => native_build_program(
            program,
            path.clone(),
            project_root.clone(),
            Some(temp_output.to_string_lossy().into_owned()),
            profile,
            explain,
            false,
            None,
            static_link,
            pgo_gen,
            pgo_u,
            NativeSemantics::Js,
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
