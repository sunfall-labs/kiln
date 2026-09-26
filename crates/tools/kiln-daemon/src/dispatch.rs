//! Turning one [`Request`] into one [`Response`] against the query engine.
//!
//! Deliberately free of I/O on the socket and of any knowledge of threads. Query preparation runs
//! while the engine lock is held; owned interpreter/native work can then execute without it.

use crate::graph_cache::{GraphCache, GraphCacheKey, GraphCacheLookup, GraphSourceRevision};
use kiln_db::{FileId as DbFileId, QueryEngine, Revision};
use kiln_module_eval::ModuleEvaluationError;
use kiln_module_eval::ModuleGraphExecution;
use kiln_module_graph::ModuleGraph;
use kiln_proto::{
    Diagnostic, FileId, ModuleSummary, NativeBuildSummary, NativeProfile, NativeRunSummary,
    NativeSharedLibrarySummary, PROTOCOL_VERSION, ProjectSummary, Request, ResolutionMap, Response,
    RunSummary, ScalarValueSummary, SourceView, Stats, StepBudget,
};
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
        args: Vec<String>,
        file: kiln_db::FileId,
        revision: u64,
        overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
    },
    Program {
        program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
        path: String,
        args: Vec<String>,
        file: kiln_db::FileId,
        revision: u64,
        overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
    },
}

/// Owned source state captured while the daemon lock is held. Graph parsing and linking can then
/// run against this immutable view while SetSource remains available to other clients.
#[derive(Debug)]
pub(crate) struct OwnedGraphSnapshot {
    pub(crate) source_view: SourceView,
    pub(crate) project_root: PathBuf,
    pub(crate) entry: PathBuf,
    pub(crate) resolutions: Option<ResolutionMap>,
    pub(crate) node: bool,
    pub(crate) sources: BTreeMap<PathBuf, OwnedGraphSource>,
    pub(crate) revisions: Vec<GraphSourceRevision>,
}

#[derive(Debug)]
pub(crate) struct OwnedGraphSource {
    pub(crate) file: DbFileId,
    pub(crate) text: String,
    pub(crate) revision: Revision,
}

