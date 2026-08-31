//! Turning one [`Request`] into one [`Response`] against the query engine.
//!
//! Deliberately free of I/O on the socket and of any knowledge of threads. Query preparation runs
//! while the engine lock is held; owned interpreter/native work can then execute without it.

use crate::graph_cache::{GraphCache, GraphCacheKey, GraphSourceRevision};
use kiln_db::QueryEngine;
use kiln_module_graph::ModuleGraph;
use kiln_proto::{
    Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary,
    NativeSharedLibrarySummary, PROTOCOL_VERSION, ProjectSummary, Request, ResolutionMap, Response,
    RunSummary, ScalarValueSummary, Stats,
};
#[cfg(test)]
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

/// What the connection loop should do once the response has been written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    /// Keep reading requests from this client.
    Continue,
    /// Answer, then hang up on this client. The daemon keeps serving everyone else.
    CloseConnection,
    /// Answer, hang up, and stop the daemon.
    StopDaemon,
}

/// The owned input for an interpreted run. Preparation is performed while the query-engine
/// mutex is held; execution consumes this value after the mutex has been released.
pub(crate) enum PreparedRun {
    Graph {
        graph: Result<Arc<ModuleGraph>, kiln_module_graph::ModuleGraphError>,
        path: String,
        file: kiln_db::FileId,
        revision: u64,
    },
    Program {
        program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
        path: String,
        file: kiln_db::FileId,
        revision: u64,
    },
}

impl PreparedRun {
    #[cfg(test)]
    fn path(&self) -> &str {
        match self {
            Self::Graph { path, .. } | Self::Program { path, .. } => path,
        }
    }
}

pub fn dispatch<E: QueryEngine>(engine: &mut E, request: Request) -> (Response, Next) {
    if matches!(
        request,
        Request::SetSource { .. }
            | Request::Check { .. }
            | Request::Run { .. }
            | Request::NativeBuild { .. }
            | Request::NativeBuildAtRevision { .. }
            | Request::NativeSharedBuild { .. }
            | Request::NativeRun { .. }
            | Request::Project { .. }
    ) {
        return (
            Response::Error {
                message: "source-consuming requests require daemon state handling".into(),
            },
            Next::Continue,
        );
    }
    dispatch_source(engine, None, request)
}