#[allow(clippy::large_enum_variant)]
pub(crate) enum GraphRunPlan {
    Cached {
        graph: Arc<ModuleGraph>,
        path: String,
        args: Vec<String>,
        file: DbFileId,
        revision: u64,
    },
    Build {
        snapshot: OwnedGraphSnapshot,
        key: GraphCacheKey,
        stale: Option<(Arc<ModuleGraph>, GraphSourceRevision)>,
        path: String,
        args: Vec<String>,
        file: DbFileId,
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
        Request::Ping { protocol, .. } if protocol != PROTOCOL_VERSION => (
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
                build: crate::build_identity(),
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
            args,
            node,
            steps,
            source_view: _,
        } => (
            run(engine, resolutions, path, project_root, args, node, steps),
            Next::Continue,
        ),
        Request::NativeBuild {
            path,
            project_root,
            node,
            output,
            profile,
            source_view: _,
            explain,
            static_link,
            pgo_generate,
            pgo_use,
        } => {
            let pgo_gen = pgo_generate.map(PathBuf::from);
            let pgo_u = pgo_use.map(PathBuf::from);
            (
                match native_graph(engine, resolutions, &path, &project_root, node) {
                    Ok(Some(graph)) => native_build_graph(
                        &graph,
                        path,
                        project_root,
                        output,
                        profile,
                        node,
                        explain,
                        static_link,
                        pgo_gen,
                        pgo_u,
                    ),
                    Ok(None) => match native_program(engine, &path) {
                        Ok(program) => native_build_program(
                            program,
                            path,
                            project_root,
                            output,
                            profile,
                            explain,
                            static_link,
                            pgo_gen,
                            pgo_u,
                        ),
                        Err(message) => Response::Error { message },
                    },
                    Err(message) => Response::Error { message },
                },
                Next::Continue,
            )
        }
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
            match native_graph(engine, resolutions, &path, &project_root, false) {
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
            args: _,
            profile,
            node,
            source_view: _,
            static_link,
            pgo_generate,
            pgo_use,
        } => {
            let pgo_gen = pgo_generate.map(PathBuf::from);
            let pgo_u = pgo_use.map(PathBuf::from);
            (
                match native_graph(engine, resolutions, &path, &project_root, node) {
                    Ok(Some(graph)) => native_run_graph(
                        &graph,
                        path,
                        project_root,
                        profile,
                        node,
                        static_link,
                        pgo_gen,
                        pgo_u,
                    ),
                    Ok(None) => match native_program(engine, &path) {
                        Ok(program) => native_run_program(
                            program,
                            path,
                            project_root,
                            profile,
                            static_link,
                            pgo_gen,
                            pgo_u,
                        ),
                        Err(message) => Response::Error { message },
                    },
                    Err(message) => Response::Error { message },
                },
                Next::Continue,
            )
        }
        Request::Test {
            root,
            mode,
            changed_root,
            changed_paths,
            previously_failed,
        } => (
            match crate::testing::run_with_changed_root(
                &root,
                mode,
                changed_root,
                changed_paths,
                previously_failed,
            ) {
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
            let node = resolutions.is_some_and(|r| !r.node_formats.is_empty());
            match graph_snapshot(engine, cache, &entry, &project_root, resolutions, node) {
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
    args: Vec<String>,
    project_root: String,
    node: bool,
) -> Result<PreparedRun, String> {
    prepare_run_with_cache(engine, None, path, args, project_root, resolutions, node)
}

pub fn build_vfs_overlay<E: QueryEngine>(
    engine: &E,
) -> std::collections::HashMap<PathBuf, Vec<u8>> {
    let mut files = std::collections::HashMap::new();
    for (id, path_str) in engine.source_files() {
        if let Some(src) = engine.source(id) {
            let path = PathBuf::from(&path_str);
            if let Ok(canon) = std::fs::canonicalize(&path) {
                files.insert(canon, src.as_bytes().to_vec());
            }
            files.insert(path, src.as_bytes().to_vec());
        }
    }
    files
}

pub(crate) fn prepare_run_with_cache<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    path: String,
    args: Vec<String>,
    project_root: String,
    resolutions: Option<&kiln_proto::ResolutionMap>,
    node: bool,
) -> Result<PreparedRun, String> {
    let overlay = build_vfs_overlay(engine);
    let entry = resolve_source_path(&path, &project_root);
    let prepared = if let Some(entry) = entry {
        let file = ensure_canonical_source(engine, &path, &entry, &project_root)?;
        PreparedRun::Graph {
            graph: graph_snapshot(engine, cache, &entry, &project_root, resolutions, node),
            path,
            args,
            file,
            revision: engine.revision().0,
            overlay,
        }
    } else {
        let file = ensure_source(engine, &path)?;
        PreparedRun::Program {
            program: engine.ir(file),
            path,
            args,
            file,
            revision: engine.revision().0,
            overlay,
        }
    };
    Ok(prepared)
}

/// Capture all active source bytes under the daemon lock and return a graph build plan. The
/// first implementation snapshots the complete active set for correctness; a future refinement
/// can narrow the copy once graph reachability and cancellation are explicit APIs.
#[allow(clippy::too_many_arguments)]
pub(crate) fn graph_run_plan<E: QueryEngine>(
    engine: &E,
    cache: &mut GraphCache,
    source_view: SourceView,
    path: String,
    args: Vec<String>,
    file: DbFileId,
    project_root: String,
    entry: PathBuf,
    resolutions: Option<&ResolutionMap>,
    node: bool,
) -> Result<GraphRunPlan, String> {
    let canonical_root = std::fs::canonicalize(&project_root)
        .map_err(|error| format!("cannot canonicalize request project root: {error}"))?;
    if let Some(resolutions) = resolutions
        && canonical_root != Path::new(&resolutions.project_root)
    {
        return Err(format!(
            "request project root {} does not match pinned resolution root {}",
            canonical_root.display(),
            resolutions.project_root
        ));
    }
    let canonical_entry = std::fs::canonicalize(&entry)
        .map_err(|error| format!("cannot canonicalize graph entry: {error}"))?;
    let key = GraphCacheKey::new(
        canonical_root.clone(),
        canonical_entry.clone(),
        resolutions,
        node,
    );
    let stale = match cache.lookup(&key, |source| engine.source_revision(source)) {
        GraphCacheLookup::Hit(graph) => {
            return Ok(GraphRunPlan::Cached {
                graph,
                path,
                args,
                file,
                revision: engine.revision().0,
            });
        }
        GraphCacheLookup::Stale { graph, changed } if changed.len() == 1 => {
            Some((graph, changed[0]))
        }
        GraphCacheLookup::Stale { .. } | GraphCacheLookup::Miss => None,
    };

    let mut sources = BTreeMap::new();
    let mut revisions = Vec::new();
    for (source_file, source_path) in engine.source_files() {
        let Some(revision) = engine.source_revision(source_file) else {
            return Err(format!(
                "query engine listed {source_path} without a source revision"
            ));
        };
        let Some(text) = engine.source(source_file) else {
            return Err(format!(
                "query engine listed {source_path} without source bytes"
            ));
        };
        revisions.push(GraphSourceRevision {
            file: source_file,
            revision,
        });
        let canonical_path = canonical_source_path(&source_path, &canonical_root)
            .unwrap_or_else(|| PathBuf::from(&source_path));
        sources.insert(
            canonical_path,
            OwnedGraphSource {
                file: source_file,
                text: text.to_owned(),
                revision,
            },
        );
    }
    Ok(GraphRunPlan::Build {
        snapshot: OwnedGraphSnapshot {
            source_view,
            project_root: canonical_root,
            entry: canonical_entry,
            resolutions: resolutions.cloned(),
            node,
            sources,
            revisions,
        },
        key,
        stale,
        path,
        args,
        file,
        revision: engine.revision().0,
    })
}

fn canonical_source_path(path: &str, project_root: &Path) -> Option<PathBuf> {
    let path = Path::new(path);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    std::fs::canonicalize(absolute).ok()
}

pub(crate) fn build_graph_from_snapshot(
    snapshot: &OwnedGraphSnapshot,
) -> Result<ModuleGraph, kiln_module_graph::ModuleGraphError> {
    let entries = snapshot
        .resolutions
        .iter()
        .flat_map(|map| &map.entries)
        .map(|entry| {
            (
                PathBuf::from(&entry.importer),
                entry.specifier.clone(),
                match entry.condition {
                    kiln_proto::NodeRequestKind::Import => {
                        kiln_module_resolver::NodeRequestKind::Import
                    }
                    kiln_proto::NodeRequestKind::Require => {
                        kiln_module_resolver::NodeRequestKind::Require
                    }
                },
                PathBuf::from(&entry.target),
            )
        })
        .collect::<Vec<_>>();
    let formats = snapshot
        .resolutions
        .iter()
        .flat_map(|map| &map.node_formats)
        .map(|format| {
            (
                PathBuf::from(&format.path),
                match format.format {
                    kiln_proto::NodeModuleFormat::EsModule => {
                        kiln_module_resolver::NodeModuleFormat::EsModule
                    }
                    kiln_proto::NodeModuleFormat::CommonJs => {
                        kiln_module_resolver::NodeModuleFormat::CommonJs
                    }
                },
            )
        })
        .collect::<Vec<_>>();
    let resolver = if snapshot.node {
        kiln_module_resolver::PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
            &snapshot.project_root,
            entries,
            formats,
        )
    } else {
        kiln_module_resolver::PinnedPackageResolver::new_with_requests_and_formats(
            &snapshot.project_root,
            entries,
            formats,
        )
    }
    .map_err(|error| kiln_module_graph::ModuleGraphError::Resolve {
        importer: PathBuf::new(),
        specifier: String::new(),
        error,
    })?;
    ModuleGraph::build_with_typed_policy(&snapshot.entry, resolver, |path, source_kind| {
        snapshot_module_payload(snapshot, path, source_kind)
    })
}

/// Patch one ordinary ESM module in an old immutable graph. Returning `None` asks the caller to
/// fall back to a complete rebuild (CommonJS, JSON, builtins, and topology changes need that).
pub(crate) fn patch_graph_from_snapshot(
    snapshot: &OwnedGraphSnapshot,
    old: &ModuleGraph,
    changed: GraphSourceRevision,
) -> Result<Option<ModuleGraph>, kiln_module_graph::ModuleGraphError> {
    let Some((path, _source)) = snapshot
        .sources
        .iter()
        .find(|(_, source)| source.file == changed.file)
    else {
        return Ok(None);
    };
    if old
        .module(path)
        .is_none_or(|module| module.source != kiln_module_graph::ModuleSource::Source)
    {
        return Ok(None);
    }
    let kiln_module_graph::ModulePayload::Program(program) =
        snapshot_module_payload(snapshot, path, kiln_module_graph::ModuleSource::Source)?
    else {
        unreachable!("ordinary source snapshot payload is a program");
    };
    Ok(old.patch_module(path, program))
}

fn snapshot_module_payload(
    snapshot: &OwnedGraphSnapshot,
    path: &Path,
    source_kind: kiln_module_graph::ModuleSource,
) -> Result<kiln_module_graph::ModulePayload, kiln_module_graph::ModuleGraphError> {
    if source_kind == kiln_module_graph::ModuleSource::Asset {
        return Ok(kiln_module_graph::ModulePayload::AssetPath(
            path.to_path_buf(),
        ));
    }
    let source =
        snapshot
            .sources
            .get(path)
            .ok_or_else(|| kiln_module_graph::ModuleGraphError::Source {
                path: path.to_path_buf(),
                message: "source is not synchronized".to_owned(),
            })?;
    if source_kind == kiln_module_graph::ModuleSource::Json {
        let json_text =
            kiln_data_formats::parse_data_to_json(path, &source.text).map_err(|err| {
                kiln_module_graph::ModuleGraphError::Source {
                    path: path.to_path_buf(),
                    message: err.to_string(),
                }
            })?;
        return Ok(kiln_module_graph::ModulePayload::JsonText(json_text));
    }
    let path_string =
        path.to_str()
            .ok_or_else(|| kiln_module_graph::ModuleGraphError::FrontendPath {
                path: path.to_path_buf(),
            })?;
    #[cfg(test)]
    wait_for_graph_gate(path);
    if source_kind == kiln_module_graph::ModuleSource::CommonJs {
        let (hir, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(path_string, &source.text);
        if hir.has_static_module_syntax() {
            return Err(kiln_module_graph::ModuleGraphError::CjsStaticModuleSyntax {
                path: path.to_path_buf(),
                diagnostics: kiln_ir::cjs_static_module_syntax_diagnostics(&hir),
            });
        }
        let mut program = kiln_ir::compile_commonjs(&hir).map_err(|diagnostics| {
            kiln_module_graph::ModuleGraphError::Compile {
                path: path.to_path_buf(),
                diagnostics,
            }
        })?;
        program.dependencies = static_requires;
        return Ok(kiln_module_graph::ModulePayload::Program(program));
    }
    let hir = kiln_frontend_oxc::parse(path_string, &source.text);
    let program = kiln_ir::compile_module(&hir).map_err(|diagnostics| {
        kiln_module_graph::ModuleGraphError::Compile {
            path: path.to_path_buf(),
            diagnostics,
        }
    })?;
    Ok(kiln_module_graph::ModulePayload::Program(program))
}

pub(crate) fn graph_snapshot_is_current<E: QueryEngine>(
    snapshot: &OwnedGraphSnapshot,
    engine: &E,
    source_view: SourceView,
) -> bool {
    source_view == snapshot.source_view
        && snapshot
            .revisions
            .iter()
            .all(|source| engine.source_revision(source.file) == Some(source.revision))
}

pub(crate) fn graph_sources_from_snapshot(
    snapshot: &OwnedGraphSnapshot,
    graph: &ModuleGraph,
) -> Option<Vec<GraphSourceRevision>> {
    graph
        .module_order()
        .iter()
        .map(|path| {
            snapshot
                .sources
                .get(path)
                .map(|source| GraphSourceRevision {
                    file: source.file,
                    revision: source.revision,
                })
        })
        .collect()
}

/// Execute an interpreted run after preparation has released the query-engine mutex.
pub(crate) fn execute_prepared_run(
    prepared: PreparedRun,
    node: bool,
    steps: StepBudget,
) -> Response {
    #[cfg(test)]
    wait_for_run_gate(prepared.path());

    let limits = execution_limits(steps);
    match prepared {
        PreparedRun::Graph {
            graph,
            path,
            args,
            file,
            revision,
            overlay,
        } => execute_graph(graph, path, args, file, revision, node, limits, overlay),
        PreparedRun::Program {
            program,
            path,
            args,
            file,
            revision,
            overlay,
        } => execute_program(program, path, args, file, revision, node, limits, overlay),
    }
}

/// The engine limits one run request asked for.
///
/// Only the step budget is the client's to choose. The call-depth and native-depth bounds guard
/// the *host* stack, whose exhaustion aborts the process rather than raising an error, so they
/// are not negotiable by anyone.
fn execution_limits(steps: StepBudget) -> kiln_interpreter::ExecutionLimits {
    kiln_interpreter::ExecutionLimits::default().with_max_steps(match steps {
        StepBudget::Unmetered => kiln_interpreter::ExecutionLimits::UNMETERED_STEPS,
        StepBudget::Limited(steps) => steps,
    })
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
pub(crate) fn install_graph_gate(
    path: &str,
) -> (std::sync::mpsc::Receiver<()>, ExecutionGateGuard) {
    install_execution_gate(format!("graph:{path}"))
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
fn wait_for_graph_gate(path: &Path) {
    wait_for_execution_gate(&format!("graph:{}", path.display()));
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
    args: Vec<String>,
    node: bool,
    steps: StepBudget,
) -> Response {
    match prepare_run(engine, resolutions, path, args, project_root, node) {
        Ok(prepared) => execute_prepared_run(prepared, node, steps),
        Err(message) => Response::Error { message },
    }
}

/// What `process.argv` reports for an interpreted run, in node's spelling: the executable that
/// is running, then the entry it was pointed at, followed by any arguments passed to the script.
///
/// A natively compiled program answers the same question with the same two slots, but both hold
/// its own path -- there is no separate interpreter and no script file, which is the single
/// executable convention. Either way `process.argv[2..]` is where the program's own arguments
/// begin.
///
/// `current_exe` can fail on an exotic host. The entry path is still worth reporting when it
/// does, so the executable slot falls back to the name every launcher of this program has:
/// leaving both slots off would move the entry to index 0 and shift what a program reads.
pub(crate) fn interpreted_argv(path: &str, args: &[String]) -> Vec<String> {
    let executable = std::env::current_exe()
        .map(|exe| exe.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "kiln".to_owned());
    let mut argv = vec![executable, path.to_owned()];
    argv.extend(args.iter().cloned());
    argv
}

#[allow(clippy::too_many_arguments)]
fn execute_graph(
    graph: Result<Arc<ModuleGraph>, kiln_module_graph::ModuleGraphError>,
    path: String,
    args: Vec<String>,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
    limits: kiln_interpreter::ExecutionLimits,
    overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
) -> Response {
    let capabilities =
        kiln_conformance::source_compiler_capability().with_argv(interpreted_argv(&path, &args));
    let mut host = kiln_runtime::CaptureHost::new().with_fs(Box::new(
        kiln_host_abi::OverlayFileSystem::new(kiln_host_abi::OsFileSystem, overlay),
    ));
    // One shaken graph feeds execution: reduce to the closed-world live
    // subset first, so dead modules never evaluate. Development selection
    // keeps the interpreter conservative; a reduction failure runs the full
    // graph rather than refusing a program liveness could not classify.
    let graph = graph.map(|graph| {
        kiln_liveness::executable_demand(&graph, &kiln_liveness::SelectionOptions::development())
            .and_then(|demand| {
                graph
                    .reduce_to_live(&demand)
                    .map(|reduced| Arc::new(reduced.0))
            })
            .unwrap_or(graph)
    });
    let result = match graph {
        Ok(graph) if node => {
            let capabilities = capabilities.with_module_loader_service(std::sync::Arc::new(
                kiln_module_eval::GraphModuleLoader::new(std::sync::Arc::clone(&graph)),
            ));
            graph.execute_node(&mut host, limits, capabilities)
        }
        Ok(graph) => {
            let capabilities = capabilities.with_module_loader_service(std::sync::Arc::new(
                kiln_module_eval::GraphModuleLoader::new(std::sync::Arc::clone(&graph)),
            ));
            graph.execute(&mut host, limits, capabilities)
        }
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: true,
                exit_code: Some(0),
                stdout: host.stdout().to_owned(),
                stderr: host.stderr().to_owned(),
                value: Some(execution.value.to_display_string()),
                scalar_value,
                instructions: execution.steps,
                diagnostics: Vec::new(),
            })
        }
        Err(ModuleEvaluationError::Execution {
            error: kiln_interpreter::ExecutionError::ProcessExited(code),
            ..
        }) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: code == 0,
            exit_code: Some(code),
            stdout: host.stdout().to_owned(),
            stderr: host.stderr().to_owned(),
            value: None,
            scalar_value: None,
            instructions: 0,
            diagnostics: Vec::new(),
        }),
        Err(error) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: false,
            exit_code: Some(1),
            stdout: host.stdout().to_owned(),
            stderr: host.stderr().to_owned(),
            value: None,
            scalar_value: None,
            instructions: 0,
            diagnostics: module_evaluation_diagnostics(error),
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
                match entry.condition {
                    kiln_proto::NodeRequestKind::Import => {
                        kiln_module_resolver::NodeRequestKind::Import
                    }
                    kiln_proto::NodeRequestKind::Require => {
                        kiln_module_resolver::NodeRequestKind::Require
                    }
                },
                PathBuf::from(&entry.target),
            )
        })
        .collect::<Vec<_>>();
    let formats = resolutions
        .into_iter()
        .flat_map(|map| &map.node_formats)
        .map(|format| {
            (
                PathBuf::from(&format.path),
                match format.format {
                    kiln_proto::NodeModuleFormat::EsModule => {
                        kiln_module_resolver::NodeModuleFormat::EsModule
                    }
                    kiln_proto::NodeModuleFormat::CommonJs => {
                        kiln_module_resolver::NodeModuleFormat::CommonJs
                    }
                },
            )
        })
        .collect::<Vec<_>>();
    let provider = |module_path: &Path, source_kind: kiln_module_graph::ModuleSource| {
        module_payload(engine, module_path, source_kind)
    };
    let resolver = if node {
        kiln_module_resolver::PinnedPackageResolver::new_with_node_builtins_requests_and_formats(
            project_root,
            entries,
            formats,
        )
    } else {
        kiln_module_resolver::PinnedPackageResolver::new_with_requests_and_formats(
            project_root,
            entries,
            formats,
        )
    }
    .map_err(|error| kiln_module_graph::ModuleGraphError::Resolve {
        importer: PathBuf::new(),
        specifier: String::new(),
        error,
    })?;
    ModuleGraph::build_with_typed_policy(entry, resolver, provider)
}