/// Dispatch a source-consuming request after the server has checked its SourceView while holding
/// the DaemonState mutex. This is crate-visible rather than public so no authority-free path is
/// exposed to callers.
pub(crate) fn dispatch_source<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    request: Request,
) -> (Response, Next) {
    match request {
        Request::Ping { protocol } if protocol != PROTOCOL_VERSION => (
            Response::Error {
                message: format!(
                    "protocol mismatch: client speaks version {protocol}, this daemon speaks \
                     {PROTOCOL_VERSION}; restart the daemon so it matches the client"
                ),
            },
            // Nothing this client goes on to send can be trusted to mean what we think it
            // means, so refuse the whole connection rather than only this request.
            Next::CloseConnection,
        ),
        Request::Ping { .. } => (
            Response::Pong {
                protocol: PROTOCOL_VERSION,
                pid: std::process::id(),
            },
            Next::Continue,
        ),

        // Transactional Sync is owned by the connection loop. Keeping this arm as an explicit
        // error makes accidental use of the old one-shot helper fail closed rather than
        // publishing a partially staged source set.
        Request::Sync { .. } | Request::StageSource { .. } | Request::CommitSync { .. } => (
            Response::Error {
                message: "transactional sync requests require daemon connection state".into(),
            },
            Next::Continue,
        ),

        Request::SetSource {
            path,
            text,
            source_view,
        } => {
            let file = engine.intern(&path);
            engine.set_source(file, text);
            (Response::SourceUpdated { source_view }, Next::Continue)
        }

        Request::Check {
            path,
            project_root,
            source_view: _,
        } => (
            check(engine, resolutions, path, project_root),
            Next::Continue,
        ),
        Request::Run {
            path,
            project_root,
            node,
            source_view: _,
        } => (
            run(engine, resolutions, path, project_root, node),
            Next::Continue,
        ),
        Request::NativeBuild {
            path,
            project_root,
            output,
            profile,
            source_view: _,
        } => (
            match native_graph(engine, resolutions, &path, &project_root) {
                Ok(Some(graph)) => native_build_graph(&graph, path, project_root, output, profile),
                Ok(None) => match native_program(engine, &path) {
                    Ok(program) => {
                        native_build_program(program, path, project_root, output, profile)
                    }
                    Err(message) => Response::Error { message },
                },
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::NativeBuildAtRevision { .. } => (
            Response::Error {
                message: "revision-pinned native builds require daemon snapshot handling".into(),
            },
            Next::Continue,
        ),
        Request::NativeSharedBuild {
            path,
            project_root,
            output,
            profile,
            source_view: _,
        } => (
            match native_graph(engine, resolutions, &path, &project_root) {
                Ok(Some(graph)) if graph.module_count() > 1 => {
                    native_build_shared_graph(&graph, path, project_root, output, profile)
                }
                Ok(Some(_)) | Ok(None) => match native_program(engine, &path) {
                    Ok(program) => {
                        native_build_shared_program(program, path, project_root, output, profile)
                    }
                    Err(message) => Response::Error { message },
                },
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::NativeRun {
            path,
            project_root,
            profile,
            source_view: _,
        } => (
            match native_graph(engine, resolutions, &path, &project_root) {
                Ok(Some(graph)) => native_run_graph(&graph, path, project_root, profile),
                Ok(None) => match native_program(engine, &path) {
                    Ok(program) => native_run_program(program, path, project_root, profile),
                    Err(message) => Response::Error { message },
                },
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),
        Request::Test {
            root,
            mode,
            changed_paths,
            previously_failed,
        } => (
            match crate::testing::run(&root, mode, changed_paths, previously_failed) {
                Ok(summary) => Response::Tested(summary),
                Err(message) => Response::Error { message },
            },
            Next::Continue,
        ),

        Request::Project { source_view: _ } => {
            let project = engine.project();
            (
                Response::Project(ProjectSummary {
                    modules: project.modules,
                    exports: project.exports,
                    interface_hash: project.interface_hash,
                }),
                Next::Continue,
            )
        }

        // The cache is content addressed, so a sweep neither reads nor advances the source set.
        // It needs no engine, and the server routes it off the engine lock for that reason.
        Request::CacheGc {
            project_root,
            dry_run,
        } => (cache_gc(&project_root, dry_run), Next::Continue),

        Request::Stats => (Response::Stats(stats(engine)), Next::Continue),

        Request::ResetStats => {
            engine.reset_stats();
            (Response::Ok, Next::Continue)
        }

        Request::Shutdown => (Response::Ok, Next::StopDaemon),
    }
}

/// Run the pipeline for one module.
///
/// Reading from disk only when the file has no source yet is what makes an editor-driven
/// `SetSource` authoritative: once a client has pushed unsaved buffer contents, later `Check`s
/// must see the buffer, not the stale bytes on disk.
fn check<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    path: String,
    project_root: String,
) -> Response {
    check_with_cache(engine, None, path, project_root, resolutions)
}

pub(crate) fn check_with_cache<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    path: String,
    project_root: String,
    resolutions: Option<&kiln_proto::ResolutionMap>,
) -> Response {
    let entry = resolve_source_path(&path, &project_root);
    let source = match entry.as_deref() {
        Some(entry) => ensure_canonical_source(engine, &path, entry, &project_root),
        None => ensure_source(engine, &path),
    };
    let file = match source {
        Ok(file) => file,
        Err(message) => return Response::Error { message },
    };

    let parsed = engine.parse(file);
    // The normalized interface is the per-file declared surface and incremental cutoff. Graph
    // resolution below remains separate because stars acquire names only in an entry/root context.
    let interface = engine.interface(file);

    let mut diagnostics = parsed
        .module
        .diagnostics
        .into_iter()
        .map(|diagnostic| Diagnostic {
            path: None,
            message: diagnostic.message,
            start: diagnostic.span.map(|span| span.start),
            end: diagnostic.span.map(|span| span.end),
        })
        .collect::<Vec<_>>();
    let resolved_exports = if diagnostics.is_empty() {
        entry.and_then(|entry| {
            match graph_snapshot(engine, cache, &entry, &project_root, resolutions, false) {
                Ok(graph) => graph.resolved_export_names(&entry),
                Err(error) => {
                    diagnostics.extend(module_graph_diagnostics(error));
                    None
                }
            }
        })
    } else {
        None
    };

    Response::Checked(ModuleSummary {
        path,
        file: FileId(file.0),
        implementation_hash: parsed.implementation_hash,
        interface_hash: interface.interface_hash,
        exports: interface.exports,
        resolved_exports,
        diagnostics,
    })
}

/// Prepare one interpreted run from a single engine snapshot.
pub(crate) fn prepare_run<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    path: String,
    project_root: String,
    node: bool,
) -> Result<PreparedRun, String> {
    prepare_run_with_cache(engine, None, path, project_root, resolutions, node)
}

pub(crate) fn prepare_run_with_cache<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    path: String,
    project_root: String,
    resolutions: Option<&kiln_proto::ResolutionMap>,
    node: bool,
) -> Result<PreparedRun, String> {
    let entry = resolve_source_path(&path, &project_root);
    let prepared = if let Some(entry) = entry {
        let file = ensure_canonical_source(engine, &path, &entry, &project_root)?;
        PreparedRun::Graph {
            graph: graph_snapshot(engine, cache, &entry, &project_root, resolutions, node),
            path,
            file,
            revision: engine.revision().0,
        }
    } else {
        let file = ensure_source(engine, &path)?;
        PreparedRun::Program {
            program: engine.ir(file),
            path,
            file,
            revision: engine.revision().0,
        }
    };
    Ok(prepared)
}

/// Execute an interpreted run after preparation has released the query-engine mutex.
pub(crate) fn execute_prepared_run(prepared: PreparedRun, node: bool) -> Response {
    #[cfg(test)]
    wait_for_run_gate(prepared.path());

    match prepared {
        PreparedRun::Graph {
            graph,
            path,
            file,
            revision,
        } => execute_graph(graph, path, file, revision, node),
        PreparedRun::Program {
            program,
            path,
            file,
            revision,
        } => execute_program(program, path, file, revision, node),
    }
}

#[cfg(test)]
struct ExecutionGate {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
fn execution_gates() -> &'static std::sync::Mutex<BTreeMap<String, std::sync::Arc<ExecutionGate>>> {
    static GATES: std::sync::OnceLock<
        std::sync::Mutex<BTreeMap<String, std::sync::Arc<ExecutionGate>>>,
    > = std::sync::OnceLock::new();
    GATES.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
pub(crate) struct ExecutionGateGuard {
    key: String,
    release: Option<std::sync::mpsc::Sender<()>>,
}

#[cfg(test)]
pub(crate) fn install_run_gate(path: &str) -> (std::sync::mpsc::Receiver<()>, ExecutionGateGuard) {
    install_execution_gate(format!("run:{path}"))
}

#[cfg(test)]
pub(crate) fn install_test_gate(root: &str) -> (std::sync::mpsc::Receiver<()>, ExecutionGateGuard) {
    install_execution_gate(format!("test:{root}"))
}

#[cfg(test)]
fn install_execution_gate(key: String) -> (std::sync::mpsc::Receiver<()>, ExecutionGateGuard) {
    let (entered, entered_rx) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let previous = execution_gates()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(
            key.clone(),
            std::sync::Arc::new(ExecutionGate {
                entered,
                release: std::sync::Mutex::new(release_rx),
            }),
        );
    assert!(
        previous.is_none(),
        "execution gate `{key}` is already installed"
    );
    (
        entered_rx,
        ExecutionGateGuard {
            key,
            release: Some(release),
        },
    )
}

#[cfg(test)]
impl ExecutionGateGuard {
    pub(crate) fn release(mut self) {
        self.finish();
    }

    fn finish(&mut self) {
        execution_gates()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.key);
        if let Some(release) = self.release.take() {
            let _ = release.send(());
        }
    }
}

#[cfg(test)]
impl Drop for ExecutionGateGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
pub(crate) fn wait_for_test_gate(root: &str) {
    wait_for_execution_gate(&format!("test:{root}"));
}

#[cfg(test)]
fn wait_for_run_gate(path: &str) {
    wait_for_execution_gate(&format!("run:{path}"));
}

#[cfg(test)]
fn wait_for_execution_gate(key: &str) {
    let gate = execution_gates()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(key)
        .cloned();
    if let Some(gate) = gate {
        gate.entered
            .send(())
            .expect("execution gate receiver remains alive");
        gate.release
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .recv()
            .expect("execution gate release remains alive");
    }
}

fn run<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    path: String,
    project_root: String,
    node: bool,
) -> Response {
    match prepare_run(engine, resolutions, path, project_root, node) {
        Ok(prepared) => execute_prepared_run(prepared, node),
        Err(message) => Response::Error { message },
    }
}

fn execute_graph(
    graph: Result<Arc<ModuleGraph>, kiln_module_graph::ModuleGraphError>,
    path: String,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
) -> Response {
    let mut host = kiln_runtime::CaptureHost::new();
    let result = match graph {
        Ok(graph) if node => graph.execute_node(&mut host, Default::default()),
        Ok(graph) => graph.execute(&mut host, Default::default()),
        Err(error) => Err(error),
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: true,
                stdout: host.into_stdout(),
                value: Some(execution.value.to_display_string()),
                scalar_value,
                instructions: execution.steps,
                diagnostics: Vec::new(),
            })
        }
        Err(error) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: false,
            stdout: host.into_stdout(),
            value: None,
            scalar_value: None,
            instructions: 0,
            diagnostics: module_graph_diagnostics(error),
        }),
    }
}

fn build_graph<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    entry: &Path,
    project_root: &str,
    node: bool,
) -> Result<ModuleGraph, kiln_module_graph::ModuleGraphError> {
    let entries = resolutions
        .into_iter()
        .flat_map(|map| &map.entries)
        .map(|entry| {
            (
                PathBuf::from(&entry.importer),
                entry.specifier.clone(),
                PathBuf::from(&entry.target),
            )
        })
        .collect::<Vec<_>>();
    let provider = |module_path: &Path| {
        let module_path_string = module_path.to_str().ok_or_else(|| {
            kiln_module_graph::ModuleGraphError::FrontendPath {
                path: module_path.to_path_buf(),
            }
        })?;
        let module_file = engine.intern(module_path_string);
        if engine.source(module_file).is_none() {
            return Err(kiln_module_graph::ModuleGraphError::Source {
                path: module_path.to_path_buf(),
                message: "source is not synchronized".to_owned(),
            });
        }
        engine.module_ir(module_file).map_err(|diagnostics| {
            kiln_module_graph::ModuleGraphError::Compile {
                path: module_path.to_path_buf(),
                diagnostics,
            }
        })
    };
    let resolver = if node {
        kiln_module_resolver::PinnedPackageResolver::new_with_node_builtins(project_root, entries)
    } else {
        kiln_module_resolver::PinnedPackageResolver::new(project_root, entries)
    }
    .map_err(|error| kiln_module_graph::ModuleGraphError::Resolve {
        importer: PathBuf::new(),
        specifier: String::new(),
        error,
    })?;
    ModuleGraph::build_with_policy(entry, resolver, provider)
}