fn module_payload<E: QueryEngine>(
    engine: &mut E,
    module_path: &Path,
    source_kind: kiln_module_graph::ModuleSource,
) -> Result<kiln_module_graph::ModulePayload, kiln_module_graph::ModuleGraphError> {
    if source_kind == kiln_module_graph::ModuleSource::Asset {
        return Ok(kiln_module_graph::ModulePayload::AssetPath(
            module_path.to_path_buf(),
        ));
    }
    let module_path_string =
        module_path
            .to_str()
            .ok_or_else(|| kiln_module_graph::ModuleGraphError::FrontendPath {
                path: module_path.to_path_buf(),
            })?;
    let module_file = engine.intern(module_path_string);
    if engine.source(module_file).is_none() {
        return Err(kiln_module_graph::ModuleGraphError::Source {
            path: module_path.to_path_buf(),
            message: "source is not synchronized".to_owned(),
        });
    }
    if source_kind == kiln_module_graph::ModuleSource::Json {
        let raw_text = engine.source(module_file).expect("source checked above");
        let json_text =
            kiln_data_formats::parse_data_to_json(module_path, raw_text).map_err(|err| {
                kiln_module_graph::ModuleGraphError::Source {
                    path: module_path.to_path_buf(),
                    message: err.to_string(),
                }
            })?;
        return Ok(kiln_module_graph::ModulePayload::JsonText(json_text));
    }
    if source_kind == kiln_module_graph::ModuleSource::CommonJs {
        let source = engine.source(module_file).expect("source checked above");
        let (hir, static_requires) =
            kiln_frontend_oxc::parse_commonjs_with_requires(module_path_string, source);
        if hir.has_static_module_syntax() {
            return Err(kiln_module_graph::ModuleGraphError::CjsStaticModuleSyntax {
                path: module_path.to_path_buf(),
                diagnostics: kiln_ir::cjs_static_module_syntax_diagnostics(&hir),
            });
        }
        let mut program = kiln_ir::compile_commonjs(&hir).map_err(|diagnostics| {
            kiln_module_graph::ModuleGraphError::Compile {
                path: module_path.to_path_buf(),
                diagnostics,
            }
        })?;
        program.dependencies = static_requires;
        return Ok(kiln_module_graph::ModulePayload::Program(program));
    }
    engine
        .module_ir(module_file)
        .map(kiln_module_graph::ModulePayload::Program)
        .map_err(|diagnostics| kiln_module_graph::ModuleGraphError::Compile {
            path: module_path.to_path_buf(),
            diagnostics,
        })
}