/// Return one immutable graph snapshot for the exact committed daemon state.  The caller holds
/// the daemon-state lock while this function runs; the returned `Arc` is then safe to move into
/// interpreter or native work after that lock is released.
pub(crate) fn graph_snapshot<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    entry: &Path,
    project_root: &str,
    resolutions: Option<&kiln_proto::ResolutionMap>,
    node: bool,
) -> Result<Arc<ModuleGraph>, kiln_module_graph::ModuleGraphError> {
    let canonical_root = std::fs::canonicalize(project_root).map_err(|error| {
        kiln_module_graph::ModuleGraphError::Source {
            path: PathBuf::from(project_root),
            message: format!("cannot canonicalize request project root: {error}"),
        }
    })?;
    if let Some(resolutions) = resolutions
        && canonical_root != Path::new(&resolutions.project_root)
    {
        return Err(kiln_module_graph::ModuleGraphError::Source {
            path: entry.to_path_buf(),
            message: format!(
                "request project root {} does not match pinned resolution root {}",
                canonical_root.display(),
                resolutions.project_root
            ),
        });
    }
    let canonical_entry = std::fs::canonicalize(entry).map_err(|error| {
        kiln_module_graph::ModuleGraphError::Entry {
            path: entry.to_path_buf(),
            kind: error.kind(),
        }
    })?;
    let key = GraphCacheKey::new(canonical_root, canonical_entry.clone(), resolutions, node);
    match cache {
        Some(cache) => {
            if let Some(graph) = cache.get(&key, |file| engine.source_revision(file)) {
                return Ok(graph);
            }
            let graph = build_graph(engine, resolutions, &canonical_entry, project_root, node)?;
            let sources = graph
                .module_order()
                .iter()
                .map(|path| {
                    let path = path.to_str()?;
                    let file = engine.intern(path);
                    Some(GraphSourceRevision {
                        file,
                        revision: engine.source_revision(file)?,
                    })
                })
                .collect::<Option<Vec<_>>>();
            Ok(match sources {
                Some(sources) => cache.insert(key, sources, graph),
                // Graph construction rejects non-UTF-8 and unsynchronized modules, so this is a
                // defensive fallback for an external QueryEngine that violates that contract.
                None => Arc::new(graph),
            })
        }
        None => {
            build_graph(engine, resolutions, &canonical_entry, project_root, node).map(Arc::new)
        }
    }
}

fn module_graph_diagnostics(error: kiln_module_graph::ModuleGraphError) -> Vec<Diagnostic> {
    match error {
        kiln_module_graph::ModuleGraphError::Compile { path, diagnostics } => {
            let path = path.to_string_lossy().into_owned();
            diagnostics
                .into_iter()
                .map(|diagnostic| Diagnostic {
                    path: Some(path.clone()),
                    message: diagnostic.message,
                    start: diagnostic.span.map(|span| span.start),
                    end: diagnostic.span.map(|span| span.end),
                })
                .collect()
        }
        error => {
            use kiln_module_graph::ModuleGraphError;
            let path = match &error {
                ModuleGraphError::NonAbsoluteEntry { path }
                | ModuleGraphError::Entry { path, .. }
                | ModuleGraphError::Source { path, .. }
                | ModuleGraphError::FrontendPath { path }
                | ModuleGraphError::InternalMissingModule { path } => Some(path),
                ModuleGraphError::Resolve { importer, .. }
                | ModuleGraphError::UnresolvedImport { importer, .. }
                | ModuleGraphError::MissingExport { importer, .. }
                | ModuleGraphError::AmbiguousExport { importer, .. } => Some(importer),
                ModuleGraphError::Cycle { paths } => paths.first(),
                ModuleGraphError::DuplicateExport { module, .. }
                | ModuleGraphError::Execution { module, .. } => Some(module),
                ModuleGraphError::Compile { .. } | ModuleGraphError::StepOverflow => None,
            }
            .map(|path| path.to_string_lossy().into_owned());
            vec![Diagnostic {
                path,
                message: error.to_string(),
                start: None,
                end: None,
            }]
        }
    }
}

pub(crate) fn native_graph<E: QueryEngine>(
    engine: &mut E,
    resolutions: Option<&ResolutionMap>,
    path: &str,
    project_root: &str,
) -> Result<Option<ModuleGraph>, String> {
    let Some(entry) = resolve_source_path(path, project_root) else {
        ensure_source(engine, path)?;
        return Ok(None);
    };
    ensure_canonical_source(engine, path, &entry, project_root)?;
    build_graph(engine, resolutions, &entry, project_root, false)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn execute_program(
    program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
    path: String,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
) -> Response {
    let program = match program {
        Ok(program) => program,
        Err(diagnostics) => {
            return Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: false,
                stdout: String::new(),
                value: None,
                scalar_value: None,
                instructions: 0,
                diagnostics: diagnostics
                    .into_iter()
                    .map(|diagnostic| Diagnostic {
                        path: None,
                        message: diagnostic.message,
                        start: diagnostic.span.map(|span| span.start),
                        end: diagnostic.span.map(|span| span.end),
                    })
                    .collect(),
            });
        }
    };

    let mut host = kiln_runtime::CaptureHost::new();
    let result = if node {
        kiln_interpreter::execute_node(std::rc::Rc::new(program), &mut host, Default::default())
    } else {
        kiln_interpreter::execute(std::rc::Rc::new(program), &mut host, Default::default())
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: true,
                stdout: host.into_stdout(),
                value: Some(execution.value.to_display_string()),
                scalar_value,
                instructions: execution.steps,
                diagnostics: Vec::new(),
            })
        }
        Err(error) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: false,
            stdout: host.into_stdout(),
            value: None,
            scalar_value: None,
            instructions: 0,
            diagnostics: vec![Diagnostic {
                path: None,
                message: error.to_string(),
                start: None,
                end: None,
            }],
        }),
    }
}