fn graph_sources<E: QueryEngine>(
    engine: &mut E,
    graph: &ModuleGraph,
) -> Option<Vec<GraphSourceRevision>> {
    graph
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
        .collect()
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
    if let Some(cache) = cache {
        match cache.lookup(&key, |file| engine.source_revision(file)) {
            GraphCacheLookup::Hit(graph) => return Ok(graph),
            GraphCacheLookup::Stale { graph, changed } if changed.len() == 1 => {
                let changed = changed[0];
                let path = engine
                    .source_files()
                    .into_iter()
                    .find_map(|(file, path)| (file == changed.file).then_some(path))
                    .and_then(|path| std::fs::canonicalize(path).ok());
                // Partial reuse is intentionally limited to ordinary ESM source modules.
                // CommonJS, JSON, and Node-builtin snapshots take the full rebuild path below;
                // their materialisation/link semantics are not represented by ModuleProgram.
                if let Some(path) = path
                    && graph.module(&path).is_some_and(|module| {
                        module.source == kiln_module_graph::ModuleSource::Source
                    })
                {
                    let payload = match module_payload(
                        engine,
                        &path,
                        kiln_module_graph::ModuleSource::Source,
                    ) {
                        Ok(payload) => payload,
                        Err(error) => {
                            // The old snapshot is still retained, but this request did not
                            // reuse it: expose the failed patch attempt as a cache miss.
                            cache.record_miss();
                            return Err(error);
                        }
                    };
                    let kiln_module_graph::ModulePayload::Program(program) = payload else {
                        unreachable!("source module provider returns a program");
                    };
                    if let Some(patched) = graph.patch_module(&path, program)
                        && let Some(sources) = graph_sources(engine, &patched)
                    {
                        cache.record_partial_hit();
                        return Ok(cache.insert(key, sources, patched));
                    }
                }
                cache.record_miss();
            }
            GraphCacheLookup::Stale { .. } => cache.record_miss(),
            GraphCacheLookup::Miss => {}
        }
        let graph = build_graph(engine, resolutions, &canonical_entry, project_root, node)?;
        return Ok(match graph_sources(engine, &graph) {
            Some(sources) => cache.insert(key, sources, graph),
            // Graph construction rejects non-UTF-8 and unsynchronized modules, so this is a
            // defensive fallback for an external QueryEngine that violates that contract.
            None => Arc::new(graph),
        });
    }
    build_graph(engine, resolutions, &canonical_entry, project_root, node).map(Arc::new)
}