fn scalar_value_summary(value: &kiln_interpreter::Value) -> Option<ScalarValueSummary> {
    match value {
        kiln_interpreter::Value::Undefined => Some(ScalarValueSummary::Undefined),
        kiln_interpreter::Value::Null => Some(ScalarValueSummary::Null),
        kiln_interpreter::Value::Boolean(value) => Some(ScalarValueSummary::Boolean(*value)),
        kiln_interpreter::Value::Number(value) => Some(ScalarValueSummary::Number(value.to_bits())),
        kiln_interpreter::Value::String(value) => Some(ScalarValueSummary::String(value.clone())),
        _ => None,
    }
}

/// Answer a finished executable build, accounting for what it published into the cache.
///
/// Every native build path funnels its report through here, so the opportunistic sweep is paced
/// by everything the daemon publishes rather than by whichever entry point a client happened to
/// use. The accounting runs after the build is complete and only ever spawns a sweep on another
/// thread, so it adds nothing to the latency of the response it precedes.
fn native_built(project_root: &Path, report: kiln_native::NativeBuildReport) -> Response {
    crate::cache_gc::record_publication(project_root, crate::cache_gc::published_bytes(&report));
    Response::NativeBuilt(native_summary(report))
}

/// Answer a finished shared-library build, accounting for what it published into the cache.
fn native_shared_built(
    project_root: &Path,
    report: kiln_native::NativeSharedLibraryReport,
) -> Response {
    crate::cache_gc::record_publication(
        project_root,
        crate::cache_gc::published_shared_bytes(&report),
    );
    Response::NativeSharedBuilt(native_shared_summary(report))
}

/// Answer a finished native run, accounting for what its build published into the cache.
fn native_ran(project_root: &Path, report: kiln_native::NativeRunReport) -> Response {
    crate::cache_gc::record_publication(
        project_root,
        crate::cache_gc::published_bytes(&report.build),
    );
    Response::NativeRan(NativeRunSummary {
        exit_code: report.exit_code,
        stdout: String::from_utf8_lossy(&report.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&report.stderr).into_owned(),
        scalar_value: report.result.map(native_scalar_value_summary),
        build: native_summary(report.build),
    })
}

/// Sweep one project's artifact cache and report what a sweep did, or would do.
pub(crate) fn cache_gc(project_root: &str, dry_run: bool) -> Response {
    match crate::cache_gc::sweep(Path::new(project_root), dry_run) {
        Ok(summary) => Response::CacheSwept(summary),
        Err(message) => Response::Error { message },
    }
}