fn module_evaluation_diagnostics(error: ModuleEvaluationError) -> Vec<Diagnostic> {
    if let ModuleEvaluationError::Graph(error) = error {
        return module_graph_diagnostics(error);
    }
    let path = match &error {
        ModuleEvaluationError::Execution { module, .. } => {
            Some(module.to_string_lossy().into_owned())
        }
        _ => None,
    };
    vec![Diagnostic {
        path,
        message: error.to_string(),
        start: None,
        end: None,
    }]
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
                ModuleGraphError::DuplicateExport { module, .. } => Some(module),
                // The signal never escapes graph construction (it becomes a retry or a
                // `Compile`), so this arm is unreachable; it renders through `Display`.
                ModuleGraphError::Compile { .. }
                | ModuleGraphError::CjsStaticModuleSyntax { .. } => None,
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
    node: bool,
) -> Result<Option<Arc<ModuleGraph>>, String> {
    native_graph_with_cache(engine, None, resolutions, path, project_root, node)
}

/// Prepare an entry-rooted native graph through the daemon cache when one is available.
/// One-shot/direct dispatch passes `None`; persistent daemon requests pass their shared cache so
/// native graph preparation has the same immutable snapshot and partial-body-patch semantics as
/// interpreted runs.
pub(crate) fn native_graph_with_cache<E: QueryEngine>(
    engine: &mut E,
    cache: Option<&mut GraphCache>,
    resolutions: Option<&ResolutionMap>,
    path: &str,
    project_root: &str,
    node: bool,
) -> Result<Option<Arc<ModuleGraph>>, String> {
    let Some(entry) = resolve_source_path(path, project_root) else {
        ensure_source(engine, path)?;
        return Ok(None);
    };
    ensure_canonical_source(engine, path, &entry, project_root)?;
    graph_snapshot(engine, cache, &entry, project_root, resolutions, node)
        .map(Some)
        .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn execute_program(
    program: Result<kiln_ir::Program, Vec<kiln_ir::CompileDiagnostic>>,
    path: String,
    args: Vec<String>,
    file: kiln_db::FileId,
    revision: u64,
    node: bool,
    limits: kiln_interpreter::ExecutionLimits,
    overlay: std::collections::HashMap<std::path::PathBuf, Vec<u8>>,
) -> Response {
    let capabilities =
        kiln_conformance::source_compiler_capability().with_argv(interpreted_argv(&path, &args));
    let program = match program {
        Ok(program) => program,
        Err(diagnostics) => {
            return Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: false,
                exit_code: Some(1),
                stdout: String::new(),
                stderr: String::new(),
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

    let mut host = kiln_runtime::CaptureHost::new().with_fs(Box::new(
        kiln_host_abi::OverlayFileSystem::new(kiln_host_abi::OsFileSystem, overlay),
    ));
    let result = if node {
        kiln_interpreter::execute_node(std::rc::Rc::new(program), &mut host, limits, capabilities)
    } else {
        kiln_interpreter::execute(std::rc::Rc::new(program), &mut host, limits, capabilities)
    };
    match result {
        Ok(execution) => {
            let scalar_value = scalar_value_summary(&execution.value);
            Response::Ran(RunSummary {
                revision,
                path,
                file: FileId(file.0),
                succeeded: true,
                exit_code: Some(0),
                stdout: host.stdout().to_owned(),
                stderr: host.stderr().to_owned(),
                value: Some(execution.value.to_display_string()),
                scalar_value,
                instructions: execution.steps,
                diagnostics: Vec::new(),
            })
        }
        Err(kiln_interpreter::ExecutionError::ProcessExited(code)) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: code == 0,
            exit_code: Some(code),
            stdout: host.stdout().to_owned(),
            stderr: host.stderr().to_owned(),
            value: None,
            scalar_value: None,
            instructions: 0,
            diagnostics: Vec::new(),
        }),
        Err(error) => Response::Ran(RunSummary {
            revision,
            path,
            file: FileId(file.0),
            succeeded: false,
            exit_code: Some(1),
            stdout: host.stdout().to_owned(),
            stderr: host.stderr().to_owned(),
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
        kiln_interpreter::Value::String(value) => Some(ScalarValueSummary::String(
            value.to_scalar_string().unwrap_or_else(|_| {
                value
                    .units()
                    .map(|unit| format!("\\u{{{unit:04X}}}"))
                    .collect()
            }),
        )),
        _ => None,
    }
}

/// Answer a finished executable build, accounting for what it published into the cache.
///
/// Every native build path funnels its report through here, so the opportunistic sweep is paced
/// by everything the daemon publishes rather than by whichever entry point a client happened to
/// use. The accounting runs after the build is complete and only ever spawns a sweep on another
/// thread, so it adds nothing to the latency of the response it precedes.
fn native_built(
    project_root: &Path,
    builder: &kiln_native::NativeBuilder,
    report: kiln_native::NativeBuildReport,
    explanation: Option<String>,
) -> Response {
    crate::cache_gc::record_publication(
        project_root,
        crate::cache_gc::published_bytes(&report),
        native_cache_identities(builder),
    );
    let mut summary = native_summary(report);
    summary.explanation = explanation;
    Response::NativeBuilt(summary)
}

/// Answer a finished shared-library build, accounting for what it published into the cache.
fn native_shared_built(
    project_root: &Path,
    builder: &kiln_native::NativeBuilder,
    report: kiln_native::NativeSharedLibraryReport,
) -> Response {
    crate::cache_gc::record_publication(
        project_root,
        crate::cache_gc::published_shared_bytes(&report),
        native_cache_identities(builder),
    );
    Response::NativeSharedBuilt(native_shared_summary(report))
}

fn discover_native_builder(project_root: &Path) -> Result<kiln_native::NativeBuilder, String> {
    kiln_native::NativeBuilder::discover_cached(project_root.join(".kiln/cache"))
        .map_err(|error| error.to_string())
}

fn default_native_output_for_builder(
    builder: &kiln_native::NativeBuilder,
    project_root: &Path,
    path: &Path,
    shared: bool,
) -> Result<PathBuf, String> {
    let platform = builder.toolchain().identity().target_platform();
    if shared {
        let suffix = platform.shared_library_suffix().ok_or_else(|| {
            format!(
                "native shared-library output naming is unsupported for target {}",
                builder.toolchain().identity().target
            )
        })?;
        Ok(crate::output::default_native_shared_output_with_suffix(
            project_root,
            path,
            suffix,
        ))
    } else {
        Ok(crate::output::default_native_output_with_suffix(
            project_root,
            path,
            platform.executable_suffix(),
        ))
    }
}

/// Answer a finished native run, accounting for what its build published into the cache.
fn native_ran(
    project_root: &Path,
    builder: &kiln_native::NativeBuilder,
    report: kiln_native::NativeRunReport,
) -> Response {
    crate::cache_gc::record_publication(
        project_root,
        crate::cache_gc::published_bytes(&report.build),
        native_cache_identities(builder),
    );
    Response::NativeRan(NativeRunSummary {
        exit_code: report.exit_code,
        stdout: String::from_utf8_lossy(&report.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&report.stderr).into_owned(),
        scalar_value: report.result.map(native_scalar_value_summary),
        build: native_summary(report.build),
    })
}

fn native_cache_identities(
    builder: &kiln_native::NativeBuilder,
) -> std::collections::BTreeSet<kiln_artifact_cache::Digest> {
    builder.live_cache_identities().unwrap_or_else(|error| {
        // A successful build has already loaded the same provider inputs, so this is defensive.
        // Cache maintenance is never allowed to turn a successful native build into a failure;
        // an empty set merely disables identity-stale eviction for this process.
        eprintln!("kiln-daemon: cannot enumerate live native cache identities: {error}");
        std::collections::BTreeSet::new()
    })
}

/// Sweep one project's artifact cache and report what a sweep did, or would do.
pub(crate) fn cache_gc(project_root: &str, dry_run: bool) -> Response {
    match crate::cache_gc::sweep(Path::new(project_root), dry_run) {
        Ok(summary) => Response::CacheSwept(summary),
        Err(message) => Response::Error { message },
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_build_program(
    program: kiln_ir::Program,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
    explain: bool,
    static_link: bool,
    pgo_generate: Option<PathBuf>,
    pgo_use: Option<PathBuf>,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let builder = match discover_native_builder(&project_root) {
        Ok(builder) => builder,
        Err(message) => return Response::Error { message },
    };
    let output = match output {
        Some(output) => PathBuf::from(output),
        None => match default_native_output_for_builder(
            &builder,
            &project_root,
            Path::new(&path),
            false,
        ) {
            Ok(output) => output,
            Err(message) => return Response::Error { message },
        },
    };
    if output == Path::new(&path)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&path))
    {
        return Response::Error {
            message: format!("native output would overwrite source file {path}"),
        };
    }
    match builder.build(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            realm: kiln_native::NativeRealmPolicy::host_clock(),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node: false,
        },
    ) {
        Ok(report) => {
            let explanation = if explain {
                Some(kiln_backend_c::facts::analyze_program(&program, Some(&path)).format_explain())
            } else {
                None
            };
            native_built(&project_root, &builder, report, explanation)
        }
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
) -> Result<PathBuf, String> {
    match output {
        Some(output) => Ok(PathBuf::from(output)),
        None => {
            let project_root = Path::new(project_root);
            let builder = discover_native_builder(project_root)?;
            default_native_output_for_builder(&builder, project_root, Path::new(path), false)
        }
    }
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
        Ok(report) => native_shared_built(Path::new(&project_root), &builder, report),
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
        Ok(report) => native_shared_built(Path::new(&project_root), &builder, report),
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
    let builder = discover_native_builder(&project_root)?;
    let output = match output {
        Some(output) => {
            let output = PathBuf::from(output);
            if output.is_absolute() {
                output
            } else {
                project_root.join(output)
            }
        }
        None => default_native_output_for_builder(&builder, &project_root, Path::new(path), true)?,
    };
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
    Ok((
        builder,
        kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            realm: kiln_native::NativeRealmPolicy::default(),
            march_native: false,
            ..kiln_native::NativeBuildOptions::default()
        },
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_build_graph(
    graph: &ModuleGraph,
    path: String,
    project_root: String,
    output: Option<String>,
    profile: NativeProfile,
    node: bool,
    explain: bool,
    static_link: bool,
    pgo_generate: Option<PathBuf>,
    pgo_use: Option<PathBuf>,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let builder = match discover_native_builder(&project_root) {
        Ok(builder) => builder,
        Err(message) => return Response::Error { message },
    };
    let output = match output {
        Some(output) => PathBuf::from(output),
        None => match default_native_output_for_builder(
            &builder,
            &project_root,
            Path::new(&path),
            false,
        ) {
            Ok(output) => output,
            Err(message) => return Response::Error { message },
        },
    };
    if output == Path::new(&path)
        || std::fs::canonicalize(&output).is_ok_and(|resolved| resolved == Path::new(&path))
    {
        return Response::Error {
            message: format!("native output would overwrite source file {path}"),
        };
    }
    match builder.build_graph(
        graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            realm: kiln_native::NativeRealmPolicy::host_clock(),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node,
        },
    ) {
        Ok(report) => {
            let explanation = if explain {
                let mut text = kiln_backend_c::facts::analyze_graph(graph).format_explain();
                // Out-of-band attribution: failures here must never fail the
                // build, so each section is skipped rather than propagated.
                if let Ok(demand) = kiln_liveness::executable_demand(
                    graph,
                    &kiln_native::selection_options(&native_profile(profile)),
                ) {
                    if let Ok(sizes) =
                        kiln_liveness::executable_module_sizes_from_demand(graph, &demand)
                    {
                        text.push_str(&kiln_backend_c::facts::format_module_sizes(&sizes));
                    }
                    let members = kiln_backend_c::facts::analyze_demanded_members(
                        graph,
                        &demand.live,
                        &demand.slots,
                    );
                    text.push_str(&kiln_backend_c::facts::format_demanded_members(&members));
                    let mask = kiln_liveness::retained_method_groups(graph, &demand, &members);
                    text.push_str(&kiln_backend_c::facts::format_retained_groups(mask));
                }
                Some(text)
            } else {
                None
            };
            native_built(&project_root, &builder, report, explanation)
        }
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
    static_link: bool,
    pgo_generate: Option<PathBuf>,
    pgo_use: Option<PathBuf>,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let builder = match discover_native_builder(&project_root) {
        Ok(builder) => builder,
        Err(message) => return Response::Error { message },
    };
    let output =
        match default_native_output_for_builder(&builder, &project_root, Path::new(&path), false) {
            Ok(output) => output,
            Err(message) => return Response::Error { message },
        };
    match builder.run(
        &program,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            realm: kiln_native::NativeRealmPolicy::default(),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node: false,
        },
    ) {
        Ok(report) => native_ran(&project_root, &builder, report),
        Err(error) => Response::Error {
            message: error.to_string(),
        },
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn native_run_graph(
    graph: &ModuleGraph,
    path: String,
    project_root: String,
    profile: NativeProfile,
    node: bool,
    static_link: bool,
    pgo_generate: Option<PathBuf>,
    pgo_use: Option<PathBuf>,
) -> Response {
    let project_root = PathBuf::from(project_root);
    let builder = match discover_native_builder(&project_root) {
        Ok(builder) => builder,
        Err(message) => return Response::Error { message },
    };
    let output =
        match default_native_output_for_builder(&builder, &project_root, Path::new(&path), false) {
            Ok(output) => output,
            Err(message) => return Response::Error { message },
        };
    match builder.run_graph(
        graph,
        &kiln_native::NativeBuildOptions {
            output,
            profile: native_profile(profile),
            realm: kiln_native::NativeRealmPolicy::default(),
            march_native: false,
            static_link,
            pgo_generate,
            pgo_use,
            node,
        },
    ) {
        Ok(report) => native_ran(&project_root, &builder, report),
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
        kiln_native::HostValue::String(value) => {
            ScalarValueSummary::String(value.to_scalar_string().unwrap_or_else(|_| {
                value
                    .units()
                    .map(|unit| format!("\\u{{{unit:04X}}}"))
                    .collect()
            }))
        }
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
        NativeProfile::ReleaseSize => kiln_native::NativeProfile::ReleaseSize,
        NativeProfile::Profile => kiln_native::NativeProfile::Profile,
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
        prelude_object_cache_hit: report.cache_hits.prelude_object,
        runtime_object_cache_hit: report.cache_hits.runtime_object,
        launcher_object_cache_hit: report.cache_hits.launcher_object,
        executable_cache_hit: report.cache_hits.executable,
        generated_c_bytes: report.sizes.generated_c,
        app_object_bytes: report.sizes.application_object,
        prelude_object_bytes: report.sizes.prelude_object,
        runtime_object_bytes: report.sizes.runtime_object,
        launcher_object_bytes: report.sizes.launcher_object,
        executable_bytes: report.sizes.executable,
        provider_required: report
            .required_provider_capabilities
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        provider_supplied: report
            .provider_capabilities
            .names()
            .into_iter()
            .map(str::to_owned)
            .collect(),
        emit_millis: millis(report.timings.emit),
        compile_millis: millis(report.timings.compile),
        link_millis: millis(report.timings.link),
        total_millis: millis(report.timings.total),
        explanation: None,
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

pub(crate) fn resolve_source_path(path: &str, project_root: &str) -> Option<PathBuf> {
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
pub(crate) fn ensure_canonical_source<E: QueryEngine>(
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