pub(crate) fn native_build_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || crate::default_native_output(&project_root, Path::new(&path)),
        PathBuf::from,
    );
    if output == Path::new(&path)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&path))
    {
        return Response::Error {
            message: format!("native output would overwrite source file {path}"),
        };
    }
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.build(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => native_built(&project_root, report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

/// Resolve the requested executable output using the same defaults as NativeBuild.
pub(crate) fn native_build_output(
    project_root: &str,
    path: &str,
    output: Option<String>,
) -> PathBuf {
    output.map_or_else(
        || crate::default_native_output(Path::new(project_root), Path::new(path)),
        PathBuf::from,
    )
}

pub(crate) fn native_build_shared_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let (builder, options) =
        match prepare_native_shared_build(&path, &project_root, output, profile) {
            Ok(build) => build,
            Err(message) => return Response::Error { message },
        };
    match builder.build_shared_library(&program, &options) {
        Ok(report) => native_shared_built(Path::new(&project_root), report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_build_shared_graph(
    graph: &ModuleGraph,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let (builder, options) =
        match prepare_native_shared_build(&path, &project_root, output, profile) {
            Ok(build) => build,
            Err(message) => return Response::Error { message },
        };
    match builder.build_shared_graph(graph, &options) {
        Ok(report) => native_shared_built(Path::new(&project_root), report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

fn prepare_native_shared_build(
    path: &str,
    project_root: &str,
    output: Option<String>,
    profile: NativeProfile,
) -> Result<(kiln_native::NativeBuilder, kiln_native::NativeBuildOptions), String> {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || crate::default_native_shared_output(&project_root, Path::new(path)),
        |output| {
            let output = PathBuf::from(output);
            if output.is_absolute() {
                output
            } else {
                project_root.join(output)
            }
        },
    );
    let source = PathBuf::from(path);
    let project_source = if source.is_absolute() {
        source.clone()
    } else {
        project_root.join(&source)
    };
    let canonical_source = std::fs::canonicalize(&project_source).ok();
    let same_source = |candidate: &Path| {
        candidate == source.as_path()
            || candidate == project_source.as_path()
            || canonical_source.as_ref().is_some_and(|source| {
                std::fs::canonicalize(candidate).is_ok_and(|resolved| resolved == *source)
            })
    };
    let header = kiln_native::shared_library_header_path(&output);
    if same_source(&output) {
        return Err(format!("native output would overwrite source file {path}"));
    }
    if same_source(&header) {
        return Err(format!(
            "native shared-library header would overwrite source file {path}"
        ));
    }
    if output
        .extension()
        .is_some_and(|extension| extension.to_string_lossy().eq_ignore_ascii_case("h"))
    {
        return Err("native shared-library header path must differ from library output".to_owned());
    }
    let builder = kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache"))
        .map_err(|error| error.to_string())?;
    Ok((
        builder,
        kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ))
}

pub(crate) fn native_build_graph(
    graph: &ModuleGraph,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = output.map_or_else(
        || crate::default_native_output(&project_root, Path::new(&path)),
        PathBuf::from,
    );
    if output == Path::new(&path)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&path))
    {
        return Response::Error {
            message: format!("native output would overwrite source file {path}"),
        };
    }
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.build_graph(
        graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => native_built(&project_root, report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_run_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = crate::default_native_output(&project_root, Path::new(&path));
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.run(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => native_ran(&project_root, report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

pub(crate) fn native_run_graph(
    graph: &ModuleGraph,
    path: String,
    project_root: String,
    profile: NativeProfile,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let output = crate::default_native_output(&project_root, Path::new(&path));
    let builder =
        match kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache")) {
            Ok(builder) => builder,
            Err(error) => {
                return Response::Error {
                    message: error.to_string(),
                };
            }
        };
    match builder.run_graph(
        graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
        },
    ) {
        Ok(report) => native_ran(&project_root, report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

fn native_scalar_value_summary(value: kiln_native::HostValue) -> ScalarValueSummary {
    match value {
        kiln_native::HostValue::Undefined => ScalarValueSummary::Undefined,
        kiln_native::HostValue::Null => ScalarValueSummary::Null,
        kiln_native::HostValue::Boolean(value) => ScalarValueSummary::Boolean(value),
        kiln_native::HostValue::Number(bits) => ScalarValueSummary::Number(bits),
        kiln_native::HostValue::String(value) => ScalarValueSummary::String(value),
    }
}

pub(crate) fn native_program<E: QueryEngine>(
    engine: &mut E,
    path: &str,
) -> Result<kiln_ir::Program, String> {
    let file = ensure_source(engine, path)?;
    engine.ir(file).map_err(|diagnostics| {
        diagnostics
            .into_iter()
            .map(|diagnostic| match diagnostic.span {
                Some(span) => format!(
                    "{path}:{}..{}: {}",
                    span.start, span.end, diagnostic.message
                ),
                None => format!("{path}: {}", diagnostic.message),
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

fn native_profile(profile: NativeProfile) -> kiln_native::NativeProfile {
    match profile {
        NativeProfile::Dev => kiln_native::NativeProfile::Dev,
        NativeProfile::Check => kiln_native::NativeProfile::Check,
        NativeProfile::Release => kiln_native::NativeProfile::Release,
    }
}

fn native_summary(report: kiln_native::NativeBuildReport) -> NativeBuildSummary {
    NativeBuildSummary {
        module_symbol: report.module_symbol,
        output_path: report.output.to_string_lossy().into_owned(),
        compiler_identity: format!(
            "{} {} targeting {} ({})",
            report.tool.kind,
            report.tool.version,
            report.tool.target,
            report.tool.path.display()
        ),
        generated_c_cache_hit: report.cache_hits.generated_c,
        app_object_cache_hit: report.cache_hits.application_object,
        runtime_object_cache_hit: report.cache_hits.runtime_object,
        launcher_object_cache_hit: report.cache_hits.launcher_object,
        executable_cache_hit: report.cache_hits.executable,
        generated_c_bytes: report.sizes.generated_c,
        app_object_bytes: report.sizes.application_object,
        runtime_object_bytes: report.sizes.runtime_object,
        launcher_object_bytes: report.sizes.launcher_object,
        executable_bytes: report.sizes.executable,
        emit_millis: millis(report.timings.emit),
        compile_millis: millis(report.timings.compile),
        link_millis: millis(report.timings.link),
        total_millis: millis(report.timings.total),
    }
}

fn native_shared_summary(
    report: kiln_native::NativeSharedLibraryReport,
) -> NativeSharedLibrarySummary {
    NativeSharedLibrarySummary {
        module_symbol: report.module_symbol,
        module_api_symbol: report.module_api_symbol,
        output_path: report.output.to_string_lossy().into_owned(),
        loader_path: report.loader.to_string_lossy().into_owned(),
        header_path: report.header.to_string_lossy().into_owned(),
        compiler_identity: format!(
            "{} {} targeting {} ({})",
            report.tool.kind,
            report.tool.version,
            report.tool.target,
            report.tool.path.display()
        ),
        generated_c_cache_hit: report.cache_hits.generated_c,
        object_cache_hit: report.cache_hits.object,
        library_cache_hit: report.cache_hits.library,
        generated_c_bytes: report.sizes.generated_c,
        object_bytes: report.sizes.object,
        library_bytes: report.sizes.library,
        emit_millis: millis(report.timings.emit),
        compile_millis: millis(report.timings.compile),
        link_millis: millis(report.timings.link),
        total_millis: millis(report.timings.total),
    }
}

fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

fn ensure_source<E: QueryEngine>(engine: &mut E, path: &str) -> Result<kiln_db::FileId, String> {
    let file = engine.intern(path);
    if engine.source(file).is_none() {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        engine.set_source(file, text);
    }
    Ok(file)
}

fn resolve_source_path(path: &str, project_root: &str) -> Option<PathBuf> {
    let path = Path::new(path);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(project_root).join(path)
    };
    std::fs::canonicalize(&candidate)
        .or_else(|_| std::fs::canonicalize(lexically_normalize(&candidate)))
        .ok()
}

fn lexically_normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() && !normalized.has_root() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// Collapse every currently resolvable active spelling of one graph entry onto its canonical
/// [`FileId`]. Protocol clients should send canonical paths; this compatibility path preserves
/// relative editor overlays, including lexical `.`/`..` spellings whose intermediate component
/// no longer exists.
///
/// The newest active source wins, so a relative unsaved overlay remains authoritative even when
/// the subsequent run request uses the canonical spelling. Preparation holds the engine lock,
/// making the source snapshot and the following graph build one atomic observation. Activating
/// the canonical input and removing an alias remain two visible source mutations by the
/// [`QueryEngine`] revision contract; subsequent preparations are mutation-free.
fn ensure_canonical_source<E: QueryEngine>(
    engine: &mut E,
    requested_path: &str,
    entry: &Path,
    project_root: &str,
) -> Result<kiln_db::FileId, String> {
    let Some(entry_string) = entry.to_str() else {
        return ensure_source(engine, requested_path);
    };
    let canonical_file = engine.intern(entry_string);

    let mut aliases = Vec::new();
    for (file, path) in engine.source_files() {
        if resolve_source_path(&path, project_root).as_deref() != Some(entry) {
            continue;
        }
        let source = engine
            .source(file)
            .ok_or_else(|| format!("query engine listed {path} as active without source bytes"))?;
        let revision = engine.source_revision(file).ok_or_else(|| {
            format!("query engine listed {path} as active without a source revision")
        })?;
        aliases.push((revision, path, file, source.to_owned()));
    }

    if aliases.is_empty() {
        return ensure_source(engine, entry_string);
    }
    aliases.sort_by(
        |(left_revision, left_path, ..), (right_revision, right_path, ..)| {
            left_revision
                .cmp(right_revision)
                .then_with(|| left_path.cmp(right_path))
        },
    );
    let source = aliases.last().expect("aliases is not empty").3.clone();
    if engine.source(canonical_file) != Some(source.as_str()) {
        engine.set_source(canonical_file, source);
    }
    for (_, _, file, _) in aliases {
        if file != canonical_file {
            engine.remove_source(file);
        }
    }
    Ok(canonical_file)
}

pub(crate) fn stats<E: QueryEngine>(engine: &E) -> Stats {
    let stats = engine.stats();
    Stats {
        revision: stats.revision,
        parse_computed: stats.parse_computed,
        ir_computed: stats.ir_computed,
        interface_computed: stats.interface_computed,
        project_computed: stats.project_computed,
        validated: stats.validated,
        early_cutoff: stats.early_cutoff,
        ..Stats::default()
    }
}
